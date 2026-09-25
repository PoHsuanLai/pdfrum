//! A new document with nothing on it, for a caller that writes a PDF from
//! scratch rather than editing one.
//!
//! [`EditDoc`](crate::EditDoc) edits a parsed base, so a fresh file starts as
//! a base too: a catalog, a page tree and one empty page per size asked for,
//! written here as bytes and read back by the parser. Drawing then goes
//! through [`EditDoc::draw_pages`](crate::EditDoc::draw_pages) like any other
//! page, and a full save writes the result.

use std::fmt::Write as _;
use std::sync::Arc;

use kurbo::Size;
use pdfrum_parser::{Document, LoadOptions, load};

use crate::Error;

/// A document of blank pages, one per entry of `pages`, each `width` by
/// `height` points and in that order.
///
/// Every page has only `/Type`, `/Parent` and `/MediaBox`, so a canvas drawn
/// on it sees exactly that box as its space. An empty `pages` is a document
/// with an empty page tree.
///
/// # Errors
///
/// [`Error::BadPageSize`] when a size is not finite and positive: a page with
/// no area has no space to draw in.
///
/// ```
/// use pdfrum_edit::{Size, blank_document};
///
/// let doc = blank_document(&[Size::new(595.0, 842.0), Size::new(612.0, 792.0)])?;
/// assert_eq!(doc.page_count(), 2);
/// # Ok::<(), pdfrum_edit::Error>(())
/// ```
pub fn blank_document(pages: &[Size]) -> Result<Document, Error> {
    if let Some(bad) = pages.iter().find(|size| {
        !(size.width.is_finite()
            && size.height.is_finite()
            && size.width > 0.0
            && size.height > 0.0)
    }) {
        return Err(Error::BadPageSize(*bad));
    }
    load(Arc::from(file(pages).into_bytes()), &LoadOptions::default())
        .map_err(|error| Error::BlankDocument(error.to_string()))
}

/// The file: catalog (1), page tree (2), then the pages from 3.
fn file(pages: &[Size]) -> String {
    let kids: Vec<String> = (0..pages.len()).map(|i| format!("{} 0 R", i + 3)).collect();
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        format!(
            "<< /Type /Pages /Count {} /Kids [{}] >>",
            pages.len(),
            kids.join(" ")
        ),
    ];
    objects.extend(pages.iter().map(|size| {
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {} {}] >>",
            pdfrum_object::fmt_number(narrow(size.width)),
            pdfrum_object::fmt_number(narrow(size.height))
        )
    }));
    let mut out = String::from("%PDF-1.7\n%\u{e2}\u{e3}\u{cf}\u{d3}\n");
    let mut offsets = Vec::with_capacity(objects.len());
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        let _ = writeln!(out, "{} 0 obj\n{body}\nendobj", i + 1);
    }
    let xref = out.len();
    let _ = writeln!(out, "xref\n0 {}\n0000000000 65535 f ", objects.len() + 1);
    for offset in offsets {
        let _ = writeln!(out, "{offset:010} 00000 n ");
    }
    let _ = writeln!(
        out,
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF",
        objects.len() + 1
    );
    out
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "PDF numbers are f32; the geometry vocabulary is f64"
)]
fn narrow(value: f64) -> f32 {
    value as f32
}

#[cfg(test)]
mod tests {
    use super::blank_document;
    use crate::Error;
    use kurbo::Size;

    #[test]
    fn every_size_becomes_a_page_in_order() {
        let doc = blank_document(&[Size::new(100.0, 200.0), Size::new(300.5, 50.0)])
            .expect("a blank document parses");
        assert_eq!(doc.page_count(), 2);
    }

    #[test]
    fn a_page_without_area_is_refused() {
        for size in [
            Size::new(0.0, 10.0),
            Size::new(10.0, -1.0),
            Size::new(f64::NAN, 10.0),
        ] {
            assert!(
                matches!(blank_document(&[size]), Err(Error::BadPageSize(_))),
                "{size:?}"
            );
        }
    }

    #[test]
    fn no_pages_is_an_empty_tree() {
        let doc = blank_document(&[]).expect("an empty tree parses");
        assert_eq!(doc.page_count(), 0);
    }
}
