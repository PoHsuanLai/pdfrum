//! Shaped glyph runs written with `Canvas::glyphs`, read back with pdfrum's
//! own reader: the text a reader copies, where each glyph landed, the subset
//! that was embedded, and — with `variable-fonts` — the instance it was cut at.

#![expect(
    clippy::expect_used,
    reason = "helpers shared by the tests below; a panic here is a failure"
)]

use kurbo::Affine;
use pdfrum_common::Limits;
use pdfrum_edit::{
    EditDoc, FontInstance, GlyphFont, GlyphRun, Paint, RunGlyph, SaveOptions, Size, blank_document,
    save,
};
use pdfrum_object::ByteSpan;
use peniko::Color;

/// Karla (OFL-1.1, `files/karla-wght.OFL.txt`): a variable TrueType face,
/// `wght` 400-800, Google Fonts' Latin subset.
const KARLA: &[u8] = include_bytes!("files/karla-wght.ttf");

/// The glyph `font` maps `c` to.
fn gid(font: &[u8], c: char) -> u16 {
    use skrifa::{FontRef, MetadataProvider as _};
    let face = FontRef::new(font).expect("the face parses");
    let id = face.charmap().map(c).expect("the face has the character");
    u16::try_from(id.to_u32()).expect("a 16-bit glyph ID")
}

/// One glyph per character of `text` at the face's own advances (20 pt, the
/// default instance), each its own cluster: what a shaper without kerning
/// would hand over.
fn spelled(font: &[u8], text: &str) -> Vec<RunGlyph> {
    use skrifa::instance::{LocationRef, Size};
    use skrifa::{FontRef, GlyphId, MetadataProvider as _};
    let face = FontRef::new(font).expect("the face parses");
    let metrics = face.glyph_metrics(Size::new(20.0), LocationRef::default());
    let mut x = 0.0;
    text.char_indices()
        .map(|(at, c)| {
            let id = gid(font, c);
            let glyph = RunGlyph {
                id,
                x,
                y: 0.0,
                text: at..at + c.len_utf8(),
            };
            x += f64::from(
                metrics
                    .advance_width(GlyphId::new(u32::from(id)))
                    .unwrap_or(10.0),
            );
            glyph
        })
        .collect()
}

/// `glyphs` spaced `advance` apart, whatever the face says.
fn spaced(mut glyphs: Vec<RunGlyph>, advance: f64) -> Vec<RunGlyph> {
    for (n, glyph) in (0u32..).zip(glyphs.iter_mut()) {
        glyph.x = f64::from(n) * advance;
    }
    glyphs
}

/// A 300 x 120 pt page with `draw` run on it, saved and reopened.
fn page(
    program: &[u8],
    instance: FontInstance,
    draw: impl Fn(&GlyphFont) -> Vec<(Vec<RunGlyph>, String, f64)>,
) -> pdfrum::Document {
    let base = blank_document(&[Size::new(300.0, 120.0)]).expect("a blank page");
    let mut edit = EditDoc::new(&base);
    let font = edit
        .embed_glyph_font(ByteSpan::from(program.to_vec()), 0, instance)
        .expect("the face embeds");
    let runs = draw(&font);
    edit.draw_page(0, &Limits::default(), |canvas| {
        for (glyphs, text, y) in &runs {
            canvas.glyphs(&GlyphRun {
                font: &font,
                size: 20.0,
                transform: Affine::translate((20.0, *y)),
                glyphs,
                text,
                paint: Paint::Fill(Color::BLACK),
            });
        }
    })
    .expect("the page draws");
    let mut out = Vec::new();
    save(&edit, &SaveOptions::default(), &mut out).expect("the file saves");
    pdfrum::Document::from_bytes(out).expect("the file reopens")
}

fn text(doc: &pdfrum::Document) -> String {
    doc.page(0u32)
        .expect("page 1")
        .text()
        .to_string()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
fn a_run_copies_as_the_text_it_was_given() {
    let doc = page(KARLA, FontInstance::Default, |_| {
        vec![(spelled(KARLA, "Hello"), "Hello".to_owned(), 60.0)]
    });
    assert_eq!(text(&doc), "Hello");
}

#[test]
fn a_ligature_glyph_copies_as_every_letter_it_joins() {
    // One glyph (Karla's `f`, standing in for a shaper's `fi` ligature) whose
    // cluster covers two letters, then `nd`.
    let doc = page(KARLA, FontInstance::Default, |_| {
        let mut glyphs = vec![RunGlyph {
            id: gid(KARLA, 'f'),
            x: 0.0,
            y: 0.0,
            text: 0..2,
        }];
        let f_advance = spelled(KARLA, "fn")[1].x;
        glyphs.extend(spelled(KARLA, "nd").into_iter().map(|mut glyph| {
            glyph.x += f_advance;
            glyph.text = glyph.text.start + 2..glyph.text.end + 2;
            glyph
        }));
        vec![(glyphs, "find".to_owned(), 60.0)]
    });
    assert_eq!(text(&doc), "find");
}

#[test]
fn a_glyph_drawn_for_two_texts_keeps_both() {
    // The same glyph for `A` and, in a second run, for `Λ`-that-looks-alike:
    // the first run's text goes into /ToUnicode, the second's into /ActualText.
    let doc = page(KARLA, FontInstance::Default, |_| {
        let glyph = gid(KARLA, 'A');
        let one = |text: &str| {
            vec![RunGlyph {
                id: glyph,
                x: 0.0,
                y: 0.0,
                text: 0..text.len(),
            }]
        };
        vec![
            (one("A"), "A".to_owned(), 80.0),
            (one("Å"), "Å".to_owned(), 40.0),
        ]
    });
    assert_eq!(text(&doc).replace(' ', ""), "AÅ");
}

#[test]
fn a_cluster_of_several_glyphs_copies_once() {
    // A base and a mark shaped as two glyphs of one cluster: the text is said
    // once, over both, not once per glyph.
    let doc = page(KARLA, FontInstance::Default, |_| {
        let glyphs = vec![
            RunGlyph {
                id: gid(KARLA, 'e'),
                x: 0.0,
                y: 0.0,
                text: 0..3,
            },
            RunGlyph {
                id: gid(KARLA, '\u{308}'),
                x: 0.0,
                y: 0.0,
                text: 0..3,
            },
            RunGlyph {
                id: gid(KARLA, 't'),
                x: spelled(KARLA, "et")[1].x,
                y: 0.0,
                text: 3..4,
            },
        ];
        vec![(glyphs, "e\u{308}t".to_owned(), 60.0)]
    });
    // Said once over both glyphs, not once per glyph. (The reader may see a
    // gap after a zero-width mark and insert a space; that is its heuristic.)
    assert_eq!(text(&doc).replace(' ', ""), "e\u{308}t");
}

#[test]
fn every_glyph_lands_at_its_origin() {
    let advance = 17.5;
    let doc = page(KARLA, FontInstance::Default, |_| {
        vec![(
            spaced(spelled(KARLA, "IIIII"), advance),
            "IIIII".to_owned(),
            60.0,
        )]
    });
    let page = doc.page(0u32).expect("page 1");
    let origins: Vec<f64> = page
        .text()
        .chars
        .iter()
        .filter(|glyph| glyph.unicode == u32::from('I'))
        .map(|glyph| glyph.origin.x)
        .collect();
    assert_eq!(origins.len(), 5);
    for (n, x) in (0u32..).zip(&origins) {
        let want = 20.0 + f64::from(n) * advance;
        assert!((x - want).abs() < 0.01, "glyph {n} at {x}, not {want}");
    }
}

#[test]
fn the_face_is_embedded_as_a_tagged_subset() {
    let doc = page(KARLA, FontInstance::Default, |_| {
        vec![(spelled(KARLA, "Hi"), "Hi".to_owned(), 60.0)]
    });
    let fonts = doc.embedded_fonts();
    assert_eq!(fonts.len(), 1);
    let font = &fonts[0];
    let (tag, name) = font.name.split_once('+').expect("a subset tag");
    assert!(
        tag.len() == 6 && tag.chars().all(|c| c.is_ascii_uppercase()),
        "{tag}"
    );
    assert!(name.contains("Karla"), "{name}");
    assert!(
        font.data.len() * 10 < KARLA.len(),
        "{} of {} bytes",
        font.data.len(),
        KARLA.len()
    );
}

#[test]
fn the_page_rasterises_with_ink_where_the_run_is() {
    use pdfrum::{RenderOptions, VelloCpuBackend};
    let doc = page(KARLA, FontInstance::Default, |_| {
        vec![(spelled(KARLA, "HHHH"), "HHHH".to_owned(), 60.0)]
    });
    let options = RenderOptions::builder().background(Color::WHITE).build();
    let pixmap = doc
        .page(0u32)
        .expect("page 1")
        .render_with(VelloCpuBackend, &options)
        .expect("the page rasterises");
    // The run's baseline is 60 pt up a 120 pt page: rows 47-59 from the top
    // hold the capitals' stems, from x = 20.
    let ink = (22..70)
        .flat_map(|x| (47..59).map(move |y| (x, y)))
        .filter(|&(x, y)| pixmap.pixel(x, y).is_some_and(|[r, ..]| r < 128))
        .count();
    assert!(ink > 100, "{ink} dark pixels where the run is");
}

#[cfg(feature = "variable-fonts")]
mod variable {
    use super::{KARLA, page, spaced, spelled};
    use pdfrum_edit::{AxisValue, FontInstance};
    use skrifa::{FontRef, MetadataProvider as _, Tag};

    fn width_of(instance: FontInstance) -> (f64, usize) {
        let doc = page(KARLA, instance, |_| {
            vec![(
                spaced(spelled(KARLA, "mmmm"), 30.0),
                "mmmm".to_owned(),
                60.0,
            )]
        });
        let chars = doc.page(0u32).expect("page 1").text().chars;
        let first = &chars[0];
        let font = &doc.embedded_fonts()[0];
        (first.char_box.width(), font.data.len())
    }

    #[test]
    fn each_instance_embeds_its_own_outlines_and_advances() {
        let regular = width_of(FontInstance::User(vec![AxisValue {
            tag: *b"wght",
            value: 400.0,
        }]));
        let bold = width_of(FontInstance::User(vec![AxisValue {
            tag: *b"wght",
            value: 700.0,
        }]));
        assert!(
            bold.0 > regular.0 * 1.03,
            "bold m {} vs regular {}",
            bold.0,
            regular.0
        );
    }

    #[test]
    fn normalized_coordinates_name_the_same_instance_as_user_values() {
        // What a shaper holds for wght 700: fvar-normalized, then through
        // Karla's avar. The writer must undo both to name the same instance.
        let location = FontRef::new(KARLA)
            .expect("the face parses")
            .axes()
            .location([(Tag::new(b"wght"), 700.0)]);
        let coords: Vec<i16> = location
            .coords()
            .iter()
            .map(|coord| coord.to_bits())
            .collect();
        assert_ne!(
            coords,
            vec![12288],
            "Karla's avar moves 700 off the plain 0.75"
        );
        let user = width_of(FontInstance::User(vec![AxisValue {
            tag: *b"wght",
            value: 700.0,
        }]));
        let normalized = width_of(FontInstance::Normalized(coords));
        assert!(
            (user.0 - normalized.0).abs() < 1e-6,
            "{user:?} vs {normalized:?}"
        );
    }
}

/// `font` wrapped as a one-face collection (`ttcf`): the table directory's
/// offsets move by the 16 bytes the collection header adds.
fn collection_of(font: &[u8]) -> Vec<u8> {
    let tables = usize::from(u16::from_be_bytes([font[4], font[5]]));
    let mut face = font.to_vec();
    for table in 0..tables {
        let at = 12 + 16 * table + 8;
        let offset = u32::from_be_bytes(face[at..at + 4].try_into().expect("four bytes")) + 16;
        face[at..at + 4].copy_from_slice(&offset.to_be_bytes());
    }
    let mut out = b"ttcf".to_vec();
    out.extend_from_slice(&[0, 1, 0, 0]);
    out.extend_from_slice(&1u32.to_be_bytes());
    out.extend_from_slice(&16u32.to_be_bytes());
    out.extend(face);
    out
}

#[test]
fn a_face_of_a_collection_is_embedded_on_its_own() {
    let collection = collection_of(KARLA);
    let doc = page(&collection, FontInstance::Default, |_| {
        vec![(spelled(KARLA, "Kestrel"), "Kestrel".to_owned(), 60.0)]
    });
    assert_eq!(text(&doc), "Kestrel");
    let fonts = doc.embedded_fonts();
    assert_eq!(fonts.len(), 1);
    assert!(
        !fonts[0].data.starts_with(b"ttcf"),
        "the subset is one face, not a collection"
    );
}
