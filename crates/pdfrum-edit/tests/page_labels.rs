//! Written page labels are the labels the reader reports back.

#![expect(
    clippy::expect_used,
    reason = "helpers shared by the tests below; a panic here is a failure"
)]

use std::sync::Arc;

use pdfrum_common::{Diagnostics, Limits};
use pdfrum_doc::page_label;
use pdfrum_edit::{
    EditDoc, PageLabelRange, PageLabelStyle, SaveOptions, add_blank_page, save, set_page_labels,
};
use pdfrum_object::Name;
use pdfrum_parser::{Document, LoadOptions, load};

const HELLO: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/hello_world.pdf"
);

fn blank_doc() -> Document {
    let bytes: Arc<[u8]> = Arc::from(&std::fs::read(HELLO).expect("fixture")[..]);
    load(bytes, &LoadOptions::default()).expect("load")
}

fn roundtrip(edit: &EditDoc<'_>) -> Document {
    let mut out = Vec::new();
    save(edit, &SaveOptions::default(), &mut out).expect("save");
    load(Arc::from(&out[..]), &LoadOptions::default()).expect("reload")
}

/// Every page's label, as the reader answers.
fn labels(doc: &Document, count: u32) -> Vec<Option<String>> {
    let catalog = doc.catalog().expect("a catalog");
    let (limits, mut diags) = (Limits::default(), Diagnostics::default());
    (0..count)
        .map(|page| page_label(&catalog, page, count, doc, &limits, &mut diags))
        .collect()
}

/// A document of `count` pages, the fixture's one plus blanks.
fn with_pages(doc: &Document, count: u32) -> EditDoc<'_> {
    let mut edit = EditDoc::new(doc);
    for index in 1..count {
        add_blank_page(&mut edit, 612.0, 792.0, index).expect("a blank page");
    }
    edit
}

#[test]
fn one_range_labels_every_page_from_its_start() {
    let doc = blank_doc();
    let mut edit = with_pages(&doc, 3);
    set_page_labels(
        &mut edit,
        &[PageLabelRange::new(0u32, PageLabelStyle::LowerRoman)],
    )
    .expect("set");

    let saved = roundtrip(&edit);
    assert_eq!(
        labels(&saved, 3),
        vec![
            Some("i".to_owned()),
            Some("ii".to_owned()),
            Some("iii".to_owned())
        ],
    );
}

#[test]
fn a_second_range_restarts_the_numbering() {
    let doc = blank_doc();
    let mut edit = with_pages(&doc, 5);
    set_page_labels(
        &mut edit,
        &[
            PageLabelRange::new(0u32, PageLabelStyle::LowerRoman),
            PageLabelRange::new(2u32, PageLabelStyle::Decimal),
        ],
    )
    .expect("set");

    let saved = roundtrip(&edit);
    // The body restarts at 1 rather than continuing from the front matter —
    // the whole reason a document has more than one range.
    assert_eq!(
        labels(&saved, 5),
        vec![
            Some("i".to_owned()),
            Some("ii".to_owned()),
            Some("1".to_owned()),
            Some("2".to_owned()),
            Some("3".to_owned()),
        ],
    );
}

#[test]
fn a_prefix_and_a_starting_number_both_reach_the_reader() {
    let doc = blank_doc();
    let mut edit = with_pages(&doc, 3);
    set_page_labels(
        &mut edit,
        &[PageLabelRange::new(0u32, PageLabelStyle::Decimal)
            .prefix("A-")
            .first(7)],
    )
    .expect("set");

    let saved = roundtrip(&edit);
    assert_eq!(
        labels(&saved, 3),
        vec![
            Some("A-7".to_owned()),
            Some("A-8".to_owned()),
            Some("A-9".to_owned())
        ],
    );
}

#[test]
fn a_style_of_none_labels_with_the_prefix_alone() {
    let doc = blank_doc();
    let mut edit = with_pages(&doc, 2);
    set_page_labels(
        &mut edit,
        &[PageLabelRange::new(0u32, PageLabelStyle::None).prefix("Cover")],
    )
    .expect("set");

    let saved = roundtrip(&edit);
    // Every page in the range takes the same label, which is how a run of
    // unnumbered front matter is spelled.
    assert_eq!(
        labels(&saved, 2),
        vec![Some("Cover".to_owned()), Some("Cover".to_owned())],
    );
}

#[test]
fn ranges_may_be_given_out_of_order() {
    let doc = blank_doc();
    let mut edit = with_pages(&doc, 4);
    set_page_labels(
        &mut edit,
        &[
            PageLabelRange::new(2u32, PageLabelStyle::Decimal),
            PageLabelRange::new(0u32, PageLabelStyle::UpperLetters),
        ],
    )
    .expect("set");

    let saved = roundtrip(&edit);
    // A number tree's keys must ascend or the reader's lower-bound scan gives
    // the wrong rule, so the write sorts rather than trusting the caller.
    assert_eq!(
        labels(&saved, 4),
        vec![
            Some("A".to_owned()),
            Some("B".to_owned()),
            Some("1".to_owned()),
            Some("2".to_owned()),
        ],
    );
}

#[test]
fn the_last_range_at_a_starting_page_wins() {
    let doc = blank_doc();
    let mut edit = with_pages(&doc, 2);
    set_page_labels(
        &mut edit,
        &[
            PageLabelRange::new(0u32, PageLabelStyle::LowerRoman),
            PageLabelRange::new(0u32, PageLabelStyle::Decimal),
        ],
    )
    .expect("set");

    let saved = roundtrip(&edit);
    assert_eq!(
        labels(&saved, 2),
        vec![Some("1".to_owned()), Some("2".to_owned())],
    );
}

#[test]
fn an_empty_slice_removes_the_tree() {
    let doc = blank_doc();
    let mut edit = with_pages(&doc, 2);
    set_page_labels(
        &mut edit,
        &[PageLabelRange::new(0u32, PageLabelStyle::LowerRoman)],
    )
    .expect("set");
    set_page_labels(&mut edit, &[]).expect("clear");

    let saved = roundtrip(&edit);
    let catalog = saved.catalog().expect("a catalog");
    assert!(!catalog.contains_key(&Name::from("PageLabels")));
    // The reader answers `None` for a document with no tree at all — its
    // decimal fallback is for a page a *present* tree does not cover, which is
    // a different thing. Either way the document is back to unlabelled.
    assert_eq!(labels(&saved, 2), vec![None, None]);
}
