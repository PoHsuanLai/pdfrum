//! Waiting on `wgpu` without an async runtime.
//!
//! `wgpu` 29's `request_adapter` and `request_device` are async and its
//! `map_async` is callback-shaped, while this crate is a synchronous
//! rasterizer behind a synchronous trait. The ecosystem's answer is
//! `pollster`, which DEPS.md's closed set does not contain and which is not
//! worth a `[spec]` change for forty lines — STYLE.md §5 says to write the
//! forty lines.
//!
//! Two primitives, and the distinction between them is a deadlock rather than
//! a slowdown:
//!
//! - [`block_on`] spins on a future that completes on its own. Correct for
//!   adapter and device requests, which resolve on the calling thread.
//! - [`poll_until`] drives the *device* and checks a condition between polls.
//!   Required for anything awaiting GPU progress, because `map_async`'s
//!   callback fires from inside `Device::poll` — on this very thread. A
//!   blocking receive here would park the only thread that can deliver the
//!   thing it is waiting for.
//!
//! Both are bounded. `wgpu`'s own `wait_indefinitely` is the obvious spelling
//! and it is the wrong one for a benchmark: a lost device or a wedged driver
//! would hang the suite forever, and PLAN.md §M12c's guardrails say a GPU test
//! must degrade rather than hang.
//!
//! Bounded is not the same as spinning, and only [`block_on`] spins. It has to:
//! a plain future has no object to wait *on*, so the loop re-polls, and the
//! futures it drives — adapter and device requests — resolve on the calling
//! thread in microseconds. [`poll_until`] waits for a GPU, which takes as long
//! as a page takes, so it hands the driver a bounded [`POLL_SLICE`] and is
//! parked for it rather than burning a core.

use std::future::Future;
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

use crate::wgpu;

/// How long either primitive waits before giving up.
///
/// Thirty seconds is far past any legitimate page render on this hardware —
/// the slowest corpus document is under a second wall-clock — and far short of
/// "the suite appears to have died". A caller that hits it gets an error it
/// can report, which is the whole point.
const TIMEOUT: Duration = Duration::from_secs(30);

/// Drive a future that needs no device progress to completion.
///
/// `None` if it had not finished within [`TIMEOUT`].
///
/// Only [`crate::request_adapter`] awaits a future at all — the borrowing
/// constructor receives a device that is already open.
pub fn block_on<F: Future>(fut: F) -> Option<F::Output> {
    // A no-op waker: the loop re-polls unconditionally rather than waiting to
    // be woken, so there is nothing for a wake to signal.
    let mut cx = Context::from_waker(Waker::noop());
    let mut fut = std::pin::pin!(fut);
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Poll::Ready(value) = fut.as_mut().poll(&mut cx) {
            return Some(value);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::yield_now();
    }
}

/// How long one `poll` blocks before the loop checks the clock again.
///
/// `PollType::Wait` with a timeout is `wgpu` 29's bounded wait: it parks the
/// thread inside the driver until the submission completes *or* this elapses,
/// and returns either way. That is the whole reason the slice exists — an
/// indefinite wait would never come back to check [`TIMEOUT`], and
/// `PollType::Poll`, which returns immediately, turns the loop into a busy
/// spin that burns a core for as long as the GPU takes.
///
/// Ten milliseconds is short enough that [`TIMEOUT`] is honoured to within one
/// slice and long enough that a page-sized render costs a handful of wakeups
/// rather than millions. Nothing is measured on it; it is a bound, not a
/// tuning parameter.
const POLL_SLICE: Duration = Duration::from_millis(10);

/// Poll `device` until `ready` yields a value, or until [`TIMEOUT`].
///
/// The device is polled *first*, before `ready` is consulted, because that is
/// what runs the callbacks `ready` is waiting on. Each poll is a bounded wait
/// of [`POLL_SLICE`] rather than `wgpu`'s `wait_indefinitely`, which is what
/// makes the outer timeout reachable at all — an indefinite poll never returns
/// to check the clock — and rather than a non-blocking `Poll`, which would
/// reach the clock by spinning on it.
pub fn poll_until<T>(device: &wgpu::Device, mut ready: impl FnMut() -> Option<T>) -> Option<T> {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        // A poll error means the device is gone, or that this slice expired
        // with work outstanding; either way the loop falls through to the
        // deadline rather than spinning on a corpse, and the caller reports a
        // readback failure. STYLE §3 forbids the `unwrap` wgpu's own examples
        // use here.
        let _ = device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(POLL_SLICE),
        });
        if let Some(value) = ready() {
            return Some(value);
        }
        if Instant::now() >= deadline {
            return None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_already_ready_future_returns_immediately() {
        assert_eq!(block_on(std::future::ready(7)), Some(7));
    }

    #[test]
    fn a_future_that_pends_once_still_completes() {
        // The spin loop must re-poll rather than assume the first poll wins,
        // which is exactly the case a `ready` future cannot exercise.
        struct PendOnce(bool);
        impl Future for PendOnce {
            type Output = u8;
            fn poll(mut self: std::pin::Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<u8> {
                if self.0 {
                    Poll::Ready(9)
                } else {
                    self.0 = true;
                    cx.waker().wake_by_ref();
                    Poll::Pending
                }
            }
        }
        assert_eq!(block_on(PendOnce(false)), Some(9));
    }

    #[test]
    fn the_wait_is_bounded_rather_than_indefinite() {
        // The property the guardrails actually ask for: a GPU that never
        // answers must produce a reportable failure, not a hung suite. Proved
        // on the timeout constant rather than by waiting thirty seconds for
        // it, because a test that takes the timeout to pass is the same
        // problem in miniature.
        assert!(TIMEOUT >= Duration::from_secs(5), "long enough for a page");
        assert!(TIMEOUT <= Duration::from_mins(1), "short enough to report");
    }

    #[test]
    fn the_device_wait_is_a_slice_of_the_timeout_not_a_spin() {
        // The property that distinguishes a bounded wait from a busy loop: the
        // slice must be short enough that `TIMEOUT` is still honoured to
        // within one of them, and long enough that a render costs wakeups in
        // the hundreds rather than a saturated core. Asserted on the constants
        // because the alternative is a timing test.
        assert!(POLL_SLICE > Duration::ZERO, "zero would be the spin again");
        assert!(POLL_SLICE <= TIMEOUT / 100, "the timeout stays reachable");
    }
}
