//! The two corpus files the paragraph rules were tuned against, read the
//! way a caller reads them and held to the Python `MinerU` reference's
//! paragraph count: `text_foxit_products.pdf`, untagged, a sheet of
//! one-line feature items, and `text_quick_start.pdf`, tagged, whose
//! paragraphs come from its structure tree and must not move.

// An integration test is not `#[cfg(test)]`, so the workspace's no-panic
// lints apply here as if this were library code. A corpus file that will
// not open is a broken test, and failing loudly is the point.
#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "a corpus file that will not open must fail loudly"
)]

use pdfrum::Document;
use pdfrum_common::Limits;
use pdfrum_markdown::{Options, page_blocks, render};

/// Every page's Markdown, in order.
fn markdown_of(file: &str) -> String {
    let path = format!("{}/../../benches/corpus/{file}", env!("CARGO_MANIFEST_DIR"));
    let doc = Document::open(&path).expect("the corpus file opens");
    doc.pages()
        .map(|page| {
            let graph = page.objects();
            let tree = page.structure();
            render(&page_blocks(
                &graph,
                tree.as_ref(),
                &doc,
                Options::default(),
                &Limits::default(),
            ))
        })
        .collect()
}

/// Headings and text lines, counted the way the comparison script counts
/// them: a line starting `#` is a heading; any other non-empty line not
/// starting `!`, `-` or `|` is a paragraph.
fn counts(markdown: &str) -> (usize, usize) {
    let headings = markdown.lines().filter(|l| l.starts_with('#')).count();
    let text = markdown
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with(['#', '!', '-', '|']))
        .count();
    (headings, text)
}

#[test]
fn the_products_sheet_keeps_each_feature_item_a_paragraph() {
    let md = markdown_of("text_foxit_products.pdf");
    let (headings, text) = counts(&md);
    // The reference has 8 headings and 225 paragraphs. Seven headings: the
    // eighth, "Business Ready PDF – Robust and Secure", is set like its
    // body. The paragraphs run over: the reference joins a feature's name
    // to the benefit under it on the "Features & Benefits" pages, where
    // each is its own short line here, and a paragraph that crosses a page
    // break is one there and two in a page-at-a-time reading. Both ways of
    // getting it wrong — one paragraph per line, and every item joined
    // into one paragraph — came to 23.
    assert_eq!(headings, 7, "{md}");
    assert!((240..=280).contains(&text), "{text} text lines\n{md}");
    for item in [
        "XFA Form Filling - XFA (XML Form Architecture) form allows you to leverage existing XFA forms.",
        "High Performance - Up to 3 times faster PDF creation from over 200 of the most common office file types and convert multiple files to PDF in a single operation.",
        "Redaction - Lets you permanently remove (redact) visible text and images from PDF documents.",
        "Email and Phone Support - helps when you need it.",
        "System Requirements",
        "Operating Systems",
        "Windows 7 (32-bit & 64-bit).",
    ] {
        assert!(
            md.lines().any(|l| l == item),
            "not a paragraph of its own: {item}\n{md}"
        );
    }
    // A wrapped item still joins, and a sentence after a full line stays in
    // its paragraph.
    assert!(md.contains(
        "Form Design - Easy to use electronic forms design tools to make your office forms work \
         harder. Enables you to create or convert static PDF files into professional looking forms."
    ));
}

#[test]
fn the_guide_keeps_its_tree_paragraphs() {
    let md = markdown_of("text_quick_start.pdf");
    let (headings, text) = counts(&md);
    // The reference finds 19 headings and 90 paragraphs; the tree names 8
    // `H1`s and the rest are bold lines the reference promotes. 85 text
    // lines before this pass, 85 after.
    assert_eq!(headings, 8, "{md}");
    assert!((80..=95).contains(&text), "{text} text lines\n{md}");
}
