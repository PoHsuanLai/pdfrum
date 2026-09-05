//! The two corpus files the paragraph rules were tuned against, read the
//! way a caller reads them and held to the Python `MinerU` reference's
//! paragraph count: `text_foxit_products.pdf`, untagged, a sheet of
//! one-line feature items, and `text_quick_start.pdf`, tagged, whose
//! paragraphs come from its structure tree and must not move. The guide
//! also stands for the figure-to-image link and, read as a document, for
//! the running header and footer it repeats on every page.

// An integration test is not `#[cfg(test)]`, so the workspace's no-panic
// lints apply here as if this were library code. A corpus file that will
// not open is a broken test, and failing loudly is the point.
#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "a corpus file that will not open must fail loudly"
)]

use kurbo::Rect;
use pdfrum::Document;
use pdfrum_common::Limits;
use pdfrum_markdown::{
    Block, Line, Options, PageInput, document_blocks, page_blocks, page_lines, render, running,
};

fn open(file: &str) -> Document {
    let path = format!("{}/../../benches/corpus/{file}", env!("CARGO_MANIFEST_DIR"));
    Document::open(&path).expect("the corpus file opens")
}

/// Every page's Markdown, in order, page by page.
fn markdown_of(file: &str) -> String {
    let doc = open(file);
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

#[test]
fn a_figure_is_the_image_drawn_under_its_marked_content_id() {
    // Page 2 of the guide has thirteen figures. The first is the screenshot
    // Word drew thirteenth on the page (`/P <</MCID 101>> BDC … /Image80
    // Do`), then come the ten 18x19 icons drawn second to eleventh; the
    // bullet drawn first, under a `P`, is no figure. The index is into the
    // facade's list, which walks the page the same way.
    let doc = open("text_quick_start.pdf");
    let page = doc.page(1u32).expect("page 2 loads");
    let graph = page.objects();
    let tree = page.structure();
    let blocks = page_blocks(
        &graph,
        tree.as_ref(),
        &doc,
        Options::default(),
        &Limits::default(),
    );
    let indices: Vec<Option<usize>> = blocks
        .iter()
        .filter_map(|b| match b {
            Block::Image { index, .. } => Some(*index),
            _ => None,
        })
        .collect();
    assert_eq!(indices.len(), 13, "{blocks:?}");
    assert!(indices.iter().all(Option::is_some), "{indices:?}");
    assert_eq!(indices.first().copied().flatten(), Some(12));
    assert_eq!(indices.get(1).copied().flatten(), Some(1));
    let images = page.images();
    assert_eq!(
        images.get(12).map(|i| (i.width, i.height)),
        Some((1203, 705))
    );
    assert_eq!(images.get(1).map(|i| (i.width, i.height)), Some((18, 19)));
}

#[test]
fn the_guides_running_title_and_folio_go_when_it_is_read_as_a_document() {
    let doc = open("text_quick_start.pdf");
    let pages: Vec<_> = doc.pages().collect();
    let graphs: Vec<_> = pages.iter().map(pdfrum::Page::objects).collect();
    let trees: Vec<_> = pages.iter().map(pdfrum::Page::structure).collect();
    let inputs: Vec<PageInput<'_>> = graphs
        .iter()
        .zip(&trees)
        .map(|(page, tree)| PageInput {
            page,
            tree: tree.as_ref(),
        })
        .collect();
    let md: String = document_blocks(&inputs, &doc, Options::default(), &Limits::default())
        .iter()
        .map(|blocks| render(blocks))
        .collect();
    assert!(
        !md.lines().any(|l| l == "Foxit MobilePDF"
            || l == "Quick Guide"
            || l.contains("www.foxitsoftware.com")),
        "{md}"
    );
    assert!(md.contains("# Different Views"), "{md}");

    // The document rule itself, on the lines: the running title's two
    // lines and the `N / 11 www.foxitsoftware.com` folio on all eleven
    // pages, and nothing else.
    let lines: Vec<Vec<Line>> = graphs
        .iter()
        .map(|g| page_lines(g, &doc, Options::default(), &Limits::default()))
        .collect();
    let per_page: Vec<(&[Line], Rect)> = lines
        .iter()
        .zip(&graphs)
        .map(|(l, g)| (l.as_slice(), g.crop_box))
        .collect();
    assert_eq!(per_page.len(), 11);
    for (page, mask) in lines.iter().zip(running::mask(&per_page)) {
        let dropped: Vec<&str> = page
            .iter()
            .zip(&mask)
            .filter(|(_, gone)| **gone)
            .map(|(l, _)| l.text.as_str())
            .collect();
        assert!(dropped.contains(&"Foxit MobilePDF"), "{dropped:?}");
        assert!(dropped.contains(&"Quick Guide"), "{dropped:?}");
        assert!(
            dropped
                .iter()
                .any(|t| t.ends_with("/ 11 www.foxitsoftware.com")),
            "{dropped:?}"
        );
        assert_eq!(dropped.len(), 3, "{dropped:?}");
    }
    // Two pages of it are below the three-page floor: nothing goes.
    let two = per_page.get(1..3).expect("pages 2 and 3");
    assert!(running::mask(two).iter().flatten().all(|gone| !gone));
}
