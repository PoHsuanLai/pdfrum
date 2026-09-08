//! The cancellation flag a host raises to stop a long operation.

/// A flag a host raises to stop work already in progress.
///
/// A distinct newtype rather than a bare pointer to [`pdfrum::Deadline`], so
/// the boundary's signatures say which handle each function takes.
///
/// The handle is **thread-safe on purpose** — that is what it is for. A worker
/// renders while another thread calls `pdfrum_cancel_stop`, and the render
/// fails with `PDFRUM_CODE_LIMIT` at its next check. It is the one handle in
/// this header that may be used from several threads at once.
#[derive(Debug)]
pub struct pdfrum_cancel(pdfrum::Deadline);

impl pdfrum_cancel {
    /// The deadline this flag stands for, cloned for an `OpenOptions`.
    ///
    /// [`pdfrum::Deadline`] shares its flag across clones, so the copy an
    /// open keeps and the handle C holds raise together.
    pub(crate) fn deadline(&self) -> pdfrum::Deadline {
        self.0.clone()
    }

    /// This flag, plus a budget of `ms` milliseconds.
    ///
    /// Built *from* this flag rather than beside it — the returned deadline
    /// shares the flag, so it answers to both the timer and
    /// [`pdfrum_cancel_stop`]. A free-standing `Deadline::after` would carry a
    /// flag no C handle can raise, and since the facade holds one deadline,
    /// naming both a `cancel` and a `time_limit_ms` would then silently lose
    /// one of them.
    pub(crate) fn with_time_limit(&self, ms: u64) -> pdfrum::Deadline {
        self.0.with_budget(std::time::Duration::from_millis(ms))
    }

    /// A budget with no flag beside it, for a `pdfrum_limits` that names a
    /// `time_limit_ms` and no `cancel`.
    pub(crate) fn time_limit_only(ms: u64) -> pdfrum::Deadline {
        pdfrum::Deadline::after(std::time::Duration::from_millis(ms))
    }
}

/// A new cancellation flag, unraised.
///
/// The caller owns the returned handle and frees it with
/// [`pdfrum_cancel_free`] — which is safe to call while a document opened with
/// the flag is still alive, because the flag itself is reference-counted
/// inside.
///
/// Returns null only if the allocation fails.
#[unsafe(no_mangle)]
pub extern "C" fn pdfrum_cancel_new() -> *mut pdfrum_cancel {
    Box::into_raw(Box::new(pdfrum_cancel(pdfrum::Deadline::manual())))
}

/// Raises the flag.
///
/// Every operation running under a `pdfrum_limits` that names this flag stops
/// at its next check and fails with `PDFRUM_CODE_LIMIT`. The flag stays raised:
/// there is no reset, and a host that wants to work again makes a new one.
///
/// A null pointer is a no-op. This is the one function in the header that may
/// be called concurrently with any other on the same handle — indeed it is
/// meant to be called while another thread renders.
///
/// # Safety
///
/// `cancel` is null or a live handle from [`pdfrum_cancel_new`] that has not
/// been freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pdfrum_cancel_stop(cancel: *const pdfrum_cancel) {
    // SAFETY: the caller's contract says `cancel` is null or a live handle.
    if let Some(cancel) = unsafe { cancel.as_ref() } {
        cancel.0.stop();
    }
}

/// Frees a cancellation flag.
///
/// Safe to call while documents opened under the flag are still open: those
/// hold their own reference to the shared flag, and freeing this handle only
/// gives up C's. A null pointer is a no-op.
///
/// # Safety
///
/// `cancel` is null or a live handle from [`pdfrum_cancel_new`] that has not
/// already been freed, and no other thread is using it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pdfrum_cancel_free(cancel: *mut pdfrum_cancel) {
    if cancel.is_null() {
        return;
    }
    // SAFETY: the caller's contract says `cancel` is a live, unfreed handle
    // from `pdfrum_cancel_new`, which made it with `Box::into_raw`.
    drop(unsafe { Box::from_raw(cancel) });
}

#[cfg(test)]
mod tests {
    use super::{pdfrum_cancel_free, pdfrum_cancel_new, pdfrum_cancel_stop};

    /// The flag a host raises is the one an operation reads.
    #[test]
    fn stop_raises_the_shared_flag() {
        let handle = pdfrum_cancel_new();
        assert!(!handle.is_null());
        // SAFETY: `handle` is live and this thread has it to itself.
        let deadline = unsafe { (*handle).deadline() };
        assert!(!deadline.passed());
        // SAFETY: same handle, still live.
        unsafe { pdfrum_cancel_stop(handle) };
        assert!(deadline.passed(), "the clone sees the raise");
        // SAFETY: same handle, not yet freed.
        unsafe { pdfrum_cancel_free(handle) };
    }

    /// Null is a no-op in both directions.
    #[test]
    fn null_is_a_no_op() {
        // SAFETY: a null pointer is explicitly allowed by both contracts.
        unsafe {
            pdfrum_cancel_stop(core::ptr::null());
            pdfrum_cancel_free(core::ptr::null_mut());
        }
    }
}
