//! PDF/A conversion (ISO 19005), parts 3 and 4.
//!
//! [`check`](pdfrum_doc::pdfa::check) reports what a document fails; this
//! applies the repairs. The checker's [`Clause`](crate::PdfaClause) enum is
//! the work list, and [`Policy`](crate::PdfaPolicy) decides what the caller
//! will trade for conformance.
//!
//! The conversion lives here because it writes: it needs `pdfrum-edit`'s
//! overlay and the writer, and the facade's own `Metadata`. The ICC profile
//! it embeds comes from the `moxcms` already in `pdfrum-page` (§6).
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

pub(crate) use convert::convert;
