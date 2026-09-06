//! The `svg` feature's facade path: a page opened through [`pdfrum::Document`]
//! and exported with nothing named but `pdfrum::*`.
//!
//! What is asserted is the conversion's output, not that it compiles: the
//! document's `viewBox` is the device box the same page and options render
//! into, and the content is really vectors. A page of rectangles that came
//! back as one embedded `<image>` would mean the walk fell back to pixels
//! wholesale — the failure this export exists to avoid, and the one a
//! compile-only test would not notice.

use pdfrum::{Document, RenderOptions, VelloCpuBackend};

fn open(name: &str) -> pdfrum::Result<Document> {
    Document::open(format!("tests/fixtures/{name}.pdf"))
}

#[test]
fn a_page_exports_to_svg_through_the_facade() {
    let doc = open("rectangles").expect("the fixture opens");
    let page = doc.page(0).expect("it has a first page");
    let options = RenderOptions::default();
    let backend = VelloCpuBackend::new();

    let pixels = page
        .render(&backend, &options)
        .expect("the page rasterizes");
    let converted = page.to_svg(&backend, &options).expect("the page converts");

    assert!(converted.svg.starts_with("<svg "), "a root element");
    assert!(converted.svg.contains("</svg>"), "a closed document");
    // Same coordinates as the raster render, which is what makes an SVG and a
    // pixmap of the same page comparable.
    let view_box = format!("viewBox=\"0 0 {} {}\"", pixels.width(), pixels.height());
    assert!(
        converted.svg.contains(&view_box),
        "the viewBox is the device box ({view_box})"
    );
    // Rectangles are paths, and a page of them needs no fallback at all.
    assert!(converted.svg.contains("<path "), "vector geometry");
    assert!(
        !converted.svg.contains("<image "),
        "nothing was embedded as pixels"
    );
    assert!(
        converted.report.is_empty(),
        "a page of rectangles needs no rasterized region, got {:?}",
        converted.report.counts()
    );
}

/// The prepared-page path is the same conversion on a page interpreted once,
/// so the two must agree byte for byte.
#[test]
fn preparing_first_gives_the_same_document() {
    let doc = open("rectangles").expect("the fixture opens");
    let page = doc.page(0).expect("it has a first page");
    let options = RenderOptions::default();
    let backend = VelloCpuBackend::new();
    let mut session = pdfrum::RenderSession::new();

    let direct = page.to_svg(&backend, &options).expect("converts");
    let prepared = page
        .prepare(&options, &mut session)
        .to_svg_on(&backend, &mut session)
        .expect("converts");
    assert_eq!(direct.svg, prepared.svg);
    assert_eq!(direct.report, prepared.report);
}
