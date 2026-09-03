//! `app.setTimeOut` and `app.setInterval`, and the clock a host steps them by.
//!
//! # There is no wall clock here, and that is the design
//!
//! A timer fires when a **caller says time passed**, through
//! [`ScriptCascade::advance_time`](super::ScriptCascade::advance_time). This
//! library starts no thread, installs no callback and reads no clock: a host
//! with a real event loop drives it from its own, and a test drives it from a
//! number. That is exactly the shape the oracle's own tests use —
//! `EmbedderTestTimerHandlingDelegate` keeps a fake elapsed count and an
//! expiry-keyed multimap, and `AdvanceTime(msecs)` is the whole of what makes
//! a timer fire (`testing/embedder_test_timer_handling_delegate.h`).
//!
//! Upstream's production timers are the embedder's:
//! `FPDF_FORMFILLINFO::FFI_SetTimer` is a host callback, and `GlobalTimer`
//! only records the id it returns. So "the host owns the clock" is not a
//! simplification of PDFium — it is PDFium's arrangement, with the registry
//! made per-session instead of process-wide.
//!
//! # Four rules that are not obvious, all from `global_timer.cpp`
//!
//! 1. **A timer is re-armed before it fires**, not after. `AdvanceTime`
//!    removes the due entry, reinserts it at `now + interval`, and only then
//!    calls it — so a callback that cancels its own timer cancels the *next*
//!    firing, and a callback that runs for a long time does not drift.
//! 2. **A timer fires at most once per `advance_time` call.** The reinsertion
//!    is at `now + interval` where `now` is the *whole* increment already
//!    applied, so a 1000 ms interval advanced by 5000 ms fires once, not five
//!    times. The oracle's tests say so in a comment and prove it by calling
//!    `AdvanceTime(1000)` five times over.
//! 3. **A one-shot with `dwTimeOut == 0` never runs its script.**
//!    `CJS_App::TimerProc` guards on `!IsOneShot() || GetTimeOut() > 0`, and
//!    `setTimeOut` passes the interval as *both* the elapse and the timeout —
//!    so `app.setTimeOut(s, 0)` arms a timer that fires, is cancelled, and
//!    runs nothing.
//! 4. **A timer already being processed does not re-enter.** `Trigger`
//!    returns immediately when `processing_` is set, which is what stops a
//!    script that advances time from inside a timer callback.

use std::collections::BTreeMap;

/// Which kind of timer, which decides what happens after it fires.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TimerKind {
    /// `app.setTimeOut` — cancelled the moment it fires.
    OneShot,
    /// `app.setInterval` — re-armed for another interval.
    Repeating,
}

/// One armed timer.
#[derive(Debug, Clone)]
pub(crate) struct Timer {
    /// The id `app.clearTimeOut`/`clearInterval` cancels it by, and the value
    /// the returned `TimerObj`'s `timeOut` property carries.
    pub(crate) id: i32,
    /// One-shot or repeating.
    pub(crate) kind: TimerKind,
    /// The script source, run whole each time it fires.
    pub(crate) script: String,
    /// How long between firings, in milliseconds. Never below one, because a
    /// zero interval would make `advance_time` a loop that never ends.
    pub(crate) interval: u64,
    /// Whether a one-shot's script runs at all — `dwTimeOut > 0`, which is
    /// rule 3 above. Always true for an interval.
    pub(crate) runs: bool,
    /// When it is next due, in milliseconds of session-elapsed time.
    pub(crate) due: u64,
    /// Whether a firing is already in progress. Re-entry is refused, not
    /// queued.
    pub(crate) processing: bool,
}

/// Every timer a session has armed, and how much time it has been told
/// passed.
#[derive(Debug, Clone, Default)]
pub(crate) struct Timers {
    /// By id, so cancelling is a lookup and firing order is decided by `due`
    /// with the id as the tie-break — which makes the order deterministic
    /// where a multimap's equal keys would not be.
    armed: BTreeMap<i32, Timer>,
    /// Milliseconds a caller has said elapsed since the session began.
    elapsed: u64,
    /// The last id handed out. Ids are never reused, so a stale
    /// `clearTimeOut` cancels nothing rather than something else.
    last_id: i32,
    /// Whether the **next** `set` refuses to arm anything.
    ///
    /// `SetFailNextTimer` in the oracle's delegate, which models a host that
    /// cannot give out another timer. `Bug679649` is the regression: the
    /// script arms a timer, the host refuses, `clearTimeOut` is called on the
    /// refusal, and nothing must fire or crash.
    fail_next: bool,
}

impl Timers {
    /// Arms a timer and answers its id, or **`0`** when the host refused.
    ///
    /// Zero is upstream's `kInvalidTimerID`: `GlobalTimer` keeps itself out of
    /// the registry when it gets one, so the timer object a script holds is
    /// real and cancels nothing.
    pub(crate) fn set(&mut self, kind: TimerKind, script: String, interval: i32) -> i32 {
        if std::mem::take(&mut self.fail_next) {
            return 0;
        }
        // A one-shot's script runs only when its timeout is positive, and
        // `setTimeOut` passes the same number as both. A negative interval is
        // an `int32` widened to `uint32` upstream, which is an enormous delay
        // — here it saturates to "never due within a session", which is the
        // same observable answer without the wrap.
        let runs = kind == TimerKind::Repeating || interval > 0;
        let interval = u64::try_from(interval)
            .unwrap_or(u64::from(u32::MAX))
            .max(1);
        self.last_id = self.last_id.saturating_add(1);
        let id = self.last_id;
        self.armed.insert(
            id,
            Timer {
                id,
                kind,
                script,
                interval,
                runs,
                due: self.elapsed.saturating_add(interval),
                processing: false,
            },
        );
        id
    }

    /// Cancels one timer by id. An id that names nothing is not an error —
    /// `GlobalTimer::Cancel` returns at its first line for a missing entry.
    pub(crate) fn cancel(&mut self, id: i32) {
        self.armed.remove(&id);
    }

    /// Makes the next [`set`](Self::set) refuse.
    pub(crate) fn fail_next(&mut self) {
        self.fail_next = true;
    }

    /// Everything a session has armed, as `(script, interval_ms)`.
    pub(crate) fn listed(&self) -> Vec<(String, i32)> {
        self.armed
            .values()
            .map(|timer| {
                (
                    timer.script.clone(),
                    i32::try_from(timer.interval).unwrap_or(i32::MAX),
                )
            })
            .collect()
    }

    /// Advances the clock and answers the scripts that came due, in order.
    ///
    /// Each due timer is **re-armed before it is handed back** (rule 1) and at
    /// `elapsed + interval` after the *whole* increment (rule 2), so one call
    /// fires each timer at most once. A one-shot is removed rather than
    /// re-armed, and one whose script does not run is removed with nothing
    /// handed back.
    ///
    /// The scripts are returned rather than run here because running one needs
    /// the engine, and holding this borrow across an `eval` would be a
    /// re-entrant `RefCell` borrow — the same reason every other host side
    /// effect in this module is a value the caller spends.
    pub(crate) fn advance(&mut self, millis: u64) -> Vec<(i32, String)> {
        self.elapsed = self.elapsed.saturating_add(millis);
        let now = self.elapsed;
        let mut due: Vec<(u64, i32)> = self
            .armed
            .values()
            .filter(|timer| timer.due <= now && !timer.processing)
            .map(|timer| (timer.due, timer.id))
            .collect();
        // Earliest first, and by id where two share an expiry — the oracle's
        // multimap is keyed by expiry alone and leaves ties to insertion
        // order, which is the same order ids are handed out in.
        due.sort_unstable();

        let mut ready = Vec::new();
        for (_, id) in due {
            let Some(timer) = self.armed.get_mut(&id) else {
                continue;
            };
            let runs = timer.runs;
            let one_shot = timer.kind == TimerKind::OneShot;
            let script = timer.script.clone();
            if one_shot {
                self.armed.remove(&id);
            } else {
                timer.due = now.saturating_add(timer.interval);
            }
            if runs {
                ready.push((id, script));
            }
        }
        ready
    }

    /// Marks a timer as firing, so a nested `advance_time` skips it.
    ///
    /// Answers whether it is still armed: a one-shot has already been removed
    /// by the time its script runs, which is upstream's order too.
    pub(crate) fn begin(&mut self, id: i32) {
        if let Some(timer) = self.armed.get_mut(&id) {
            timer.processing = true;
        }
    }

    /// The other half of [`begin`](Self::begin). A timer the script cancelled
    /// meanwhile is simply gone.
    pub(crate) fn end(&mut self, id: i32) {
        if let Some(timer) = self.armed.get_mut(&id) {
            timer.processing = false;
        }
    }
}
