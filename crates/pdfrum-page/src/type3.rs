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

/// What a Type 3 glyph procedure declares about one character, and what it
/// paints.
#[derive(Debug, Clone, PartialEq)]
pub struct Type3Metrics {
    /// The advance in glyph units, rounded as the C++ rounds it.
    pub width: f32,
    /// The bounding box in glyph units, after the font matrix.
    ///
    /// This is what the glyph *declares*, and it is the right box for
    /// measurement. It is the wrong box for a buffer: `d1`'s operands are
    /// scaled by a thousand on the way in and put back through the font matrix
    /// on the way out, so a font whose `/FontMatrix` is not the conventional
    /// thousandth leaves the scale uncancelled and the box far off the page.
    /// Use [`Type3Metrics::painted`] to size anything.
    pub bbox: Rect,
    /// The extent the procedure's objects actually cover, in **glyph space**.
    ///
    /// The objects' own union, with neither the thousandth scale nor the font
    /// matrix applied — `CalcBoundingBox`'s answer, and unaffected by an
    /// unconventional `/FontMatrix` because it never consults one.
    pub painted: Rect,
    /// Whether the procedure declared its own colour with `d0` rather than
    /// only a width and box with `d1`.
    ///
    /// This is the coloured/uncoloured distinction the renderer needs: a `d1`
    /// glyph takes the *text object's* colour for every drawing operation
    /// inside it, whatever colours the procedure sets, while a `d0` one keeps
    /// whatever it sets and falls back to the text object's only where it
    /// sets none.
    pub colored: bool,
    /// The objects the procedure paints, in **glyph space** — the procedure's
    /// own coordinate system, before the font matrix.
    ///
    /// A Type 3 glyph has no outline to cache: it is a content stream, and
    /// drawing it means walking these. Interpreting them here rather than at
    /// render time is what keeps the renderer free of the resolver, and it
    /// costs nothing extra — the metrics already had to open the stream.
    pub objects: Vec<PageObject>,
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
    // `d0` is the coloured form and `d1` the uncoloured one, and which
    // appeared decides whether the glyph keeps its own colours.
    let mut declared: Option<(f32, Option<Rect>, bool)> = None;
    for op in &ops {
        match op {
            Op::Type3Width(wx, _) => {
                declared = Some((*wx, None, true));
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
                    false,
                ));
                break;
            }
            _ => {}
        }
    }
    let (width, stated, colored) = declared.unwrap_or((0.0, None, false));

    // The procedure's objects are needed either way — to *draw* the glyph
    // always, and to measure it when the stated box is not believed — so they
    // are built once here rather than conditionally.
    //
    // A procedure may show text in a Type 3 font, so this recurses; the form
    // guard catches a procedure that invokes *itself*, and `kMaxType3FormLevel`
    // is what bounds a pair that invoke each other through two distinct
    // streams. Without it such a pair overflows the stack.
    let objects = if ctx.enter_type3() {
        let resources = Resources::choose(
            font.resources.clone(),
            page_resources.cloned(),
            page_resources.cloned(),
        );
        let page = crate::build_page(&ops, &resources, r, ctx, limits, diags);
        ctx.leave_type3();
        page.objects
    } else {
        diags.record(
            pdfrum_common::Severity::Recovered,
            pdfrum_common::DiagKind::FormRecursionRefused,
            None,
        );
        Vec::new()
    };

    // A stated box with no positive extent in either direction is not
    // believed; the procedure's own painted extent stands in for it.
    let usable = stated.filter(|b| b.x1 > b.x0 && b.y1 > b.y0);
    let painted = painted_extent(&objects);
    let text_box = match usable {
        Some(rect) => scale(rect, TEXT_UNIT_IN_GLYPH_UNITS),
        None => scale(painted, TEXT_UNIT_IN_GLYPH_UNITS),
    };

    Some(Type3Metrics {
        width: round(f64::from(width) * TEXT_UNIT_IN_GLYPH_UNITS),
        bbox: transform_rect(font.font_matrix, text_box),
        painted,
        colored,
        objects,
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

    /// One indirect object, which is all a one-glyph font needs to resolve.
    struct OneStream(std::sync::Arc<pdfrum_object::Object>);

    impl pdfrum_object::Resolve for OneStream {
        fn fetch(
            &self,
            r: pdfrum_object::ObjRef,
        ) -> Result<std::sync::Arc<pdfrum_object::Object>, pdfrum_object::Error> {
            if r.num == 1 {
                Ok(std::sync::Arc::clone(&self.0))
            } else {
                Err(pdfrum_object::Error::UnresolvedRef(r))
            }
        }
    }

    /// A Type 3 font with one glyph procedure, loaded the way the page
    /// interpreter loads one.
    fn one_glyph_font(proc_body: &[u8]) -> (pdfrum_font::Font, OneStream) {
        use pdfrum_object::{Array, ByteSpan, Dict, Name, Object, Stream};
        let proc = Stream::new(Dict::new(), ByteSpan::from(proc_body.to_vec()));
        let store = OneStream(std::sync::Arc::new(Object::Stream(Box::new(proc))));
        let font = Dict::from_pairs([
            (Name::from("Type"), Object::Name(Name::from("Font"))),
            (Name::from("Subtype"), Object::Name(Name::from("Type3"))),
            (
                Name::from("CharProcs"),
                Object::Dict(Dict::from_pairs([(
                    Name::from("g"),
                    Object::Ref(pdfrum_object::ObjRef {
                        num: 1,
                        generation: 0,
                    }),
                )])),
            ),
            (
                Name::from("Encoding"),
                Object::Dict(Dict::from_pairs([(
                    Name::from("Differences"),
                    Object::Array(
                        [Object::Int(65), Object::Name(Name::from("g"))]
                            .into_iter()
                            .collect::<Array>(),
                    ),
                )])),
            ),
            (Name::from("FirstChar"), Object::Int(65)),
            (Name::from("LastChar"), Object::Int(65)),
            (
                Name::from("Widths"),
                Object::Array([Object::Int(100)].into_iter().collect::<Array>()),
            ),
        ]);
        let cache = pdfrum_font::FontCache::new();
        let mut diags = Diagnostics::default();
        let loaded = pdfrum_font::load(&font, &store, &cache, &Limits::default(), &mut diags)
            .expect("a type 3 font");
        (loaded, store)
    }

    fn glyph_metrics(proc_body: &[u8]) -> Type3Metrics {
        let (font, store) = one_glyph_font(proc_body);
        let type3 = font.type3().expect("a type 3 font");
        let mut ctx = BuildContext::new();
        let mut diags = Diagnostics::default();
        metrics(
            type3,
            pdfrum_font::CharCode(65),
            None,
            &store,
            &mut ctx,
            &Limits::default(),
            &mut diags,
        )
        .expect("the glyph names a procedure")
    }

    #[test]
    fn a_d1_procedure_is_uncoloured_and_a_d0_one_is_coloured() {
        // The whole colour rule turns on which operator opened the procedure:
        // `d1` glyphs take the text object's colour for every operation
        // inside, `d0` glyphs keep their own.
        assert!(!glyph_metrics(b"100 0 0 0 50 50 d1 0 0 50 50 re f").colored);
        assert!(glyph_metrics(b"100 0 d0 1 0 0 rg 0 0 50 50 re f").colored);
        // A procedure that declares neither is uncoloured, which is the
        // default the C++ constructs `CPDF_Type3Char` with.
        assert!(!glyph_metrics(b"0 0 50 50 re f").colored);
    }

    #[test]
    fn a_procedure_yields_the_objects_it_paints() {
        // The renderer draws a Type 3 glyph by walking these; an empty list
        // means the glyph paints nothing at all.
        let m = glyph_metrics(b"100 0 0 0 50 50 d1 0 0 50 50 re f");
        assert_eq!(m.objects.len(), 1, "one rectangle: {:?}", m.objects);
        assert!(matches!(m.objects.first(), Some(PageObject::Path(_))));
        // And a procedure that paints nothing yields none.
        assert!(glyph_metrics(b"100 0 0 0 50 50 d1").objects.is_empty());
    }

    #[test]
    fn a_declared_box_is_believed_and_a_degenerate_one_is_not() {
        // `d1` states 0 0 10 10; with no font matrix that scales by a
        // thousand into glyph units.
        let stated = glyph_metrics(b"100 0 0 0 10 10 d1 0 0 40 40 re f");
        assert_eq!(stated.bbox, Rect::new(0.0, 0.0, 10_000.0, 10_000.0));
        // A degenerate stated box is discarded for the painted extent, which
        // is the rectangle the procedure actually drew.
        let computed = glyph_metrics(b"100 0 10 10 0 0 d1 0 0 40 40 re f");
        assert_eq!(computed.bbox, Rect::new(0.0, 0.0, 40_000.0, 40_000.0));
    }

    #[test]
    fn the_advance_is_the_declared_width_in_glyph_units() {
        // `100` in text space is 100_000 in glyph units, rounded.
        assert_eq!(glyph_metrics(b"100 0 d0 0 0 1 1 re f").width, 100_000.0);
    }

    #[test]
    fn the_painted_extent_is_the_objects_own_and_ignores_the_declared_box() {
        // `painted` is what a buffer must be sized from. It is the objects'
        // union in glyph space, with neither the thousandth scale nor the font
        // matrix applied — so a declared box that disagrees with the drawing
        // does not move it, and a font matrix that is not the conventional
        // thousandth cannot push it off the page.
        let m = glyph_metrics(b"100 0 0 0 10 10 d1 0 0 40 40 re f");
        assert_eq!(m.painted, Rect::new(0.0, 0.0, 40.0, 40.0));
        // The declared box says 10x10 and scales to 10 000 glyph units; the
        // painted extent is unmoved by either.
        assert_eq!(m.bbox, Rect::new(0.0, 0.0, 10_000.0, 10_000.0));
        // A procedure that paints nothing has an empty one, and the buffer
        // sized from it is empty rather than wrong.
        assert_eq!(glyph_metrics(b"100 0 0 0 10 10 d1").painted, Rect::ZERO);
    }

    #[test]
    fn a_font_matrix_transforms_the_glyph_box() {
        // The usual thousandth-scale matrix takes a 1000-unit box back to 1.
        let matrix = Affine::new([0.001, 0.0, 0.0, 0.001, 0.0, 0.0]);
        let out = transform_rect(matrix, Rect::new(0.0, 0.0, 1000.0, 1000.0));
        assert_eq!(out, Rect::new(0.0, 0.0, 1.0, 1.0));
    }
}
