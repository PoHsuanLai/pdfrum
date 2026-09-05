//! `pdfrum` for the web — the WebAssembly binding over the [`pdfrum`] facade.
//!
//! This crate is a translation layer and nothing else: every export is an
//! opaque handle plus one call into the facade. It holds no parsing, no
//! rendering and no policy. It is the same shape `pdfrum-capi` gives C,
//! said in JavaScript.
//!
//! ```js
//! import init, { Document } from "pdfrum";
//! await init();
//! const doc = Document.open(new Uint8Array(bytes));
//! const page = doc.page(0);
//! const r = page.render(2);
//! ctx.putImageData(new ImageData(r.data, r.width, r.height), 0, 0);
//! page.free();
//! doc.free();
//! ```
//!
//! # Memory
//!
//! **Every handle has a `free()`, and a caller is expected to call it.**
//! JavaScript's garbage collector sees a small wrapper object; it does not
//! see the parsed object store, the font cache and the file bytes that
//! wrapper points at inside the module's linear memory, so it has no reason to
//! collect promptly and no way to know the cost of not doing so. `free()` is
//! wasm-bindgen's own convention for every exported struct, and this binding
//! keeps it rather than inventing a second one.
//!
//! The *values* — [`Word`], [`Link`], [`RenderResult`] and the rest — are
//! plain data that has already been copied out; freeing one is allowed and
//! pointless. The handles whose `free()` matters are [`Document`], [`Page`],
//! [`Form`] and [`Cancel`].
//!
//! A handle keeps its document alive, so the order does not matter: freeing a
//! document while a page of it is still open is defined, and the bytes stay
//! until the last handle goes.
//!
//! # Errors
//!
//! A failed call **throws a JavaScript `Error`** whose `message` is the
//! facade's own words and whose `code` is a number to branch on: the facade's
//! [`pdfrum::ErrorCode`] — `Io` 1, `Open` 2, `WrongPassword` 3, `Read` 4,
//! `Render` 5, `Doc` 6, `Save` 7, `Text` 8, `Limit` 9 — plus 100 for an
//! argument this layer rejected. The same two scales `pdfrum.h` names
//! `PDFRUM_CODE_*`.
//!
//! ```js
//! try {
//!   Document.open(bytes, "wrong");
//! } catch (e) {
//!   if (e.code === 3) console.log("bad password:", e.message);
//! }
//! ```
//!
//! # Threads
//!
//! There are none. A WebAssembly module without the threads proposal runs on
//! one thread, and this binding compiles the facade with no thread pool: a
//! handle is used from wherever its module is. Two workers each want their own
//! module and their own copy of the bytes.
//!
//! # Time
//!
//! `Instant::now()` panics on `wasm32-unknown-unknown`, so there is no
//! wall-clock limit in this binding and no `timeLimitMs` option. The one
//! stopping mechanism is [`Cancel`], the host-set flag, which a caller raises
//! from its own `setTimeout` or abort handler. The facade checks it at exactly
//! the points it would check a clock.
//!
//! # Features
//!
//! The facade is compiled with `vello-cpu`, `codecs-all`, `forms`, `edit` and
//! `markdown`. `system-fonts` is off because a browser has no host font
//! directory — the bundled base-14 faces carry text — and `javascript` is off
//! because running a document's own script in a page is a different security
//! proposition from rendering it. `Cargo.toml` says the same at more length.

mod document;
mod error;
mod form;
mod options;
mod page;
mod value;

pub use document::Document;
pub use form::Form;
pub use options::{Cancel, OpenOptions, RenderOptions, SaveOptions};
pub use page::Page;
pub use value::{
    Attachment, Bookmark, Field, FieldKind, Hit, ImageInfo, Link, Metadata, RenderResult, Word,
};

use wasm_bindgen::prelude::wasm_bindgen;

/// The library's version, as `Cargo.toml` declares it.
///
/// The same string `pdfrum_version()` returns from the C library, from the
/// same source.
#[wasm_bindgen]
#[must_use]
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_owned()
}
