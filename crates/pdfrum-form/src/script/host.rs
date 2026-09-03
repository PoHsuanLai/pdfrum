//! The state a native function reaches, and the two host hooks boa needs.

use std::cell::RefCell;
use std::rc::Rc;

use super::transcript::TranscriptLine;

/// Everything the bound objects write to, behind one handle.
///
/// Stored in boa's `Context` through `insert_data`, so a native function —
/// which is handed `&mut Context` and nothing else — can reach it without a
/// global, a thread local, or a `Copy` closure capture. `Context::get_data`
/// hands back `&T`, hence the `RefCell`: the mutation is one `borrow_mut` at
/// the point of use and never spans a call back into the engine.
#[allow(
    clippy::struct_excessive_bools,
    reason = "each flag is one JavaScript property a script reads and writes \
              — `Doc.delay`, `Doc.dirty`, `app.calculate`, \
              `app.runtimeHighlight`, and the two request flags. They are not \
              a state machine and grouping them into an enum would put names \
              between the binding and the value it answers."
)]
#[derive(Debug)]
pub(crate) struct HostState {
    /// Every line a script has asked the host to print, in order.
    pub(crate) transcript: Vec<TranscriptLine>,
    /// Every timer a script armed, and how much time the caller has said
    /// passed.
    ///
    /// **Per-session, where upstream's registry is process-wide**, and driven
    /// by a caller rather than by a clock — see [`super::timer`].
    pub(crate) timers: super::timer::Timers,
    /// What the `Doc` object answers from, and what a `Field` reads and
    /// writes.
    ///
    /// Installed by the caller rather than read here, because reading it
    /// needs a document and [`ScriptCascade`](super::ScriptCascade)
    /// deliberately holds none. See [`super::model`] for the bargain.
    pub(crate) document: super::model::DocumentModel,
    /// The twelve named `color` slots, by name.
    ///
    /// Per-session variables rather than constants: `color.black = [...]`
    /// succeeds and is read back, so a document that overwrites one has
    /// overwritten it for every script after it.
    pub(crate) colors: std::collections::BTreeMap<String, super::color::Color>,
    /// The `global` property bag, whose deletions are tombstones.
    pub(crate) globals: super::global::Bag,
    /// Icon names `Doc.addIcon` was given, in order.
    ///
    /// Append-only: `addIcon` and `getIcon` are real and `removeIcon` is a
    /// no-op, so the list only grows and duplicates are allowed. The icon's
    /// *contents* are discarded, as upstream discards them — only the name is
    /// kept.
    pub(crate) icon_names: Vec<String>,
    /// Fields a script wrote through `Field.value`, by `/Fields` position.
    ///
    /// A record the host reads back after a script runs, so a value a script
    /// set reaches the session's own state and the appearance regenerates.
    /// The model's own `value` is updated in step, so a later script in the
    /// same run reads what an earlier one wrote.
    pub(crate) field_writes: Vec<(u32, String)>,
    /// Whether a script called `Doc.calculateNow()`.
    ///
    /// A **request**, not a call: running the sweep from inside a native
    /// function would re-enter the cascade the script is already inside, which
    /// the oracle refuses too. The caller reads the flag after the script
    /// returns and sweeps then.
    pub(crate) calculate_requested: bool,
    /// `Doc.baseURL` — pure JavaScript-side state.
    ///
    /// A real read/write property that reaches nothing else — reproducing it
    /// is reproducing a variable, and a golden reads back each of the six
    /// values it is assigned.
    pub(crate) base_url: String,
    /// `Doc.delay` — the document-wide batching flag.
    pub(crate) delay: bool,
    /// `app.calculate` — whether recalculation runs, as `app` reports it.
    ///
    /// Defaults **on**, which is `CPDFSDK_InteractiveForm`'s own initial
    /// state and what `app_properties_expected.txt` reads first.
    pub(crate) app_calculate: bool,
    /// `app.runtimeHighlight` — defaults off.
    pub(crate) app_runtime_highlight: bool,
    /// `Doc.dirty` — the change mark, which a script may set and clear.
    pub(crate) dirty: bool,
    /// The live `event` object's fields, as plain data.
    ///
    /// One per realm and re-`Initialize`d per trigger — see
    /// [`super::event`] for why every field is reset every time.
    pub(crate) event: super::event::EventState,
    /// The field a script asked for the keyboard for, by `/Fields` position.
    ///
    /// Recorded for the same reason: focus is the session's, and moving it
    /// mid-script would re-enter routing.
    pub(crate) focus_requested: Option<u32>,
}

/// The handle the context holds.
pub(crate) type Host = Rc<RefCell<HostState>>;

/// A `HostHooks` that reports a fixed local timezone offset.
///
/// PDFium's test runner sets `TZ=America/Los_Angeles` and freezes the clock at
/// `--time=1399672130`, and **both leak into the expected bytes**:
/// `public_methods_expected.txt` pins `AFParseDateEx(1, 2) = 1399672130000`,
/// and every `util.printd` line is shifted to the Los Angeles offset. So the
/// offset is configuration rather than something read from the machine — a
/// golden run on a machine in another zone must produce the same bytes.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FixedZone {
    /// Seconds east of UTC. `-25200` is `GMT-0700`, PDFium's own.
    pub(crate) offset_secs: i32,
}

impl boa_engine::context::HostHooks for FixedZone {
    fn local_timezone_offset_seconds(&self, _unix_time_seconds: i64) -> i32 {
        self.offset_secs
    }
}

impl Default for HostState {
    /// The state a fresh realm starts in.
    ///
    /// Not derived, because two of the flags default **on** and a derived
    /// `Default` would say otherwise: `app.calculate` reports `true` before
    /// any script touches it, and so does `Doc.calculate`.
    fn default() -> HostState {
        HostState {
            transcript: Vec::new(),
            timers: super::timer::Timers::default(),
            document: super::model::DocumentModel::empty(),
            colors: super::color::initial(),
            globals: super::global::Bag::new(),
            icon_names: Vec::new(),
            field_writes: Vec::new(),
            base_url: String::new(),
            app_calculate: true,
            app_runtime_highlight: false,
            delay: false,
            dirty: false,
            calculate_requested: false,
            focus_requested: None,
            event: super::event::EventState::default(),
        }
    }
}
