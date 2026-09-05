//! The one conversion that turns any failure into a JavaScript `Error`.
//!
//! There is deliberately no per-function string anywhere in this crate. A
//! caller catches an `Error`, reads `.message` for the facade's own words and
//! `.code` for the number to branch on, and both come from here.

use wasm_bindgen::JsValue;

/// Anything a binding function can fail with.
///
/// The boundary's own `Result` error, exactly as `pdfrum_capi::Failure` is:
/// the facade already carries a code and a message, and [`Failure::Argument`]
/// is the one kind it never produces because it is about the JavaScript call
/// rather than the PDF.
#[derive(Debug)]
pub enum Failure {
    /// The facade said no.
    Facade(pdfrum::Error),
    /// The caller's arguments were not usable: a scale that is not a positive
    /// finite number, an index past the end, a field that is not in the form.
    Argument(&'static str),
}

impl From<pdfrum::Error> for Failure {
    fn from(error: pdfrum::Error) -> Failure {
        Failure::Facade(error)
    }
}

/// The code a caller reads off a thrown `Error`.
///
/// The facade's [`pdfrum::ErrorCode`] number for a facade failure — `Io` 1,
/// `Open` 2, `WrongPassword` 3, `Read` 4, `Render` 5, `Doc` 6, `Save` 7,
/// `Text` 8, `Limit` 9 — and 100 for an argument this layer rejected, which is
/// the same pair of scales `pdfrum.h` uses (`PDFRUM_CODE_*`). One table, two
/// bindings.
const ARGUMENT: u32 = 100;

impl Failure {
    /// The `.code` and `.message` this failure throws with.
    fn parts(&self) -> (u32, String) {
        match self {
            Failure::Facade(error) => (u32::from(error.code()), error.to_string()),
            Failure::Argument(what) => (ARGUMENT, (*what).to_owned()),
        }
    }
}

/// The binding's result type.
pub type Result<T> = core::result::Result<T, Failure>;

impl From<Failure> for JsValue {
    /// Builds the JavaScript `Error` a failed call throws.
    ///
    /// A real `Error`, not a string: a caller writes `catch (e)` and gets
    /// `e.message`, `e.stack` and `instanceof Error` for free. The `code`
    /// property is set on it afterwards, which is how a JavaScript library
    /// carries a machine-readable kind beside a human-readable message.
    fn from(failure: Failure) -> JsValue {
        let (code, message) = failure.parts();
        let error = js_sys::Error::new(&message);
        // `Reflect::set` on an `Error` cannot fail: the target is a live
        // object this function just made and the key is a plain string. The
        // result is dropped rather than unwrapped because this crate denies
        // `unwrap`, and because a binding that threw while building a throw
        // would be worse than one whose `.code` is absent.
        let _ = js_sys::Reflect::set(&error, &JsValue::from_str("code"), &JsValue::from(code));
        error.into()
    }
}
