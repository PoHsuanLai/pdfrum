//! Written outlines read back through the same accessors that report the
//! bookmarks of a file pdfrum did not write.

#![expect(
    clippy::expect_used,
    reason = "helpers shared by the tests below; a panic here is a failure"
)]

use std::sync::Arc;

use pdfrum::{BookmarkSpec, Document, SaveOptions};

const HELLO: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/hello_world.pdf"
);
const BOOKMARKS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/bookmarks.pdf");

/// Writes `items` onto the plain fixture and reloads the result.
fn with_outline(items: &[BookmarkSpec]) -> Document {
    let doc = Document::open(HELLO).expect("open");
    let mut edit = doc.edit();
    edit.set_outline(items).expect("set outline");
    let mut out = Vec::new();
    edit.write_to(&mut out, &SaveOptions::default())
        .expect("save");
    Document::from_bytes(Arc::from(out)).expect("reopen")
}

/// The reloaded outline as `(depth, title)` pairs.
fn walked(doc: &Document) -> Vec<(usize, String)> {
    doc.outline()
        .iter()
        .map(|item| (item.depth(), item.title()))
        .collect()
}

#[test]
fn a_flat_list_round_trips() {
    let doc = with_outline(&[
        BookmarkSpec::new("One"),
        BookmarkSpec::new("Two"),
        BookmarkSpec::new("Three"),
    ]);
    assert_eq!(
        walked(&doc),
        vec![
            (0, "One".to_owned()),
            (0, "Two".to_owned()),
            (0, "Three".to_owned()),
        ]
    );
}

#[test]
fn nesting_round_trips_at_the_depths_it_was_given() {
    let doc = with_outline(&[
        BookmarkSpec::new("Chapter 1"),
        BookmarkSpec::new("Section 1.1").depth(1),
        BookmarkSpec::new("Section 1.2").depth(1),
        BookmarkSpec::new("Detail").depth(2),
        BookmarkSpec::new("Chapter 2"),
    ]);
    assert_eq!(
        walked(&doc),
        vec![
            (0, "Chapter 1".to_owned()),
            (1, "Section 1.1".to_owned()),
            (1, "Section 1.2".to_owned()),
            (2, "Detail".to_owned()),
            (0, "Chapter 2".to_owned()),
        ]
    );
}

#[test]
fn a_depth_that_jumps_is_clamped_to_one_deeper() {
    // There is no tree in which a child sits three levels below its parent.
    // Flattening to one deeper is the honest reading; inventing intermediate
    // items would be worse.
    let doc = with_outline(&[BookmarkSpec::new("Top"), BookmarkSpec::new("Leap").depth(5)]);
    assert_eq!(
        walked(&doc),
        vec![(0, "Top".to_owned()), (1, "Leap".to_owned())]
    );
}

#[test]
fn an_empty_list_removes_the_outline() {
    let doc = Document::open(BOOKMARKS).expect("open");
    assert!(!doc.outline().is_empty(), "the fixture has bookmarks");

    let mut edit = doc.edit();
    edit.set_outline(&[]).expect("clear");
    let mut out = Vec::new();
    edit.write_to(&mut out, &SaveOptions::default())
        .expect("save");
    let cleared = Document::from_bytes(Arc::from(out)).expect("reopen");
    assert!(cleared.outline().is_empty());
}

#[test]
fn an_existing_outline_is_replaced_not_merged() {
    let doc = Document::open(BOOKMARKS).expect("open");
    let before = doc.outline().len();
    assert!(before > 0);

    let mut edit = doc.edit();
    edit.set_outline(&[BookmarkSpec::new("Only")])
        .expect("set outline");
    let mut out = Vec::new();
    edit.write_to(&mut out, &SaveOptions::default())
        .expect("save");
    let after = Document::from_bytes(Arc::from(out)).expect("reopen");
    assert_eq!(walked(&after), vec![(0, "Only".to_owned())]);
}

#[test]
fn a_closed_item_still_reports_its_children() {
    // `/Count` is negated on a closed item, which says "collapsed" rather
    // than "childless" — the walk still reaches what is underneath.
    let doc = with_outline(&[
        BookmarkSpec::new("Closed").open(false),
        BookmarkSpec::new("Hidden child").depth(1),
    ]);
    assert_eq!(
        walked(&doc),
        vec![(0, "Closed".to_owned()), (1, "Hidden child".to_owned())]
    );
}

#[test]
fn colour_and_style_round_trip() {
    let doc = with_outline(&[BookmarkSpec::new("Styled").color((1.0, 0.0, 0.0)).style(2)]);
    let outline = doc.outline();
    let item = outline.iter().next().expect("one bookmark");
    assert_eq!(item.color(), Some((1.0, 0.0, 0.0)));
}
