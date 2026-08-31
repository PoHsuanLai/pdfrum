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

/// Poll `device` until `ready` yields a value, or until [`TIMEOUT`].
///
/// The device is polled *first*, before `ready` is consulted, because that is
/// what runs the callbacks `ready` is waiting on. Polling with a short bounded
/// wait rather than `wait_indefinitely` is what makes the timeout reachable at
/// all — an indefinite poll never returns to check the clock.
pub fn poll_until<T>(device: &wgpu::Device, mut ready: impl FnMut() -> Option<T>) -> Option<T> {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        // A poll error means the device is gone; the loop then falls through
        // to the deadline rather than spinning on a corpse, and the caller
        // reports a readback failure. STYLE §3 forbids the `unwrap` wgpu's
        // own examples use here.
        let _ = device.poll(wgpu::PollType::Poll);
        if let Some(value) = ready() {
            return Some(value);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::yield_now();
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
}
