//! A link annotation with no `/AP` is drawn as its border, and a border of
//! width zero is not drawn (ISO 32000-2 §12.5.2, §12.5.4): the usual way to
//! make a link invisible. With neither `/Border` nor `/BS` the PDF 1.x default
//! `[0 0 1]` applies.

#![allow(clippy::expect_used)]

use std::fmt::Write as _;
use std::sync::Arc;

use pdfrum::{Document, RenderOptions, VelloCpuBackend};

/// One 200 x 100 page with four links side by side, each 40 wide.
fn four_links() -> Vec<u8> {
    let link = |x0: u32, extra: &str| {
        format!(
            "<< /Type /Annot /Subtype /Link /Rect [{x0} 20 {} 80] {extra} >>",
            x0 + 40
        )
    };
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] \
         /Annots [4 0 R 5 0 R 6 0 R 7 0 R] >>"
            .to_owned(),
        link(5, "/Border [0 0 0]"),
        link(55, "/BS << /W 0 >>"),
        link(105, ""),
        link(155, "/Border [0 0 4]"),
    ];
    let mut pdf = String::from("%PDF-1.7\n");
    let mut xref = String::new();
    write!(xref, "xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).expect("string");
    for (index, body) in objects.iter().enumerate() {
        writeln!(xref, "{:010} 00000 n ", pdf.len()).expect("string");
        writeln!(pdf, "{} 0 obj\n{body}\nendobj", index + 1).expect("string");
    }
    let start = pdf.len();
    pdf += &xref;
    write!(
        pdf,
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{start}\n%%EOF\n",
        objects.len() + 1
    )
    .expect("string");
    pdf.into_bytes()
}

#[test]
fn a_zero_width_border_is_not_drawn_and_the_default_and_explicit_widths_are() {
    let doc = Document::from_bytes(Arc::from(four_links())).expect("open");
    let page = doc.page(0).expect("page");
    let pixmap = page
        .render_with(VelloCpuBackend, &RenderOptions::scaled(1.0))
        .expect("render");
    let inked = |x: u32| {
        let [r, g, b, _] = pixmap.pixel(x, 50).expect("in the page");
        (r, g, b) != (255, 255, 255)
    };
    assert!(!inked(5), "/Border [0 0 0] draws nothing");
    assert!(!inked(55), "/BS << /W 0 >> draws nothing");
    assert!(inked(105), "no border keys: the default width of one draws");
    assert!(inked(155) && inked(158), "/Border [0 0 4] is four wide");
    assert!(!inked(160), "and no wider");
}
