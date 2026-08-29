//! Type 3 glyph metrics: what a glyph procedure says about itself.
//!
//! A Type 3 font has no glyph program — each "glyph" is a content stream, and
//! its advance and bounding box come from the `d0`/`d1` operator that stream
//! is required to start with. So the only way to know how wide a Type 3
//! character is, or what box it occupies, is to look inside the procedure,
//! which is why this lives here rather than in `pdfrum-font`.
//!
//! Two rules decide the box:
//!
//! - **`d1` states it**, as `wx wy llx lly urx ury`. `d0` states only the
//!   advance and leaves the box to be computed.
//! - **A stated box that is degenerate is ignored** and the procedure's own
//!   painted extent is used instead — `right <= left` or `bottom >= top` in
//!   the C++'s y-down reading, which is `x1 <= x0 || y0 >= y1` here.
//!
//! Either way the result is scaled from text space to glyph units (×1000) and
//! then transformed by the font's `/FontMatrix`, which is what puts a
//! `/FontMatrix [0.001 0 0 0.001 0 0]` font's glyphs at the same scale as
//! every other font's.

use crate::build::BuildContext;
use crate::ops::Op;
use crate::page::PageObject;
use crate::resources::Resources;
use kurbo::{Affine, Rect};
use pdfrum_common::{Diagnostics, Limits};
use pdfrum_font::{CharCode, Type3Font};
use pdfrum_object::Resolve;

/// Text space is a thousandth of glyph space.
const TEXT_UNIT_IN_GLYPH_UNITS: f64 = 1000.0;

/// What a Type 3 glyph procedure declares about one character.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Type3Metrics {
    /// The advance in glyph units, rounded as the C++ rounds it.
    pub width: f32,
    /// The bounding box in glyph units, after the font matrix.
    pub bbox: Rect,
}

/// Reads one Type 3 character's metrics out of its glyph procedure.
///
/// `None` when the code names no procedure, which for a font with no
/// `/Differences` is every code.
#[must_use]
pub fn metrics<R: Resolve>(
    font: &Type3Font,
    code: CharCode,
    page_resources: Option<&pdfrum_object::Dict>,
    r: &R,
    ctx: &mut BuildContext,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> Option<Type3Metrics> {
    let stream = font.char_proc(code, r)?;
    let content = pdfrum_filters::decode_chain(&stream, 0, r, limits, diags).data;
    let ops = crate::parse_content(&content, limits, diags);

    // The declaration is whichever of the two operators the procedure used;
    // a procedure that used neither declares nothing and gets a computed box.
    let mut declared: Option<(f32, Option<Rect>)> = None;
    for op in &ops {
        match op {
            Op::Type3Width(wx, _) => {
                declared = Some((*wx, None));
                break;
            }
            Op::Type3WidthBBox(wx, _, llx, lly, urx, ury) => {
                declared = Some((
                    *wx,
                    Some(Rect::new(
                        f64::from(*llx),
                        f64::from(*lly),
                        f64::from(*urx),
                        f64::from(*ury),
                    )),
                ));
                break;
            }
            _ => {}
        }
    }
    let (width, stated) = declared.unwrap_or((0.0, None));

    // A stated box with no positive extent in either direction is not
    // believed; the procedure's own painted extent stands in for it.
    let usable = stated.filter(|b| b.x1 > b.x0 && b.y1 > b.y0);
    let text_box = if let Some(rect) = usable {
        scale(rect, TEXT_UNIT_IN_GLYPH_UNITS)
    } else {
        let resources = Resources::choose(
            font.resources.clone(),
            page_resources.cloned(),
            page_resources.cloned(),
        );
        let page = crate::build_page(&ops, &resources, r, ctx, limits, diags);
        scale(painted_extent(&page.objects), TEXT_UNIT_IN_GLYPH_UNITS)
    };

    Some(Type3Metrics {
        width: round(f64::from(width) * TEXT_UNIT_IN_GLYPH_UNITS),
        bbox: transform_rect(font.font_matrix, text_box),
    })
}

/// The union of every object's extent, or an empty rectangle when the
/// procedure painted nothing.
fn painted_extent(objects: &[PageObject]) -> Rect {
    let mut out: Option<Rect> = None;
    let mut add = |rect: Rect| {
        out = Some(match out {
            Some(current) => current.union(rect),
            None => rect,
        });
    };
    for object in objects {
        match object {
            PageObject::Path(content) => {
                let path = &content.object;
                add(transform_rect(
                    path.matrix,
                    kurbo::Shape::bounding_box(&path.path),
                ));
            }
            PageObject::Image(content) => {
                add(transform_rect(
                    content.object.matrix,
                    Rect::new(0.0, 0.0, 1.0, 1.0),
                ));
            }
            PageObject::Shading(content) => add(content.object.bounds),
            PageObject::Form(content) => {
                let inner = painted_extent(&content.object.objects);
                if inner.width() > 0.0 || inner.height() > 0.0 {
                    add(inner);
                }
            }
            // A Type 3 glyph procedure that shows text is legal but does not
            // contribute an extent here, matching the C++'s own bounding-box
            // walk, which measures painted geometry.
            PageObject::Text(_) => {}
        }
    }
    out.unwrap_or(Rect::ZERO)
}

fn scale(rect: Rect, factor: f64) -> Rect {
    Rect::new(
        rect.x0 * factor,
        rect.y0 * factor,
        rect.x1 * factor,
        rect.y1 * factor,
    )
}

fn transform_rect(matrix: Affine, rect: Rect) -> Rect {
    let corners = [
        matrix * kurbo::Point::new(rect.x0, rect.y0),
        matrix * kurbo::Point::new(rect.x1, rect.y0),
        matrix * kurbo::Point::new(rect.x0, rect.y1),
        matrix * kurbo::Point::new(rect.x1, rect.y1),
    ];
    let xs = corners.map(|p| p.x);
    let ys = corners.map(|p| p.y);
    Rect::new(
        xs.iter().copied().fold(f64::INFINITY, f64::min),
        ys.iter().copied().fold(f64::INFINITY, f64::min),
        xs.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        ys.iter().copied().fold(f64::NEG_INFINITY, f64::max),
    )
}

/// Half-away-from-zero rounding, which is `FXSYS_roundf`.
fn round(value: f64) -> f32 {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a glyph advance outside f32 is nonsense whatever we do"
    )]
    let rounded = value.round() as f32;
    rounded
}

#[cfg(test)]
mod tests {
    // Test fixtures quote the oracle's own vectors, compare floats exactly
    // where the behaviour being pinned is exact, and index arrays whose
    // length the fixture itself fixes.
    #![allow(
        clippy::float_cmp,
        clippy::indexing_slicing,
        clippy::unreadable_literal,
        reason = "test fixtures quote oracle vectors verbatim and compare exactly"
    )]

    use super::*;

    #[test]
    fn a_text_space_box_scales_by_a_thousand() {
        assert_eq!(
            scale(Rect::new(0.0, 0.0, 0.5, 0.75), 1000.0),
            Rect::new(0.0, 0.0, 500.0, 750.0)
        );
    }

    #[test]
    fn rounding_is_half_away_from_zero() {
        assert_eq!(round(0.5), 1.0);
        assert_eq!(round(-0.5), -1.0);
        assert_eq!(round(1.4), 1.0);
        assert_eq!(round(1.6), 2.0);
    }

    #[test]
    fn a_procedure_that_paints_nothing_has_an_empty_extent() {
        assert_eq!(painted_extent(&[]), Rect::ZERO);
    }

    #[test]
    fn a_font_matrix_transforms_the_glyph_box() {
        // The usual thousandth-scale matrix takes a 1000-unit box back to 1.
        let matrix = Affine::new([0.001, 0.0, 0.0, 0.001, 0.0, 0.0]);
        let out = transform_rect(matrix, Rect::new(0.0, 0.0, 1000.0, 1000.0));
        assert_eq!(out, Rect::new(0.0, 0.0, 1.0, 1.0));
    }
}
