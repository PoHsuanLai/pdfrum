//! `AnnotSpec` metadata (`/T`, `/NM`, `/M`) round-trip.

#![expect(
    clippy::expect_used,
    reason = "helpers shared by the tests below; a panic here is a failure"
)]

use std::sync::Arc;

use pdfrum::{AnnotSpec, Color, Document, Rect, SaveOptions};

fn hello() -> Document {
    Document::open(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/hello_world.pdf"
    ))
    .expect("open hello_world")
}

#[test]
fn write_includes_author_name_and_modified() {
    let doc = hello();
    let mut edit = doc.edit();
    let rect = Rect::new(72.0, 700.0, 200.0, 720.0);
    edit.add_annotation(
        0,
        AnnotSpec::highlight(rect, Color::from_rgb8(255, 230, 0))
            .with_contents("note body")
            .with_author("Po-Hsuan Lai")
            .with_name("ann-42")
            .with_modified("D:20260912013000Z"),
    )
    .expect("add");

    let mut out = Vec::new();
    edit.write_to(&mut out, &SaveOptions::default())
        .expect("write");

    let reopened = Document::from_bytes(Arc::<[u8]>::from(out)).expect("reopen");
    let page = reopened.page(0).expect("page");
    let annots: Vec<_> = page.annotations().collect();
    assert_eq!(annots.len(), 1);
    assert_eq!(annots[0].title().as_deref(), Some("Po-Hsuan Lai"));
    assert_eq!(annots[0].name().as_deref(), Some("ann-42"));
    assert_eq!(annots[0].modified().as_deref(), Some("D:20260912013000Z"));
    assert_eq!(annots[0].contents().as_deref(), Some("note body"));
}

#[test]
fn write_respects_custom_flags() {
    use pdfrum::AnnotFlags;

    let doc = hello();
    let mut edit = doc.edit();
    let rect = Rect::new(72.0, 700.0, 200.0, 720.0);
    edit.add_annotation(
        0,
        AnnotSpec::highlight(rect, Color::from_rgb8(255, 230, 0))
            .with_flags(AnnotFlags::PRINT | AnnotFlags::NO_ZOOM),
    )
    .expect("add");

    let mut out = Vec::new();
    edit.write_to(&mut out, &SaveOptions::default())
        .expect("write");

    let reopened = Document::from_bytes(Arc::<[u8]>::from(out)).expect("reopen");
    let page = reopened.page(0).expect("page");
    let annots: Vec<_> = page.annotations().collect();
    assert_eq!(annots.len(), 1);
    let flags = annots[0].flags();
    assert!(flags.prints());
    assert!(!flags.zooms());
    assert_eq!(
        flags.bits(),
        (AnnotFlags::PRINT | AnnotFlags::NO_ZOOM).bits()
    );
}

#[test]
fn default_flags_remain_print_only() {
    use pdfrum::AnnotFlags;

    let doc = hello();
    let mut edit = doc.edit();
    edit.add_annotation(
        0,
        AnnotSpec::text(
            Rect::new(10.0, 10.0, 30.0, 30.0),
            Color::from_rgb8(255, 200, 0),
        ),
    )
    .expect("add");

    let mut out = Vec::new();
    edit.write_to(&mut out, &SaveOptions::default())
        .expect("write");
    let reopened = Document::from_bytes(Arc::<[u8]>::from(out)).expect("reopen");
    let annot = reopened
        .page(0)
        .expect("page")
        .annotations()
        .next()
        .expect("annot");
    assert_eq!(annot.flags().bits(), AnnotFlags::PRINT.bits());
}

#[test]
fn text_icon_and_open_round_trip() {
    use pdfrum::Name;

    let doc = hello();
    let mut edit = doc.edit();
    let rect = Rect::new(40.0, 40.0, 60.0, 60.0);
    edit.add_annotation(
        0,
        AnnotSpec::text(rect, Color::from_rgb8(255, 200, 0))
            .with_contents("keyed")
            .with_icon(Name::from("Key"))
            .with_open(true),
    )
    .expect("add");

    let mut out = Vec::new();
    edit.write_to(&mut out, &SaveOptions::default())
        .expect("write");
    let reopened = Document::from_bytes(Arc::<[u8]>::from(out)).expect("reopen");
    let annot = reopened
        .page(0)
        .expect("page")
        .annotations()
        .next()
        .expect("annot");
    assert_eq!(
        annot.dict().name(&Name::from("Name")).map(Name::as_bytes),
        Some(&b"Key"[..])
    );
    assert_eq!(annot.dict().bool(&Name::from("Open")), Some(true));
    assert_eq!(annot.contents().as_deref(), Some("keyed"));
}

#[test]
fn square_and_ink_custom_border_round_trip() {
    use pdfrum::{AnnotBorder, AnnotBorderStyle, Name};

    let doc = hello();
    let mut edit = doc.edit();
    edit.add_annotation(
        0,
        AnnotSpec::square(Rect::new(100.0, 100.0, 200.0, 180.0), Color::from_rgb8(0, 128, 255))
            .with_border(AnnotBorder::solid(1.0).with_style(AnnotBorderStyle::Dashed)),
    )
    .expect("square");
    edit.add_annotation(
        0,
        AnnotSpec::ink(
            Rect::new(50.0, 50.0, 150.0, 150.0),
            Color::from_rgb8(0, 0, 0),
            vec![vec![
                kurbo::Point::new(60.0, 60.0),
                kurbo::Point::new(140.0, 140.0),
            ]],
        )
        .with_border(AnnotBorder {
            width: 3.0,
            style: AnnotBorderStyle::Underline,
        }),
    )
    .expect("ink");

    let mut out = Vec::new();
    edit.write_to(&mut out, &SaveOptions::default())
        .expect("write");
    let reopened = Document::from_bytes(Arc::<[u8]>::from(out)).expect("reopen");
    let page = reopened.page(0).expect("page");
    let annots: Vec<_> = page.annotations().collect();
    assert_eq!(annots.len(), 2);

    let square_bs = annots[0]
        .dict()
        .dict(&Name::from("BS"), reopened.parser())
        .expect("square BS");
    assert_eq!(
        square_bs.number(&Name::from("W"), reopened.parser()),
        Some(1.0)
    );
    assert_eq!(
        square_bs
            .name(&Name::from("S"))
            .map(Name::as_bytes),
        Some(&b"D"[..])
    );
    assert_eq!(
        square_bs
            .name(&Name::from("Type"))
            .map(Name::as_bytes),
        Some(&b"Border"[..])
    );

    let ink_bs = annots[1]
        .dict()
        .dict(&Name::from("BS"), reopened.parser())
        .expect("ink BS");
    assert_eq!(ink_bs.number(&Name::from("W"), reopened.parser()), Some(3.0));
    assert_eq!(
        ink_bs.name(&Name::from("S")).map(Name::as_bytes),
        Some(&b"U"[..])
    );
    assert!(ink_bs.name(&Name::from("Type")).is_none());
}
