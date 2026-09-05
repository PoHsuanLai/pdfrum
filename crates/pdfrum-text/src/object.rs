//! Typeset representation of a page text object.
//!
//! Derives glyph positions, advances, and bounding boxes from a
//! [`pdfrum_page::TextObject`] for extraction heuristics.

// `pdfrum-page` hands over a `TextObject` holding the *content stream's*
// view: byte strings with the adjustments between them, one position, one
// matrix. The extraction heuristics want the *typeset* view: one character
// code per glyph, the text-space x each one sits at, the adjustment that
// followed it, and the object's bounding box.
// (`docs/design/pdfrum-text.md` §4.2.)
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
    /// The bounding box in the object's own text space, before the matrix.
    pub original_rect: Rect,
    /// Total advance width in page space: `w0` summed over the object's
    /// glyphs (ISO 32000-1 §9.4.3), measured through the text matrix.
    ///
    /// Distinct from the bounding box (§9.2.2): a space has an empty box and
    /// a non-zero `w0`, so an object made only of spaces still has an
    /// advance.
    // `[oracle-bug]` PDFium has no such field: `cpdf_textpage.cpp:881` and
    // `:1076` decide whether a text object exists at all from
    // `GetRect().Width()`, which `cpdf_textobject.cpp:305-331` builds from the
    // glyph **bounding boxes**. §9.2.2 keeps displacement and bounding box
    // distinct, and a space's box is empty while its `w0` is not, so any
    // object made only of spaces vanishes before extraction
    // (`crbug.com/40643656`, `crbug.com/444176962`). pdf.js keeps such a
    // character two independent ways (`evaluator.js:3079-3084`,
    // `:2924-2939`) and makes its whitespace drop an **opt-out**
    // (`keepWhiteSpace: true`), not a loss.
    pub advance: f64,
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

/// The width of one character code in glyph units, through the extractor's
/// own three-rung fallback ladder (`GetCharWidth`, design brief §1.7c).
///
/// Distinct from [`Font::char_width`], which is only the first rung. The
/// second re-encodes the code to bytes and re-decodes them, so it differs
/// from the first exactly when that round trip is lossy — a simple font's
/// code above 255, say. The third falls back to the glyph's bounding box.
#[must_use]
pub fn ladder_char_width(run: &TextRun, code: Option<CharCode>) -> f32 {
    let Some(code) = code else {
        return 0.0;
    };
    let font = &run.font;
    let width = run.glyph_width(code);
    if width > 0.0 {
        return width;
    }
    let mut bytes = Vec::new();
    font.append_char(&mut bytes, code);
    let width = font.string_width(&bytes);
    if width > 0.0 {
        return width;
    }
    let bbox = run.glyph_bbox(code);
    // `FX_RECT::Valid`'s overflow check has no analogue on an `f64` rect; a
    // non-finite box is the equivalent nonsense and yields zero.
    if !bbox.x0.is_finite() || !bbox.x1.is_finite() {
        return 0.0;
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "glyph-unit widths are small integers"
    )]
    let width = (bbox.x1 - bbox.x0) as f32;
    width.max(0.0)
}

/// Builds the extraction view of one text object.
///
/// Returns `None` for an object with no font, which cannot show anything.
#[must_use]
pub fn build(content: &Content<TextObject>, index: ObjectIndex) -> Option<TextRun> {
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
        original_rect: Rect::ZERO,
        advance: 0.0,
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
    run.original_rect = Rect::new(
        f64::from(min_x),
        f64::from(min_y),
        f64::from(max_x),
        f64::from(max_y),
    );
    let mut rect = transform_rect(run.text_matrix, run.original_rect);
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
    // `[oracle-bug]` The advance the pen actually travelled, measured in page
    // space through the same matrix the box goes through, so the two are
    // comparable against one epsilon. `pen` is signed — a negative font size
    // or a leading kern runs it backwards — so the magnitude is what the
    // "does this object occupy space" question wants.
    let m = run.text_matrix.as_coeffs();
    let (dx, dy) = if vertical {
        (m[2] * f64::from(pen), m[3] * f64::from(pen))
    } else {
        (m[0] * f64::from(pen), m[1] * f64::from(pen))
    };
    run.advance = dx.hypot(dy);
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
