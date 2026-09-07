//! PDF/A conversion (ISO 19005), M26 parts 3 and 4.
//!
//! The checker in `pdfrum-doc` reports what a document fails; this repairs it.
//! The two are deliberately separate crates and deliberately in that order,
//! and the checker's [`Clause`](crate::PdfaClause) enum is this module's
//! work list.
//!
//! # Why the conversion is here and the checker is not
//!
//! The checker reads the object graph, which `pdfrum-doc` already has. The
//! conversion **writes**, which needs `pdfrum-edit`'s overlay and the writer,
//! and it needs the facade's own `Metadata`. `pdfrum-doc` depends on neither
//! and should not: the checker ships to a caller who only wants to know, and
//! making it depend on the writer to serve the caller who wants to repair
//! would be the wrong edge. So the conversion sits in the facade beside
//! `flatten` and `stamp`, which are the other write-side document operations,
//! and it takes **no new dependency** — the ICC profile it embeds comes from
//! the `moxcms` already in `pdfrum-page` (§6).
//!
//! # The one-line version
//!
//! ```no_run
//! use pdfrum::{Document, PdfaLevel, PdfaPolicy};
//!
//! let doc = Document::open("in.pdf")?;
//! let outcome = doc.to_pdfa("out.pdf", PdfaLevel::A2b, &PdfaPolicy::lossy())?;
//! if outcome.converted() {
//!     for compromise in &outcome.compromises {
//!         eprintln!("cost: {compromise}");
//!     }
//! }
//! # Ok::<(), pdfrum::Error>(())
//! ```

mod convert;
mod policy;
mod xmp_write;

pub use policy::{Compromise, Concession, Conversion, Dpi, Policy, RasterCause, Refusal};

pub(crate) use convert::convert;
