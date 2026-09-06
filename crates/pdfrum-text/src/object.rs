//! Typeset representation of a page text object.
//!
//! Derives glyph positions, advances, and bounding boxes from a
//! [`pdfrum_page::TextObject`] for extraction heuristics.

// `pdfrum-page` hands over a `TextObject` holding the *content stream's*
// view: byte strings with the adjustments between them, one position, one
// matrix. The extraction heuristics want the *typeset* view: one character
// code per glyph, the text-space x each one sits at, the adjustment that
// followed it, and the object's bounding box.
//
// Deriving that here rather than storing it in the page crate is the Q2
// resolution: it is a pure function of data the page crate already publishes,
// it is only ever wanted by this crate, and the page object stays a small
// record (STYLE.md §1). The derivation is one pass with a running pen —
// the same accumulation the renderer performs when it draws the run, and
// the same one `CPDF_TextObject::CalcPositionDataInternal` performs to fill
// its own arrays.
//
// # Form objects and the composed matrix
//
// The C++ threads a separate `form_matrix` through the whole pipeline
// because its text objects carry positions in the *form's* space. Ours do
// not: `pdfrum-page` composes a form's `/Matrix` into the CTM before
// interpreting its content, so a text object inside a form already reports
// page-space geometry. The form matrix is therefore the identity everywhere
// in this crate — which is exactly what makes a generated character's matrix
// come out as the identity, as the oracle's `GetMatrix` assertions require.

use crate::charinfo::{ObjectIndex, transform_rect};
use kurbo::{Affine, Point, Rect};
use pdfrum_font::{CharCode, Font};
use pdfrum_page::{Content, PageObject, TextObject, TextRenderMode};
use std::sync::Arc;

/// One glyph's place in a text object.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Item {
    /// The character code.
    pub code: CharCode,
    /// The origin in the object's own text space. For horizontal writing
    /// this is `(pen, 0)`; for a vertical CID font the pen moves down y and
    /// the glyph's vertical origin shifts it.
    pub origin: Point,
}

/// A text object with everything the heuristics ask of it.
///
/// Built once per object per page and then read many times, because almost
/// every decision in the pipeline needs at least the first and last item.
#[derive(Debug, Clone)]
pub struct TextRun {
    /// Where this object sits in the page's flattened walk.
    pub index: ObjectIndex,
    /// The font, shared with every other object using the same resource —
    /// duplicate suppression compares fonts by this pointer's identity.
    pub font: Arc<Font>,
    /// The font size. **May be negative**, and is never rescued.
    pub font_size: f32,
    /// `|hypot(a, b)| * font_size` of the composed matrix: "the horizontal
    /// scale of the font in device units", which every space threshold is
    /// measured in.
    pub font_size_h: f32,
    /// `Tc`, the character spacing.
    pub char_space: f32,
    /// `Tw`, the word spacing.
    pub word_space: f32,
    /// One entry per glyph, in content order.
    pub items: Vec<Item>,
    /// The adjustment following each glyph, in thousandths of a text-space
    /// unit. Same length as [`items`](Self::items); the last is always zero.
    pub kernings: Vec<f32>,
    /// The object's position in page space.
    pub position: Point,
    /// The composed text matrix: the object's linear transform with its
    /// position as the translation.
    pub text_matrix: Affine,
    /// The object's bounding box in page space, stroke-inflated when the
    /// render mode strokes.
    pub rect: Rect,
    /// The marks enclosing the object.
    pub marks: pdfrum_page::ContentMarks,
    /// What each shown character's Type 3 glyph procedure declared, empty for
    /// every other kind of font. A Type 3 glyph's box and advance live inside
    /// a content stream, so they arrive from the page layer rather than from
    /// the font.
    pub type3: std::collections::BTreeMap<u32, pdfrum_page::Type3Metrics>,
}

impl TextRun {
    /// How many glyphs the object shows.
    #[must_use]
    pub fn count(&self) -> usize {
        self.items.len()
    }

    /// One glyph, or `None` past the end.
    #[must_use]
    pub fn item(&self, index: usize) -> Option<Item> {
        self.items.get(index).copied()
    }

    /// The adjustment following a glyph; zero past the end.
    #[must_use]
    pub fn kerning(&self, index: usize) -> f32 {
        self.kernings.get(index).copied().unwrap_or(0.0)
    }

    /// The advance width of one character code, already scaled by
    /// `font_size / 1000`.
    ///
    /// A vertical CID font reports its (negative) vertical advance instead.
    #[must_use]
    pub fn scaled_char_width(&self, code: CharCode) -> f32 {
        let scale = self.font_size / 1000.0;
        if self.font.is_vertical()
            && let Some(width) = self.font.vert_width(code)
        {
            return width * scale;
        }
        self.glyph_width(code) * scale
    }

    /// The advance width of one character code in glyph units.
    ///
    /// A Type 3 font whose `/Widths` said nothing defers to what the glyph
    /// procedure's `d0`/`d1` declared, which is the only place the number
    /// exists.
    #[must_use]
    pub fn glyph_width(&self, code: CharCode) -> f32 {
        let declared = self.font.char_width(code);
        if declared != 0.0 || self.font.type3().is_none() {
            return declared;
        }
        self.type3.get(&code.0).map_or(0.0, |m| m.width)
    }

    /// The bounding box of one character code in glyph units, y-up.
    #[must_use]
    pub fn glyph_bbox(&self, code: CharCode) -> Rect {
        if self.font.type3().is_some()
            && let Some(metrics) = self.type3.get(&code.0)
        {
            return metrics.bbox;
        }
        self.font.char_bbox(code)
    }
}

/// A width in glyph units as the extractor's ladder yields it: a **whole
/// number**, because every rung of PDFium's `GetCharWidth` is `int`
/// (`core/fpdftext/cpdf_textpage.cpp:185-208`).
///
/// The type exists so that the integrality is stated once, here, at the
/// ladder's exit — rather than left implicit in what the width sources
/// happen to store, or spelled as an `as i32` scattered over the call sites.
/// Everything downstream (the space threshold, the newline test, the dedup
/// test) is arithmetic PDFium does on an `int` that stays integral until it
/// is scaled by the font size, so those callers take [`GlyphWidth::as_f64`]
/// and cannot see a fraction the C++ does not have.
///
/// Our own geometry — the pen advance in [`build`], glyph boxes — keeps its
/// `f32` and is untouched by this type: there the fraction is real and
/// PDFium keeps it too.
///
/// # Where the truncation lives on each side — and why this is a no-op
///
/// PDFium never rounds a float here, because it never holds one: a simple
/// font's `/Widths` entry is read with `CPDF_Array::GetIntegerAt`
/// (`core/fpdfapi/parser/cpdf_array.cpp:147-151`), which is
/// `FX_Number::GetSigned`'s `saturated_cast<int32_t>` over the parsed float
/// (`core/fxcrt/fx_number.cpp:97-105`) — a **truncation toward zero**, not a
/// round. A CID font's widths come from an already-integer `width_list_`
/// (`cpdf_cidfont.cpp:573-585`), and rung three is `FX_RECT::Width()`, an
/// integer subtraction.
///
/// **So does ours, already.** `pdfrum-font` truncates at the same place
/// PDFium does, at parse: `SimpleWidths` stores `raw: [u16; 256]` filled
/// from `Array::int_at` (`crates/pdfrum-font/src/widths.rs`), `CidWidths`
/// keeps `records: Vec<[i32; 3]>`, and the face fallback is
/// `f32::from(advance_tt(gid) as i16)`
/// (`crates/pdfrum-font/src/simple/mod.rs:111-130`). Every value that
/// reaches this ladder is therefore already a whole number in an `f32`, and
/// truncating it changes nothing.
///
/// That was measured, not assumed. With this type's truncation instrumented
/// to report any fractional input, **zero fired across 1 420 PDFs** — the
/// 44-file benchmark corpus and the 1 376-file conformance corpus — and the
/// 1 759-file board came back byte-identical, every row, as did all 44
/// benchmark text rows.
///
/// The type is kept anyway, because it turns that agreement from an accident
/// of two crates into something the compiler holds: the ladder's output can
/// no longer acquire a fraction, whatever a future width source does, and a
/// caller cannot silently multiply one in. It documents the invariant at the
/// boundary where the two engines have to agree.
///
/// # Not the oracle-bug case either
///
/// ISO 32000-1 §9.2.4 Table 111 gives `/Widths` as *numbers*, so a
/// fractional width is legal and PDFium's integer read would lose it. But
/// PDFium loses it **everywhere**, not only in extraction: the glyph pen in
/// `CPDF_TextObject::CalcPositionDataInternal` advances by the same
/// `font->GetCharWidth` int (`core/fpdfapi/page/cpdf_textobject.cpp:185`,
/// `:250-255`, `:333`). The integer is the width PDFium *positions the
/// glyphs with*, so the extractor's "is this gap wider than a character"
/// rule is asking about the geometry actually on the page, and matching it
/// is matching the input to a heuristic rather than adopting a wrong width.
/// The question stays hypothetical here regardless: no corpus file has a
/// fractional declared width to lose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct GlyphWidth(i32);

impl GlyphWidth {
    /// Zero — the ladder's answer for no code, an invalid code, and a
    /// nonsense bounding box.
    pub const ZERO: Self = Self(0);

    /// Truncates a glyph-unit width toward zero, which is what
    /// `saturated_cast<int32_t>` does to the float a `/Widths` entry parsed
    /// to. A non-finite width is nonsense and yields zero.
    #[must_use]
    fn truncating(width: f32) -> Self {
        if !width.is_finite() {
            return Self::ZERO;
        }
        #[expect(
            clippy::cast_possible_truncation,
            reason = "the saturating cast is `FX_Number::GetSigned`; a width outside i32 is nonsense"
        )]
        let truncated = width.trunc() as i32;
        Self(truncated)
    }

    /// The width as the `f64` every downstream threshold scales by the font
    /// size, which is `nLastWidth * GetFontSize() / 1000` on the C++ side
    /// (`cpdf_textpage.cpp:1234-1238`).
    #[must_use]
    pub fn as_f64(self) -> f64 {
        f64::from(self.0)
    }

    /// Whether the rung produced a usable width, i.e. `w > 0` — the test
    /// each of `GetCharWidth`'s first two exits makes before returning.
    #[must_use]
    fn is_positive(self) -> bool {
        self.0 > 0
    }
}

/// The width of one character code in glyph units, through the extractor's
/// own three-rung fallback ladder (`GetCharWidth`, design brief §1.7c).
///
/// Distinct from [`Font::char_width`], which is only the first rung and stays
/// fractional. The second re-encodes the code to bytes and re-decodes them,
/// so it differs from the first exactly when that round trip is lossy — a
/// simple font's code above 255, say. The third falls back to the glyph's
/// bounding box.
///
/// Returns a [`GlyphWidth`], an integer, because PDFium's `GetCharWidth`
/// returns `int` at all three of its exits — see that type for which rung
/// truncates on each side, why the truncation is measurably a no-op on every
/// corpus here, and why this is not an oracle bug.
#[must_use]
pub fn ladder_char_width(run: &TextRun, code: Option<CharCode>) -> GlyphWidth {
    let Some(code) = code else {
        return GlyphWidth::ZERO;
    };
    let font = &run.font;
    // Rung one: the font's own declared width. PDFium's is an int because
    // `/Widths` was read with `GetIntegerAt`; ours is an `f32` that
    // `pdfrum-font` has already made integral at the same point. The
    // truncation here is the type-level restatement of that, not a change.
    let width = GlyphWidth::truncating(run.glyph_width(code));
    if width.is_positive() {
        return width;
    }
    // Rung two: the round trip through the encoding. `GetStringWidth` sums
    // `GetCharWidth` over the re-decoded codes, so the C++ sums *integers*.
    // Our `string_width` sums the same per-code values, which `pdfrum-font`
    // already stores as whole numbers, so summing then truncating and
    // truncating then summing agree — there is no fraction to carry across
    // the sum.
    let mut bytes = Vec::new();
    font.append_char(&mut bytes, code);
    let width = GlyphWidth::truncating(font.string_width(&bytes));
    if width.is_positive() {
        return width;
    }
    // Rung three: `std::max(rect.Width(), 0)` over an `FX_RECT`, whose
    // `Width()` is already an integer subtraction. `FX_RECT::Valid`'s
    // overflow check has no analogue on an `f64` rect; a non-finite box is
    // the equivalent nonsense and yields zero, which `truncating` gives.
    let bbox = run.glyph_bbox(code);
    #[expect(
        clippy::cast_possible_truncation,
        reason = "glyph-unit widths are small integers"
    )]
    let width = (bbox.x1 - bbox.x0) as f32;
    GlyphWidth::truncating(width).max(GlyphWidth::ZERO)
}

/// Builds the extraction view of one text object.
///
/// Returns `None` for an object with no font, which cannot show anything.
#[must_use]
pub(crate) fn build(content: &Content<TextObject>, index: ObjectIndex) -> Option<TextRun> {
    let object = &content.object;
    let (font, font_size) = object.font.as_ref()?;
    let state = &content.state;

    // One item per decoded character code, and the adjustment that followed
    // the *string* attaches to its last character.
    let mut items: Vec<CharCode> = Vec::new();
    let mut kernings: Vec<f32> = Vec::new();
    for segment in &object.segments {
        let before = items.len();
        for item in font.decode(&segment.codes) {
            items.push(item.code);
            kernings.push(0.0);
        }
        if items.len() > before
            && let Some(last) = kernings.last_mut()
        {
            *last = segment.kerning;
        }
    }
    // The C++'s `SetSegments` leaves the final kerning at zero whatever the
    // array said, because there is no glyph after it to displace.
    if let Some(last) = kernings.last_mut() {
        *last = 0.0;
    }

    let [a, b, ..] = object.matrix.as_coeffs();
    #[expect(
        clippy::cast_possible_truncation,
        reason = "matrix coefficients are page-space floats"
    )]
    let font_size_h = (a.hypot(b) as f32 * font_size).abs();

    let mut run = TextRun {
        index,
        font: Arc::clone(font),
        font_size: *font_size,
        font_size_h,
        char_space: state.text.char_space,
        word_space: state.text.word_space,
        items: Vec::with_capacity(items.len()),
        kernings,
        position: object.position,
        // The object's matrix carries no translation of its own; the
        // position is the translation, exactly as `GetTextMatrix` assembles
        // it from the stored four coefficients plus `pos_`.
        text_matrix: with_translation(object.matrix, object.position),
        rect: Rect::ZERO,
        marks: content.marks.clone(),
        type3: object.type3_metrics.clone(),
    };
    layout(
        &mut run,
        &items,
        object.render_mode,
        state.stroke_params.width,
    );
    Some(run)
}

/// Replaces a matrix's translation, leaving its linear part alone.
fn with_translation(matrix: Affine, position: Point) -> Affine {
    let [a, b, c, d, ..] = matrix.as_coeffs();
    Affine::new([a, b, c, d, position.x, position.y])
}

/// Walks the pen across the run, filling in item origins and both boxes.
///
/// This is `CalcPositionDataInternal`: the pen advances by the glyph's width,
/// then by the word space when the code is a single-byte space, then by the
/// character space, then *back* by the adjustment. The bounding box grows
/// from the glyph boxes along the way, and the two writing directions
/// accumulate their extents in opposite roles.
fn layout(run: &mut TextRun, codes: &[CharCode], mode: TextRenderMode, line_width: f32) {
    let vertical = run.font.is_vertical();
    let font_size = run.font_size;
    let (mut min_x, mut max_x) = (10000.0f32, -10000.0f32);
    let (mut min_y, mut max_y) = (10000.0f32, -10000.0f32);
    let mut pen = 0.0f32;

    for (index, &code) in codes.iter().enumerate() {
        let bbox = run.glyph_bbox(code);
        #[expect(
            clippy::cast_possible_truncation,
            reason = "glyph boxes are 1000/em integers"
        )]
        let (bl, bb, br, bt) = (
            bbox.x0 as f32,
            bbox.y0 as f32,
            bbox.x1 as f32,
            bbox.y1 as f32,
        );

        let width = if vertical {
            let (ox, oy) = run.font.vert_origin(code).unwrap_or((0.0, 880.0));
            // The vertical origin shifts the glyph box before it is measured.
            let (left, right) = (bl - ox, br - ox);
            let (top, bottom) = (bt - oy, bb - oy);
            min_x = min_x.min(left).min(right);
            max_x = max_x.max(left).max(right);
            let char_top = pen + top * font_size / 1000.0;
            let char_bottom = pen + bottom * font_size / 1000.0;
            min_y = min_y.min(char_top).min(char_bottom);
            max_y = max_y.max(char_top).max(char_bottom);
            run.items.push(Item {
                code,
                origin: Point::new(
                    f64::from(-(font_size * ox / 1000.0)),
                    f64::from(pen - font_size * oy / 1000.0),
                ),
            });
            run.font.vert_width(code).unwrap_or(-1000.0) * font_size / 1000.0
        } else {
            min_y = min_y.min(bt).min(bb);
            max_y = max_y.max(bt).max(bb);
            let char_left = pen + bl * font_size / 1000.0;
            let char_right = pen + br * font_size / 1000.0;
            min_x = min_x.min(char_left).min(char_right);
            max_x = max_x.max(char_left).max(char_right);
            run.items.push(Item {
                code,
                origin: Point::new(f64::from(pen), 0.0),
            });
            run.glyph_width(code) * font_size / 1000.0
        };

        pen += width;
        // Word spacing applies to a **single-byte** space only, which for a
        // composite font means one whose CMap gives that code one byte.
        if code.0 == 0x20 && (!vertical || run.font.cid_from_charcode(code).is_none()) {
            let mut encoded = Vec::new();
            run.font.append_char(&mut encoded, code);
            if encoded.len() == 1 {
                pen += run.word_space;
            }
        }
        pen += run.char_space;
        pen -= run.kerning(index) * font_size / 1000.0;
    }

    if vertical {
        min_x = min_x * font_size / 1000.0;
        max_x = max_x * font_size / 1000.0;
    } else {
        min_y = min_y * font_size / 1000.0;
        max_y = max_y * font_size / 1000.0;
    }
    let original_rect = Rect::new(
        f64::from(min_x),
        f64::from(min_y),
        f64::from(max_x),
        f64::from(max_y),
    );
    let mut rect = transform_rect(run.text_matrix, original_rect);
    if matches!(
        mode,
        TextRenderMode::Stroke
            | TextRenderMode::FillStroke
            | TextRenderMode::StrokeClip
            | TextRenderMode::FillStrokeClip
    ) {
        let half = f64::from(line_width) / 2.0;
        rect = Rect::new(
            rect.x0 - half,
            rect.y0 - half,
            rect.x1 + half,
            rect.y1 + half,
        );
    }
    run.rect = rect;
}

/// Every text object on a page, in the order a pre-order walk reaches them,
/// paired with the flattened index a [`CharBox`](crate::CharBox) refers to.
///
/// Form `XObject`s are walked in place, so a form's text objects sit between
/// the page-level objects that surround the `Do`.
#[must_use]
pub fn walk(objects: &[PageObject]) -> Vec<TextRun> {
    let mut out = Vec::new();
    let mut next = 0u32;
    collect(objects, &mut next, &mut out);
    out
}

fn collect(objects: &[PageObject], next: &mut u32, out: &mut Vec<TextRun>) {
    for object in objects {
        match object {
            PageObject::Text(content) => {
                let index = ObjectIndex(*next);
                *next += 1;
                if let Some(run) = build(content, index) {
                    out.push(run);
                }
            }
            PageObject::Form(content) => {
                *next += 1;
                collect(&content.object.objects, next, out);
            }
            PageObject::Path(_) | PageObject::Image(_) | PageObject::Shading(_) => {
                *next += 1;
            }
        }
    }
}

/// The flattened walk index of each *page-level* text object, ascending.
///
/// [`walk`] numbers every object it reaches, descending into forms; the
/// page-global orientation guess counts only the objects the page itself
/// lists (a form object is not a text object, so its contents never reach
/// the mask). This reproduces `collect`'s numbering without building
/// anything, so the guess can read the runs [`walk`] already built instead
/// of building them a second time.
#[must_use]
pub fn top_level_text_indices(objects: &[PageObject]) -> Vec<ObjectIndex> {
    let mut out = Vec::new();
    let mut next = 0u32;
    for object in objects {
        let index = ObjectIndex(next);
        next = next.saturating_add(1);
        match object {
            PageObject::Text(_) => out.push(index),
            PageObject::Form(content) => next = skip(&content.object.objects, next),
            PageObject::Path(_) | PageObject::Image(_) | PageObject::Shading(_) => {}
        }
    }
    out
}

/// Advances the walk counter past a subtree without building anything.
fn skip(objects: &[PageObject], mut next: u32) -> u32 {
    for object in objects {
        next = next.saturating_add(1);
        if let PageObject::Form(content) = object {
            next = skip(&content.object.objects, next);
        }
    }
    next
}

#[cfg(test)]
mod tests {
    // The widths compared here are whole numbers held exactly in `f64`, and
    // the point of each assertion is which exact one comes out.
    #![allow(clippy::float_cmp, reason = "test fixtures pin exact values")]

    use super::GlyphWidth;

    #[test]
    fn a_width_truncates_toward_zero_as_the_saturated_cast_does() {
        // `FX_Number::GetSigned` is `saturated_cast<int32_t>`, which rounds
        // toward zero in both directions rather than to nearest.
        assert_eq!(GlyphWidth::truncating(722.5).as_f64(), 722.0);
        assert_eq!(GlyphWidth::truncating(722.9).as_f64(), 722.0);
        assert_eq!(GlyphWidth::truncating(-722.9).as_f64(), -722.0);
        // A whole number is untouched, which is every width the corpora hold.
        assert_eq!(GlyphWidth::truncating(722.0).as_f64(), 722.0);
        assert_eq!(GlyphWidth::truncating(0.0), GlyphWidth::ZERO);
    }

    #[test]
    fn a_nonsense_width_is_zero_rather_than_a_saturated_extreme() {
        // The `FX_RECT::Valid` analogue: a non-finite box yields no width.
        assert_eq!(GlyphWidth::truncating(f32::NAN), GlyphWidth::ZERO);
        assert_eq!(GlyphWidth::truncating(f32::INFINITY), GlyphWidth::ZERO);
        assert_eq!(GlyphWidth::truncating(f32::NEG_INFINITY), GlyphWidth::ZERO);
    }

    #[test]
    fn only_a_strictly_positive_width_ends_the_ladder() {
        // Each of `GetCharWidth`'s first two exits tests `w > 0`, so a zero
        // or negative width falls through to the next rung.
        assert!(GlyphWidth::truncating(1.0).is_positive());
        assert!(!GlyphWidth::ZERO.is_positive());
        assert!(!GlyphWidth::truncating(-1.0).is_positive());
        // A fraction under one truncates to zero and so does *not* stop the
        // ladder, which is the C++ behaviour it mirrors.
        assert!(!GlyphWidth::truncating(0.5).is_positive());
    }

    #[test]
    fn the_max_of_two_widths_is_taken_on_the_integers() {
        // `std::max(nLastWidth, nThisWidth)` at `cpdf_textpage.cpp:1303` is
        // an integer max, which `Ord` on the newtype gives directly.
        let a = GlyphWidth::truncating(500.0);
        let b = GlyphWidth::truncating(722.0);
        assert_eq!(a.max(b), b);
        assert_eq!(b.max(a), b);
    }
}
