//! A minimal worker pool for corpus-wide jobs.
//!
//! Both `generate-goldens` and `run` are embarrassingly parallel over ~1400
//! independent files whose work is dominated by a subprocess. `rayon` is a
//! library-ring dependency reserved for parallel page rendering in the facade
//!, so the harness uses `std::thread` with a shared index: a dozen
//! lines, no dependency, and the ordering guarantee we actually need — results
//! come back keyed by input index and are sorted before use, so output does
//! not depend on scheduling.

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Runs `job` over every input, returning results in input order.
///
/// `workers` is clamped to at least one and at most the number of inputs.
/// A job that panics takes the process down rather than being swallowed: in
/// this harness a panicking job means the harness itself is broken, and the
/// per-file failure channel is `Result`, not unwinding.
pub fn map<In, Out, F>(inputs: &[In], workers: usize, job: F) -> Vec<Out>
where
    In: Sync,
    Out: Send,
    F: Fn(&In) -> Out + Sync,
{
    if inputs.is_empty() {
        return Vec::new();
    }
    let workers = workers.clamp(1, inputs.len());
    let next = AtomicUsize::new(0);
    let slots: Vec<Mutex<Option<Out>>> = inputs.iter().map(|_| Mutex::new(None)).collect();
    let job = &job;
    let slots_ref = &slots;
    let next_ref = &next;

    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(move || {
                loop {
                    let index = next_ref.fetch_add(1, Ordering::Relaxed);
                    let Some(input) = inputs.get(index) else {
                        break;
                    };
                    let output = job(input);
                    if let Some(slot) = slots_ref.get(index)
                        && let Ok(mut guard) = slot.lock()
                    {
                        *guard = Some(output);
                    }
                }
            });
        }
    });

    slots
        .into_iter()
        .filter_map(|slot| slot.into_inner().ok().flatten())
        .collect()
}

/// A sensible default worker count: available parallelism, capped so a huge
/// machine does not fork a hundred oracle processes at once.
pub fn default_workers() -> usize {
    std::thread::available_parallelism()
        .map_or(4, std::num::NonZeroUsize::get)
        .clamp(1, 32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn results_come_back_in_input_order() {
        let inputs: Vec<u32> = (0..200).collect();
        let doubled = map(&inputs, 8, |n| n * 2);
        assert_eq!(doubled, inputs.iter().map(|n| n * 2).collect::<Vec<_>>());
    }

    #[test]
    fn order_holds_even_when_jobs_finish_out_of_order() {
        // Early items sleep longest, so completion order is reversed.
        let inputs: Vec<u64> = (0..16).collect();
        let out = map(&inputs, 8, |n| {
            std::thread::sleep(std::time::Duration::from_millis(16 - n));
            *n
        });
        assert_eq!(out, inputs);
    }

    #[test]
    fn every_input_is_visited_exactly_once() {
        let seen = Mutex::new(Vec::new());
        let inputs: Vec<u32> = (0..500).collect();
        let out = map(&inputs, 16, |n| {
            if let Ok(mut guard) = seen.lock() {
                guard.push(*n);
            }
            *n
        });
        assert_eq!(out.len(), 500);
        let mut visited = seen.into_inner().unwrap();
        visited.sort_unstable();
        assert_eq!(visited, (0..500).collect::<Vec<u32>>());
    }

    #[test]
    fn an_empty_input_list_does_no_work() {
        let out: Vec<u32> = map(&[], 8, |n| *n);
        assert!(out.is_empty());
    }

    #[test]
    fn a_worker_count_of_zero_still_runs() {
        assert_eq!(map(&[1, 2, 3], 0, |n| n * 10), [10, 20, 30]);
    }

    #[test]
    fn more_workers_than_inputs_is_fine() {
        assert_eq!(map(&[7], 64, |n| *n), [7]);
    }

    #[test]
    fn the_default_worker_count_is_in_range() {
        let workers = default_workers();
        assert!((1..=32).contains(&workers));
    }
}
