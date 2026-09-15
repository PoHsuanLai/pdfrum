//! Reordering pages, which the CLI previously faked by building a throwaway
//! document and deleting its template page.

#![expect(
    clippy::expect_used,
    clippy::panic,
    clippy::cast_possible_truncation,
    reason = "helpers shared by the tests below; a panic here is a failure, \
              and the fixtures' page widths are whole numbers that fit"
)]

use std::sync::Arc;

use pdfrum_edit::{EditDoc, SaveOptions, add_blank_page, reorder_pages, save};
use pdfrum_object::{Name, Object, Resolve};
use pdfrum_parser::{Document, LoadOptions, load};

fn hello() -> Document {
    let bytes: Arc<[u8]> = Arc::from(
        &include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/files/hello.pdf"
        ))[..],
    );
    load(bytes, &LoadOptions::default()).expect("the fixture loads")
}

fn roundtrip(edit: &EditDoc<'_>) -> Document {
    let mut out = Vec::new();
    save(edit, &SaveOptions::default(), &mut out).expect("save");
    load(Arc::from(&out[..]), &LoadOptions::default()).expect("reload")
}

/// A document of `n` pages, each a distinct width so order is observable.
///
/// The fixture's own page is 200 wide and stays first; the added ones are
/// 101, 102, … so a walk of widths names the arrangement.
fn numbered(n: u32) -> Document {
    let doc = hello();
    let mut edit = EditDoc::new(&doc);
    for i in 1..n {
        let width = 100.0 + f32::from(u16::try_from(i).unwrap_or(0));
        add_blank_page(&mut edit, width, 200.0, i).expect("add");
    }
    let saved = roundtrip(&edit);
    assert_eq!(
        saved.page_count(),
        n,
        "the fixture page plus the added ones"
    );
    saved
}

/// Each page's width, resolving `/MediaBox` through `/Parent` the way a
/// reader does.
///
/// The fixture's own page inherits its box from the `/Pages` node, which is
/// exactly the case a reorder has to preserve — so the helper has to follow
/// inheritance or it would be testing the wrong thing.
fn widths(doc: &Document) -> Vec<i64> {
    (0..doc.page_count())
        .map(|index| {
            let page = doc.page(index).expect("page");
            let mut dict = page.dict.clone();
            let key = Name::from("MediaBox");
            for _ in 0..8 {
                if let Some(media) = dict.array(&key, doc) {
                    return media.number_at(2).expect("a width") as i64;
                }
                let Some(Object::Ref(parent)) = dict.raw(&Name::from("Parent")).cloned() else {
                    break;
                };
                let Some(next) = doc
                    .fetch(parent)
                    .ok()
                    .and_then(|object| object.as_dict().cloned())
                else {
                    break;
                };
                dict = next;
            }
            panic!("page {index} has no /MediaBox, inherited or otherwise")
        })
        .collect()
}

#[test]
fn a_reversal_puts_the_pages_in_the_opposite_order() {
    let doc = numbered(3);
    let before = widths(&doc);
    assert_eq!(before.len(), 3);

    let mut edit = EditDoc::new(&doc);
    reorder_pages(&mut edit, &[2, 1, 0]).expect("reorder");
    let after = widths(&roundtrip(&edit));

    let mut expected = before.clone();
    expected.reverse();
    assert_eq!(after, expected);
}

#[test]
fn a_rotation_moves_the_first_page_to_the_end() {
    let doc = numbered(3);
    let before = widths(&doc);

    let mut edit = EditDoc::new(&doc);
    reorder_pages(&mut edit, &[1, 2, 0]).expect("reorder");
    let after = widths(&roundtrip(&edit));

    assert_eq!(after, vec![before[1], before[2], before[0]]);
}

#[test]
fn the_identity_order_changes_nothing() {
    let doc = numbered(3);
    let before = widths(&doc);

    let mut edit = EditDoc::new(&doc);
    reorder_pages(&mut edit, &[0, 1, 2]).expect("reorder");
    assert_eq!(widths(&roundtrip(&edit)), before);
}

#[test]
fn a_list_that_is_not_a_permutation_is_refused() {
    let doc = numbered(3);

    // Too short: that is a deletion, which has its own call.
    let mut edit = EditDoc::new(&doc);
    assert!(reorder_pages(&mut edit, &[0, 1]).is_err());

    // A repeat: that is a duplication.
    let mut edit = EditDoc::new(&doc);
    assert!(reorder_pages(&mut edit, &[0, 0, 1]).is_err());

    // Out of range.
    let mut edit = EditDoc::new(&doc);
    assert!(reorder_pages(&mut edit, &[0, 1, 9]).is_err());
}

#[test]
fn a_refused_reorder_leaves_the_document_alone() {
    let doc = numbered(3);
    let before = widths(&doc);

    let mut edit = EditDoc::new(&doc);
    assert!(reorder_pages(&mut edit, &[0, 0, 1]).is_err());
    assert_eq!(
        widths(&roundtrip(&edit)),
        before,
        "the validation runs before anything is written"
    );
}

#[test]
fn page_content_survives_the_move() {
    // The page objects are relinked, not rebuilt: whatever each page carried
    // comes with it.
    let doc = numbered(2);
    let mut edit = EditDoc::new(&doc);
    reorder_pages(&mut edit, &[1, 0]).expect("reorder");
    let saved = roundtrip(&edit);

    assert_eq!(saved.page_count(), 2);
    let first = saved.page(0u32).expect("page");
    assert!(
        first.dict.raw(&Name::from("Type")).is_some_and(
            |object| matches!(object, Object::Name(name) if name.as_bytes() == b"Page")
        ),
        "still a page dictionary"
    );
}
