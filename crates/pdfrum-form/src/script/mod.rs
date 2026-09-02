//! A `boa`-backed [`Cascade`](crate::Cascade): a document's own scripts, run.
//!
//! Behind the default-off `script` feature. With the feature off `boa_engine`
//! and the 116 crates it brings are not in the tree at all, which
//! `scripts/check-no-boa.sh` asserts in both directions.
//!
//! # This is `Cascade`'s second implementation, not a fourth seam
//!
//! STYLE §2b closes the trait-seam list at three, and this does not open it.
//! [`ScriptCascade`] implements the trait `NoScripts` already implements, and
//! everything a script asks of the host comes back as a **value the host
//! reads** — [`ScriptCascade::transcript`] — rather than through a
//! `ScriptHost` trait the host implements. §2b's own rule is the test: invert
//! only when the library must ask a question it cannot answer and cannot
//! proceed without the reply. `app.alert` is not that: with no form-fill
//! environment upstream it returns `0` and the script carries on
//! (`fxjs/cjs_app.cpp:233-236`), so the line goes on a list.
//!
//! # The sandbox is a property, and it is tested
//!
//! Three of boa's four `RuntimeLimits` are wired to [`Limits`] fields, and a
//! script that exhausts one produces a [`Diagnostic`] and the *refusing*
//! answer from whichever hook was running — `Reject` for `keystroke`, `false`
//! for `keystroke_commit` and `validate`, no writes for `calculate`, `None`
//! for `format`. Never a hang, never a panic, and never a silent acceptance:
//! a script that ran out of budget did not say "accept", and inventing an
//! acceptance on its behalf is the failure mode that lets a hostile file walk
//! past a validator.
//!
//! What the limits do **not** bound is heap growth and regex backtracking —
//! and neither does a V8-enabled PDFium, measured rather than assumed
//! (`docs/status/data/v8probe/REPORT.md`; SPEC §10 carries the ruling and the
//! reopening condition). So pdfrum is bounded where the oracle hangs, and
//! unbounded only where the oracle is too.
//!
//! # No I/O is reachable from a script
//!
//! Not "removed" — **never built**. `Doc.submitForm`, `Doc.print`,
//! `Doc.mailDoc`, `app.launchURL` and `app.response` are transcript lines, so
//! the URL and the form bytes come back to the host rather than going out;
//! nothing here opens a socket, a file, or a process, and
//! `sandbox.rs`'s tests assert the absence rather than trusting the reading.
//!
//! [`Limits`]: pdfrum_common::Limits
//! [`Diagnostic`]: pdfrum_common::Diagnostic

mod af;
mod bind;
mod host;
pub mod transcript;

use std::rc::Rc;

use boa_engine::context::Context;
use pdfrum_common::{Diagnostics, Limits};

use crate::cascade::{Cascade, FieldRef, FieldWrites, Keystroke, KeystrokeOutcome};

pub use transcript::TranscriptLine;

/// PDFium's own test seed, in **seconds**: the value the conformance harness
/// passes as `pdfium_test --time=1399672130`.
///
/// Not a default and not something this crate applies on its own — a *constant
/// a golden run passes in*, through [`ScriptConfig::frozen_at`]. The frozen
/// clock leaks into the expected bytes (`public_methods_expected.txt` pins
/// `AFParseDateEx(1, 2) = 1399672130000`), so a conformance run must set it
/// and an ordinary embedder must not.
///
/// Exported for tests and for a harness that wants to name the seed; the tool
/// reads its own `--time=` rather than reaching for this.
pub const PDFIUM_TEST_CLOCK_SECS: u64 = 1_399_672_130;

/// The timezone the **engine's `Date`** sees: `TZ=America/Los_Angeles` as V8
/// resolves it for the fixtures' July dates, which is `GMT-0700`.
///
/// This is `Date`'s offset only. `util.printd` uses a *different* one — see
/// [`PDFIUM_TEST_FX_LOCALTIME_OFFSET_SECS`], and read that doc before
/// assuming the two should agree.
pub const PDFIUM_TEST_TZ_OFFSET_SECS: i32 = -7 * 3600;

/// The offset **`FX_LocalTime` applies**, which is not the one `Date` uses:
/// a flat `GMT-0800`, with no daylight saving, whatever the date.
///
/// # Why the two differ, which is not a bug in either
///
/// `CJS_Util::printd` converts its instant with `FX_LocalTime`
/// (`fxjs/cjs_util.cpp:185`) before reading the components, and
/// `FX_LocalTime` is `d + GetLocalTZA() + GetDaylightSavingTA(d)`
/// (`fxjs/fx_date_helpers.cpp:254-256`). Both terms go through PDFium's own
/// overridable clock hooks, and the test binary overrides them:
///
/// ```text
/// FSDK_SetTimeFunction([]() { return time_ret; });
/// FSDK_SetLocaltimeFunction([](const time_t* tp) { return gmtime(tp); });
/// ```
/// (`testing/pdfium_test/pdfium_test.cc:2133-2134`)
///
/// **`localtime` is replaced by `gmtime`.** So `GetDaylightSavingTA` reads
/// `tm_isdst == 0` for every instant and contributes nothing (`:54-68`),
/// while `GetLocalTZA` still reads glibc's `timezone` global — the zone's
/// **standard** offset, PST, −8 hours (`:38-52`). V8's own `Date`, which
/// never goes through those hooks, keeps the real −7. The two are one hour
/// apart, all summer, by construction.
///
/// Confirmed against the goldens rather than reasoned:
/// `util_printd_expected.txt:4` prints `14:59:58` from an instant
/// `new Date(2014, 6, 4, 15, 59, 58)` places at `22:59:58Z` — −8, not −7 —
/// while `util_scand`'s every line round-trips to the UTC string it was
/// given, which only holds if `Date` and the parser agree on −7.
pub const PDFIUM_TEST_FX_LOCALTIME_OFFSET_SECS: i32 = -8 * 3600;

/// What a scripting session is allowed to do, and what it sees.
#[derive(Debug, Clone, Default)]
pub struct ScriptConfig {
    /// The bounds. Exhausting one is a diagnostic and a refusal.
    pub limits: Limits,
    /// The clock scripts see, in milliseconds since the epoch.
    ///
    /// `None` reads the **host clock**, which is the ordinary case and is the
    /// oracle's too: `pdfium_test` installs its time hooks only when
    /// `--time=` was given (`testing/pdfium_test/pdfium_test.cc:2129-2135`),
    /// and without them `FXSYS_time` is libc's
    /// (`core/fxcrt/fx_extension.cpp:110-116`). `Some` freezes it, which is
    /// what a golden run needs — see [`ScriptConfig::frozen_at`].
    pub clock_ms: Option<i64>,
    /// The local timezone offset scripts see, in seconds east of UTC.
    ///
    /// Configuration rather than something read from the machine, because it
    /// is in the expected bytes: every `util.printd` golden line is shifted to
    /// PDFium's Los Angeles offset, and a golden run in another zone must
    /// still produce them.
    pub timezone_offset_secs: i32,
    /// The offset `util.printd` applies before reading a date's components —
    /// `FX_LocalTime`'s, which is **not** the one `Date` uses. See
    /// [`PDFIUM_TEST_FX_LOCALTIME_OFFSET_SECS`] for why they differ.
    pub printd_offset_secs: i32,
}

impl ScriptConfig {
    /// The configuration for a run whose clock is frozen at `seconds` since
    /// the epoch — `pdfium_test --time=<seconds>`.
    ///
    /// **The seed is the caller's**, which is the whole point: `--time=` is
    /// the single source of the scripting clock, and this crate no longer
    /// knows which instant a golden run wants. A conformance run passes
    /// [`PDFIUM_TEST_CLOCK_SECS`].
    ///
    /// The two timezone offsets come with it rather than being separately
    /// configurable, because upstream installs both hooks under the *same*
    /// guard: `FSDK_SetTimeFunction` and `FSDK_SetLocaltimeFunction` are set
    /// together inside `if (options.time > -1)`
    /// (`testing/pdfium_test/pdfium_test.cc:2129-2135`), and the second one —
    /// `localtime` replaced by `gmtime` — is what makes `util.printd`'s offset
    /// differ from `Date`'s. Freezing the instant without freezing the zone
    /// would reproduce neither.
    ///
    /// A `seconds` past `i64` milliseconds saturates rather than wrapping; no
    /// `time_t` a command line can carry reaches that.
    #[must_use]
    pub fn frozen_at(seconds: u64) -> ScriptConfig {
        let millis = i64::try_from(seconds)
            .unwrap_or(i64::MAX)
            .saturating_mul(1000);
        ScriptConfig {
            limits: Limits::default(),
            clock_ms: Some(millis),
            timezone_offset_secs: PDFIUM_TEST_TZ_OFFSET_SECS,
            printd_offset_secs: PDFIUM_TEST_FX_LOCALTIME_OFFSET_SECS,
        }
    }

    /// The configuration for a run with **no `--time=`**: the real wall
    /// clock, and the host's own zone offsets left at whatever
    /// [`Default`] gives them.
    ///
    /// This is the ordinary embedder's configuration and the one a tool
    /// invoked without the flag builds.
    #[must_use]
    pub fn wall_clock() -> ScriptConfig {
        ScriptConfig {
            clock_ms: None,
            ..ScriptConfig::default()
        }
    }
}

/// Why a script did not finish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScriptStop {
    /// It exhausted one of the [`Limits`] — a loop, recursion or stack bound.
    /// The refusing answer follows.
    LimitReached,
    /// It threw, or would not parse. The message is the engine's.
    Threw(String),
}

/// The four `/AA` entries a field can carry, as their JavaScript source.
///
/// `None` is the ordinary case and is not an absence to be filled in later:
/// nearly every field in nearly every document has no script at all, and a
/// hook with no script takes the permissive answer — which is `NoScripts`'s
/// answer, and is correct rather than a fallback.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FieldActions {
    /// `/AA /K` — the keystroke hook, run per character and again on commit.
    pub keystroke: Option<String>,
    /// `/AA /V` — the validation hook.
    pub validate: Option<String>,
    /// `/AA /C` — the calculation hook.
    pub calculate: Option<String>,
    /// `/AA /F` — the format hook.
    pub format: Option<String>,
}

/// A `boa`-backed [`Cascade`].
///
/// Built over one realm, which the whole session shares — because a document's
/// scripts share a `global` and expect to, and because building a realm per
/// event would lose it.
pub struct ScriptCascade {
    context: Context,
    host: host::Host,
    /// Each field's `/AA` scripts, by the index [`FieldRef`] carries.
    ///
    /// **Installed by the caller rather than read here**, because reading
    /// `/AA` needs a document and this type deliberately holds none — which
    /// is what lets the whole engine be tested against a script string and no
    /// PDF. `pdfrum-doc`'s `nav::additional_action` is what produces them.
    actions: std::collections::BTreeMap<u32, FieldActions>,
    /// Each field's fully-qualified name, for `event.targetName`.
    names: std::collections::BTreeMap<u32, String>,
    /// Each field's current value, as the calculation sweep sees it.
    ///
    /// A snapshot the sweep updates as its own writes land, so a later field
    /// in the `/CO` order reads what an earlier one computed — which is what
    /// `NotificationOption::kNotify` achieves upstream by re-entering the
    /// notifier chain.
    values: std::collections::BTreeMap<u32, String>,
    /// The `/CO` order, which is the whole of what a calculation sweep
    /// visits. **Empty means no calculation runs at all**, however many
    /// fields carry `/AA /C` — see `Form::calculation_order`.
    order: Vec<u32>,
    /// What a script last did wrong, for the caller's diagnostics.
    stops: Vec<(String, ScriptStop)>,
    /// How deep a calculation may nest. One, upstream.
    max_calculate_depth: u32,
    /// `CJS_EventContext::busy_` — a script running inside a script is
    /// refused rather than re-entered.
    busy: bool,
}

impl std::fmt::Debug for ScriptCascade {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `boa_engine::Context` is not `Debug`, and printing a realm would be
        // useless anyway. What a reader wants is what the session has done.
        f.debug_struct("ScriptCascade")
            .field("transcript", &self.transcript().len())
            .field("stops", &self.stops)
            .field("busy", &self.busy)
            .finish_non_exhaustive()
    }
}

/// Building a realm failed, which only a broken engine build can cause.
#[derive(Debug, thiserror::Error)]
#[error("the script engine could not build a realm: {message}")]
pub struct BuildError {
    message: String,
}

impl ScriptCascade {
    /// Builds a session with its own realm.
    ///
    /// # Errors
    ///
    /// Only if `boa` cannot build a context at all, which no input can cause.
    pub fn new(config: &ScriptConfig) -> Result<ScriptCascade, BuildError> {
        let mut builder = Context::builder();
        // The instant and the zone are frozen **together or not at all**,
        // which is upstream's single guard: `FSDK_SetTimeFunction` and
        // `FSDK_SetLocaltimeFunction` are installed inside one
        // `if (options.time > -1)` (`pdfium_test.cc:2129-2135`). Absent
        // `--time=`, `Date` reads the machine's real clock through boa's
        // default hooks and its real zone through boa's default
        // `local_timezone_offset_seconds` — the answer an embedder wants,
        // and the one a golden run must never see.
        if let Some(millis) = config.clock_ms {
            let millis = u64::try_from(millis).unwrap_or(0);
            builder = builder
                .host_hooks(Rc::new(host::FixedZone {
                    offset_secs: config.timezone_offset_secs,
                }))
                .clock(Rc::new(boa_engine::context::time::FixedClock::from_millis(
                    millis,
                )));
        }
        let mut context = builder.build().map_err(|error| BuildError {
            message: error.to_string(),
        })?;

        let mut runtime_limits = context.runtime_limits();
        runtime_limits.set_loop_iteration_limit(config.limits.max_script_loop_iterations);
        runtime_limits.set_recursion_limit(config.limits.max_script_recursion);
        runtime_limits.set_stack_size_limit(config.limits.max_script_stack);
        context.set_runtime_limits(runtime_limits);

        context.insert_data(bind::PrintdOffset(config.printd_offset_secs));
        let host = bind::new_host();
        bind::install(&mut context, Rc::clone(&host)).map_err(|error| BuildError {
            message: error.to_string(),
        })?;

        Ok(ScriptCascade {
            context,
            host,
            actions: std::collections::BTreeMap::new(),
            names: std::collections::BTreeMap::new(),
            values: std::collections::BTreeMap::new(),
            order: Vec::new(),
            stops: Vec::new(),
            max_calculate_depth: config.limits.max_calculate_depth,
            busy: false,
        })
    }

    /// Everything a script asked the host to do, in order.
    ///
    /// The value the goldens are scored against. See
    /// [`transcript::render`] for the bytes.
    #[must_use]
    pub fn transcript(&self) -> Vec<TranscriptLine> {
        self.host.borrow().transcript.clone()
    }

    /// Timers a script asked for, as `(script, interval_ms)`.
    ///
    /// **Recorded and never fired.** A later milestone fires them from
    /// `advance_time`, which M14's D14 reserved as the step function; until
    /// then this is the honest answer to "what did the document want", and it
    /// is a *value the host reads* rather than a process-wide registry, which
    /// STYLE §1 forbids and which is what upstream uses
    /// (`fxjs/global_timer.cpp:18-19`).
    #[must_use]
    pub fn timers(&self) -> Vec<(String, i32)> {
        self.host.borrow().timers.clone()
    }

    /// The transcript, rendered the way `pdfium_test` writes stdout.
    #[must_use]
    pub fn transcript_text(&self) -> String {
        transcript::render(&self.transcript())
    }

    /// What went wrong, and in which script — the diagnostics a caller reads
    /// after a run.
    #[must_use]
    pub fn stops(&self) -> &[(String, ScriptStop)] {
        &self.stops
    }

    /// Runs one script, recording anything it asked for and anything that
    /// stopped it.
    ///
    /// **Never panics and never propagates an engine error to the caller.** A
    /// script is untrusted input: a parse failure, a thrown exception and an
    /// exhausted limit are the three ordinary outcomes, and all three answer
    /// `false` here with a recorded reason.
    ///
    /// `where` names the script for the diagnostic — a field name, or
    /// `"/OpenAction"`.
    pub fn run(&mut self, source: &str, whence: &str) -> bool {
        if self.busy {
            // `CJS_EventContext::busy_` (`fxjs/cjs_event_context.cpp:32-38`):
            // a script provoked by a script is refused, not re-entered.
            self.stops.push((
                whence.to_string(),
                ScriptStop::Threw("System is busy.".to_string()),
            ));
            return false;
        }
        self.busy = true;
        let result = self
            .context
            .eval(boa_engine::Source::from_bytes(source.as_bytes()));
        self.busy = false;

        match result {
            Ok(_) => true,
            Err(error) => {
                let message = error.to_string();
                // boa reports an exhausted `RuntimeLimits` as a
                // `RuntimeLimitError`, which is the one failure that is about
                // the *sandbox* rather than about the script's own logic.
                let stop = if message.contains("RuntimeLimit") {
                    ScriptStop::LimitReached
                } else {
                    ScriptStop::Threw(message)
                };
                self.stops.push((whence.to_string(), stop));
                false
            }
        }
    }

    /// Whether the last thing that stopped a script was a limit rather than
    /// the script's own logic.
    ///
    /// The distinction a caller cares about: a script that *threw* said
    /// something about the document, and one that ran out of budget said
    /// nothing at all.
    #[must_use]
    pub fn last_stop_was_a_limit(&self) -> bool {
        matches!(
            self.stops.last().map(|(_, stop)| stop),
            Some(ScriptStop::LimitReached)
        )
    }

    /// Sets `event`'s fields for one kind and runs the script.
    ///
    /// # Every kind resets every field first
    ///
    /// `Initialize(kind)` (`fxjs/cjs_event_context.cpp:289-310`) clears
    /// everything before a setup method fills in what its kind makes live, so
    /// a Validate event never sees the selection a preceding Keystroke event
    /// wrote. That reset is reproduced here by writing every field on every
    /// event rather than only the live ones — the same observable behaviour,
    /// and one fewer thing to get wrong than a per-kind write list.
    ///
    /// **Which fields are read back afterwards is where the kinds differ**,
    /// and that lives in the [`Cascade`] methods rather than here: a write to
    /// a field that is dead for the kind lands in the object and is dropped,
    /// which is exactly what upstream's dummy-fallback pointers do without
    /// any pointer aliasing.
    fn run_event(&mut self, source: &str, live: &EventFields<'_>, whence: &str) -> bool {
        let setup = format!(
            "event.name = {};\n\
             event.targetName = {};\n\
             event.type = \"Field\";\n\
             event.value = {};\n\
             event.change = {};\n\
             event.changeEx = \"\";\n\
             event.selStart = {};\n\
             event.selEnd = {};\n\
             event.willCommit = {};\n\
             event.fieldFull = {};\n\
             event.commitKey = {};\n\
             event.shift = false;\n\
             event.modifier = false;\n\
             event.keyDown = false;\n\
             event.rc = true;\n",
            quote(live.name),
            quote(&live.target_name),
            quote(&live.value),
            quote(&live.change),
            live.selection_start,
            live.selection_end,
            live.will_commit,
            live.field_full,
            live.commit_key,
        );
        if !self.run(&setup, whence) {
            return false;
        }
        self.run(source, whence)
    }

    /// The script one trigger runs for one field, if it has one.
    fn script_for(&self, field: &FieldRef, trigger: Trigger) -> Option<String> {
        let actions = self.actions.get(&field.index)?;
        match trigger {
            Trigger::Keystroke => actions.keystroke.clone(),
            Trigger::Validate => actions.validate.clone(),
            Trigger::Format => actions.format.clone(),
        }
    }

    /// Installs one field's scripts, its name and its current value.
    ///
    /// The caller reads `/AA` and hands it over, because reading it needs a
    /// document and this type deliberately holds none.
    pub fn set_field(
        &mut self,
        index: u32,
        name: impl Into<String>,
        value: impl Into<String>,
        actions: FieldActions,
    ) {
        self.names.insert(index, name.into());
        self.values.insert(index, value.into());
        self.actions.insert(index, actions);
    }

    /// Installs the `/CO` calculation order — the field indices a calculation
    /// sweep visits, in the order it visits them.
    ///
    /// **An empty order means no calculation runs**, which is the answer for
    /// a document with no `/CO` array and is not a fallback to "every field":
    /// `Form::calculation_order` explains why, and
    /// `cpdf_interactiveform.cpp:739-745` is where the rule lives.
    pub fn set_calculation_order(&mut self, order: Vec<u32>) {
        self.order = order;
    }

    /// How deep a calculation may nest, for a caller building a
    /// [`FieldWrites`].
    #[must_use]
    pub fn max_calculate_depth(&self) -> u32 {
        self.max_calculate_depth
    }

    /// Records this session's stops as diagnostics on the caller's sink.
    ///
    /// Kept separate from [`ScriptCascade::run`] so a caller decides when
    /// diagnostics are drained, and so the cascade methods — whose signatures
    /// take no `Diagnostics` — can still be honest about what happened.
    pub fn drain_diagnostics(&mut self, diags: &mut Diagnostics) {
        for (_whence, stop) in self.stops.drain(..) {
            diags.record(
                pdfrum_common::Severity::Suspicious,
                match stop {
                    ScriptStop::LimitReached => pdfrum_common::DiagKind::ScriptLimitReached,
                    ScriptStop::Threw(_) => pdfrum_common::DiagKind::ScriptFailed,
                },
                None,
            );
        }
    }

    /// Reads `event.rc` back as JavaScript truthiness.
    ///
    /// `ToBooleanReentrant`, not a type check (`fxjs/cjs_event.cpp`), so
    /// `event.rc = 'boo'` is `true` — upstream's behaviour, reproduced rather
    /// than tightened.
    fn event_rc(&mut self) -> bool {
        self.context
            .eval(boa_engine::Source::from_bytes(b"!!event.rc"))
            .is_ok_and(|value| value.to_boolean())
    }

    /// Reads a string field back off `event`.
    fn event_string(&mut self, field: &str) -> Option<String> {
        let source = format!("String(event.{field})");
        let value = self
            .context
            .eval(boa_engine::Source::from_bytes(source.as_bytes()))
            .ok()?;
        Some(
            value
                .to_string(&mut self.context)
                .ok()?
                .to_std_string_lossy(),
        )
    }

    /// Reads an integer field back off `event`.
    fn event_i32(&mut self, field: &str) -> Option<i32> {
        let source = format!("event.{field}");
        let value = self
            .context
            .eval(boa_engine::Source::from_bytes(source.as_bytes()))
            .ok()?;
        value.to_i32(&mut self.context).ok()
    }
}

/// Which `/AA` entry a hook runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Trigger {
    /// `/AA /K`, with or without a commit imminent — one key, two methods.
    Keystroke,
    /// `/AA /V`.
    Validate,
    /// `/AA /F`.
    Format,
}

/// The `event` fields one kind makes live (brief §4.4).
struct EventFields<'a> {
    name: &'a str,
    /// `event.targetName` — the **fully-qualified** name, always set by value.
    target_name: String,
    /// `event.source`'s name. **Set only by Calculate**
    /// (`fxjs/cjs_event_context.cpp`), so everywhere else `event.source` is a
    /// field attached to the empty name — which is upstream's behaviour and
    /// is reproduced rather than tidied.
    source_name: Option<String>,
    value: String,
    change: String,
    selection_start: i32,
    selection_end: i32,
    will_commit: bool,
    field_full: bool,
    commit_key: i32,
}

impl EventFields<'_> {
    /// The `Initialize` defaults every kind starts from.
    ///
    /// `commit_key` resets to **`-1`**, not 0
    /// (`fxjs/cjs_event_context.cpp:289-310`); only Keystroke and Format set
    /// it to 0.
    fn blank(name: &str) -> EventFields<'_> {
        EventFields {
            name,
            target_name: String::new(),
            source_name: None,
            value: String::new(),
            change: String::new(),
            selection_start: 0,
            selection_end: 0,
            will_commit: false,
            field_full: false,
            commit_key: -1,
        }
    }
}

/// A JavaScript string literal, escaped.
///
/// A field value is untrusted document content and goes into a script the
/// engine then parses, so this is the one place where getting the escaping
/// wrong would be a *code injection into our own sandbox*. Everything outside
/// a small safe set is emitted as `\u{XXXX}`, which cannot end the literal
/// whatever it is.
fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, ' ' | '.' | ',' | '-' | '_' | '+' | '/') {
            out.push(ch);
        } else {
            for unit in ch.encode_utf16(&mut [0u16; 2]) {
                let _ = std::fmt::Write::write_fmt(&mut out, format_args!("\\u{unit:04x}"));
            }
        }
    }
    out.push('"');
    out
}

impl Cascade for ScriptCascade {
    /// The keystroke hook: `/AA /K` with `willCommit` false.
    ///
    /// The script may **rewrite `event.change` and move the selection**, and
    /// what it left is what gets applied — `SetActionData`
    /// (`fpdfsdk/formfiller/cffl_textfield.cpp:216-222`) reads
    /// `fa.nSelStart`, `fa.nSelEnd` and `fa.sChange` back and nothing else.
    /// A write to `event.value` on this path compiles, does not throw, and is
    /// **discarded**: no code upstream ever reads it back.
    fn keystroke(&mut self, field: &FieldRef, change: Keystroke) -> KeystrokeOutcome {
        let Some(source) = self.script_for(field, Trigger::Keystroke) else {
            return KeystrokeOutcome::Accept(change);
        };
        let live = EventFields {
            name: "Keystroke",
            target_name: field.name.clone(),
            // Only Calculate sets a source.
            source_name: None,
            value: change.value.clone(),
            change: change.change.clone(),
            selection_start: change.selection_start,
            selection_end: change.selection_end,
            will_commit: false,
            field_full: false,
            // Keystroke and Format are the only kinds that set it to 0.
            commit_key: 0,
        };
        if !self.run_event(&source, &live, &field.name) {
            // A script that threw or ran out of budget did not say "accept".
            return KeystrokeOutcome::Reject;
        }
        if !self.event_rc() {
            return KeystrokeOutcome::Reject;
        }
        KeystrokeOutcome::Accept(Keystroke {
            change: self.event_string("change").unwrap_or(change.change),
            value: change.value,
            selection_start: self.event_i32("selStart").unwrap_or(change.selection_start),
            selection_end: self.event_i32("selEnd").unwrap_or(change.selection_end),
        })
    }

    /// The same `/AA /K` with a commit imminent, which is the only thing that
    /// differs: `willCommit` is `true` and `event.value` is the whole value
    /// rather than the text before an insertion.
    fn keystroke_commit(&mut self, field: &FieldRef, value: &str) -> bool {
        let Some(source) = self.script_for(field, Trigger::Keystroke) else {
            return true;
        };
        let live = EventFields {
            name: "Keystroke",
            target_name: field.name.clone(),
            // Only Calculate sets a source.
            source_name: None,
            value: value.to_string(),
            change: String::new(),
            selection_start: 0,
            selection_end: 0,
            will_commit: true,
            field_full: false,
            commit_key: 0,
        };
        self.run_event(&source, &live, &field.name) && self.event_rc()
    }

    /// `/AA /V`. `event.rc` is the whole answer; a write to `event.value` is
    /// discarded exactly as on the keystroke path.
    fn validate(&mut self, field: &FieldRef, value: &str) -> bool {
        let Some(source) = self.script_for(field, Trigger::Validate) else {
            return true;
        };
        let mut live = EventFields::blank("Validate");
        live.target_name.clone_from(&field.name);
        live.value = value.to_string();
        self.run_event(&source, &live, &field.name) && self.event_rc()
    }

    /// `/AA /C`, over the whole calculation order.
    ///
    /// **One call runs the entire sweep**, which is `OnCalculate`
    /// (`fpdfsdk/cpdfsdk_interactiveform.cpp:270-310`) walking `/CO` and
    /// writing each field in turn. The three-way gate at `:307` is normative
    /// and is reproduced: a calculated value is written **only if** the script
    /// did not throw, **and** `event.rc` is still truthy, **and** the string
    /// actually changed.
    ///
    /// The `busy_` guard is [`FieldWrites`]'s depth budget, whose default is
    /// 1 because upstream permits no nesting at all.
    fn calculate(&mut self, writes: &mut FieldWrites, trigger: &FieldRef) {
        if !writes.enter() {
            // The budget is spent: this is a nested sweep, and upstream's
            // `busy_` makes every one of those a no-op.
            return;
        }
        let order: Vec<u32> = self.order.clone();
        for index in order {
            let Some(source) = self
                .actions
                .get(&index)
                .and_then(|actions| actions.calculate.clone())
            else {
                continue;
            };
            let before = self.values.get(&index).cloned().unwrap_or_default();
            let mut live = EventFields::blank("Calculate");
            live.target_name = self.names.get(&index).cloned().unwrap_or_default();
            live.value.clone_from(&before);
            // `event.source` is meaningful only here, and it names the field
            // whose change provoked the sweep.
            live.source_name = Some(trigger.name.clone());

            if !self.run_event(&source, &live, &live.target_name.clone()) {
                continue;
            }
            if !self.event_rc() {
                continue;
            }
            let Some(after) = self.event_string("value") else {
                continue;
            };
            if after == before {
                continue;
            }
            self.values.insert(index, after.clone());
            writes.set(index, after);
        }
        writes.leave();
    }

    /// `/AA /F`.
    ///
    /// **The write reaches the appearance only, never `/V`.** `OnFormat`
    /// (`fpdfsdk/cpdfsdk_interactiveform.cpp:313-346`) binds `event.value` to
    /// a *local* string, runs the script, and returns the mutated local as an
    /// optional that reaches
    /// `pEdit->SetText(sValue.value_or(pField->GetValue()))`
    /// (`cpdfsdk_appstream.cpp:1752`) — one line, and the whole mechanism.
    /// Re-running with no formatter reverts the appearance to the raw value.
    ///
    /// Format also **hard-codes `willCommit = true`** (`:282`), which is why
    /// `event_properties.in` — a Format handler — reads `true`, and leaves
    /// `rc` unbound, so a Format script's `event.rc` writes reach nothing.
    fn format(&mut self, field: &FieldRef, value: &str) -> Option<String> {
        let source = self.script_for(field, Trigger::Format)?;
        let live = EventFields {
            name: "Format",
            target_name: field.name.clone(),
            // Only Calculate sets a source.
            source_name: None,
            value: value.to_string(),
            change: String::new(),
            selection_start: 0,
            selection_end: 0,
            // Hard-coded, not inherited.
            will_commit: true,
            field_full: false,
            commit_key: 0,
        };
        if !self.run_event(&source, &live, &field.name) {
            return None;
        }
        let formatted = self.event_string("value")?;
        (formatted != value).then_some(formatted)
    }
}

#[cfg(test)]
mod tests;
