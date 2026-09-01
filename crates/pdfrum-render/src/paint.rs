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
use crate::zero_area::{Scratch, scan_into, thin_alpha};

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
///
/// `zero_area` is the caller's scratch for the degenerate-sub-path scan in step
/// 3, which runs on every fill-only path object and on the corpus finds nothing
/// almost every time. Passing it in rather than allocating one here is what
/// makes the scan free after the first path of a page — see
/// [`crate::zero_area::Scratch`].
#[expect(
    clippy::too_many_arguments,
    reason = "the decision tree needs the device, the backend behind its \
              knockout buffer, the geometry, the paint, the stroke parameters, \
              the options and the caller's scan scratch"
)]
pub fn draw_path<B: RasterBackend>(
    device: &mut dyn RenderDevice,
    backend: &B,
    path: &BezPath,
    to_device: Affine,
    paint: PathPaint,
    params: &StrokeParams,
    opts: &RenderOptions,
    zero_area: &mut Scratch,
) -> bool {
    // The phase covers this function's own decision tree *and* the device
    // calls it makes, which the seam decorator counts separately — so the
    // walk report subtracts nothing here and the two numbers are read side by
    // side rather than nested. See `walkprofile`'s docs on overlapping buckets.
    crate::walkprofile::phase(crate::walkprofile::Phase::PathPrep, || {
        draw_path_inner(
            device, backend, path, to_device, paint, params, opts, zero_area,
        )
    })
}

/// [`draw_path`] without the phase timer around it.
#[expect(
    clippy::too_many_lines,
    reason = "see `draw_path`: one numbered decision tree whose five cases are \
              mutually exclusive early returns read top to bottom"
)]
#[expect(
    clippy::too_many_arguments,
    reason = "the same inputs `draw_path` takes"
)]
fn draw_path_inner<B: RasterBackend>(
    device: &mut dyn RenderDevice,
    backend: &B,
    path: &BezPath,
    to_device: Affine,
    paint: PathPaint,
    params: &StrokeParams,
    opts: &RenderOptions,
    zero_area: &mut Scratch,
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
        && let Some(rect_f) = crate::walkprofile::phase(crate::walkprofile::Phase::RectTest, || {
            path_rect(path, to_device)
        })
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
    //
    // **This does not replace the fill, it precedes it.** The C++ loop runs
    // `DrawZeroAreaPath` over each sub-path and then falls through to the
    // ordinary `DrawPath` at the end of the function — there is no early
    // return anywhere in it (`cfx_renderdevice.cpp:772-804`).
    //
    // The distinction is invisible on a sub-path that really is degenerate,
    // because filling one paints nothing anyway. It is decisive on a path
    // that merely *contains* a fold: `GetZeroAreaPath`'s third case scans an
    // otherwise ordinary sub-path for a segment that doubles back
    // (`IsFoldingVerticalLine` and its two siblings) and emits **just that
    // segment** as a hairline. Returning here on the strength of it threw
    // away the polygon the fold was attached to — `bug_1338` draws five such
    // shapes and we painted only their spikes.
    if let Some(fill) = paint.fill
        && paint.stroke.is_none()
        && !paint.text_mode
    {
        for zero in scan_into(zero_area, path, Some(to_device), true) {
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
        let geometry = crate::walkprofile::phase(crate::walkprofile::Phase::PathXform, || {
            // One buffer, reserved to the source's length: the transform and
            // the clamp are one pass. It used to be two whole `BezPath`s, the
            // first discarded at this call — see `transform_hard_clip`.
            crate::walkprofile::alloc_items(
                crate::walkprofile::Site::PathGeometry,
                path.elements().len(),
                core::mem::size_of::<PathEl>(),
            );
            crate::path::transform_hard_clip(to_device, path)
        });
        device.fill_path(
            &geometry,
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
        // stroke width isotropic under an anisotropic CTM. `BuildAggPath` is
        // called with that same matrix1, so the degenerate-subpath nudge is
        // one pixel *there* — see `nudge_degenerate_subpaths`.
        let geometry = crate::walkprofile::phase(crate::walkprofile::Phase::PathXform, || {
            // Three buffers on this arm: the transformed copy, the nudge's
            // output, and the clamp. The nudge's own two element copies went
            // with the borrow — see `nudge_degenerate_subpaths`.
            for _ in 0..3 {
                crate::walkprofile::alloc_items(
                    crate::walkprofile::Site::PathGeometry,
                    path.elements().len(),
                    core::mem::size_of::<PathEl>(),
                );
            }
            hard_clip(&crate::path::nudge_degenerate_subpaths(
                &(matrices.pre * path.clone()),
                path,
            ))
        });
        device.stroke_path(
            &geometry,
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
            &crate::path::transform_hard_clip(offset * to_device, path),
            Affine::IDENTITY,
            &Brush::Solid(fill.to_peniko()),
            rule,
            aa,
        );
    }
    if !stroke_color.is_invisible() {
        // The buffer's translation composes *outermost*, after `post`, so it
        // rides on the transform argument rather than the geometry. Folding it
        // into the geometry instead would put it inside the split — the
        // rasterizer would then apply `post * offset`, and `post` carries the
        // page's y-flip, so the buffer's upward shift would come back out
        // downward and drop the stroke off the bottom of the buffer.
        sub.stroke_path(
            &hard_clip(&crate::path::nudge_degenerate_subpaths(
                &(matrices.pre * path.clone()),
                path,
            )),
            offset * matrices.post,
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

    /// A knockout buffer's translation must reach the rasterizer *outside*
    /// the stroke split, because the split's `post` half carries the page's
    /// y-flip and would otherwise invert the shift.
    ///
    /// The stroke is handed `pre * path` as geometry and a transform the
    /// rasterizer applies per vertex. Composing the offset as
    /// `offset * post` puts it last; folding it into the geometry instead
    /// would make the rasterizer compute `post * offset`, and for a page
    /// whose device matrix is a pure y-flip those two disagree in sign on y.
    #[test]
    fn a_knockout_buffer_offset_survives_the_stroke_split() {
        // A 200-high page's object-to-device matrix: y-up to y-down.
        let to_device = Affine::new([1.0, 0.0, 0.0, -1.0, 0.0, 200.0]);
        let matrices = crate::stroke::split_for_stroke(to_device);

        let same = |got: [f64; 6], want: [f64; 6]| {
            got.iter()
                .zip(want)
                .all(|(a, b)| (a - b).abs() < 1e-9)
                .then_some(())
                .ok_or(format!("{got:?} != {want:?}"))
        };

        // The split leaves a translation-free y-flip in `post`, which is what
        // makes the composition order observable at all.
        same(matrices.post.as_coeffs(), [1.0, 0.0, 0.0, -1.0, 0.0, 0.0]).expect("a bare y-flip");

        let offset = Affine::translate((-45.0, -45.0));

        // Outermost: the shift stays a shift.
        same(
            (offset * matrices.post).as_coeffs(),
            [1.0, 0.0, 0.0, -1.0, -45.0, -45.0],
        )
        .expect("the offset survives");
        // Innermost: the y-flip turns the upward shift downward, which is the
        // 90-row displacement that clipped the stroke out of the buffer.
        same(
            (matrices.post * offset).as_coeffs(),
            [1.0, 0.0, 0.0, -1.0, -45.0, 45.0],
        )
        .expect("the offset is flipped");

        // A point on the path lands where the fill puts it only under the
        // outermost spelling. The fill draws `offset * to_device * path`, so
        // that composition is the reference the stroke has to match.
        let point = kurbo::Point::new(50.0, 150.0);
        let fill_lands = (offset * to_device) * point;
        let stroke_lands = (offset * matrices.post) * (matrices.pre * point);
        assert!(
            (fill_lands - stroke_lands).hypot() < 1e-9,
            "the stroke must land on the fill: {fill_lands:?} vs {stroke_lands:?}"
        );
    }
}
