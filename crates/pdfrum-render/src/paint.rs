//! Drawing one path object: the fast paths that change pixels
//! (`CFX_RenderDevice::DrawPath`, `cfx_renderdevice.cpp:688-830`).
//!
//! Four of them fire before an ordinary fill ever does, in this order: a
//! two-point "fill" becomes a cosmetic line, an axis-aligned rectangle is
//! integer-snapped and drawn hard-edged, a zero-area sub-path becomes a
//! quarter-alpha hairline, and a translucent stroke over a fill goes through
//! a knockout buffer so the two do not accumulate where they overlap.

use kurbo::{Affine, BezPath, PathEl, Shape, Stroke};
use pdfrum_page::StrokeParams;

use crate::color::Argb;
use crate::device::{AntiAlias, Brush, FillRule, RasterBackend, RenderDevice};
use crate::options::RenderOptions;
use crate::path::{hard_clip, is_available_matrix, path_rect, snap_rect};
use crate::stroke::{hairline_matrices, resolve_stroke, split_for_stroke};
use crate::zero_area::{thin_alpha, zero_area_sub_paths};

/// What a path object asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PathPaint {
    /// The fill colour, or `None` when the object is stroke-only.
    pub fill: Option<Argb>,
    /// The stroke colour, or `None` when it is fill-only.
    pub stroke: Option<Argb>,
    /// The fill rule, meaningless when `fill` is `None`.
    pub rule: FillRule,
    /// Whether this path is a glyph outline, which excludes it from the
    /// zero-area hairline conversion — the reason glyph stems never turn
    /// into quarter-alpha lines.
    pub text_mode: bool,
}

/// Draw one path.
///
/// `to_device` maps the path's own space to device space and has already
/// absorbed the object's matrix. Returns whether anything was drawn, which
/// the walk records as a diagnostic when it is `false` for a visible object.
#[expect(
    clippy::too_many_lines,
    reason = "one numbered decision tree ported from `ProcessPath`, whose five \
              cases are mutually exclusive early returns read top to bottom; \
              splitting them into helpers would scatter the precedence that is \
              the entire content of the function"
)]
pub fn draw_path<B: RasterBackend>(
    device: &mut dyn RenderDevice,
    backend: &B,
    path: &BezPath,
    to_device: Affine,
    paint: PathPaint,
    params: &StrokeParams,
    opts: &RenderOptions,
) -> bool {
    // A matrix that collapses in either of the two ways `IsAvailableMatrix`
    // rejects drops the object silently — deliberately *not* a determinant
    // test, so a singular matrix like (1,1,1,1) still draws.
    if !is_available_matrix(to_device) {
        return false;
    }
    let aa = opts.path_aa();

    // 1. A two-point path with no stroke becomes a one-pixel cosmetic line,
    //    drawn in the *fill* colour. It fires on a fill, not a stroke.
    if paint.stroke.is_none()
        && let Some(fill) = paint.fill
        && let Some(line) = two_point_line(path)
    {
        if fill.is_invisible() {
            return true;
        }
        let stroke = Stroke::new(1.0);
        device.stroke_path(
            &hard_clip(&line),
            to_device,
            &Brush::Solid(fill.to_peniko()),
            &stroke,
            aa,
        );
        return true;
    }

    // 2. An axis-aligned rectangle, never antialiased unless `bRectAA`.
    if paint.stroke.is_none()
        && !opts.rect_aa
        && let Some(fill) = paint.fill
        && let Some(rect_f) = path_rect(path, to_device)
        && let Some(snapped) = snap_rect(rect_f)
    {
        if fill.is_invisible() {
            return true;
        }
        let mut r = BezPath::new();
        let d = snapped.to_rect();
        r.move_to((d.x0, d.y0));
        r.line_to((d.x1, d.y0));
        r.line_to((d.x1, d.y1));
        r.line_to((d.x0, d.y1));
        r.close_path();
        device.fill_path(
            &r,
            Affine::IDENTITY,
            &Brush::Solid(fill.to_peniko()),
            FillRule::Winding,
            AntiAlias::Off,
        );
        return true;
    }

    // 3. Zero-area sub-paths become hairlines at a quarter of the fill alpha.
    let mut drew_thin = false;
    if let Some(fill) = paint.fill
        && paint.stroke.is_none()
        && !paint.text_mode
    {
        for zero in zero_area_sub_paths(path, Some(to_device), true) {
            drew_thin = true;
            if zero.path.elements().is_empty() {
                continue; // The all-points-identical case draws nothing.
            }
            let alpha = if zero.thin {
                thin_alpha(fill.a)
            } else {
                fill.a
            };
            let color = fill.with_alpha(alpha);
            if color.is_invisible() {
                continue;
            }
            // `zero_area` skips the matrix split entirely: the path is
            // already in device space and the stroke runs at scale 1, which
            // is what guarantees exactly one device pixel.
            let (geometry, matrix) = if zero.identity {
                (zero.path.clone(), Affine::IDENTITY)
            } else {
                (zero.path.clone(), to_device)
            };
            let stroke = resolve_stroke(
                &StrokeParams {
                    width: 0.0,
                    ..params.clone()
                },
                hairline_matrices(Affine::IDENTITY),
            );
            device.stroke_path(
                &hard_clip(&(matrix * geometry)),
                Affine::IDENTITY,
                &Brush::Solid(color.to_peniko()),
                &stroke,
                aa,
            );
        }
        if drew_thin {
            return true;
        }
    }

    // 4. A fill under a translucent stroke goes through a knockout buffer, so
    //    the stroke does not accumulate over the fill where they overlap.
    //    This is the whole of "knockout" in PDFium — a device flag, not the
    //    PDF `/K` group attribute, which core/ never parses.
    if let (Some(fill), Some(stroke_color)) = (paint.fill, paint.stroke)
        && fill.a != 0
        && stroke_color.a < 0xFF
    {
        return draw_fill_stroke_knockout(
            device,
            backend,
            path,
            to_device,
            fill,
            stroke_color,
            paint.rule,
            params,
            opts,
        );
    }

    // 5. The ordinary case.
    let matrices = split_for_stroke(to_device);
    let mut drew = false;
    if let Some(fill) = paint.fill
        && !fill.is_invisible()
    {
        device.fill_path(
            &hard_clip(&(to_device * path.clone())),
            Affine::IDENTITY,
            &Brush::Solid(fill.to_peniko()),
            paint.rule,
            aa,
        );
        drew = true;
    }
    if let Some(color) = paint.stroke
        && !color.is_invisible()
    {
        let stroke = resolve_stroke(params, matrices);
        // The stroke's geometry is pre-transformed by matrix1 and the
        // residual matrix2 is applied per vertex, which is what keeps the
        // stroke width isotropic under an anisotropic CTM.
        device.stroke_path(
            &hard_clip(&(matrices.pre * path.clone())),
            matrices.post,
            &Brush::Solid(color.to_peniko()),
            &stroke,
            aa,
        );
        drew = true;
    }
    drew
}

/// Draw a fill and a translucent stroke into their own buffer so they do not
/// accumulate, then blit the result.
#[expect(
    clippy::too_many_arguments,
    reason = "the knockout case needs every input the fast path had"
)]
fn draw_fill_stroke_knockout<B: RasterBackend>(
    device: &mut dyn RenderDevice,
    backend: &B,
    path: &BezPath,
    to_device: Affine,
    fill: Argb,
    stroke_color: Argb,
    rule: FillRule,
    params: &StrokeParams,
    opts: &RenderOptions,
) -> bool {
    let matrices = split_for_stroke(to_device);
    let stroke = resolve_stroke(params, matrices);
    // The buffer is sized to the stroked path's bounding box, so a wide
    // stroke is not clipped away by the fill's own extent.
    let bbox = (to_device * path.clone())
        .bounding_box()
        .inflate(stroke.width, stroke.width);
    let rect = crate::path::outer_rect(bbox);
    let (Ok(w), Ok(h)) = (u32::try_from(rect.width()), u32::try_from(rect.height())) else {
        return false;
    };
    if w == 0
        || h == 0
        || w > crate::device::MAX_TARGET_DIMENSION
        || h > crate::device::MAX_TARGET_DIMENSION
    {
        return false;
    }
    let mut sub = backend.new_target(w, h, peniko::Color::TRANSPARENT);
    let offset = Affine::translate((-f64::from(rect.left), -f64::from(rect.top)));
    let aa = opts.path_aa();
    if !fill.is_invisible() {
        sub.fill_path(
            &hard_clip(&(offset * to_device * path.clone())),
            Affine::IDENTITY,
            &Brush::Solid(fill.to_peniko()),
            rule,
            aa,
        );
    }
    if !stroke_color.is_invisible() {
        sub.stroke_path(
            &hard_clip(&(offset * matrices.pre * path.clone())),
            matrices.post,
            &Brush::Solid(stroke_color.to_peniko()),
            &stroke,
            aa,
        );
    }
    let pixels = backend.finish(sub);
    device.draw_image(
        &pixels,
        Affine::translate((f64::from(rect.left), f64::from(rect.top))),
        crate::device::ImageQuality::Nearest,
        1.0,
    );
    true
}

/// A path that is exactly a two-point `Move, Line`, as the cosmetic-line fast
/// path requires.
fn two_point_line(path: &BezPath) -> Option<BezPath> {
    let mut els = path.elements().iter();
    let PathEl::MoveTo(a) = *els.next()? else {
        return None;
    };
    let PathEl::LineTo(b) = *els.next()? else {
        return None;
    };
    // A trailing close is permitted; anything else is not a two-point path.
    match els.next() {
        None => {}
        Some(PathEl::ClosePath) if els.next().is_none() => {}
        Some(_) => return None,
    }
    let mut out = BezPath::new();
    out.move_to(a);
    out.line_to(b);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_point_line_accepts_only_a_bare_line() {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((4.0, 4.0));
        assert!(two_point_line(&p).is_some());
        p.line_to((8.0, 0.0));
        assert!(two_point_line(&p).is_none());

        let mut closed = BezPath::new();
        closed.move_to((0.0, 0.0));
        closed.line_to((4.0, 4.0));
        closed.close_path();
        assert!(
            two_point_line(&closed).is_some(),
            "a trailing close still counts"
        );
    }

    #[test]
    fn a_curve_is_not_a_two_point_line() {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.curve_to((1.0, 1.0), (2.0, 2.0), (3.0, 3.0));
        assert!(two_point_line(&p).is_none());
    }
}
