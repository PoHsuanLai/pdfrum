//! Page-tree edits that are not imports: deleting pages, adding a blank
//! one, and the per-page rotation and boxes a save carries.

#![expect(
    clippy::expect_used,
    reason = "helpers shared by the tests below; a panic here is a failure"
)]

use std::fmt::Write;
use std::sync::Arc;

use pdfrum_common::PageIndex;
use pdfrum_edit::{
    EditDoc, IdSource, PageBox, PageRange, SaveMode, SaveOptions, add_blank_page, delete_pages,
    save, set_page_box, set_page_rotation,
};
use pdfrum_object::{Object, Resolve, names};
use pdfrum_parser::{Document, LoadOptions, load};

fn open(bytes: &[u8]) -> Document {
    load(Arc::from(bytes), &LoadOptions::default()).expect("opens")
}

fn commit(edit: &EditDoc<'_>) -> Document {
    let options = SaveOptions {
        mode: SaveMode::Full,
        id_source: IdSource::Fixed([0x22; 16]),
        ..SaveOptions::default()
    };
    let mut out = Vec::new();
    save(edit, &options, &mut out).expect("saves");
    open(&out)
}

/// `count` pages under one `/Pages` node, each carrying `/Tag (pN)` so the
/// order survives a round trip.
fn flat(count: usize) -> Vec<u8> {
    let mut out = String::from("%PDF-1.7\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    let kids: Vec<String> = (0..count).map(|i| format!("{} 0 R", i + 3)).collect();
    let _ = writeln!(
        out,
        "2 0 obj\n<< /Type /Pages /Count {count} /Kids [{}] /MediaBox [0 0 400 500] >>\nendobj",
        kids.join(" ")
    );
    for i in 0..count {
        let _ = writeln!(
            out,
            "{} 0 obj\n<< /Type /Page /Parent 2 0 R /Tag (p{i}) >>\nendobj",
            i + 3
        );
    }
    out.push_str("trailer\n<< /Root 1 0 R >>\n");
    out.into_bytes()
}

/// Three pages under two intermediate nodes: `[[p0 p1] [p2]]`, so a delete
/// has two `/Count`s to decrement.
fn nested() -> Vec<u8> {
    let mut out = String::from("%PDF-1.7\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    out.push_str("2 0 obj\n<< /Type /Pages /Count 3 /Kids [3 0 R 4 0 R] /MediaBox [0 0 400 500] >>\nendobj\n");
    out.push_str(
        "3 0 obj\n<< /Type /Pages /Parent 2 0 R /Count 2 /Kids [5 0 R 6 0 R] >>\nendobj\n",
    );
    out.push_str("4 0 obj\n<< /Type /Pages /Parent 2 0 R /Count 1 /Kids [7 0 R] >>\nendobj\n");
    for (num, tag) in [(5, "p0"), (6, "p1"), (7, "p2")] {
        let _ = writeln!(
            out,
            "{num} 0 obj\n<< /Type /Page /Parent {} 0 R /Tag ({tag}) >>\nendobj",
            if num == 7 { 4 } else { 3 }
        );
    }
    out.push_str("trailer\n<< /Root 1 0 R >>\n");
    out.into_bytes()
}

fn num(object: &Object) -> Option<f64> {
    match object {
        Object::Int(i) => i32::try_from(*i).ok().map(f64::from),
        Object::Real(r) => Some(f64::from(*r)),
        _ => None,
    }
}

fn tags(doc: &Document) -> Vec<String> {
    (0..doc.page_count())
        .map(|i| {
            doc.page(PageIndex::from(i))
                .expect("page")
                .dict
                .raw(&pdfrum_object::Name::from("Tag"))
                .and_then(Object::as_string)
                .map(|s| String::from_utf8_lossy(&s.bytes).into_owned())
                .unwrap_or_default()
        })
        .collect()
}

#[test]
fn deleting_pages_keeps_the_others_in_order() {
    let bytes = flat(5);
    let doc = open(&bytes);
    let mut edit = EditDoc::new(&doc);
    delete_pages(&mut edit, &PageRange::of([1u32, 3, 3])).expect("deletes");
    let saved = commit(&edit);
    assert_eq!(saved.page_count(), 3);
    assert_eq!(tags(&saved), ["p0", "p2", "p4"]);
}

#[test]
fn deleting_from_a_nested_tree_decrements_every_count_above() {
    let bytes = nested();
    let doc = open(&bytes);
    let mut edit = EditDoc::new(&doc);
    delete_pages(&mut edit, &PageRange::of([0u32])).expect("deletes");
    let saved = commit(&edit);
    assert_eq!(saved.page_count(), 2);
    assert_eq!(tags(&saved), ["p1", "p2"]);
    let root = saved
        .fetch(pdfrum_object::ObjRef::new(2, 0))
        .expect("root")
        .as_dict()
        .expect("dict")
        .raw(names::COUNT)
        .and_then(Object::as_int);
    assert_eq!(root, Some(2), "the root's /Count follows the deletion");
}

#[test]
fn a_page_that_does_not_exist_is_refused_and_nothing_moves() {
    let bytes = flat(2);
    let doc = open(&bytes);
    let mut edit = EditDoc::new(&doc);
    assert!(delete_pages(&mut edit, &PageRange::of([0u32, 7])).is_err());
    assert_eq!(commit(&edit).page_count(), 2);
}

#[test]
fn a_blank_page_lands_where_asked_with_its_media_box() {
    let bytes = flat(2);
    let doc = open(&bytes);
    let mut edit = EditDoc::new(&doc);
    add_blank_page(&mut edit, 100.0, 200.0, 1).expect("adds");
    add_blank_page(&mut edit, 300.0, 300.0, 99).expect("appends");
    let saved = commit(&edit);
    assert_eq!(saved.page_count(), 4);
    assert_eq!(tags(&saved), ["p0", "", "p1", ""]);
    let inserted = saved.page(PageIndex::from(1u32)).expect("page");
    let media = inserted
        .dict
        .raw(names::MEDIA_BOX)
        .and_then(Object::as_array)
        .expect("media box");
    assert_eq!(media.len(), 4);
    assert_eq!(media.iter().nth(2).and_then(num), Some(100.0));
    assert_eq!(media.iter().nth(3).and_then(num), Some(200.0));
}

#[test]
fn rotation_is_normalized_to_a_quarter_turn_and_a_box_is_written() {
    let bytes = flat(3);
    let doc = open(&bytes);
    let mut edit = EditDoc::new(&doc);
    set_page_rotation(&mut edit, PageIndex::from(0u32), 90).expect("sets");
    set_page_rotation(&mut edit, PageIndex::from(1u32), -90).expect("sets");
    set_page_rotation(&mut edit, PageIndex::from(2u32), 400).expect("sets");
    set_page_box(
        &mut edit,
        PageIndex::from(0u32),
        PageBox::Crop,
        [10.0, 20.0, 110.0, 220.0],
    )
    .expect("sets");
    let saved = commit(&edit);
    let rotate = |i: u32| {
        saved
            .page(PageIndex::from(i))
            .expect("page")
            .dict
            .raw(names::ROTATE)
            .and_then(Object::as_int)
    };
    assert_eq!(rotate(0), Some(90));
    assert_eq!(rotate(1), Some(270));
    assert_eq!(
        rotate(2),
        Some(0),
        "400 is nearer 360 than 450, and 360 is a full turn"
    );
    let first = saved.page(PageIndex::from(0u32)).expect("page");
    let crop = first
        .dict
        .raw(names::CROP_BOX)
        .and_then(Object::as_array)
        .expect("crop box");
    assert_eq!(crop.iter().next().and_then(num), Some(10.0));
    assert_eq!(crop.iter().nth(3).and_then(num), Some(220.0));
    assert!(set_page_rotation(&mut edit, PageIndex::from(9u32), 0).is_err());
}
