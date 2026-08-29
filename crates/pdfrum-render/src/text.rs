//! Text as filled glyph outlines (`ProcessText`,
//! `cpdf_renderstatus.cpp:824-930`, and `CFX_RenderDevice::DrawTextPath`).
//!
//! # The one large deliberate divergence
//!
//! Below `|char2device.a| + |char2device.b| > 50` the oracle rasterizes
//! *hinted FreeType glyph bitmaps* with LCD filtering, an integer-origin
//! nudge and a 256-entry gamma table. Reproducing that would mean porting
//! FreeType's hinter, which is explicitly out of scope. We render every glyph
//! as a filled `BezPath` at every size. The **geometry** — glyph origins,
//! advances, matrices — is unchanged and must match exactly; only the
//! coverage on stem edges differs, which is why text pixels are Tier B and
//! never Tier A.
//!
//! With the oracle's own flags (`bClearType` false, `bNoTextSmooth` false)
//! the aliasing type is plain grayscale antialiasing, so subpixel text never
//! runs in conformance at all — a large simplification, and the reason the
//! reconciliation needed is coverage-level rather than layout-level.

use kurbo::{Affine, BezPath};
use pdfrum_font::{Font, GlyphCache, GlyphKey};
use pdfrum_page::{TextObject, TextRenderMode};

/// Which of fill, stroke and clip a text render mode asks for
/// (`cpdf_renderstatus.cpp:752-761`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextPaintKinds {
    /// Whether the glyphs are filled.
    pub fill: bool,
    /// Whether they are stroked.
    pub stroke: bool,
    /// Whether they contribute to the clip.
    pub clip: bool,
}

/// Resolve a text render mode into what it paints.
///
/// `has_face` is whether the font has real outlines: a stroke-only mode on a
/// font without them **falls back to a fill**, which is upstream's own
/// substitution and not a rounding of intent.
#[must_use]
pub fn paint_kinds(mode: TextRenderMode, has_face: bool) -> Option<TextPaintKinds> {
    let none = TextPaintKinds {
        fill: false,
        stroke: false,
        clip: false,
    };
    match mode {
        // Tr 3: nothing at all, not even a clip contribution.
        TextRenderMode::Invisible => None,
        // Tr 7: clip only, no paint. Returns early in the C++ *before* the
        // clip accumulation, so it paints nothing here either.
        TextRenderMode::Clip => Some(TextPaintKinds { clip: true, ..none }),
        TextRenderMode::Fill => Some(TextPaintKinds { fill: true, ..none }),
        TextRenderMode::FillClip => Some(TextPaintKinds {
            fill: true,
            clip: true,
            ..none
        }),
        TextRenderMode::Stroke => Some(if has_face {
            TextPaintKinds {
                stroke: true,
                ..none
            }
        } else {
            TextPaintKinds { fill: true, ..none }
        }),
        TextRenderMode::StrokeClip => Some(if has_face {
            TextPaintKinds {
                stroke: true,
                clip: true,
                ..none
            }
        } else {
            TextPaintKinds {
                fill: true,
                clip: true,
                ..none
            }
        }),
        TextRenderMode::FillStroke => Some(TextPaintKinds {
            fill: true,
            stroke: has_face,
            ..none
        }),
        TextRenderMode::FillStrokeClip => Some(TextPaintKinds {
            fill: true,
            stroke: has_face,
            clip: true,
        }),
    }
}

/// One glyph, placed.
#[derive(Debug, Clone)]
pub struct PlacedGlyph {
    /// The outline in 1000-unit text space, straight from the cache.
    pub outline: BezPath,
    /// Text space to device space for this glyph, font size included.
    pub matrix: Affine,
}

impl PlacedGlyph {
    /// The outline in device space.
    #[must_use]
    pub fn device_path(&self) -> BezPath {
        self.matrix * self.outline.clone()
    }
}

/// The matrix one glyph is drawn under.
///
/// Three spaces compose. Outlines arrive scaled to **1000 units per em**, so
/// the font size divides by a thousand to reach text space. `pen` is the
/// glyph's origin in that same text space, where the pen advances left to
/// right and y grows *upward*. And `text_to_device` — which is
/// `pdfrum-page`'s `TextObject::matrix`, already carrying the CTM, the text
/// matrix and the horizontal scale, composed with the page-to-device
/// transform — takes it the rest of the way.
///
/// There is deliberately **no y flip here**: PDF text space and PDF user
/// space share their orientation, and the single flip that turns y-up into a
/// y-down device lives in the page matrix, where every object kind sees it.
/// Flipping again per glyph mirrors every letter about its own baseline.
#[must_use]
pub fn glyph_matrix(font_size: f32, pen: kurbo::Point, text_to_device: Affine) -> Affine {
    let s = f64::from(font_size) / 1000.0;
    text_to_device * Affine::translate((pen.x, pen.y)) * Affine::scale(s)
}

/// The stroked-text CTM un-transform (`cpdf_renderstatus.cpp:772-777`).
///
/// A stroke's width must be measured in *text* space, so when the text
/// state's CTM carries a non-unit x or y scale the text matrix is pre-divided
/// by it and the scale is folded into the device matrix instead. Returns the
/// adjusted `(text_matrix, device_matrix)` pair.
#[must_use]
#[expect(
    clippy::float_cmp,
    reason = "the exact `a == 1 && d == 1` is upstream's unit-scale short \
              circuit; with a tolerance a slightly-off-unit CTM would skip \
              the split and stroke at the wrong width, which is the whole \
              point of the function"
)]
pub fn stroke_ctm_split(text_matrix: Affine, to_device: Affine, ctm: [f64; 4]) -> (Affine, Affine) {
    let [a, b, c, d] = ctm;
    if a == 1.0 && d == 1.0 {
        return (text_matrix, to_device);
    }
    let scale = Affine::new([a, b, c, d, 0.0, 0.0]);
    let det = scale.determinant();
    if det == 0.0 || !det.is_finite() {
        return (text_matrix, to_device);
    }
    (text_matrix * scale.inverse(), scale * to_device)
}

/// Lay out one text object's glyphs.
///
/// # The two coordinate systems this has to keep straight
///
/// `pdfrum-page` hands a text object *two* pieces of placement, and they are
/// in different spaces. `TextObject::matrix` is `ctm * text_matrix *
/// horizontal_scale` — **text space to page space**, with no font size —
/// while `TextObject::position` is `ctm * text_matrix` already applied to
/// the pen, i.e. a **page-space** point.
///
/// Composing the two naively applies the matrix twice, which shifts the run
/// wherever the text matrix has a translation and is invisible wherever it
/// does not — so it survives every fixture whose `Tm` is the identity. The
/// run's origin is therefore recovered by pulling `position` *back* through
/// the matrix, and the pen then advances in text space where the advances
/// are actually defined.
///
/// Advances follow the same rules `pdfrum-page` used to build the object:
/// each code's width scaled by the font size, plus the character spacing,
/// plus the word spacing on a single-byte space only, plus any kerning
/// between segments. The horizontal scale is *not* applied again, because
/// `matrix` already carries it.
#[must_use]
pub fn place_glyphs(
    object: &TextObject,
    state: &pdfrum_page::GraphicsState,
    cache: &mut GlyphCache,
    to_device: Affine,
) -> Vec<PlacedGlyph> {
    let Some((font, size)) = &object.font else {
        return Vec::new();
    };
    let text_to_device = to_device * object.matrix;
    // Pull the page-space start back into the text space the advances live
    // in. A singular text matrix has no text space to speak of, and the
    // object would not have been drawable anyway.
    let det = object.matrix.determinant();
    if det == 0.0 || !det.is_finite() {
        return Vec::new();
    }
    let mut pen = object.matrix.inverse() * object.position;
    let mut out = Vec::new();

    for segment in &object.segments {
        // A kerning adjustment shifts the pen before the segment it precedes,
        // and is negated: a positive `TJ` number moves text *left*.
        pen.x -= f64::from(segment.kerning) / 1000.0 * f64::from(*size);
        for item in font.decode(&segment.codes) {
            let advance = f64::from(item.width) / 1000.0 * f64::from(*size)
                + f64::from(state.text.char_space);
            // Word spacing applies to a single-byte space only, which is why
            // a CID-keyed code of 0x20 does not earn it.
            let word = if item.code.0 == 0x20 && item.cid.is_none() {
                f64::from(state.text.word_space)
            } else {
                0.0
            };
            if let Some(gid) = item.glyph() {
                let key = GlyphKey::plain(font.id(), gid);
                if let Some(outline) = cache.path(font, key) {
                    out.push(PlacedGlyph {
                        outline: outline.clone(),
                        matrix: glyph_matrix(*size, pen, text_to_device),
                    });
                }
            }
            pen.x += advance + word;
        }
    }
    out
}

/// Whether a font has real outlines, which decides the stroke-to-fill
/// fallback above.
#[must_use]
pub fn has_face(font: &Font) -> bool {
    !matches!(font, Font::Type3(_))
}

/// One Type 3 character, placed.
///
/// A Type 3 glyph is a *content stream*, not an outline, so what a placement
/// yields is the character's code — with which the caller looks up the
/// procedure's objects — and the matrix taking glyph space to device space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlacedType3Char {
    /// The character code, keying `TextObject::type3_metrics`.
    pub code: u32,
    /// Glyph space to device space, `font_matrix * font_size` composed with
    /// the pen position and the text-to-device transform.
    pub matrix: Affine,
}

/// Lay out one Type 3 text object's characters.
///
/// The matrix differs from an ordinary glyph's in one way that matters: an
/// outline arrives pre-scaled to 1000 units per em, so [`glyph_matrix`]
/// divides the font size by a thousand. A Type 3 procedure's coordinates are
/// in **glyph space**, whose relationship to text space is stated by the
/// font's own `/FontMatrix` and is not a thousandth in general — a font may
/// declare any matrix at all, and several in the corpus do. So the font
/// matrix is composed in explicitly and the font size scales it, which is
/// `char_matrix = font_matrix scaled by (font_size, font_size)`.
///
/// Advances still come from `/Widths` on the thousandth convention, matching
/// how `pdfrum-page` computed the object's own advance; making the two
/// disagree would slide a Type 3 run relative to the pen the page recorded.
#[must_use]
pub fn place_type3_chars(
    object: &TextObject,
    state: &pdfrum_page::GraphicsState,
    to_device: Affine,
) -> Vec<PlacedType3Char> {
    let Some((font, size)) = &object.font else {
        return Vec::new();
    };
    let Some(type3) = font.type3() else {
        return Vec::new();
    };
    let text_to_device = to_device * object.matrix;
    let det = object.matrix.determinant();
    if det == 0.0 || !det.is_finite() {
        return Vec::new();
    }
    let char_matrix = type3.font_matrix * Affine::scale(f64::from(*size));
    let mut pen = object.matrix.inverse() * object.position;
    let mut out = Vec::new();

    for segment in &object.segments {
        pen.x -= f64::from(segment.kerning) / 1000.0 * f64::from(*size);
        for item in font.decode(&segment.codes) {
            let advance = f64::from(item.width) / 1000.0 * f64::from(*size)
                + f64::from(state.text.char_space);
            let word = if item.code.0 == 0x20 && item.cid.is_none() {
                f64::from(state.text.word_space)
            } else {
                0.0
            };
            out.push(PlacedType3Char {
                code: item.code.0,
                matrix: text_to_device * Affine::translate((pen.x, pen.y)) * char_matrix,
            });
            pen.x += advance + word;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use kurbo::Point;

    use super::*;

    #[test]
    fn invisible_paints_nothing() {
        assert_eq!(paint_kinds(TextRenderMode::Invisible, true), None);
    }

    #[test]
    fn clip_only_mode_paints_nothing_but_clips() {
        let k = paint_kinds(TextRenderMode::Clip, true).expect("Tr 7 is not invisible");
        assert!(!k.fill && !k.stroke && k.clip);
    }

    #[test]
    fn stroke_without_a_face_falls_back_to_fill() {
        let with = paint_kinds(TextRenderMode::Stroke, true).expect("some");
        assert!(with.stroke && !with.fill);
        let without = paint_kinds(TextRenderMode::Stroke, false).expect("some");
        assert!(
            without.fill && !without.stroke,
            "no outlines means fill instead"
        );
    }

    #[test]
    fn fill_stroke_keeps_the_fill_when_there_is_no_face() {
        let k = paint_kinds(TextRenderMode::FillStroke, false).expect("some");
        assert!(k.fill);
        assert!(!k.stroke, "only the stroke half is dropped");
    }

    #[test]
    fn every_clip_mode_contributes_to_the_clip() {
        for mode in [
            TextRenderMode::FillClip,
            TextRenderMode::StrokeClip,
            TextRenderMode::FillStrokeClip,
            TextRenderMode::Clip,
        ] {
            assert!(paint_kinds(mode, true).is_some_and(|k| k.clip), "{mode:?}");
        }
        for mode in [
            TextRenderMode::Fill,
            TextRenderMode::Stroke,
            TextRenderMode::FillStroke,
        ] {
            assert!(paint_kinds(mode, true).is_some_and(|k| !k.clip), "{mode:?}");
        }
    }

    #[test]
    fn glyph_matrix_scales_by_size_over_1000_without_flipping() {
        // Outlines are 1000 units per em, so a full em at size 1000 is a
        // whole unit of text space, unmirrored: the single y flip lives in
        // the page matrix, where every object kind sees it. Flipping here
        // too would mirror each letter about its own baseline.
        let m = glyph_matrix(1000.0, Point::ZERO, Affine::IDENTITY);
        let p = m * Point::new(0.0, 1000.0);
        assert!(
            (p.y - 1000.0).abs() < 1e-9,
            "y is not flipped here: {}",
            p.y
        );
        assert!((p.x - 0.0).abs() < 1e-9);

        let half = glyph_matrix(500.0, Point::ZERO, Affine::IDENTITY);
        let p = half * Point::new(1000.0, 0.0);
        assert!((p.x - 500.0).abs() < 1e-9, "half size halves the advance");
    }

    #[test]
    fn the_pen_translates_in_text_space_before_the_size_scale() {
        // A pen at x = 40 with a size of 12 puts the glyph's own origin at
        // 40 text units, not at 40 * 12 / 1000.
        let m = glyph_matrix(12.0, Point::new(40.0, 0.0), Affine::IDENTITY);
        let origin = m * Point::ZERO;
        assert!((origin.x - 40.0).abs() < 1e-9, "origin at {}", origin.x);
    }

    #[test]
    fn stroke_ctm_split_is_identity_at_unit_scale() {
        let text = Affine::translate((3.0, 4.0));
        let device = Affine::scale(2.0);
        let (t, d) = stroke_ctm_split(text, device, [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(t, text);
        assert_eq!(d, device);
    }

    #[test]
    fn stroke_ctm_split_moves_the_scale_into_the_device_matrix() {
        let text = Affine::IDENTITY;
        let device = Affine::IDENTITY;
        let (t, d) = stroke_ctm_split(text, device, [2.0, 0.0, 0.0, 3.0]);
        // The product is unchanged — the split only moves where the scale
        // lives, so the glyph lands in the same place but the stroke width is
        // measured in text space.
        let composed = t * d;
        for (a, b) in composed
            .as_coeffs()
            .iter()
            .zip(Affine::IDENTITY.as_coeffs().iter())
        {
            assert!((a - b).abs() < 1e-9, "{composed:?}");
        }
        assert!(
            (d.as_coeffs()[0] - 2.0).abs() < 1e-9,
            "the x scale moved to the device matrix"
        );
    }

    #[test]
    fn a_degenerate_ctm_leaves_the_matrices_alone() {
        let (t, d) = stroke_ctm_split(Affine::IDENTITY, Affine::IDENTITY, [0.0, 0.0, 0.0, 0.0]);
        assert_eq!(t, Affine::IDENTITY);
        assert_eq!(d, Affine::IDENTITY);
    }
}
