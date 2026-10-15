//! Update and delete existing annotations.

#![expect(
    clippy::expect_used,
    reason = "helpers shared by the tests below; a panic here is a failure"
)]

use std::sync::Arc;

use pdfrum::{
    CaretSpec, Color, Document, MarkupKind, MarkupSpec, Name, Rect, SaveOptions, SquareSpec,
    Subtype, TextSpec,
};

fn hello() -> Document {
    Document::open(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/hello_world.pdf"
    ))
    .expect("open hello_world")
}

fn save_reopen(edit: &pdfrum::DocEdit<'_>) -> Document {
    let mut bytes = Vec::new();
    edit.write_to(&mut bytes, &SaveOptions::default())
        .expect("save");
    Document::from_bytes(Arc::from(bytes)).expect("reopen")
}

#[test]
fn add_update_reopen_keeps_ref_and_regenerates_ap() {
    let doc = hello();
    let mut edit = doc.edit();
    let rect = Rect::new(72.0, 700.0, 200.0, 720.0);
    let annot_ref = edit
        .add_annotation(
            0,
            MarkupSpec::new(MarkupKind::Highlight, rect, Color::from_rgb8(255, 230, 0)),
        )
        .expect("add");

    edit.update_annotation(
        0,
        annot_ref,
        MarkupSpec::new(MarkupKind::Highlight, rect, Color::from_rgb8(0, 200, 0))
            .contents("updated"),
    )
    .expect("update");

    let saved = save_reopen(&edit);
    let page = saved.page(0).expect("page");
    let annots: Vec<_> = page.annotations().collect();
    assert_eq!(annots.len(), 1);
    assert_eq!(annots[0].subtype(), Subtype::Highlight);
    assert_eq!(annots[0].contents().as_deref(), Some("updated"));
    assert!(
        annots[0]
            .dict()
            .dict(&Name::from("AP"), saved.parser())
            .is_some(),
        "update should regenerate /AP"
    );
}

#[test]
fn add_delete_gone_on_reopen() {
    let doc = hello();
    let mut edit = doc.edit();
    let a = edit
        .add_annotation(
            0,
            TextSpec::new(
                Rect::new(10.0, 10.0, 30.0, 30.0),
                Color::from_rgb8(255, 200, 0),
            ),
        )
        .expect("add a");
    let b = edit
        .add_annotation(
            0,
            SquareSpec::new(
                Rect::new(40.0, 40.0, 80.0, 80.0),
                Color::from_rgb8(0, 0, 255),
            ),
        )
        .expect("add b");

    assert!(edit.delete_annotation(0, a).expect("delete a"));
    assert!(!edit.delete_annotation(0, a).expect("already gone"));

    let saved = save_reopen(&edit);
    let annots: Vec<_> = saved.page(0).expect("page").annotations().collect();
    assert_eq!(annots.len(), 1);
    assert_eq!(annots[0].subtype(), Subtype::Square);
    let _ = b; // kept
}

#[test]
fn update_rejects_annot_not_on_page() {
    let doc = hello();
    let mut edit = doc.edit();
    let a = edit
        .add_annotation(
            0,
            CaretSpec::new(Rect::new(1.0, 1.0, 5.0, 5.0), Color::BLACK),
        )
        .expect("add");
    assert!(edit.delete_annotation(0, a).expect("delete"));
    let err = edit
        .update_annotation(
            0,
            a,
            CaretSpec::new(Rect::new(1.0, 1.0, 5.0, 5.0), Color::BLACK).contents("nope"),
        )
        .expect_err("must fail");
    let msg = err.to_string();
    assert!(
        msg.contains("not on page") || msg.contains("AnnotNotOnPage"),
        "unexpected: {msg}"
    );
}

#[test]
fn update_and_delete_by_annots_index() {
    let doc = hello();
    let mut edit = doc.edit();
    edit.add_annotation(
        0,
        TextSpec::new(
            Rect::new(10.0, 10.0, 30.0, 30.0),
            Color::from_rgb8(255, 200, 0),
        ),
    )
    .expect("add a");
    edit.add_annotation(
        0,
        SquareSpec::new(
            Rect::new(40.0, 40.0, 80.0, 80.0),
            Color::from_rgb8(0, 0, 255),
        ),
    )
    .expect("add b");

    edit.update_annotation_at(
        0,
        0,
        TextSpec::new(
            Rect::new(10.0, 10.0, 30.0, 30.0),
            Color::from_rgb8(255, 200, 0),
        )
        .contents("first"),
    )
    .expect("update at 0");

    assert!(edit.delete_annotation_at(0, 1).expect("delete at 1"));

    let saved = save_reopen(&edit);
    let annots: Vec<_> = saved.page(0).expect("page").annotations().collect();
    assert_eq!(annots.len(), 1);
    assert_eq!(annots[0].subtype(), Subtype::Text);
    assert_eq!(annots[0].contents().as_deref(), Some("first"));
}

#[test]
fn index_helpers_reject_out_of_range() {
    let doc = hello();
    let mut edit = doc.edit();
    edit.add_annotation(
        0,
        CaretSpec::new(Rect::new(1.0, 1.0, 5.0, 5.0), Color::BLACK),
    )
    .expect("add");
    let err = edit
        .update_annotation_at(
            0,
            3,
            CaretSpec::new(Rect::new(1.0, 1.0, 5.0, 5.0), Color::BLACK),
        )
        .expect_err("oob");
    let msg = err.to_string();
    assert!(
        msg.contains("out of range") || msg.contains("AnnotIndexOutOfRange"),
        "unexpected: {msg}"
    );
    let err = edit.delete_annotation_at(0, 9).expect_err("oob delete");
    let msg = err.to_string();
    assert!(
        msg.contains("out of range") || msg.contains("AnnotIndexOutOfRange"),
        "unexpected: {msg}"
    );
}

#[test]
fn index_helpers_promote_and_delete_inline_annots() {
    use pdfrum_edit::{EditDoc, TextSpec};
    use pdfrum_object::{Array, Dict, Name, Object};
    use pdfrum_parser::{LoadOptions, load};

    let bytes = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/hello_world.pdf"
    ))
    .expect("read");
    let base = load(Arc::<[u8]>::from(bytes), &LoadOptions::default()).expect("load");
    let mut edit = EditDoc::new(&base);

    let (page_ref, mut page_dict, _) = edit.page_state(0).expect("state").expect("page");
    let inline = Dict::from_pairs([
        (Name::from("Type"), Object::Name(Name::from("Annot"))),
        (Name::from("Subtype"), Object::Name(Name::from("Text"))),
        (
            Name::from("Rect"),
            Object::Array(Array::of([
                Object::Real(1.0),
                Object::Real(1.0),
                Object::Real(20.0),
                Object::Real(20.0),
            ])),
        ),
        (Name::from("F"), Object::Int(4)),
        (Name::from("P"), Object::Ref(page_ref)),
    ]);
    page_dict.insert(
        Name::from("Annots"),
        Object::Array(Array::of([Object::Dict(inline)])),
    );
    edit.replace(page_ref, Object::Dict(page_dict));

    pdfrum_edit::update_annotation_at(
        &mut edit,
        0,
        0,
        TextSpec::new(
            Rect::new(1.0, 1.0, 20.0, 20.0),
            Color::from_rgb8(255, 200, 0),
        )
        .contents("promoted"),
    )
    .expect("update inline");

    // After promote+update the slot must be an indirect ref.
    let (_page_ref, page_dict, _) = edit.page_state(0).expect("state").expect("page");
    let annots = page_dict
        .array(&Name::from("Annots"), &edit)
        .expect("Annots");
    assert!(
        matches!(annots.raw_at(0), Some(Object::Ref(_))),
        "inline must be promoted to Ref"
    );

    let mut bytes = Vec::new();
    pdfrum_edit::save(&edit, &pdfrum_edit::SaveOptions::default(), &mut bytes).expect("save");
    let saved = Document::from_bytes(Arc::from(bytes)).expect("reopen");
    let annots: Vec<_> = saved.page(0).expect("page").annotations().collect();
    assert_eq!(annots.len(), 1);
    assert_eq!(annots[0].contents().as_deref(), Some("promoted"));

    let mut edit = saved.edit();
    assert!(edit.delete_annotation_at(0, 0).expect("delete"));
    let saved = save_reopen(&edit);
    assert_eq!(saved.page(0).expect("page").annotations().count(), 0);
}

// ---- move_annotation / modify_annotation ----

#[expect(
    clippy::similar_names,
    clippy::float_cmp,
    reason = "before/after pairs read best side by side, and every coordinate here is a small \
              integer sum, so exact comparison is what is meant"
)]
mod lossless {
    use super::{hello, save_reopen};
    use kurbo::Vec2;
    use pdfrum::{
        Color, Document, InkSpec, MarkupKind, MarkupSpec, Point, Rect, SquareSpec, TextSpec,
    };
    use pdfrum_object::{Array, Dict, Name, ObjRef, Object, PdfString};

    fn name(key: &str) -> Name {
        Name::from(key)
    }

    fn reals(values: &[f32]) -> Object {
        Object::Array(Array::of(values.iter().map(|v| Object::Real(*v))))
    }

    /// The numbers under `key`, empty when absent.
    fn numbers(doc: &Document, dict: &Dict, key: &str) -> Vec<f32> {
        let array = dict.array(&name(key), doc.parser()).expect(key);
        (0..array.len())
            .map(|i| array.number_at_or_zero(i))
            .collect()
    }

    /// The dictionary of the `index`th annotation on page 0 and its reference.
    fn annot_at(doc: &Document, index: usize) -> (ObjRef, Dict) {
        let page = doc.page(0).expect("page");
        let annot = page.annotations().nth(index).expect("annot");
        (annot.obj_ref().expect("indirect"), annot.dict().clone())
    }

    /// A square carrying keys `update_annotation` would lose, saved and reopened.
    fn square_with_extras() -> (Document, ObjRef) {
        let doc = hello();
        let mut edit = doc.edit();
        let other = edit
            .add_annotation(
                0,
                SquareSpec::new(Rect::new(100.0, 100.0, 120.0, 120.0), Color::BLACK),
            )
            .expect("other");
        let square = edit
            .add_annotation(
                0,
                SquareSpec::new(
                    Rect::new(10.0, 10.0, 60.0, 60.0),
                    Color::from_rgb8(255, 0, 0),
                ),
            )
            .expect("square");
        edit.modify_annotation(0, square, |dict| {
            dict.insert(name("NM"), Object::Str(PdfString::literal(b"abc")));
            dict.insert(name("IRT"), Object::Ref(other));
            dict.insert(name("Foo"), Object::Int(1));
            let mut bs = Dict::new();
            bs.insert(name("W"), Object::Int(2));
            bs.insert(name("S"), Object::Name(name("D")));
            bs.insert(
                name("D"),
                Object::Array(Array::of([Object::Int(3), Object::Int(2)])),
            );
            dict.insert(name("BS"), Object::Dict(bs));
        })
        .expect("modify");
        let saved = save_reopen(&edit);
        (saved, square)
    }

    #[test]
    fn move_translates_rect_and_keeps_everything_else() {
        let (doc, square) = square_with_extras();
        let (_, before) = annot_at(&doc, 1);
        let before_rect = numbers(&doc, &before, "Rect");

        let mut edit = doc.edit();
        edit.move_annotation(0, square, Vec2::new(30.0, -20.0))
            .expect("move");
        let saved = save_reopen(&edit);
        let (_, after) = annot_at(&saved, 1);

        let after_rect = numbers(&saved, &after, "Rect");
        let expected = [
            before_rect[0] + 30.0,
            before_rect[1] - 20.0,
            before_rect[2] + 30.0,
            before_rect[3] - 20.0,
        ];
        assert_eq!(after_rect, expected);

        let (mut before_rest, mut after_rest) = (before.clone(), after);
        before_rest.remove(&name("Rect"));
        after_rest.remove(&name("Rect"));
        assert_eq!(before_rest, after_rest, "only the geometry may differ");
        assert_eq!(
            before.raw(&name("AP")),
            after_rest.raw(&name("AP")),
            "the appearance is the same object"
        );
        assert!(after_rest.raw(&name("AP")).is_some());
        assert_eq!(after_rest.raw(&name("Foo")), Some(&Object::Int(1)));
    }

    #[test]
    fn move_translates_ink_list_and_quad_points() {
        let doc = hello();
        let mut edit = doc.edit();
        let ink = edit
            .add_annotation(
                0,
                InkSpec::new(
                    Rect::new(10.0, 10.0, 50.0, 50.0),
                    Color::BLACK,
                    vec![vec![Point::new(10.0, 10.0), Point::new(40.0, 50.0)]],
                ),
            )
            .expect("ink");
        let highlight = edit
            .add_annotation(
                0,
                MarkupSpec::new(
                    MarkupKind::Highlight,
                    Rect::new(72.0, 700.0, 200.0, 720.0),
                    Color::from_rgb8(255, 230, 0),
                ),
            )
            .expect("highlight");
        let doc = save_reopen(&edit);
        let (_, ink_before) = annot_at(&doc, 0);
        let (_, hl_before) = annot_at(&doc, 1);
        let quads_before = numbers(&doc, &hl_before, "QuadPoints");

        let mut edit = doc.edit();
        let by = Vec2::new(30.0, -20.0);
        edit.move_annotation(0, ink, by).expect("move ink");
        edit.move_annotation(0, highlight, by)
            .expect("move highlight");
        let saved = save_reopen(&edit);

        let (_, ink_after) = annot_at(&saved, 0);
        let strokes_before = ink_before
            .array(&name("InkList"), doc.parser())
            .expect("ink");
        let strokes_after = ink_after
            .array(&name("InkList"), saved.parser())
            .expect("ink");
        let stroke_before = strokes_before.array_at(0, doc.parser()).expect("stroke");
        let stroke_after = strokes_after.array_at(0, saved.parser()).expect("stroke");
        for i in 0..stroke_before.len() {
            let shift = if i % 2 == 0 { 30.0 } else { -20.0 };
            assert_eq!(
                stroke_after.number_at_or_zero(i),
                stroke_before.number_at_or_zero(i) + shift
            );
        }

        let (_, hl_after) = annot_at(&saved, 1);
        let quads_after = numbers(&saved, &hl_after, "QuadPoints");
        assert_eq!(quads_after.len(), quads_before.len());
        for (i, (a, b)) in quads_after.iter().zip(&quads_before).enumerate() {
            let shift = if i % 2 == 0 { 30.0 } else { -20.0 };
            assert_eq!(*a, b + shift);
        }
    }

    #[test]
    fn move_carries_the_popup_along() {
        let doc = hello();
        let mut edit = doc.edit();
        let popup = edit
            .add_annotation(
                0,
                SquareSpec::new(Rect::new(200.0, 200.0, 260.0, 240.0), Color::BLACK),
            )
            .expect("popup stand-in");
        let note = edit
            .add_annotation(
                0,
                TextSpec::new(
                    Rect::new(10.0, 10.0, 30.0, 30.0),
                    Color::from_rgb8(255, 200, 0),
                ),
            )
            .expect("note");
        edit.modify_annotation(0, note, |dict| {
            dict.insert(name("Popup"), Object::Ref(popup));
        })
        .expect("modify");
        let doc = save_reopen(&edit);
        let (_, popup_before) = annot_at(&doc, 0);
        let before = numbers(&doc, &popup_before, "Rect");

        let mut edit = doc.edit();
        edit.move_annotation(0, note, Vec2::new(30.0, -20.0))
            .expect("move");
        let saved = save_reopen(&edit);
        let (_, popup_after) = annot_at(&saved, 0);
        assert_eq!(
            numbers(&saved, &popup_after, "Rect"),
            [
                before[0] + 30.0,
                before[1] - 20.0,
                before[2] + 30.0,
                before[3] - 20.0
            ]
        );
    }

    #[test]
    fn modify_recolours_and_keeps_the_other_keys() {
        let (doc, square) = square_with_extras();
        let (_, before) = annot_at(&doc, 1);

        let mut edit = doc.edit();
        edit.modify_annotation(0, square, |dict| {
            dict.insert(name("C"), reals(&[0.0, 0.0, 1.0]));
        })
        .expect("modify");
        let saved = save_reopen(&edit);
        let (_, after) = annot_at(&saved, 1);

        assert_eq!(numbers(&saved, &after, "C"), [0.0, 0.0, 1.0]);
        for key in ["NM", "IRT", "Foo", "BS", "Rect"] {
            assert_eq!(before.raw(&name(key)), after.raw(&name(key)), "{key}");
        }
        assert!(after.raw(&name("AP")).is_some(), "an appearance exists");
        assert_ne!(
            before.raw(&name("AP")),
            after.raw(&name("AP")),
            "the appearance was regenerated"
        );
    }

    #[test]
    fn moving_an_annotation_from_another_page_fails() {
        let doc = hello();
        let mut edit = doc.edit();
        let a = edit
            .add_annotation(
                0,
                SquareSpec::new(Rect::new(1.0, 1.0, 5.0, 5.0), Color::BLACK),
            )
            .expect("add");
        assert!(edit.delete_annotation(0, a).expect("delete"));
        let err = edit
            .move_annotation(0, a, Vec2::new(1.0, 1.0))
            .expect_err("must fail");
        let msg = err.to_string();
        assert!(
            msg.contains("not on page") || msg.contains("AnnotNotOnPage"),
            "unexpected: {msg}"
        );
    }
}
