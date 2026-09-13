//! Link /A URI and `GoTo` round-trips.

#![expect(
    clippy::expect_used,
    reason = "helpers shared by the tests below; a panic here is a failure"
)]

use std::sync::Arc;

use pdfrum::{
    AnnotGoToView, AnnotLinkAction, AnnotSpec, Document, Name, Rect, SaveOptions, Subtype,
};
use pdfrum_object::ObjRef;

fn hello() -> Document {
    Document::open(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/hello_world.pdf"
    ))
    .expect("open hello_world")
}

fn hello_2() -> Document {
    Document::open(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/hello_world_2_pages.pdf"
    ))
    .expect("open hello_world_2_pages")
}

fn save_reopen(edit: &pdfrum::DocEdit<'_>) -> Document {
    let mut bytes = Vec::new();
    edit.write_to(&mut bytes, &SaveOptions::default())
        .expect("save");
    Document::from_bytes(Arc::from(bytes)).expect("reopen")
}

fn page_ref(doc: &Document, index: u32) -> ObjRef {
    doc.parser()
        .page(index)
        .expect("page")
        .reference
        .expect("page object ref")
}

#[test]
fn uri_link_still_round_trips() {
    let doc = hello();
    let mut edit = doc.edit();
    edit.add_annotation(
        0,
        AnnotSpec::link(
            Rect::new(72.0, 700.0, 200.0, 720.0),
            "https://example.test/uri",
        ),
    )
    .expect("add");
    let saved = save_reopen(&edit);
    let annot = saved
        .page(0)
        .expect("page")
        .annotations()
        .next()
        .expect("annot");
    assert_eq!(annot.subtype(), Subtype::Link);
    let action = annot
        .dict()
        .dict(&Name::from("A"), saved.parser())
        .expect("A");
    assert_eq!(
        action.name(&Name::from("S")).map(Name::as_bytes),
        Some(&b"URI"[..])
    );
    let uri = action.string(&Name::from("URI")).expect("URI");
    assert!(
        String::from_utf8_lossy(uri.as_bytes()).contains("https://example.test/uri"),
        "uri bytes"
    );
}

#[test]
fn goto_fit_round_trips_via_dict() {
    let doc = hello_2();
    let target = page_ref(&doc, 1);
    let mut edit = doc.edit();
    edit.add_annotation(
        0,
        AnnotSpec::link_goto(
            Rect::new(10.0, 10.0, 80.0, 24.0),
            target,
            AnnotGoToView::Fit,
        )
        .with_contents("to page 2"),
    )
    .expect("add");
    let saved = save_reopen(&edit);
    let annot = saved
        .page(0)
        .expect("page")
        .annotations()
        .next()
        .expect("annot");
    assert_eq!(annot.subtype(), Subtype::Link);
    assert_eq!(annot.contents().as_deref(), Some("to page 2"));
    let action = annot
        .dict()
        .dict(&Name::from("A"), saved.parser())
        .expect("A");
    assert_eq!(
        action.name(&Name::from("S")).map(Name::as_bytes),
        Some(&b"GoTo"[..])
    );
    let dest = action.array(&Name::from("D"), saved.parser()).expect("D");
    assert_eq!(dest.len(), 2);
    assert_eq!(dest.reference_at(0), Some(target));
    assert_eq!(
        dest.get(1, saved.parser())
            .and_then(|v| v.get().as_name().map(|n| n.as_bytes().to_vec())),
        Some(b"Fit".to_vec())
    );
}

#[test]
fn goto_xyz_and_named_write_expected_keys() {
    let doc = hello_2();
    let target = page_ref(&doc, 0);
    let mut edit = doc.edit();
    edit.add_annotation(
        0,
        AnnotSpec::link_goto(
            Rect::new(10.0, 40.0, 80.0, 54.0),
            target,
            AnnotGoToView::Xyz {
                left: Some(0.0),
                top: Some(792.0),
                zoom: None,
            },
        ),
    )
    .expect("xyz");
    edit.add_annotation(
        0,
        AnnotSpec::link_named(
            Rect::new(10.0, 60.0, 80.0, 74.0),
            "Chapter1",
            target,
            AnnotGoToView::Fit,
        ),
    )
    .expect("named");

    let saved = save_reopen(&edit);
    let annots: Vec<_> = saved.page(0).expect("page").annotations().collect();
    assert_eq!(annots.len(), 2);

    let xyz_action = annots[0]
        .dict()
        .dict(&Name::from("A"), saved.parser())
        .expect("A");
    let dest = xyz_action
        .array(&Name::from("D"), saved.parser())
        .expect("D");
    assert_eq!(dest.len(), 5);
    assert_eq!(
        dest.get(1, saved.parser())
            .and_then(|v| v.get().as_name().map(|n| n.as_bytes().to_vec())),
        Some(b"XYZ".to_vec())
    );
    assert!((dest.number_at(3).unwrap() - 792.0).abs() < 0.01);
    assert!(matches!(
        dest.get(4, saved.parser()).map(|v| v.get().clone()),
        Some(pdfrum_object::Object::Null)
    ));

    let named_action = annots[1]
        .dict()
        .dict(&Name::from("A"), saved.parser())
        .expect("A");
    assert_eq!(
        named_action.name(&Name::from("S")).map(Name::as_bytes),
        Some(&b"GoTo"[..])
    );
    let name = named_action.string(&Name::from("D")).expect("D string");
    assert!(
        String::from_utf8_lossy(name.as_bytes()).contains("Chapter1"),
        "named dest"
    );

    // Typed construction stays available for callers matching on the write payload.
    let _ = AnnotLinkAction::Uri(String::from("x"));
}

#[test]
fn named_dest_resolves_after_reopen() {
    use pdfrum_common::{Diagnostics, Limits};
    use pdfrum_doc::nav::lookup_named_dest;

    let doc = hello_2();
    let target = page_ref(&doc, 1);
    let mut edit = doc.edit();
    edit.add_annotation(
        0,
        AnnotSpec::link_named(
            Rect::new(10.0, 60.0, 80.0, 74.0),
            "Chapter1",
            target,
            AnnotGoToView::Fit,
        ),
    )
    .expect("named");

    let saved = save_reopen(&edit);
    let catalog = saved.parser().catalog().expect("catalog");
    let mut diags = Diagnostics::default();
    let found = lookup_named_dest(
        &catalog,
        b"Chapter1",
        saved.parser(),
        &Limits::default(),
        &mut diags,
    );
    let dest = found.expect("Names/Dests must resolve Chapter1");
    assert_eq!(dest.reference_at(0), Some(target));
    assert_eq!(
        dest.get(1, saved.parser())
            .and_then(|v| v.get().as_name().map(|n| n.as_bytes().to_vec())),
        Some(b"Fit".to_vec())
    );
}

#[test]
fn named_existing_does_not_upsert_names_tree() {
    use pdfrum_common::{Diagnostics, Limits};
    use pdfrum_doc::nav::lookup_named_dest;

    let doc = hello_2();
    let target = page_ref(&doc, 1);
    let mut edit = doc.edit();
    edit.set_named_destination("OnlyOnce", target, AnnotGoToView::Fit)
        .expect("register");
    edit.add_annotation(
        0,
        AnnotSpec::link_named_existing(Rect::new(10.0, 80.0, 80.0, 94.0), "OnlyOnce"),
    )
    .expect("named existing");

    let saved = save_reopen(&edit);
    let catalog = saved.parser().catalog().expect("catalog");
    let mut diags = Diagnostics::default();
    let found = lookup_named_dest(
        &catalog,
        b"OnlyOnce",
        saved.parser(),
        &Limits::default(),
        &mut diags,
    );
    assert!(found.is_some(), "existing name still resolves");

    let annot = saved
        .page(0)
        .expect("page")
        .annotations()
        .next()
        .expect("annot");
    let action = annot
        .dict()
        .dict(&Name::from("A"), saved.parser())
        .expect("A");
    let name = action.string(&Name::from("D")).expect("D string");
    assert!(
        String::from_utf8_lossy(name.as_bytes()).contains("OnlyOnce"),
        "D is the name"
    );
}

#[test]
fn named_dest_preserves_kids_tree() {
    use pdfrum::Resolve;
    use pdfrum_object::{Array, Dict, Object, PdfString, encode_text};
    use pdfrum_parser::{LoadOptions, load};

    let doc = hello_2();
    let target = page_ref(&doc, 1);
    let mut edit = doc.edit();
    edit.set_named_destination("Alpha", target, AnnotGoToView::Fit)
        .expect("alpha");
    let mut bytes = Vec::new();
    edit.write_to(&mut bytes, &SaveOptions::default())
        .expect("save");

    let base = load(std::sync::Arc::<[u8]>::from(bytes), &LoadOptions::default()).expect("load");
    let mut edit = pdfrum_edit::EditDoc::new(&base);
    let root = edit
        .base()
        .trailer()
        .reference(&Name::from("Root"))
        .expect("Root");
    let catalog = Resolve::fetch(&edit, root).expect("catalog");
    let catalog = catalog.as_dict().expect("dict").clone();
    let names_ref = catalog
        .raw(&Name::from("Names"))
        .and_then(pdfrum_object::Object::as_ref_id)
        .expect("Names ref");
    let names_dict = Resolve::fetch(&edit, names_ref)
        .expect("names")
        .as_dict()
        .expect("d")
        .clone();
    let dests_ref = names_dict
        .raw(&Name::from("Dests"))
        .and_then(pdfrum_object::Object::as_ref_id)
        .expect("Dests ref");
    let dests = Resolve::fetch(&edit, dests_ref)
        .expect("dests")
        .as_dict()
        .expect("d")
        .clone();
    let names_arr = dests.array(&Name::from("Names"), &edit).expect("Names");
    let mut leaf = Dict::new();
    leaf.insert(Name::from("Names"), Object::Array(names_arr));
    leaf.insert(
        Name::from("Limits"),
        Object::Array(Array::of([
            Object::Str(PdfString::literal(encode_text("Alpha"))),
            Object::Str(PdfString::literal(encode_text("Alpha"))),
        ])),
    );
    let leaf_ref = edit.add(Object::Dict(leaf));
    let mut tree = Dict::new();
    tree.insert(
        Name::from("Kids"),
        Object::Array(Array::of([Object::Ref(leaf_ref)])),
    );
    tree.insert(
        Name::from("Limits"),
        Object::Array(Array::of([
            Object::Str(PdfString::literal(encode_text("Alpha"))),
            Object::Str(PdfString::literal(encode_text("Alpha"))),
        ])),
    );
    edit.replace(dests_ref, Object::Dict(tree));

    pdfrum_edit::set_named_destination(&mut edit, "Beta", target, AnnotGoToView::Fit)
        .expect("beta");

    let tree = Resolve::fetch(&edit, dests_ref)
        .expect("dests")
        .as_dict()
        .expect("d")
        .clone();
    assert!(
        tree.array(&Name::from("Kids"), &edit).is_some(),
        "Kids must be preserved"
    );
    assert!(
        tree.array(&Name::from("Names"), &edit).is_none(),
        "root must not be flattened to Names"
    );

    let mut out = Vec::new();
    pdfrum_edit::save(&edit, &pdfrum_edit::SaveOptions::default(), &mut out).expect("save");
    let saved = Document::from_bytes(std::sync::Arc::from(out)).expect("reopen");
    let catalog = saved.parser().catalog().expect("catalog");
    let mut diags = pdfrum_common::Diagnostics::default();
    for name in [b"Alpha".as_slice(), b"Beta".as_slice()] {
        assert!(
            pdfrum_doc::nav::lookup_named_dest(
                &catalog,
                name,
                saved.parser(),
                &pdfrum_common::Limits::default(),
                &mut diags,
            )
            .is_some(),
            "resolves {}",
            String::from_utf8_lossy(name)
        );
    }
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one explicit /Kids tree built inline"
)]
fn named_dest_inserts_into_correct_kid_leaf() {
    use pdfrum::Resolve;
    use pdfrum_object::{Array, Dict, Object, PdfString, encode_text};
    use pdfrum_parser::{LoadOptions, load};

    let doc = hello_2();
    let target = page_ref(&doc, 1);
    let mut edit = doc.edit();
    // Seed a flat tree so we can lift its dest value out for reuse.
    edit.set_named_destination("Alpha", target, AnnotGoToView::Fit)
        .expect("alpha");
    let mut bytes = Vec::new();
    edit.write_to(&mut bytes, &SaveOptions::default())
        .expect("save");

    let base = load(std::sync::Arc::<[u8]>::from(bytes), &LoadOptions::default()).expect("load");
    let mut edit = pdfrum_edit::EditDoc::new(&base);
    let root = edit
        .base()
        .trailer()
        .reference(&Name::from("Root"))
        .expect("Root");
    let catalog = Resolve::fetch(&edit, root).expect("catalog");
    let catalog = catalog.as_dict().expect("dict").clone();
    let names_ref = catalog
        .raw(&Name::from("Names"))
        .and_then(pdfrum_object::Object::as_ref_id)
        .expect("Names ref");
    let names_dict = Resolve::fetch(&edit, names_ref)
        .expect("names")
        .as_dict()
        .expect("d")
        .clone();
    let dests_ref = names_dict
        .raw(&Name::from("Dests"))
        .and_then(pdfrum_object::Object::as_ref_id)
        .expect("Dests ref");
    let dests = Resolve::fetch(&edit, dests_ref)
        .expect("dests")
        .as_dict()
        .expect("d")
        .clone();
    let names_arr = dests.array(&Name::from("Names"), &edit).expect("Names");
    // The seeded dest value for "Alpha" — reused as the payload for both kids.
    let dest_value = names_arr.raw_at(1).cloned().expect("dest value");

    let mk_leaf = |edit: &mut pdfrum_edit::EditDoc<'_>, key: &str| {
        let mut leaf = Dict::new();
        leaf.insert(
            Name::from("Names"),
            Object::Array(Array::of([
                Object::Str(PdfString::literal(encode_text(key))),
                dest_value.clone(),
            ])),
        );
        leaf.insert(
            Name::from("Limits"),
            Object::Array(Array::of([
                Object::Str(PdfString::literal(encode_text(key))),
                Object::Str(PdfString::literal(encode_text(key))),
            ])),
        );
        edit.add(Object::Dict(leaf))
    };
    // Two disjoint, ordered leaves: ["Alpha"] and ["Zeta"].
    let first = mk_leaf(&mut edit, "Alpha");
    let last = mk_leaf(&mut edit, "Zeta");

    let mut tree = Dict::new();
    tree.insert(
        Name::from("Kids"),
        Object::Array(Array::of([Object::Ref(first), Object::Ref(last)])),
    );
    tree.insert(
        Name::from("Limits"),
        Object::Array(Array::of([
            Object::Str(PdfString::literal(encode_text("Alpha"))),
            Object::Str(PdfString::literal(encode_text("Zeta"))),
        ])),
    );
    edit.replace(dests_ref, Object::Dict(tree));

    // "Beta" sorts into the FIRST leaf, not the last.
    pdfrum_edit::set_named_destination(&mut edit, "Beta", target, AnnotGoToView::Fit)
        .expect("beta");

    // Sibling /Limits must stay disjoint and ascending (ISO 32000-1 §7.9.6):
    // each leaf's high key must sort before the next leaf's low key.
    let low_high = |r: ObjRef| -> (String, String) {
        let leaf = Resolve::fetch(&edit, r)
            .expect("leaf")
            .as_dict()
            .expect("d")
            .clone();
        let lim = leaf.array(&Name::from("Limits"), &edit).expect("Limits");
        let get = |i: usize| {
            pdfrum_object::decode_text(lim.string_at(i).expect("bound").as_bytes()).into_owned()
        };
        (get(0), get(1))
    };
    let (first_low, first_high) = low_high(first);
    let (last_low, last_high) = low_high(last);
    assert!(
        first_low <= first_high && last_low <= last_high,
        "each leaf's own /Limits must be ordered: [{first_low} {first_high}] [{last_low} {last_high}]"
    );
    assert!(
        first_high < last_low,
        "sibling /Limits must not overlap: [{first_low} {first_high}] then [{last_low} {last_high}]"
    );

    let mut out = Vec::new();
    pdfrum_edit::save(&edit, &pdfrum_edit::SaveOptions::default(), &mut out).expect("save");
    let saved = Document::from_bytes(std::sync::Arc::from(out)).expect("reopen");
    let catalog = saved.parser().catalog().expect("catalog");
    let mut diags = pdfrum_common::Diagnostics::default();
    for name in [b"Alpha".as_slice(), b"Beta".as_slice(), b"Zeta".as_slice()] {
        assert!(
            pdfrum_doc::nav::lookup_named_dest(
                &catalog,
                name,
                saved.parser(),
                &pdfrum_common::Limits::default(),
                &mut diags,
            )
            .is_some(),
            "resolves {}",
            String::from_utf8_lossy(name)
        );
    }
}
