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
#[derive(Debug, Default)]
pub(crate) struct HostState {
    /// Every line a script has asked the host to print, in order.
    pub(crate) transcript: Vec<TranscriptLine>,
    /// Timers a script asked for: the script source and its interval in
    /// milliseconds. **Recorded and never fired** — see `app.setTimeOut`.
    /// Per-session, because upstream's registry is process-wide and STYLE §1
    /// forbids that outright.
    pub(crate) timers: Vec<(String, i32)>,
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
