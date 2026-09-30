//! A scene recorded through anyrender, written as a
//! page, read back with pdfrum.

// Helpers here are not `#[test]` fns, so clippy.toml's test allowance does not reach them.
#![allow(clippy::expect_used)]

use anyrender::{Glyph, PaintScene, Scene};
use pdfrum_anyrender::{GlyphArea, GlyphSource, Page, RunKey, RunText, Sources, write};
use peniko::kurbo::{Affine, Rect, Size, Vec2};
use peniko::{Blob, Color, Fill, FontData};
use skrifa::{FontRef, MetadataProvider as _};
use std::sync::{Arc, LazyLock};

/// Noto Serif (Latin subset, SIL OFL: see `fixtures/OFL-noto-serif.txt`), a variable face at its default instance here.
static SERIF: &[u8] = include_bytes!("fixtures/noto-serif-latin.ttf");

/// The face as a renderer holds it: one blob, whose id names the face in every run key.
static FONT: LazyLock<FontData> = LazyLock::new(|| FontData::new(Blob::new(Arc::new(SERIF)), 0));

/// `text`'s glyphs in the serif at 20 px from x = 10, baseline 40, one per character.
fn glyphs(text: &str) -> Vec<Glyph> {
    let face = FontRef::new(SERIF).expect("the face parses");
    let metrics = face.glyph_metrics(
        skrifa::instance::Size::new(20.0),
        skrifa::instance::LocationRef::default(),
    );
    let mut x = 10.0;
    text.chars()
        .map(|c| {
            let id = face.charmap().map(c).expect("the face has the character");
            let glyph = Glyph {
                id: id.to_u32(),
                x,
                y: 40.0,
            };
            x += metrics.advance_width(id).unwrap_or(10.0);
            glyph
        })
        .collect()
}

/// `run` over a pale rectangle on a 200 x 100 pt page (scene pixels are points, y down).
fn written(run: &[Glyph], sources: &Sources, area: GlyphArea) -> pdfrum_anyrender::Written {
    let mut scene = Scene::new();
    scene.fill(
        Fill::NonZero,
        Affine::IDENTITY,
        Color::from_rgb8(200, 220, 255),
        None,
        &Rect::new(0.0, 0.0, 200.0, 100.0),
    );
    scene.draw_glyphs(
        &FONT,
        20.0,
        false,
        &[],
        Vec2::ZERO,
        Fill::NonZero,
        Color::BLACK,
        1.0,
        Affine::IDENTITY,
        None,
        run.iter().copied(),
    );
    let page = Page {
        size: Size::new(200.0, 100.0),
        scene,
        placement: Affine::new([1.0, 0.0, 0.0, -1.0, 0.0, 100.0]),
        clip: Rect::new(0.0, 0.0, 200.0, 100.0),
        area,
    };
    write(&[page], sources).expect("the page writes")
}

fn text(bytes: &[u8]) -> String {
    let doc = pdfrum::Document::from_bytes(bytes.to_vec()).expect("opens");
    doc.page(0u32)
        .expect("page 1")
        .text()
        .to_string()
        .trim()
        .to_owned()
}

#[test]
fn a_run_prints_the_text_it_was_given() {
    let run = glyphs("Hello");
    let mut sources = Sources::default();
    // Deliberately not what the glyphs spell, so the text can only have come from the caller.
    let given = RunText::from_glyphs("HELLO".char_indices().map(|(at, _)| GlyphSource {
        cluster: at,
        text: &"HELLO"[at..=at],
    }));
    sources
        .texts
        .insert(RunKey::new(&FONT, 20.0, run.iter().copied()), given);
    let written = written(&run, &sources, GlyphArea::Anywhere);
    assert_eq!(text(&written.bytes), "HELLO");
    let tally = written.glyphs;
    assert_eq!((tally.drawn, tally.layout_text, tally.cmap_text), (5, 5, 0));
}

#[test]
fn a_run_without_text_falls_back_to_the_cmap() {
    let written = written(&glyphs("Hello"), &Sources::default(), GlyphArea::Anywhere);
    assert_eq!(text(&written.bytes), "Hello");
    assert_eq!((written.glyphs.drawn, written.glyphs.cmap_text), (5, 5));
}

#[test]
fn glyphs_outside_the_area_are_not_written() {
    // The area ends 34 px in: "He" (box centres near 17 and 30) stays, "llo" goes.
    let area = GlyphArea::Within(Rect::new(0.0, 0.0, 34.0, 100.0));
    let written = written(&glyphs("Hello"), &Sources::default(), area);
    assert_eq!(text(&written.bytes), "He");
    assert_eq!((written.glyphs.drawn, written.glyphs.dropped), (2, 3));
}

#[test]
fn the_page_holds_the_run_where_the_scene_put_it() {
    let written = written(&glyphs("Hello"), &Sources::default(), GlyphArea::Anywhere);
    let doc = pdfrum::Document::from_bytes(written.bytes).expect("opens");
    let first = doc.page(0u32).expect("page 1").text().chars[0].origin;
    // Scene (10, 40), y down on a 100 pt page, is PDF (10, 60).
    assert!(
        (first.x - 10.0).abs() < 0.01 && (first.y - 60.0).abs() < 0.01,
        "{first:?}"
    );
}
