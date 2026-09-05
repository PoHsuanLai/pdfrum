//! The option objects a caller passes in, and the cancellation flag.
//!
//! Options are exported structs with a `constructor`, not plain JavaScript
//! objects: a plain object reaches TypeScript as `any` and reaches Rust as a
//! `JsValue` this crate would have to reflect fields out of one string at a
//! time. A class gives the caller `new OpenOptions()` with typed setters and
//! gives the binding a Rust struct — the same trade `pdfrum_limits` and
//! `pdfrum_save_options` make in the C header, where a zeroed struct is the
//! default.

use std::sync::Arc;

use wasm_bindgen::prelude::wasm_bindgen;

/// A flag a caller raises to stop work in progress.
///
/// The one handle that may be touched while another call is running — indeed
/// that is the only way it is useful. On the web that means from a
/// `setTimeout`, a message from the main thread to a worker, or an abort
/// button's click handler.
///
/// **This is the only time limit this binding has.** `Instant::now()` panics
/// on `wasm32-unknown-unknown`, so the facade's `Deadline::after` — a
/// wall-clock budget the library polls — cannot be built here at all. A host
/// that wants a time limit sets its own timer and calls
/// [`Cancel::stop`][Self::stop] when it fires, which is what
/// `Deadline::manual` is for and is exactly as good: the deadline is checked
/// at the same points either way.
#[wasm_bindgen]
#[derive(Debug, Clone)]
pub struct Cancel {
    /// Shared with every document opened under it, and with every clone of
    /// this handle, so raising the flag reaches a render already in progress.
    /// `Clone` is what lets an `OpenOptions` carry one without taking the
    /// caller's away.
    deadline: Arc<pdfrum::Deadline>,
}

impl Default for Cancel {
    fn default() -> Cancel {
        Cancel::new()
    }
}

#[wasm_bindgen]
impl Cancel {
    /// A fresh flag, not yet raised.
    #[wasm_bindgen(constructor)]
    #[must_use]
    pub fn new() -> Cancel {
        Cancel {
            deadline: Arc::new(pdfrum::Deadline::manual()),
        }
    }

    /// Raises the flag.
    ///
    /// Work in progress under any document opened with this flag stops at its
    /// next check and reports a `Limit` failure — `.code === 9`. Raising an
    /// already-raised flag does nothing; there is no way to lower one, because
    /// a caller who wants to work again makes a new flag.
    pub fn stop(&self) {
        self.deadline.stop();
    }
}

impl Cancel {
    /// The deadline a document opened under this flag runs on.
    pub(crate) fn deadline(&self) -> pdfrum::Deadline {
        pdfrum::Deadline::clone(&self.deadline)
    }
}

/// The limits and cancellation a document runs under.
///
/// Every field is optional and `undefined` means "as if I had not asked",
/// which is the same contract a zeroed `pdfrum_limits` has in C.
#[wasm_bindgen(getter_with_clone)]
#[derive(Debug, Default)]
pub struct OpenOptions {
    /// The largest render any page of this document will attempt, in pixels.
    /// A render past it fails with `.code === 9` rather than allocating.
    #[wasm_bindgen(js_name = maxRenderPixels)]
    pub max_render_pixels: Option<f64>,
    /// A flag from `new Cancel()`. Raising it stops work in progress.
    ///
    /// Taken by value because wasm-bindgen moves an owned handle into the
    /// call: pass a fresh `new Cancel()` per document, or keep your own
    /// reference by constructing one per open. The flag's own state is shared
    /// internally, so the caller's handle stays usable.
    pub cancel: Option<Cancel>,
}

#[wasm_bindgen]
impl OpenOptions {
    /// Options that ask for nothing: no pixel cap, no cancellation.
    #[wasm_bindgen(constructor)]
    #[must_use]
    pub fn new() -> OpenOptions {
        OpenOptions::default()
    }
}

impl OpenOptions {
    /// The facade options these describe.
    pub(crate) fn to_facade(&self, password: Option<&str>) -> pdfrum::OpenOptions {
        let mut options = pdfrum::OpenOptions {
            password: password.map(|password| password.as_bytes().to_vec()),
            ..pdfrum::OpenOptions::default()
        };
        // `as` on a checked finite non-negative `f64` truncates toward zero,
        // and the guard keeps it in `u64`. A caller who passes a fraction or a
        // negative gets the facade's own default rather than a saturating
        // surprise.
        if let Some(pixels) = self.max_render_pixels
            && pixels.is_finite()
            && (1.0..1.8e19).contains(&pixels)
        {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            {
                options.limits.max_render_pixels = Some(pixels as u64);
            }
        }
        options.limits.deadline = self.cancel.as_ref().map(Cancel::deadline);
        options
    }
}

/// How a page is rendered.
#[wasm_bindgen(getter_with_clone)]
#[derive(Debug, Default)]
pub struct RenderOptions {
    /// Draw the document's annotations and form widgets on top of the page
    /// content. Off by default, which is the facade's own default and what a
    /// thumbnail wants.
    #[wasm_bindgen(js_name = withAnnotations)]
    pub with_annotations: bool,
}

#[wasm_bindgen]
impl RenderOptions {
    /// Page content only, no annotations.
    #[wasm_bindgen(constructor)]
    #[must_use]
    pub fn new() -> RenderOptions {
        RenderOptions::default()
    }
}

/// How a save is written.
#[wasm_bindgen(getter_with_clone)]
#[derive(Debug, Default)]
pub struct SaveOptions {
    /// Write the same bytes for the same input every time: no timestamps, and
    /// a file identifier derived from a fixed seed rather than the clock.
    /// What a test or a content-addressed store wants — and, on this target,
    /// what a caller gets anyway for the clock half, since there is no clock.
    pub deterministic: bool,
    /// Drop the document's encryption, writing the result in the clear. The
    /// caller decides whether that is allowed; the library reports the
    /// document's permissions and does not enforce them.
    #[wasm_bindgen(js_name = removeSecurity)]
    pub remove_security: bool,
}

/// The seed `SaveOptions.deterministic` uses.
///
/// The same shape of constant `pdfrum-capi` has, and deliberately a different
/// value: the two bindings are two products, and a byte-for-byte match between
/// their deterministic saves is not something either promises.
const DETERMINISTIC_SEED: [u8; 16] = *b"pdfrum-wasm-det\0";

#[wasm_bindgen]
impl SaveOptions {
    /// A save with the library's defaults: not deterministic, security kept.
    #[wasm_bindgen(constructor)]
    #[must_use]
    pub fn new() -> SaveOptions {
        SaveOptions::default()
    }
}

impl SaveOptions {
    /// The facade options these describe.
    pub(crate) fn to_facade(&self) -> pdfrum::SaveOptions {
        pdfrum::SaveOptions {
            id_source: if self.deterministic {
                pdfrum::IdSource::Fixed(DETERMINISTIC_SEED)
            } else {
                pdfrum::IdSource::default()
            },
            remove_security: self.remove_security,
            ..pdfrum::SaveOptions::default()
        }
    }
}
