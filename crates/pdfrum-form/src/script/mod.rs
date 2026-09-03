//! A `boa`-backed [`Cascade`](crate::Cascade): a document's own scripts, run.
//!
//! Behind the default-off `script` feature; with it off, `boa_engine` is not
//! in the dependency tree at all.
//!
//! **A script reaches no I/O.** `Doc.submitForm`, `Doc.print`, `app.launchURL`
//! and the rest are transcript lines a host reads back through
//! [`ScriptCascade::transcript`], so a URL or a form's bytes come back rather
//! than going out; nothing opens a socket, a file or a process.
//!
//! **A script that exhausts a sandbox limit refuses rather than accepting.**
//! [`Limits`] bounds loops, recursion and stack depth; not heap or regex
//! backtracking.
//!
//! [`Limits`]: pdfrum_common::Limits

mod af;
mod bind;
mod color;
mod consts;
mod doc;
pub(crate) mod event;
mod field;
mod global;
mod host;
pub mod model;
mod submit;
mod timer;
pub mod transcript;

use std::rc::Rc;

use boa_engine::context::Context;
use pdfrum_common::{Diagnostics, Limits};

use crate::cascade::{Cascade, FieldRef, FieldWrites, Keystroke, KeystrokeOutcome};
use event::EventState;

pub use model::{AnnotModel, DocumentModel, FieldModel, FieldModelFlags, FieldModelKind};
pub use transcript::TranscriptLine;

/// The instant the **goldens were recorded at**, in seconds since the epoch:
/// what a conformance run must freeze its clock to.
///
/// Not a default and not something this crate applies on its own — a *constant
/// a golden run passes in*, through [`ScriptConfig::frozen_at`]. The frozen
/// clock leaks into the expected bytes (`public_methods_expected.txt` pins
/// `AFParseDateEx(1, 2) = 1399672130000`), so a conformance run must set it
/// and an ordinary embedder must not.
///
/// Exported for tests and for a harness that wants to name the seed; the
/// conformance tool reads its own `--time=` rather than reaching for this.
pub const GOLDEN_CLOCK_SECS: u64 = 1_399_672_130;

/// The timezone the **engine's `Date`** saw when the goldens were recorded:
/// `TZ=America/Los_Angeles` as V8 resolves it for the fixtures' July dates,
/// which is `GMT-0700`.
///
/// This is `Date`'s offset only. `util.printd` uses a *different* one — see
/// [`GOLDEN_PRINTD_OFFSET_SECS`], and read that doc before assuming the two
/// should agree.
pub const GOLDEN_TIMEZONE_OFFSET_SECS: i32 = -7 * 3600;

/// The offset **`util.printd` applies** when the goldens were recorded, which
/// is not the one `Date` uses: a flat `GMT-0800`, with no daylight saving,
/// whatever the date.
///
/// # Why the two differ, which is not a bug in either
///
/// The golden harness replaces `localtime` with `gmtime`, so the daylight
/// term contributes nothing while the standard-offset term still reads the
/// zone's own — PST, −8 hours. The engine's `Date` never goes through those
/// hooks and keeps the real −7. The two are one hour apart, all summer, by
/// construction.
pub const GOLDEN_PRINTD_OFFSET_SECS: i32 = -8 * 3600;

/// The file path **the goldens were recorded with**, which is the test
/// harness's own and not any real file's.
///
/// It leaks into the expected bytes twice, as `this.URL` and as `this.path`
/// — the second with a leading separator.
///
/// A constant a golden run passes in, exactly as [`GOLDEN_CLOCK_SECS`] is —
/// an embedder passes the path it actually opened.
pub const GOLDEN_FILE_PATH: &str = "myfile.pdf";

/// What a scripting session is allowed to do, and what it sees.
#[derive(Debug, Clone, Default)]
pub struct ScriptConfig {
    /// The bounds. Exhausting one is a diagnostic and a refusal.
    pub limits: Limits,
    /// The clock scripts see, in milliseconds since the epoch.
    ///
    /// `None` reads the **host clock**, which is the ordinary case and is the
    /// oracle's too. `Some` freezes it, which is what a golden run needs — see
    /// [`ScriptConfig::frozen_at`].
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
    /// [`GOLDEN_PRINTD_OFFSET_SECS`] for why they differ.
    pub printd_offset_secs: i32,
}

impl ScriptConfig {
    /// The configuration for a run whose clock is frozen at `seconds` since
    /// the epoch.
    ///
    /// **The seed is the caller's**: a conformance run passes
    /// [`GOLDEN_CLOCK_SECS`], an embedder its own instant.
    ///
    /// The two timezone offsets come with it rather than being separately
    /// configurable, because the clock and the zone are frozen together —
    /// freezing the instant without freezing the zone reproduces neither.
    ///
    /// A `seconds` past `i64` milliseconds saturates rather than wrapping.
    #[must_use]
    pub fn frozen_at(seconds: u64) -> ScriptConfig {
        let millis = i64::try_from(seconds)
            .unwrap_or(i64::MAX)
            .saturating_mul(1000);
        ScriptConfig {
            limits: Limits::default(),
            clock_ms: Some(millis),
            timezone_offset_secs: GOLDEN_TIMEZONE_OFFSET_SECS,
            printd_offset_secs: GOLDEN_PRINTD_OFFSET_SECS,
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

/// One script that stopped, named and explained.
///
/// **An uncaught exception is never swallowed.** The error is reported on the
/// diagnostic channel and the **next script still runs**; the transcript is
/// unaffected, because a throwing script prints nothing to it. `[oracle-bug]`:
/// the oracle drops the error entirely, so a fixture that crashes on its first
/// line reads as an empty transcript and scores as a pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptFailure {
    /// Which script — a field's fully-qualified name, a `/Names /JavaScript`
    /// key, or the empty string for `/OpenAction`.
    pub whence: String,
    /// Why it stopped.
    pub stop: ScriptStop,
}

impl ScriptFailure {
    /// The failure as **one** diagnostic line: where, and what the engine
    /// said.
    ///
    /// The position rides **in** the message — a throw reads
    /// `TypeError: not a callable function (unknown at :1:25)` — so there is
    /// no separate line/column pair.
    ///
    /// Only the **first** line of it:
    ///
    /// `boa` appends a stack — `\n    at <main> (…)` — after the message.
    /// A diagnostic is a line, and a caller writing one per failure must not
    /// have a multi-line one break its format. [`ScriptStop::Threw`] keeps the
    /// whole string for a caller that wants it.
    #[must_use]
    pub fn line(&self) -> String {
        let whence = if self.whence.is_empty() {
            "/OpenAction"
        } else {
            &self.whence
        };
        match &self.stop {
            ScriptStop::LimitReached => {
                format!("script {whence}: stopped by a sandbox limit")
            }
            ScriptStop::Threw(message) => {
                let first = message.lines().next().unwrap_or("").trim_end();
                format!("script {whence}: {first}")
            }
        }
    }
}

/// The ten `/AA` entries a field can carry, as their JavaScript source.
///
/// `None` is the ordinary case and is not an absence to be filled in later:
/// nearly every field in nearly every document has no script at all, and a
/// hook with no script takes the permissive answer — which is `NoScripts`'s
/// answer, and is correct rather than a fallback.
///
/// # Four value hooks and six event ones
///
/// The first four intervene in a *value*: they can rewrite what is typed,
/// refuse a commit, compute another field or produce a display string. The
/// six below them intervene in nothing — a pointer or focus script can talk
/// to the host and read the form, and `event.value` throws for it — which is
/// why they are fired rather than consulted.
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
    /// `/AA /E` — the pointer entered the widget.
    pub mouse_enter: Option<String>,
    /// `/AA /X` — the pointer left it.
    pub mouse_exit: Option<String>,
    /// `/AA /D` — a button went down over it.
    pub mouse_down: Option<String>,
    /// `/AA /U` — a button came up over it.
    pub mouse_up: Option<String>,
    /// `/AA /Fo` — the widget took the keyboard.
    pub focus: Option<String>,
    /// `/AA /Bl` — it lost the keyboard.
    pub blur: Option<String>,
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
    stops: Vec<ScriptFailure>,
    /// How deep a calculation may nest. One, upstream.
    max_calculate_depth: u32,
    /// Whether a script is running: one running inside another is refused
    /// rather than re-entered.
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

    /// The timers this session currently has armed, as
    /// `(script, interval_ms)`.
    ///
    /// A *value the host reads* rather than a process-wide registry — and a
    /// **live** one: a timer a script has cancelled, and a one-shot that has
    /// already fired, are gone from it.
    #[must_use]
    pub fn timers(&self) -> Vec<(String, i32)> {
        self.host.borrow().timers.listed()
    }

    /// Tells the session that `elapsed` passed, and runs whatever came due.
    ///
    /// **This library never reads a clock.** A host with an event loop calls
    /// this from its own timer; a test calls it with a number. Nothing here
    /// starts a thread, and a session nobody advances fires nothing however
    /// long it lives — which is what makes a document's `app.setInterval`
    /// inert in a renderer that only draws pages.
    ///
    /// Answers how many timer scripts ran. Two rules a caller is likely to be
    /// surprised by: **a timer fires at most once per call**, however large
    /// the increment — so advancing five seconds in one step fires a
    /// one-second interval once, not five times — and a timer is **re-armed
    /// before** its script runs, so a script cancelling its own timer cancels
    /// the next firing rather than this one.
    ///
    /// A script a timer runs is an ordinary script: it may throw, and its
    /// failure is recorded on [`stops`](Self::stops) like any other.
    pub fn advance_time(&mut self, elapsed: std::time::Duration) -> usize {
        let millis = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX);
        let due = self.host.borrow_mut().timers.advance(millis);
        let mut ran = 0;
        for (id, script) in due {
            self.host.borrow_mut().timers.begin(id);
            // `CJS_App::RunJsScript` runs the source under a fresh
            // `OnExternal_Exec` event — kind `Exec`, whose `type` is
            // `External` and whose `value` is not live, so a timer script
            // reading `event.value` gets the same refusal a mouse script
            // does.
            self.host.borrow_mut().event = EventState::initialize(event::EventKind::Unknown);
            self.run(&script, "app.setTimeOut");
            self.host.borrow_mut().timers.end(id);
            ran += 1;
        }
        ran
    }

    /// Makes the **next** timer a script arms fail to arm.
    ///
    /// A host that cannot give out another timer, which upstream models with
    /// `SetFailNextTimer` — the script still gets a timer object back, its id
    /// is the invalid `0`, and nothing ever fires. Exported because it is the
    /// only way to reach the branch a crash regression pins.
    pub fn fail_next_timer(&mut self) {
        self.host.borrow_mut().timers.fail_next();
    }

    /// The transcript, rendered the way the oracle writes it to stdout.
    #[must_use]
    pub fn transcript_text(&self) -> String {
        transcript::render(&self.transcript())
    }

    /// What went wrong, and in which script — the diagnostics a caller reads
    /// after a run.
    #[must_use]
    pub fn stops(&self) -> &[ScriptFailure] {
        &self.stops
    }

    /// Runs one script, recording anything it asked for and anything that
    /// stopped it.
    ///
    /// The `bool` is the answer, not a failed mutation: `true` means the
    /// script completed, `false` means a reason was recorded on
    /// [`stops`](Self::stops). **Never panics and never propagates an engine
    /// error to the caller.** A script is untrusted input: a parse failure, a
    /// thrown exception and an exhausted limit are the three ordinary
    /// outcomes, and all three answer `false` here with a recorded reason.
    ///
    /// `whence` names the script for the diagnostic — a field name, or
    /// `"/OpenAction"`.
    pub fn run(&mut self, source: &str, whence: &str) -> bool {
        if self.busy {
            // `CJS_EventContext::busy_` (`fxjs/cjs_event_context.cpp:32-38`):
            // a script provoked by a script is refused, not re-entered.
            self.stops.push(ScriptFailure {
                whence: whence.to_string(),
                stop: ScriptStop::Threw("System is busy.".to_string()),
            });
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
                self.stops.push(ScriptFailure {
                    whence: whence.to_string(),
                    stop,
                });
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
            self.stops.last().map(|failure| &failure.stop),
            Some(ScriptStop::LimitReached)
        )
    }

    /// Installs one trigger's `event` state and runs the script.
    ///
    /// # Every trigger resets every field first
    ///
    /// [`EventState::initialize`] is `CJS_EventContext::Initialize`: the
    /// whole record goes back to its reset values before the trigger's own
    /// fields are written, so a Validate script never sees the selection a
    /// preceding Keystroke left.
    ///
    /// **Which fields are read back afterwards is where the kinds differ**,
    /// and that lives in the [`Cascade`] methods rather than here: a write to
    /// a field that is dead for the kind lands in the record and is dropped.
    fn run_event(&mut self, source: &str, live: EventState, whence: &str) -> bool {
        self.host.borrow_mut().event = live;
        self.run(source, whence)
    }

    /// The script one trigger runs for one field, if it has one.
    fn script_for(&self, field: &FieldRef, trigger: Trigger) -> Option<String> {
        // A field the form's own list does not reach has no installed scripts
        // to find, which is `NoScripts`'s answer and the oracle's.
        let actions = self.actions.get(&field.index?)?;
        match trigger {
            Trigger::Keystroke => actions.keystroke.clone(),
            Trigger::Validate => actions.validate.clone(),
            Trigger::Format => actions.format.clone(),
            Trigger::Pointer(kind) => match kind {
                event::EventKind::MouseEnter => actions.mouse_enter.clone(),
                event::EventKind::MouseExit => actions.mouse_exit.clone(),
                event::EventKind::MouseDown => actions.mouse_down.clone(),
                event::EventKind::MouseUp => actions.mouse_up.clone(),
                event::EventKind::Focus => actions.focus.clone(),
                event::EventKind::Blur => actions.blur.clone(),
                // The four value kinds never reach here: `Trigger::Pointer`
                // is only built from the six above.
                _ => None,
            },
        }
    }

    /// Runs a field's `/AA /F` the way loading its page does, discarding the
    /// display string.
    ///
    /// # Why a page load runs a formatter at all
    ///
    /// Reading a page builds every widget on it, and building a text field or
    /// a combo box runs its format script so the *stored* value can be drawn
    /// as a formatted one. The script's answer reaches the appearance and
    /// never `/V`, and for a text field upstream then drops it — only a combo
    /// box regenerates from it. What is **not** dropped is everything the
    /// script asked the host to do on its way there, which is why a document
    /// whose only script is a formatter still prints alerts on open.
    ///
    /// The `bool` is whether the script completed.
    pub fn format_on_load(&mut self, field: &FieldRef) -> bool {
        let Some(source) = self.script_for(field, Trigger::Format) else {
            return true;
        };
        let mut live = EventState::initialize(event::EventKind::Format);
        live.target_name.clone_from(&field.name);
        live.target_index = field.index;
        live.has_value = true;
        live.value = self
            .values
            .get(&field.index.unwrap_or(u32::MAX))
            .cloned()
            .unwrap_or_default();
        live.will_commit = true;
        live.commit_key = 0;
        self.run_event(&source, live, &field.name)
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

    /// Installs what the `Doc` object answers from.
    ///
    /// The document half of the same bargain [`set_field`](Self::set_field)
    /// struck: this type holds no PDF, so the caller reads one and hands over
    /// a value. [`model::read`] is that reader for a `pdfrum` catalog, and a
    /// host with its own document type writes its own.
    ///
    /// Without this the object model is still **bound** — `getField` exists
    /// and is callable — and answers as an empty document would: no pages, no
    /// fields, and `undefined` from `getField`. That is the honest answer for
    /// a realm nobody told about a document, and it is why an unbound name is
    /// never what a script meets.
    pub fn set_document(&mut self, document: model::DocumentModel) {
        self.host.borrow_mut().document = document;
    }

    /// What a script wrote through `Field.value` or `Doc.resetForm`, drained.
    ///
    /// A **value the caller reads back**, not a write this type performed:
    /// applying it from inside a native function would re-enter the cascade
    /// the script is already inside. The caller spends these through the
    /// ordinary commit path, so the appearance regenerates the way any other
    /// value change does.
    ///
    /// Each entry is a `/Fields` position and the value the script set — the
    /// same shape [`FieldWrites`] carries, and the same index space.
    pub fn drain_field_writes(&mut self) -> Vec<(u32, String)> {
        std::mem::take(&mut self.host.borrow_mut().field_writes)
    }

    /// Whether a script called `Doc.calculateNow()`, drained.
    pub fn take_calculate_request(&mut self) -> bool {
        std::mem::take(&mut self.host.borrow_mut().calculate_requested)
    }

    /// The field a script asked for the keyboard for, drained.
    ///
    /// A `/Fields` position, or `None`. Recorded rather than performed for
    /// the same reason the writes are.
    pub fn take_focus_request(&mut self) -> Option<u32> {
        self.host.borrow_mut().focus_requested.take()
    }

    /// The value the object model currently holds for a field.
    ///
    /// What a script last set through `Field.value`, or what the caller
    /// installed. The caller reads this when applying a write it drained, so
    /// the model and the session agree.
    #[must_use]
    pub fn field_value(&self, index: u32) -> Option<String> {
        let host = self.host.borrow();
        host.document
            .field_at(usize::try_from(index).ok()?)
            .map(|field| field.value.clone())
    }

    /// Tells the object model a field's value changed outside a script.
    ///
    /// A user typing into a field must be visible to the next script that
    /// reads `getField(name).value`, and this crate holds no document to
    /// re-read — so the caller says so, exactly as it says what the value was
    /// at install time.
    pub fn set_field_value(&mut self, index: u32, value: impl Into<String>) {
        let value = value.into();
        let mut host = self.host.borrow_mut();
        if let Ok(index) = usize::try_from(index)
            && let Some(field) = host.document.fields.get_mut(index)
        {
            field.value.clone_from(&value);
        }
        drop(host);
        self.values.insert(index, value);
    }

    /// Installs the `/CO` calculation order — the field indices a calculation
    /// sweep visits, in the order it visits them.
    ///
    /// **An empty order means no calculation runs**, which is the answer for a
    /// document with no `/CO` array and is not a fallback to "every field";
    /// see `pdfrum_doc`'s `Form::calculation_order`.
    pub fn set_calculation_order(&mut self, order: Vec<u32>) {
        self.order = order;
    }

    /// How deep a calculation may nest, for a caller building a
    /// [`FieldWrites`].
    #[must_use]
    pub fn max_calculate_depth(&self) -> u32 {
        self.max_calculate_depth
    }

    /// Records this session's stops as diagnostics on the caller's sink, and
    /// hands back what each of them said.
    ///
    /// Kept separate from [`ScriptCascade::run`] so a caller decides when
    /// diagnostics are drained, and so the cascade methods — whose signatures
    /// take no `Diagnostics` — can still be honest about what happened.
    ///
    /// # Why it returns the failures rather than only recording them
    ///
    /// [`Diagnostic`](pdfrum_common::Diagnostic) is a *kind*, a severity and a
    /// byte offset — a bounded sink a hostile file must not be able to grow —
    /// so it can say **that** a script threw but not *which* one or *what it
    /// said*. The kind goes on the sink and the detail comes back here.
    ///
    /// The session is drained: a second call answers nothing.
    pub fn drain_diagnostics(&mut self, diags: &mut Diagnostics) -> Vec<ScriptFailure> {
        let failures: Vec<ScriptFailure> = self.stops.drain(..).collect();
        for failure in &failures {
            diags.record(
                pdfrum_common::Severity::Suspicious,
                match failure.stop {
                    ScriptStop::LimitReached => pdfrum_common::DiagKind::ScriptLimitReached,
                    ScriptStop::Threw(_) => pdfrum_common::DiagKind::ScriptFailed,
                },
                None,
            );
        }
        failures
    }

    /// Reads `event.rc` back.
    ///
    /// The slot is a `bool` and the setter coerced on the way in, so
    /// `event.rc = 'boo'` reads back `true` — the oracle's behaviour,
    /// reproduced rather than tightened.
    fn event_rc(&self) -> bool {
        self.host.borrow().event.rc
    }

    /// Reads `event.value` back.
    fn event_value(&self) -> String {
        self.host.borrow().event.value.clone()
    }

    /// Reads `event.change` back.
    fn event_change(&self) -> String {
        self.host.borrow().event.change.clone()
    }

    /// Reads the two selection indices back.
    fn event_selection(&self) -> (i32, i32) {
        let host = self.host.borrow();
        (host.event.sel_start, host.event.sel_end)
    }

    /// Runs one field's `/AA` script for a trigger that carries no value and
    /// reads nothing back — the six mouse and focus entries.
    ///
    /// **The event is a full one**, `targetName` and the modifier flags and
    /// all: a script on `/AA /D` reads `event.name == "Mouse Down"` and
    /// `event.target.value`, and `Doc.submitForm` is permitted from it
    /// because a mouse-down *is* a user gesture. What such a trigger cannot
    /// do is change the value: `event.value` throws
    /// `Object no longer exists.`, which is `has_value` being false.
    fn run_pointer_trigger(
        &mut self,
        field: &FieldRef,
        kind: event::EventKind,
        pointer: PointerModifiers,
    ) -> bool {
        let Some(source) = self.script_for(field, Trigger::Pointer(kind)) else {
            return true;
        };
        let mut live = EventState::initialize(kind);
        live.target_name.clone_from(&field.name);
        live.target_index = field.index;
        live.modifier = pointer.modifier;
        live.shift = pointer.shift;
        // Focus and Blur carry the field's value; the four mouse kinds do
        // not (`cjs_event_context.cpp:146-205` — only the two focus
        // functions take a `WideString*`).
        if matches!(kind, event::EventKind::Focus | event::EventKind::Blur) {
            live.has_value = true;
            live.value = self
                .values
                .get(&field.index.unwrap_or(u32::MAX))
                .cloned()
                .unwrap_or_default();
        }
        self.run_event(&source, live, &field.name)
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
    /// One of the six that fire on a pointer or the keyboard focus:
    /// `/AA /E`, `/X`, `/D`, `/U`, `/Fo`, `/Bl`.
    Pointer(event::EventKind),
}

/// Whether a modifier key and Shift were held when a pointer event arrived.
///
/// Two `bool`s rather than a bitmask because that is all the `event` object
/// exposes: `event.modifier` and `event.shift`, each read-only. Private
/// because [`Cascade::pointer`] takes the session's own [`crate::Modifiers`]
/// and this is what it narrows to.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct PointerModifiers {
    /// `event.modifier` — Ctrl on Windows, Command elsewhere.
    pub modifier: bool,
    /// `event.shift`.
    pub shift: bool,
}

impl Cascade for ScriptCascade {
    /// The keystroke hook: `/AA /K` with `willCommit` false.
    ///
    /// The script may **rewrite `event.change` and move the selection**, and
    /// what it left is what gets applied — only the change and the two
    /// selection indices are read back. A write to `event.value` on this path
    /// compiles, does not throw, and is **discarded**.
    fn keystroke(&mut self, field: &FieldRef, change: Keystroke) -> KeystrokeOutcome {
        let Some(source) = self.script_for(field, Trigger::Keystroke) else {
            return KeystrokeOutcome::Accept(change);
        };
        let mut live = EventState::initialize(event::EventKind::Keystroke);
        live.target_name.clone_from(&field.name);
        live.target_index = field.index;
        live.has_value = true;
        live.value.clone_from(&change.value);
        live.change.clone_from(&change.change);
        live.sel_start = change.selection_start;
        live.sel_end = change.selection_end;
        // Keystroke and Format are the only kinds that set it to 0.
        live.commit_key = 0;
        if !self.run_event(&source, live, &field.name) {
            // A script that threw or ran out of budget did not say "accept".
            return KeystrokeOutcome::Reject;
        }
        if !self.event_rc() {
            return KeystrokeOutcome::Reject;
        }
        let (selection_start, selection_end) = self.event_selection();
        KeystrokeOutcome::Accept(Keystroke {
            change: self.event_change(),
            value: change.value,
            selection_start,
            selection_end,
        })
    }

    /// The same `/AA /K` with a commit imminent, which is the only thing that
    /// differs: `willCommit` is `true` and `event.value` is the whole value
    /// rather than the text before an insertion.
    fn keystroke_commit(&mut self, field: &FieldRef, value: &str) -> bool {
        let Some(source) = self.script_for(field, Trigger::Keystroke) else {
            return true;
        };
        let mut live = EventState::initialize(event::EventKind::Keystroke);
        live.target_name.clone_from(&field.name);
        live.target_index = field.index;
        live.has_value = true;
        live.value = value.to_string();
        live.will_commit = true;
        live.commit_key = 0;
        self.run_event(&source, live, &field.name) && self.event_rc()
    }

    /// `/AA /V`. `event.rc` is the whole answer; a write to `event.value` is
    /// discarded exactly as on the keystroke path.
    fn validate(&mut self, field: &FieldRef, value: &str) -> bool {
        let Some(source) = self.script_for(field, Trigger::Validate) else {
            return true;
        };
        let mut live = EventState::initialize(event::EventKind::Validate);
        live.target_name.clone_from(&field.name);
        live.target_index = field.index;
        live.has_value = true;
        live.value = value.to_string();
        self.run_event(&source, live, &field.name) && self.event_rc()
    }

    /// `/AA /C`, over the whole calculation order.
    ///
    /// **One call runs the entire sweep**: `/CO` is walked and each field
    /// written in turn. The three-way gate is normative — a calculated value
    /// is written **only if** the script did not throw, **and** `event.rc` is
    /// still truthy, **and** the string actually changed.
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
            let mut live = EventState::initialize(event::EventKind::Calculate);
            live.target_name = self.names.get(&index).cloned().unwrap_or_default();
            live.target_index = Some(index);
            live.has_value = true;
            live.value.clone_from(&before);
            // `event.source` is meaningful only here, and it names the field
            // whose change provoked the sweep.
            live.source_name.clone_from(&trigger.name);
            live.source_index = trigger.index;

            let whence = live.target_name.clone();
            if !self.run_event(&source, live, &whence) {
                continue;
            }
            if !self.event_rc() {
                continue;
            }
            let after = self.event_value();
            if after == before {
                continue;
            }
            self.values.insert(index, after.clone());
            writes.set(index, after);
        }
        writes.leave();
    }

    /// One of the six pointer and focus `/AA` entries.
    ///
    /// The trigger decides which script and which `event.name`; the two
    /// modifier flags are all the `event` object exposes of what was held.
    fn pointer(
        &mut self,
        field: &FieldRef,
        trigger: crate::cascade::PointerTrigger,
        held: crate::Modifiers,
    ) {
        use crate::cascade::PointerTrigger;
        let kind = match trigger {
            PointerTrigger::Enter => event::EventKind::MouseEnter,
            PointerTrigger::Exit => event::EventKind::MouseExit,
            PointerTrigger::Down => event::EventKind::MouseDown,
            PointerTrigger::Up => event::EventKind::MouseUp,
            PointerTrigger::Focus => event::EventKind::Focus,
            PointerTrigger::Blur => event::EventKind::Blur,
        };
        // `event.modifier` is the **control** key, not "any modifier":
        // `CPWL_Wnd::IsCTRLpressed` is what every `On*` call passes
        // (`fpdfsdk/formfiller/cffl_interactiveformfiller.cpp`), and
        // `event.shift` is `IsSHIFTpressed`. Alt reaches neither.
        let pointer = PointerModifiers {
            modifier: held.contains(crate::Modifiers::CONTROL),
            shift: held.contains(crate::Modifiers::SHIFT),
        };
        self.run_pointer_trigger(field, kind, pointer);
    }

    /// `/AA /F`.
    ///
    /// **The write reaches the appearance only, never `/V`.** `event.value`
    /// is bound to a *local* string; what the script leaves there is drawn,
    /// and re-running with no formatter reverts the appearance to the raw
    /// value.
    ///
    /// Format also **hard-codes `willCommit = true`** and leaves `rc` unbound,
    /// so a Format script's `event.rc` writes reach nothing.
    fn format(&mut self, field: &FieldRef, value: &str) -> Option<String> {
        let source = self.script_for(field, Trigger::Format)?;
        let mut live = EventState::initialize(event::EventKind::Format);
        live.target_name.clone_from(&field.name);
        live.target_index = field.index;
        live.has_value = true;
        live.value = value.to_string();
        // Hard-coded, not inherited.
        live.will_commit = true;
        live.commit_key = 0;
        if !self.run_event(&source, live, &field.name) {
            return None;
        }
        let formatted = self.event_value();
        (formatted != value).then_some(formatted)
    }
}

#[cfg(test)]
mod tests;
