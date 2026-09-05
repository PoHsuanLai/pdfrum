//! When work must stop: a flag a host raises, and — where the target has a
//! clock — a budget that raises it for you.
//!
//! The engine has no threads to interrupt and no signal to catch: a deadline
//! is honoured by the loops that already have a natural granularity — the
//! content interpreter per batch of operators, the rasterizer per object, the
//! extractor and the page loader per page, the cross-reference rebuild per
//! chunk of tokens, the script engine per run — each asking
//! [`Deadline::passed`] at that boundary and stopping. The reads are cheap
//! (one atomic load, and one `Instant::now()` only while a budget is set)
//! and happen only when a deadline is set; the default path pays one branch.
//!
//! **The flag is the mechanism and the clock is a convenience**, because
//! `std::time::Instant::now()` aborts at runtime on `wasm32-unknown-unknown`
//! and the facade builds for that target. A host there stops the work from
//! its own timer or event with [`Deadline::stop`]; a native host usually
//! writes [`Deadline::after`], which is the same flag plus a clock read at
//! every check.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use std::time::Instant;

use crate::{LimitExceeded, PageIndex};

/// When work must stop. Cloning shares the flag: a host keeps one clone to
/// raise and hands the other to [`Limits`](crate::Limits).
///
/// ```
/// use pdfrum_common::Deadline;
///
/// // The mechanism: a flag raised from anywhere, on every target.
/// let stop = Deadline::manual();
/// let limit = stop.clone();
/// assert!(!limit.passed());
/// stop.stop();
/// assert!(limit.passed());
/// ```
///
/// ```
/// # #[cfg(not(target_arch = "wasm32"))] {
/// use std::time::Duration;
/// use pdfrum_common::Deadline;
///
/// // The convenience, where there is a clock: a budget from now.
/// let deadline = Deadline::after(Duration::from_secs(5));
/// assert!(!deadline.passed());
/// assert_eq!(deadline.budget(), Some(Duration::from_secs(5)));
///
/// // A zero budget has passed before it is asked.
/// assert!(Deadline::after(Duration::ZERO).passed());
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct Deadline {
    /// Raised by [`Deadline::stop`]. Shared by every clone.
    stop: Arc<AtomicBool>,
    /// The budget, where one was set. Always `None` on `wasm32`, whose
    /// clock constructors do not exist, so no code path there reads a clock.
    clock: Option<Clock>,
}

/// A budget counted from the moment it was made. Stored as a start and a
/// budget rather than as one `Instant`, so the message can say how much
/// time was allowed and so no arithmetic on `Instant` can overflow.
#[derive(Debug, Clone, Copy)]
struct Clock {
    start: Instant,
    budget: Duration,
}

impl Deadline {
    /// A deadline nothing but [`Deadline::stop`] raises.
    #[must_use]
    pub fn manual() -> Deadline {
        Deadline {
            stop: Arc::new(AtomicBool::new(false)),
            clock: None,
        }
    }

    /// A deadline `budget` from now. It can still be raised early with
    /// [`Deadline::stop`].
    ///
    /// Not available on `wasm32`, which has no monotonic clock to read: a
    /// host there uses [`Deadline::manual`] and its own timer.
    #[cfg(not(target_arch = "wasm32"))]
    #[must_use]
    pub fn after(budget: Duration) -> Deadline {
        Deadline {
            stop: Arc::new(AtomicBool::new(false)),
            clock: Some(Clock {
                start: Instant::now(),
                budget,
            }),
        }
    }

    /// The deadline at `instant`; one already in the past has passed.
    ///
    /// Not available on `wasm32`, as [`Deadline::after`].
    #[cfg(not(target_arch = "wasm32"))]
    #[must_use]
    pub fn at(instant: Instant) -> Deadline {
        let start = Instant::now();
        Deadline {
            stop: Arc::new(AtomicBool::new(false)),
            clock: Some(Clock {
                start,
                budget: instant.saturating_duration_since(start),
            }),
        }
    }

    /// Raises the flag: every check from now on, on every clone, answers
    /// that the deadline has passed. Idempotent, and callable from any
    /// thread.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    /// How much time was allowed, for a deadline made with a budget.
    #[must_use]
    pub fn budget(&self) -> Option<Duration> {
        self.clock.map(|clock| clock.budget)
    }

    /// Whether the flag is raised or the budget spent. One atomic load, and
    /// a clock read only while the flag is down and a budget is set.
    #[must_use]
    pub fn passed(&self) -> bool {
        self.stop.load(Ordering::Relaxed) || self.spent_budget().is_some()
    }

    /// `Ok` while the budget lasts and the flag is down; past either, the
    /// error that names what was being done. The page, when there is one,
    /// is the caller's to add with [`LimitExceeded::on_page`].
    ///
    /// # Errors
    ///
    /// [`LimitExceeded::Stopped`] once [`Deadline::stop`] was called,
    /// [`LimitExceeded::Time`] once a budget is spent.
    pub fn check(&self, during: Operation) -> Result<(), LimitExceeded> {
        if self.stop.load(Ordering::Relaxed) {
            return Err(LimitExceeded::Stopped { during, page: None });
        }
        match self.spent_budget() {
            Some(budget) => Err(LimitExceeded::Time {
                budget,
                during,
                page: None,
            }),
            None => Ok(()),
        }
    }

    /// The budget, if there is one and it is spent. On `wasm32` there is
    /// never one, so only the flag can pass a deadline there.
    fn spent_budget(&self) -> Option<Duration> {
        self.clock
            .filter(|clock| clock.start.elapsed() >= clock.budget)
            .map(|clock| clock.budget)
    }
}

/// Two deadlines are equal when they share a flag — clones of one another —
/// which is the question a caller comparing two `Limits` is asking.
impl PartialEq for Deadline {
    fn eq(&self, other: &Deadline) -> bool {
        Arc::ptr_eq(&self.stop, &other.stop)
    }
}

impl Eq for Deadline {}

/// What the engine was doing when a deadline passed — the noun in the
/// message, and the boundary at which the check sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Operation {
    /// Opening the document: the header and cross-reference read, including
    /// the rebuild scan of a damaged file.
    Open,
    /// Loading one page's dictionary — the per-page boundary a walk over a
    /// document crosses.
    PageLoad,
    /// Interpreting a content stream into page objects.
    Interpret,
    /// Rasterizing the page objects.
    Render,
    /// Extracting text.
    Extract,
    /// Running a document script.
    Script,
}

impl Operation {
    /// The message's verb phrase, with the page where one is known.
    pub(crate) fn describe(self, page: Option<PageIndex>) -> String {
        let what = match self {
            Operation::Open => "opening the document",
            Operation::PageLoad => "loading page",
            Operation::Interpret => "interpreting page",
            Operation::Render => "rendering page",
            Operation::Extract => "extracting text from page",
            Operation::Script => "running a script",
        };
        match (self, page) {
            (Operation::Open | Operation::Script, _) | (_, None) => what.to_string(),
            (_, Some(page)) => format!("{what} {page}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Deadline, Operation};
    use crate::{LimitExceeded, PageIndex};
    use std::time::{Duration, Instant};

    #[test]
    fn a_manual_deadline_passes_only_when_stopped_and_every_clone_sees_it() {
        let stop = Deadline::manual();
        let held = stop.clone();
        assert!(!held.passed());
        assert_eq!(held.check(Operation::Open), Ok(()));
        assert_eq!(held.budget(), None);
        stop.stop();
        assert!(held.passed());
        assert_eq!(
            held.check(Operation::Render),
            Err(LimitExceeded::Stopped {
                during: Operation::Render,
                page: None,
            })
        );
        // Idempotent.
        stop.stop();
        assert!(stop.passed());
    }

    #[test]
    fn equality_is_sharing_a_flag() {
        let a = Deadline::manual();
        assert_eq!(a, a.clone());
        assert_ne!(a, Deadline::manual());
    }

    #[test]
    fn a_zero_budget_has_passed_and_an_hour_has_not() {
        assert!(Deadline::after(Duration::ZERO).passed());
        assert!(!Deadline::after(Duration::from_hours(1)).passed());
    }

    #[test]
    fn a_budget_can_still_be_stopped_early() {
        let deadline = Deadline::after(Duration::from_hours(1));
        deadline.stop();
        assert!(deadline.passed());
        assert!(matches!(
            deadline.check(Operation::Extract),
            Err(LimitExceeded::Stopped { .. })
        ));
    }

    #[test]
    fn an_instant_already_reached_has_passed() {
        let now = Instant::now();
        let deadline = Deadline::at(now);
        assert!(deadline.passed());
        assert_eq!(deadline.budget(), Some(Duration::ZERO));
        let later = Deadline::at(now + Duration::from_hours(1));
        assert!(!later.passed());
        assert!(later.budget() > Some(Duration::from_secs(3599)));
    }

    #[test]
    fn the_check_names_the_operation_and_leaves_the_page_to_the_caller() {
        let deadline = Deadline::after(Duration::from_secs(5));
        assert_eq!(deadline.check(Operation::Render), Ok(()));
        let spent = Deadline::after(Duration::ZERO);
        let error = spent.check(Operation::Render).expect_err("spent");
        assert_eq!(
            error,
            LimitExceeded::Time {
                budget: Duration::ZERO,
                during: Operation::Render,
                page: None,
            }
        );
        assert_eq!(
            error.on_page(PageIndex::from(2u32)),
            LimitExceeded::Time {
                budget: Duration::ZERO,
                during: Operation::Render,
                page: Some(PageIndex::from(2u32)),
            }
        );
    }

    #[test]
    fn descriptions_take_a_page_only_where_one_makes_sense() {
        let page = Some(PageIndex::from(3u32));
        assert_eq!(Operation::Open.describe(page), "opening the document");
        assert_eq!(Operation::Script.describe(page), "running a script");
        assert_eq!(Operation::Render.describe(page), "rendering page 3");
        assert_eq!(Operation::Render.describe(None), "rendering page");
        assert_eq!(Operation::PageLoad.describe(page), "loading page 3");
        assert_eq!(Operation::Interpret.describe(page), "interpreting page 3");
        assert_eq!(
            Operation::Extract.describe(page),
            "extracting text from page 3"
        );
    }
}
