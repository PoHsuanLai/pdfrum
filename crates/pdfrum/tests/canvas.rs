//! The canvas: what it emits, where it lands, and that we can read it back.
//!
//! The contract's hard rule is the last of those — **every emitted stream
//! round-trips through our own parser** — so most of what is asserted here is
//! asserted against a reloaded document rather than against the bytes.

#![cfg(feature = "edit")]
// `expect` is how a test states its preconditions; the float comparisons are
// exact on purpose — a size or a coordinate copied straight through, never
// computed, so a tolerance would only hide a real divergence.
#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::float_cmp)]

use pdfrum::{
    Affine, Color, Dict, Document, Fill, Name, ObjRef, Object, PageIndex, Paint, Point, Rect,
    Resolve, SaveOptions, StandardFont, Stroke,
};

/// Save `edit` and reload it, which is the round trip every test here makes.
fn round_trip(edit: &pdfrum::DocEdit<'_>) -> Document {
    let mut bytes = Vec::new();
    edit.write_to(&mut bytes, &SaveOptions::default())
        .expect("writes");
    Document::from_bytes(bytes.into()).expect("reopens")
}

/// The `/Contents` element references of page `index`, in order.
fn elements(doc: &Document, index: u32) -> Vec<ObjRef> {
    let parser = doc.parser();
    let page = parser.page(PageIndex::from(index)).expect("page");
    let key = Name::from("Contents");
    if let Some(array) = page.dict.array(&key, parser) {
        return (0..array.len())
            .filter_map(|i| array.reference_at(i))
            .collect();
    }
    page.dict.reference(&key).into_iter().collect()
}

/// Every content stream of page `index`, decoded and concatenated.
///
/// Decoded rather than read raw because the save flate-compresses what it
/// writes: reading these bytes back *is* the round trip through our own
/// filter chain and lexer that the contract asks for.
fn contents(doc: &Document, index: u32) -> String {
    let parser = doc.parser();
    let mut out = String::new();
    for reference in elements(doc, index) {
        let Ok(object) = parser.fetch(reference) else {
            continue;
        };
        let Some(stream) = object.as_stream() else {
            continue;
        };
        let mut diags = pdfrum::Diagnostics::default();
        let data =
            pdfrum_filters::decode_chain(stream, 0, parser, &pdfrum::Limits::default(), &mut diags)
                .data;
        out.push_str(&String::from_utf8_lossy(&data));
        out.push('\n');
    }
    out
}

/// Page `index`'s `/Resources`, resolved.
fn resources(doc: &Document, index: u32) -> Dict {
    let parser = doc.parser();
    let page = parser.page(PageIndex::from(index)).expect("page");
    let key = Name::from("Resources");
    page.dict
        .dict(&key, parser)
        .or_else(|| {
            page.inherited(&key, parser)?
                .resolve(parser)
                .ok()?
                .get()
                .as_dict()
                .cloned()
        })
        .unwrap_or_default()
}

/// The number of `/Contents` elements page `index` has.
fn stream_count(doc: &Document, index: u32) -> usize {
    elements(doc, index).len()
}

/// The canvas-to-page transform the drawing on page `index` was framed with:
/// the first `cm` of the appended stream.
fn canvas_transform(doc: &Document, index: u32) -> Affine {
    let text = contents(doc, index);
    let line = text
        .lines()
        .find(|line| line.ends_with(" cm"))
        .expect("the canvas transform");
    let n: Vec<f64> = line
        .trim_end_matches(" cm")
        .split_whitespace()
        .filter_map(|value| value.parse().ok())
        .collect();
    assert_eq!(n.len(), 6, "six operands: {line}");
    Affine::new([n[0], n[1], n[2], n[3], n[4], n[5]])
}

// The contract's rule, stated as a test: what the canvas writes, our own
// parser reads back — and the operators survive the trip intact.
#[test]
fn an_emitted_stream_reparses() {
    let doc = Document::open("tests/fixtures/hello_world.pdf").expect("opens");
    let mut edit = doc.edit();
    edit.draw_page(0, |c| {
        c.fill_rect(
            Rect::new(10.0, 10.0, 60.0, 40.0),
            Color::from_rgb8(255, 0, 0),
        );
        c.stroke(
            Rect::new(70.0, 10.0, 120.0, 40.0),
            Stroke::new(Color::from_rgb8(0, 0, 255), 2.0),
        );
    })
    .expect("draws");

    let saved = round_trip(&edit);
    let text = contents(&saved, 0);
    assert!(text.contains("10 10 50 30 re"), "the rect: {text}");
    assert!(text.contains(" f"), "filled: {text}");
    assert!(text.contains(" S"), "stroked: {text}");
    assert!(text.contains("2 w"), "line width: {text}");
}

// Item 3: appended, not rewritten. The page's original stream is still there,
// byte for byte, and the drawing is one more element after it.
#[test]
fn the_drawing_is_appended_and_the_page_keeps_its_own_stream() {
    let doc = Document::open("tests/fixtures/hello_world.pdf").expect("opens");
    let before = contents(&doc, 0);
    let elements_before = stream_count(&doc, 0);

    let mut edit = doc.edit();
    edit.draw_page(0, |c| {
        c.fill_rect(Rect::new(0.0, 0.0, 5.0, 5.0), Color::BLACK);
    })
    .expect("draws");
    let saved = round_trip(&edit);

    assert_eq!(stream_count(&saved, 0), elements_before + 1);
    let after = contents(&saved, 0);
    assert!(
        after.starts_with(&before),
        "the original stream is untouched and first"
    );
    // Every drawing is framed, so neither state can leak into the other.
    let drawn = after.strip_prefix(&before).expect("the appended stream");
    assert!(drawn.trim_start().starts_with('q'), "framed: {drawn}");
    assert!(drawn.trim_end().ends_with('Q'), "framed: {drawn}");
}

// The page's text survives the append: nothing was regenerated, so none of
// `pdfrum-edit`'s regeneration losses apply.
#[test]
fn drawing_does_not_regenerate_the_page() {
    let doc = Document::open("tests/fixtures/hello_world.pdf").expect("opens");
    let before = doc.page(0).expect("page").text().to_string();
    let mut edit = doc.edit();
    edit.draw_page(0, |c| {
        c.fill_rect(Rect::new(0.0, 0.0, 5.0, 5.0), Color::BLACK);
    })
    .expect("draws");
    let saved = round_trip(&edit);
    assert_eq!(saved.page(0).expect("page").text().to_string(), before);
}

// Item 2: text uses a font the session embedded, and the glyphs come back as
// text through our own extractor.
#[test]
fn text_draws_through_an_embedded_font() {
    let doc = Document::open("tests/fixtures/hello_world.pdf").expect("opens");
    let mut edit = doc.edit();
    let font = edit.standard_font(StandardFont::Helvetica).expect("font");
    edit.draw_page(0, |c| {
        c.text("Canvas", &font, 14.0, Point::new(30.0, 30.0), Color::BLACK);
    })
    .expect("draws");

    let saved = round_trip(&edit);
    assert!(
        saved
            .page(0)
            .expect("page")
            .text()
            .to_string()
            .contains("Canvas"),
        "the drawn run is extractable text"
    );
}

// Item 2's teeth: a character the font cannot draw is refused, and nothing of
// the drawing is written.
#[test]
fn a_missing_glyph_is_an_error_not_a_blank() {
    let doc = Document::open("tests/fixtures/hello_world.pdf").expect("opens");
    let before = contents(&doc, 0);
    let mut edit = doc.edit();
    let font = edit.standard_font(StandardFont::Helvetica).expect("font");
    // WinAnsi reaches no Han character.
    let refused = edit.draw_page(0, |c| {
        c.text("\u{4e00}", &font, 12.0, Point::ZERO, Color::BLACK);
    });
    let error = refused.expect_err("refused");
    assert!(error.to_string().contains("no glyph"), "{error}");

    // And nothing was written: the page is exactly as it was.
    let saved = round_trip(&edit);
    assert_eq!(contents(&saved, 0), before);
}

// A missing glyph anywhere in the string refuses the whole string, and names
// the character and its offset rather than "something went wrong".
#[test]
fn the_refusal_names_the_character_and_its_offset() {
    let doc = Document::open("tests/fixtures/hello_world.pdf").expect("opens");
    let mut edit = doc.edit();
    let font = edit.standard_font(StandardFont::Helvetica).expect("font");
    let error = edit
        .draw_page(0, |c| {
            c.text("ok \u{4e00} no", &font, 12.0, Point::ZERO, Color::BLACK);
        })
        .expect_err("refused");
    let message = error.to_string();
    assert!(message.contains("byte 3"), "{message}");
}

// Item 3's second half: resources merge under names that cannot collide with
// the page's own, and the page's existing entries survive.
#[test]
fn resources_merge_under_fresh_names() {
    let doc = Document::open("tests/fixtures/hello_world.pdf").expect("opens");
    let before = resources(&doc, 0);
    let font_names_before: Vec<Name> = before
        .dict(&Name::from("Font"), doc.parser())
        .map(|d| d.keys().cloned().collect())
        .unwrap_or_default();

    let mut edit = doc.edit();
    let font = edit.standard_font(StandardFont::Helvetica).expect("font");
    edit.draw_page(0, |c| {
        c.text("x", &font, 12.0, Point::ZERO, Color::BLACK);
    })
    .expect("draws");

    let saved = round_trip(&edit);
    let after = resources(&saved, 0);
    let fonts = after
        .dict(&Name::from("Font"), saved.parser())
        .expect("fonts");
    let names: Vec<Name> = fonts.keys().cloned().collect();

    for existing in &font_names_before {
        assert!(names.contains(existing), "kept {existing:?} of {names:?}");
    }
    assert_eq!(
        names.len(),
        font_names_before.len() + 1,
        "exactly one name added: {names:?}"
    );
    let added = names
        .iter()
        .find(|n| !font_names_before.contains(n))
        .expect("the new name");
    assert!(
        String::from_utf8_lossy(added.as_bytes()).starts_with("PdfrumC"),
        "our own series, not the producer's: {added:?}"
    );
}

// A name the page already uses is skipped rather than overwritten, even when
// it is spelled the way ours are.
#[test]
fn a_name_the_page_already_holds_is_not_reused() {
    // A page whose `/Font` already holds `PdfrumC1`.
    let file = b"%PDF-1.7\n\
1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n\
2 0 obj\n<< /Type /Pages /Count 1 /Kids [3 0 R] >>\nendobj\n\
3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R \
/Resources << /Font << /PdfrumC1 5 0 R >> >> >>\nendobj\n\
4 0 obj\n<< /Length 1 >>\nstream\n\nendstream\nendobj\n\
5 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>\nendobj\n\
trailer\n<< /Root 1 0 R /Size 6 >>\n";
    let doc = Document::from_bytes(std::sync::Arc::from(&file[..])).expect("opens");
    let mut edit = doc.edit();
    let font = edit.standard_font(StandardFont::Courier).expect("font");
    edit.draw_page(0, |c| {
        c.text("x", &font, 12.0, Point::ZERO, Color::BLACK);
    })
    .expect("draws");

    let saved = round_trip(&edit);
    let fonts = resources(&saved, 0)
        .dict(&Name::from("Font"), saved.parser())
        .expect("fonts");
    assert!(
        fonts.contains_key(&Name::from("PdfrumC1")),
        "the page's own survives"
    );
    assert!(
        fonts.contains_key(&Name::from("PdfrumC2")),
        "ours went beside it"
    );
    // And the original still names the original object.
    assert_eq!(
        fonts
            .raw(&Name::from("PdfrumC1"))
            .and_then(Object::as_ref_id),
        Some(ObjRef::new(5, 0))
    );
}

// The coordinate space, on an unrotated page: canvas (0,0) is the crop box's
// lower-left corner, so the `cm` is a plain translation by it.
#[test]
fn the_canvas_space_is_the_crop_box_on_an_unrotated_page() {
    let doc = Document::open("tests/fixtures/hello_world.pdf").expect("opens");
    let crop = doc.page(0).expect("page").crop_box();
    let mut edit = doc.edit();
    let mut seen = None;
    edit.draw_page(0, |c| {
        seen = Some(c.size());
        c.fill_rect(Rect::new(0.0, 0.0, 1.0, 1.0), Color::BLACK);
    })
    .expect("draws");
    assert_eq!(seen.expect("size").width, crop.width());
    assert_eq!(seen.expect("size").height, crop.height());

    let saved = round_trip(&edit);
    let text = contents(&saved, 0);
    let expected = format!("1 0 0 1 {} {} cm", crop.x0, crop.y0);
    assert!(text.contains(&expected), "want {expected:?} in {text}");
}

// The rotation half: a quarter-turned page swaps the canvas's sides, and
// canvas (0,0) still lands where a reader sees the bottom-left corner.
#[test]
fn a_rotated_page_swaps_the_canvas_sides() {
    let doc = Document::open("tests/fixtures/hello_world.pdf").expect("opens");
    let crop = doc.page(0).expect("page").crop_box();

    let mut edit = doc.edit();
    edit.set_rotation(0u32, 90).expect("rotates");
    let mut size = None;
    edit.draw_page(0, |c| {
        size = Some(c.size());
        c.fill_rect(Rect::new(0.0, 0.0, 1.0, 1.0), Color::BLACK);
    })
    .expect("draws");

    let size = size.expect("size");
    // The fixture is square, so this pins the swap only up to that; the
    // non-square case is `a_non_square_page_swaps_its_sides` below.
    assert_eq!(size.width, crop.height(), "sides swapped");
    assert_eq!(size.height, crop.width(), "sides swapped");

    // The composed transform is a proper rotation — no mirroring, which is
    // what would turn every drawn glyph backwards.
    let saved = round_trip(&edit);
    let n = canvas_transform(&saved, 0).as_coeffs();
    let determinant = n[0] * n[3] - n[1] * n[2];
    assert!((determinant - 1.0).abs() < 1e-6, "determinant +1: {n:?}");
}

// The swap, on a page whose sides actually differ.
#[test]
fn a_non_square_page_swaps_its_sides() {
    let doc = Document::open("tests/fixtures/hello_world.pdf").expect("opens");
    let mut edit = doc.edit();
    edit.set_page_box(
        0u32,
        pdfrum::PageBox::Crop,
        Rect::new(0.0, 0.0, 160.0, 40.0),
    )
    .expect("crops");
    edit.set_rotation(0u32, 90).expect("rotates");

    let mut size = None;
    edit.draw_page(0, |c| {
        size = Some(c.size());
        c.fill_rect(Rect::new(0.0, 0.0, 1.0, 1.0), Color::BLACK);
    })
    .expect("draws");
    let size = size.expect("size");
    assert_eq!((size.width, size.height), (40.0, 160.0));
}

// Every rotation places the canvas's four corners on the four corners of the
// page as displayed, which is the whole claim the coordinate space makes.
#[test]
fn every_rotation_places_the_canvas_on_the_displayed_page() {
    for degrees in [0, 90, 180, 270] {
        let doc = Document::open("tests/fixtures/hello_world.pdf").expect("opens");
        let crop = doc.page(0).expect("page").crop_box();
        let mut edit = doc.edit();
        edit.set_rotation(0u32, degrees).expect("rotates");

        let mut size = None;
        edit.draw_page(0, |c| {
            size = Some(c.size());
            c.fill_rect(Rect::new(0.0, 0.0, 1.0, 1.0), Color::BLACK);
        })
        .expect("draws");

        let saved = round_trip(&edit);
        let to_page = canvas_transform(&saved, 0);
        let size = size.expect("size");
        // The canvas's own rectangle, mapped to page space, is the crop box.
        let mapped = to_page.transform_rect_bbox(Rect::from_origin_size(Point::ZERO, size));
        for (got, want) in [
            (mapped.x0, crop.x0),
            (mapped.y0, crop.y0),
            (mapped.x1, crop.x1),
            (mapped.y1, crop.y1),
        ] {
            assert!(
                (got - want).abs() < 1e-6,
                "rotate {degrees}: {mapped:?} is not {crop:?}"
            );
        }

        // The bounding box alone does not pin the *orientation*, and a
        // canvas turned the wrong way still fills the same box — which is
        // exactly the bug this assertion was added for. Composing the
        // canvas transform with the display matrix must give the identity:
        // canvas space and displayed space are then the same space, corner
        // for corner and axis for axis.
        let display = pdfrum::PageRotation::from_degrees(i64::from(degrees)).display_matrix(crop);
        let composed = (display * to_page).as_coeffs();
        let identity = Affine::IDENTITY.as_coeffs();
        for (got, want) in composed.iter().zip(identity.iter()) {
            assert!(
                (got - want).abs() < 1e-6,
                "rotate {degrees}: canvas space is not displayed space: {composed:?}"
            );
        }
    }
}

// The canvas space is built from the *session's* page dictionary, not the
// base document's: a rotation set earlier in the same session is what the
// drawing is placed against. Reading the base instead placed every drawing on
// a rotated page a quarter turn out, which the rendered corners showed and the
// bounding box alone did not.
#[test]
fn a_rotation_set_this_session_reaches_the_canvas() {
    let doc = Document::open("tests/fixtures/hello_world.pdf").expect("opens");
    let mut edit = doc.edit();
    edit.set_rotation(0u32, 90).expect("rotates");
    edit.draw_page(0, |c| {
        c.fill_rect(Rect::new(0.0, 0.0, 1.0, 1.0), Color::BLACK);
    })
    .expect("draws");

    let saved = round_trip(&edit);
    let to_page = canvas_transform(&saved, 0);
    // Reading the base's `/Rotate 0` instead of the session's 90 wrote an
    // identity here, and every drawing landed a quarter turn out.
    assert_ne!(to_page, Affine::IDENTITY, "the session's rotation was read");
    let coefficients = to_page.as_coeffs();
    assert!(
        coefficients[0].abs() < 1e-6 && coefficients[1].abs() > 0.5,
        "a quarter turn, not a translation: {coefficients:?}"
    );
}

// `saved` is the only spelling of `q`/`Q`, so a stream's frames are balanced
// by construction. This checks the property on a deliberately nested drawing.
#[test]
fn every_frame_is_balanced() {
    let doc = Document::open("tests/fixtures/hello_world.pdf").expect("opens");
    let mut edit = doc.edit();
    edit.draw_page(0, |c| {
        c.saved(|c| {
            c.opacity(0.5);
            c.clip(Rect::new(0.0, 0.0, 50.0, 50.0), Fill::EvenOdd);
            c.saved(|c| {
                c.transform(Affine::rotate(0.3));
                c.fill_rect(Rect::new(0.0, 0.0, 10.0, 10.0), Color::BLACK);
            });
        });
    })
    .expect("draws");

    let saved = round_trip(&edit);
    let text = contents(&saved, 0);
    let mut depth = 0i32;
    for token in text.split_whitespace() {
        match token {
            "q" => depth += 1,
            "Q" => depth -= 1,
            _ => {}
        }
        assert!(depth >= 0, "a Q with no q: {text}");
    }
    assert_eq!(depth, 0, "unbalanced: {text}");
}

// Two pages drawn in one call each get their own canvas, and neither page's
// resources reach the other.
#[test]
fn drawing_every_page_gives_each_its_own_canvas() {
    let doc = Document::open("tests/fixtures/hello_world_2_pages.pdf").expect("opens");
    let mut edit = doc.edit();
    let font = edit.standard_font(StandardFont::Helvetica).expect("font");
    let mut pages = Vec::new();
    edit.draw_pages(|c| {
        pages.push(u32::from(c.page()));
        let label = format!("page {}", u32::from(c.page()) + 1);
        c.text(&label, &font, 9.0, Point::new(20.0, 20.0), Color::BLACK);
    })
    .expect("draws");
    assert_eq!(pages, vec![0, 1]);

    let saved = round_trip(&edit);
    assert!(
        saved
            .page(0)
            .expect("p0")
            .text()
            .to_string()
            .contains("page 1")
    );
    assert!(
        saved
            .page(1)
            .expect("p1")
            .text()
            .to_string()
            .contains("page 2")
    );
    assert!(
        !saved
            .page(0)
            .expect("p0")
            .text()
            .to_string()
            .contains("page 2")
    );
}

// Drawing twice on one page appends twice rather than replacing the first.
#[test]
fn two_drawings_both_land() {
    let doc = Document::open("tests/fixtures/hello_world.pdf").expect("opens");
    let elements = stream_count(&doc, 0);
    let mut edit = doc.edit();
    edit.draw_page(0, |c| {
        c.fill_rect(Rect::new(0.0, 0.0, 5.0, 5.0), Color::BLACK);
    })
    .expect("first");
    edit.draw_page(0, |c| {
        c.fill_rect(Rect::new(9.0, 9.0, 14.0, 14.0), Color::BLACK);
    })
    .expect("second");
    let saved = round_trip(&edit);
    assert_eq!(stream_count(&saved, 0), elements + 2);
    let text = contents(&saved, 0);
    assert!(text.contains("0 0 5 5 re"), "{text}");
    assert!(text.contains("9 9 5 5 re"), "{text}");
}

// A drawing that draws nothing writes nothing: no empty stream is appended.
#[test]
fn an_empty_drawing_writes_nothing() {
    let doc = Document::open("tests/fixtures/hello_world.pdf").expect("opens");
    let elements = stream_count(&doc, 0);
    let mut edit = doc.edit();
    edit.draw_page(0, |_| {}).expect("draws nothing");
    let saved = round_trip(&edit);
    assert_eq!(stream_count(&saved, 0), elements);
}

// The rounded rectangle, since it is the one shape that reaches the curve
// arm of the path writer.
#[test]
fn a_rounded_rectangle_writes_curves() {
    let doc = Document::open("tests/fixtures/hello_world.pdf").expect("opens");
    let mut edit = doc.edit();
    edit.draw_page(0, |c| {
        c.fill_rounded_rect(Rect::new(10.0, 10.0, 90.0, 40.0), 8.0, Color::BLACK);
    })
    .expect("draws");
    let saved = round_trip(&edit);
    let text = contents(&saved, 0);
    assert!(text.contains(" c"), "cubic segments: {text}");
    assert!(text.contains(" f"), "filled: {text}");
}

// Opacity reaches an `/ExtGState`, since `rg` carries no alpha.
#[test]
fn a_translucent_fill_writes_an_ext_g_state() {
    let doc = Document::open("tests/fixtures/hello_world.pdf").expect("opens");
    let mut edit = doc.edit();
    edit.draw_page(0, |c| {
        c.fill_rect(
            Rect::new(0.0, 0.0, 50.0, 50.0),
            Color::new([1.0, 0.0, 0.0, 0.25]),
        );
    })
    .expect("draws");
    let saved = round_trip(&edit);
    let states = resources(&saved, 0)
        .dict(&Name::from("ExtGState"), saved.parser())
        .expect("an /ExtGState was merged");
    let (_, state) = states.iter().next().expect("one entry");
    let state = state.as_dict().expect("a dictionary");
    assert_eq!(state.raw(&Name::from("ca")), Some(&Object::Real(0.25)));
    assert_eq!(state.raw(&Name::from("CA")), Some(&Object::Real(0.25)));
}

// The one measurement the API offers, and the reason it exists: centring a
// single string.
#[test]
fn text_width_measures_one_string() {
    let doc = Document::open("tests/fixtures/hello_world.pdf").expect("opens");
    let mut edit = doc.edit();
    let font = edit.standard_font(StandardFont::Helvetica).expect("font");
    edit.draw_page(0, |c| {
        let narrow = c.text_width("i", &font, 12.0);
        let wide = c.text_width("MMMM", &font, 12.0);
        assert!(wide > narrow, "{wide} should exceed {narrow}");
        // Linear in the size, which is what a caller scaling a stamp relies on.
        let doubled = c.text_width("MMMM", &font, 24.0);
        assert!((doubled - wide * 2.0).abs() < 1e-6);
    })
    .expect("draws");
}

// An image placed on the canvas reaches the page as an `/XObject` `Do`.
#[test]
fn an_image_lands_as_an_xobject() {
    let doc = Document::open("tests/fixtures/hello_world.pdf").expect("opens");
    let mut edit = doc.edit();
    let image = edit
        .embed_jpeg(include_bytes!("fixtures/mona_lisa.jpg"))
        .expect("jpeg");
    edit.draw_page(0, |c| {
        c.image(&image, Rect::new(20.0, 20.0, 80.0, 80.0));
    })
    .expect("draws");

    let saved = round_trip(&edit);
    let text = contents(&saved, 0);
    assert!(text.contains(" Do"), "{text}");
    assert!(
        text.contains("60 0 0 60 20 20 cm"),
        "placed on the rect: {text}"
    );
    assert!(
        resources(&saved, 0)
            .dict(&Name::from("XObject"), saved.parser())
            .is_some_and(|d| !d.is_empty()),
        "the image merged into /XObject"
    );
}

// Paint's three cases pick the three paint operators.
#[test]
fn paint_selects_the_operator() {
    let doc = Document::open("tests/fixtures/hello_world.pdf").expect("opens");
    let mut edit = doc.edit();
    edit.draw_page(0, |c| {
        c.draw(
            Rect::new(0.0, 0.0, 10.0, 10.0),
            Paint::FillStroke(Color::BLACK, Stroke::new(Color::BLACK, 1.0)),
            Fill::EvenOdd,
        );
    })
    .expect("draws");
    let text = contents(&round_trip(&edit), 0);
    assert!(text.contains(" B*"), "{text}");
}
