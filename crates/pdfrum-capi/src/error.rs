//! The error out-parameter, the C error enum, and the two typed helpers every
//! boundary function is written in terms of.

use core::ffi::c_char;

use crate::str::{CString, pdfrum_free};

/// Why a call failed.
///
/// One variant per [`pdfrum::ErrorCode`], with the same number, so the C enum
/// and the Rust one cannot drift: its conversion from the facade is a total match and a
/// new facade variant is a compile error here. The numbers are the facade's
/// and are documented there; nothing in this library uses a magic integer for
/// a failure kind.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum pdfrum_code {
    /// No failure. Never stored in a filled `pdfrum_error`; it is the value a
    /// zeroed one has, so a caller who forgot to check the return value still
    /// reads a defined code.
    Ok = 0,
    /// Reading or writing a file failed.
    Io = 1,
    /// The bytes are not a PDF, or are damaged past recovery.
    Open = 2,
    /// The document is encrypted and the password given does not open it.
    WrongPassword = 3,
    /// An object could not be read from the file.
    Read = 4,
    /// Rendering the page failed.
    Render = 5,
    /// The document's structure is not what the operation needs.
    Doc = 6,
    /// Writing the document out failed.
    Save = 7,
    /// Text extraction failed.
    Text = 8,
    /// A limit in `pdfrum_limits` — pixels, time, or a parser bound — was
    /// reached. A cancelled operation reports this code.
    Limit = 9,
    /// An SVG handed to the library would not resolve. Only reachable in a
    /// build whose facade carries the `svg-ingest` feature; the number is the
    /// facade's either way.
    Svg = 10,
    /// A pointer this library requires was null, an index was out of range, or
    /// a string was not UTF-8. The caller's contract was not met; the library
    /// answered rather than crashed.
    Argument = 100,
}

impl pdfrum_code {
    /// The C code for a facade error.
    ///
    /// A total match rather than a numeric cast, so adding a variant to
    /// [`pdfrum::ErrorCode`] fails to compile here instead of silently
    /// widening the header's enum.
    fn from_facade(code: pdfrum::ErrorCode) -> pdfrum_code {
        match code {
            pdfrum::ErrorCode::Io => pdfrum_code::Io,
            pdfrum::ErrorCode::Open => pdfrum_code::Open,
            pdfrum::ErrorCode::WrongPassword => pdfrum_code::WrongPassword,
            pdfrum::ErrorCode::Read => pdfrum_code::Read,
            pdfrum::ErrorCode::Render => pdfrum_code::Render,
            pdfrum::ErrorCode::Doc => pdfrum_code::Doc,
            pdfrum::ErrorCode::Save => pdfrum_code::Save,
            pdfrum::ErrorCode::Text => pdfrum_code::Text,
            pdfrum::ErrorCode::Limit => pdfrum_code::Limit,
            pdfrum::ErrorCode::Svg => pdfrum_code::Svg,
            // `ErrorCode` is `#[non_exhaustive]`, so this arm is required and
            // cannot be dropped. A variant added upstream lands here rather
            // than breaking the build of a released header; it reports as
            // `Doc`, the facade's "the document is not what this needs" code,
            // and the message still says what actually happened. Clippy sees
            // one body twice and cannot see that the two arms are reached for
            // different reasons.
            #[allow(clippy::match_same_arms)]
            _ => pdfrum_code::Doc,
        }
    }
}

/// A failed call's code and message.
///
/// The caller allocates this — on its stack is the usual place — and passes
/// its address as the trailing argument of a fallible function. On failure the
/// library sets `code` and stores a heap-allocated, NUL-terminated UTF-8
/// `message`; on success it touches neither field.
///
/// `message` is owned by the caller once it is set and is freed by
/// `pdfrum_error_free`, which is safe to call on a zeroed or already-freed
/// error.
#[repr(C)]
#[derive(Debug)]
pub struct pdfrum_error {
    /// Why the call failed. `PDFRUM_CODE_OK` in an error nothing has failed
    /// into, so a caller who zeroed the struct reads a defined value.
    pub code: pdfrum_code,
    /// The message, or null when nothing failed.
    pub message: *mut c_char,
}

/// Any failure a boundary function can report.
///
/// The boundary's own `Result`: the facade's [`pdfrum::Error`] carries a code
/// and a message already, and [`Failure::Argument`] is the one kind the
/// facade never produces because it is about the C call itself.
#[derive(Debug)]
pub enum Failure {
    /// The facade said no.
    Facade(pdfrum::Error),
    /// The caller's arguments were not usable: a null pointer where one is
    /// required, an index past the end, or a string that is not UTF-8.
    Argument(&'static str),
}

impl From<pdfrum::Error> for Failure {
    fn from(error: pdfrum::Error) -> Failure {
        Failure::Facade(error)
    }
}

impl Failure {
    /// The code and message this failure reports to C.
    fn parts(&self) -> (pdfrum_code, String) {
        match self {
            Failure::Facade(error) => (pdfrum_code::from_facade(error.code()), error.to_string()),
            Failure::Argument(what) => (pdfrum_code::Argument, (*what).to_owned()),
        }
    }
}

/// The boundary's result type.
pub type Result<T> = core::result::Result<T, Failure>;

/// Writes a failure into a caller's error slot, if it gave one.
///
/// The only place in the crate that stores into a `pdfrum_error`, so the
/// "message is a `CString` freed by `pdfrum_error_free`" invariant has one
/// site to hold.
///
/// # Safety
///
/// `out` is null or points to a writable `pdfrum_error`.
unsafe fn store(out: *mut pdfrum_error, failure: &Failure) {
    if out.is_null() {
        return;
    }
    let (code, message) = failure.parts();
    // SAFETY: `out` is non-null and the caller's contract says it is a
    // writable `pdfrum_error`.
    unsafe {
        (*out).code = code;
        (*out).message = CString::new(&message).into_raw();
    }
}

/// Runs a fallible boundary body, reporting any failure through `out`.
///
/// Every fallible `extern "C"` function in this crate is one call to this or
/// to [`with_handle`], which is what keeps the null check, the error fill and
/// the failure return in one place rather than repeated forty times.
///
/// # Safety
///
/// `out` is null or points to a writable `pdfrum_error`.
pub(crate) unsafe fn catch<T>(
    out: *mut pdfrum_error,
    body: impl FnOnce() -> Result<T>,
) -> Option<T> {
    match body() {
        Ok(value) => Some(value),
        Err(failure) => {
            // SAFETY: forwarded from this function's own contract.
            unsafe { store(out, &failure) };
            None
        }
    }
}

/// Borrows a handle, reporting a null pointer as [`pdfrum_code::Argument`].
///
/// The typed core of the boundary: `ptr` is a `*const T` for the crate's own
/// handle newtype `T`, never a `void *` cast by convention, so a
/// `pdfrum_page *` cannot be passed where a `pdfrum_document *` belongs
/// without the Rust signature saying so.
///
/// # Safety
///
/// `ptr` is null or a pointer this library returned for a live `T` that has
/// not been closed, and no other thread is using it for the duration of the
/// call. `out` is null or points to a writable `pdfrum_error`.
pub(crate) unsafe fn with_handle<T, R>(
    ptr: *const T,
    out: *mut pdfrum_error,
    body: impl FnOnce(&T) -> Result<R>,
) -> Option<R> {
    // SAFETY: forwarded from this function's own contract.
    unsafe {
        catch(out, || {
            let handle = ptr.as_ref().ok_or(Failure::Argument("null handle"))?;
            body(handle)
        })
    }
}

/// [`with_handle`] for a call that mutates its handle.
///
/// # Safety
///
/// As [`with_handle`], and the caller has exclusive use of `ptr` for the
/// duration of the call.
pub(crate) unsafe fn with_handle_mut<T, R>(
    ptr: *mut T,
    out: *mut pdfrum_error,
    body: impl FnOnce(&mut T) -> Result<R>,
) -> Option<R> {
    // SAFETY: forwarded from this function's own contract.
    unsafe {
        catch(out, || {
            let handle = ptr.as_mut().ok_or(Failure::Argument("null handle"))?;
            body(handle)
        })
    }
}

/// Frees the message a failed call stored in an error.
///
/// Sets `message` back to null, so calling this twice on the same error is
/// defined and does nothing the second time. A null `error`, or one whose
/// `message` is already null, is a no-op — an error that no call ever failed
/// into needs no cleanup, and a caller need not track which.
///
/// # Safety
///
/// `error` is null or points to a `pdfrum_error` this library filled, whose
/// `message` has not already been freed by any means other than this function.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pdfrum_error_free(error: *mut pdfrum_error) {
    if error.is_null() {
        return;
    }
    // SAFETY: `error` is non-null and the caller's contract says it points to
    // a `pdfrum_error` this library filled.
    unsafe {
        let message = (*error).message;
        (*error).message = core::ptr::null_mut();
        (*error).code = pdfrum_code::Ok;
        pdfrum_free(message.cast());
    }
}
