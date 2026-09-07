//! The structured values the binding returns.
//!
//! Every one is a `#[wasm_bindgen]` struct with `getter_with_clone`, not a
//! `serde-wasm-bindgen` blob. The reason is the deliverable: a plain object
//! reaches TypeScript as `any`, and these reach it as named classes with typed
//! fields, so a caller's editor knows that a `Word` has a `text` and a `size`
//! before the code has ever run. It costs one `pub` field per value and buys
//! the `.d.ts` that asks for.
//!
//! They are values, not handles: a caller never frees one. wasm-bindgen still
//! gives each a `free()`, because every exported struct gets one, and the
//! module's memory is reclaimed when the value is garbage-collected either
//! way. The handles — `Document`, `Page`, `Form`, `Cancel` — are the ones
//! whose `free()` matters.

use wasm_bindgen::Clamped;
use wasm_bindgen::prelude::wasm_bindgen;

/// One rendered page: the pixels and the size they are in.
///
/// `data` is RGBA8 with **straight (not premultiplied) alpha**, row-major and
/// top-down — exactly what `ImageData` wants — so a browser caller writes
/// `new ImageData(result.data, result.width, result.height)` and puts it on a
/// canvas with no conversion at all.
#[wasm_bindgen(getter_with_clone)]
#[derive(Debug)]
pub struct RenderResult {
    /// The width in pixels.
    pub width: u32,
    /// The height in pixels.
    pub height: u32,
    /// `width * height * 4` bytes, RGBA8, straight alpha, top-down.
    ///
    /// A `Uint8ClampedArray` rather than a `Uint8Array`, because that is the
    /// type `new ImageData(data, width, height)` requires: handing back the
    /// plain array would make every caller copy it once more for no reason.
    /// wasm-bindgen types the field from `Clamped<Vec<u8>>`.
    pub data: Clamped<Vec<u8>>,
}

/// One word of a page's text, with where it sits and how it was set.
#[wasm_bindgen(getter_with_clone)]
#[derive(Debug)]
pub struct Word {
    /// The word's text.
    pub text: String,
    /// Left edge, in PDF points.
    pub x0: f64,
    /// Bottom edge.
    pub y0: f64,
    /// Right edge.
    pub x1: f64,
    /// Top edge.
    pub y1: f64,
    /// The font size the word was set at, in points.
    pub size: f64,
    /// The first character of the word, as an index into `Page.text()`.
    pub start: usize,
    /// One past the last character, so `end - start` is the length.
    pub end: usize,
    /// The font the word was set in, or `undefined` when the page does not
    /// say.
    pub font: Option<String>,
}

/// One clickable area on a page.
///
/// Exactly one of `uri` and `pageIndex` is set: a link either leaves the
/// document or goes to a page of it. A link that does neither — a launch
/// action, a named destination that resolves to nothing — has both
/// `undefined`.
#[wasm_bindgen(getter_with_clone)]
#[derive(Debug)]
pub struct Link {
    /// Left edge of the clickable area, in PDF points.
    pub x0: f64,
    /// Bottom edge.
    pub y0: f64,
    /// Right edge.
    pub x1: f64,
    /// Top edge.
    pub y1: f64,
    /// The URI this link opens, or `undefined` when it goes to a page of this
    /// document instead.
    pub uri: Option<String>,
    /// The zero-based page this link goes to, or `undefined` when it does not
    /// go to a page of this document.
    #[wasm_bindgen(js_name = pageIndex)]
    pub page_index: Option<u32>,
}

/// One occurrence of a needle in a page's text.
///
/// A half-open character range into the same string `Page.text()` returns, so
/// `text.slice(hit.start, hit.end)` is the match.
#[wasm_bindgen(getter_with_clone)]
#[derive(Debug)]
pub struct Hit {
    /// The first character of the match.
    pub start: usize,
    /// One past the last character.
    pub end: usize,
}

/// One entry of a document's outline, flattened depth-first.
#[wasm_bindgen(getter_with_clone)]
#[derive(Debug)]
pub struct Bookmark {
    /// The entry's text.
    pub title: String,
    /// How deep the entry sits: 0 for a top-level entry, 1 for its children.
    /// The list is depth-first, so a run of deeper entries after one is that
    /// one's subtree.
    pub depth: usize,
    /// The zero-based page the entry goes to, or `undefined` when it does not
    /// name a page of this document.
    #[wasm_bindgen(js_name = pageIndex)]
    pub page_index: Option<u32>,
}

/// One file embedded in a document, with its bytes.
///
/// The bytes are in the value rather than behind a second call, because a
/// JavaScript caller has no buffer to fill and no free to forget: the
/// attachment list is already the moment at which a caller decided it wanted
/// the files.
#[wasm_bindgen(getter_with_clone)]
#[derive(Debug)]
pub struct Attachment {
    /// The name the document files this attachment under.
    pub name: String,
    /// The attachment's own file name.
    #[wasm_bindgen(js_name = fileName)]
    pub file_name: String,
    /// Its description; empty when the document gives none.
    pub description: String,
    /// Its MIME type, or `undefined` when the document does not say.
    pub subtype: Option<String>,
    /// The file's contents, or `undefined` when the document names a file but
    /// carries no stream for it.
    pub data: Option<Vec<u8>>,
}

/// A document's information dictionary.
///
/// Every field is `undefined` when the document does not carry that entry. The
/// two dates are the PDF date strings the file holds, not parsed: a caller who
/// wants a `Date` decides for itself what to do with a malformed one.
#[wasm_bindgen(getter_with_clone)]
#[derive(Debug)]
pub struct Metadata {
    /// `/Title`.
    pub title: Option<String>,
    /// `/Author`.
    pub author: Option<String>,
    /// `/Subject`.
    pub subject: Option<String>,
    /// `/Keywords`.
    pub keywords: Option<String>,
    /// `/Creator`.
    pub creator: Option<String>,
    /// `/Producer`.
    pub producer: Option<String>,
    /// `/CreationDate`, as the PDF date string it is.
    #[wasm_bindgen(js_name = creationDate)]
    pub creation_date: Option<String>,
    /// `/ModDate`, as the PDF date string it is.
    #[wasm_bindgen(js_name = modificationDate)]
    pub modification_date: Option<String>,
}

/// One image drawn on a page, decoded.
#[wasm_bindgen(getter_with_clone)]
#[derive(Debug)]
pub struct ImageInfo {
    /// The image's width in its own pixels, not as drawn on the page.
    pub width: u32,
    /// Its height in its own pixels.
    pub height: u32,
    /// Whether the image is a stencil mask rather than a picture: it paints
    /// the current fill colour through a one-bit shape.
    #[wasm_bindgen(js_name = isMask)]
    pub is_mask: bool,
    /// `width * height * 4` bytes, RGBA8, straight alpha, top-down — the same
    /// `Uint8ClampedArray` shape as [`RenderResult::data`], so the same
    /// `ImageData` line displays it.
    pub data: Clamped<Vec<u8>>,
}

/// The one place `missing_docs` sees generated glue rather than our own items.
///
/// wasm-bindgen's string-enum expansion adds an `__Invalid` variant and an
/// `impl` block of conversions, and documents neither. The allow is on this
/// module rather than on the enum so it reaches the generated `impl` too, and
/// the module holds exactly one item so it cannot silently cover anything
/// else. The lint stays on for every hand-written item in the crate.
#[allow(missing_docs)]
mod field_kind {
    use wasm_bindgen::prelude::wasm_bindgen;

    /// What kind of control a form field is.
    ///
    /// A string union in TypeScript rather than a number, because that is what a
    /// JavaScript caller switches on. The C binding's `PDFRUM_FIELD_KIND_*`
    /// numbers are the same set; the names here are the same names.
    #[wasm_bindgen]
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum FieldKind {
        /// A push button: it carries no value.
        Button = "button",
        /// A check box.
        Checkbox = "checkbox",
        /// One button of a radio group.
        Radio = "radio",
        /// A single- or multi-line text field.
        Text = "text",
        /// A drop-down list.
        Combo = "combo",
        /// A scrolling list.
        List = "list",
        /// A signature field.
        Signature = "signature",
    }
}

pub use field_kind::FieldKind;

impl FieldKind {
    /// The kind a facade field reports.
    ///
    /// A total match rather than a cast: [`pdfrum::FieldKind`] is not
    /// `#[non_exhaustive]`, so a kind added upstream is a compile error here
    /// rather than a silent gap in the string union a caller switches on.
    pub(crate) fn from_facade(kind: pdfrum::FieldKind) -> FieldKind {
        match kind {
            pdfrum::FieldKind::Button => FieldKind::Button,
            pdfrum::FieldKind::Check => FieldKind::Checkbox,
            pdfrum::FieldKind::Radio => FieldKind::Radio,
            pdfrum::FieldKind::Text => FieldKind::Text,
            pdfrum::FieldKind::Combo => FieldKind::Combo,
            pdfrum::FieldKind::List => FieldKind::List,
            pdfrum::FieldKind::Signature => FieldKind::Signature,
        }
    }
}

/// One field of a form, as it stands after every `set` made so far.
#[wasm_bindgen(getter_with_clone)]
#[derive(Debug)]
pub struct Field {
    /// The field's fully-qualified name — what `Form.set` takes.
    pub name: String,
    /// Its value, seen through every `Form.set` made so far.
    pub value: String,
    /// What kind of control the field is.
    pub kind: FieldKind,
    /// Whether a check box or radio button is currently selected. Always
    /// `false` for a field of another kind.
    #[wasm_bindgen(js_name = isChecked)]
    pub is_checked: bool,
    /// Whether the document marks this field as not editable.
    #[wasm_bindgen(js_name = isReadOnly)]
    pub is_read_only: bool,
    /// Whether the document marks this field as one that must be filled.
    #[wasm_bindgen(js_name = isRequired)]
    pub is_required: bool,
}
