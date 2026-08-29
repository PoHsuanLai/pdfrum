//! Painting a path or a glyph run with a pattern colour
//! (`ProcessPathPattern`, `DrawShadingPattern` and `DrawTilingPattern`,
//! `cpdf_renderstatus.cpp:1185-1295`, `cpdf_rendertiling.cpp:85-299`).
//!
//! A pattern colour is not a colour: it is an instruction to paint the
//! object's geometry with something else entirely, and the ordinary fill is
//! **drained** rather than drawn. `ProcessPathPattern` runs before the path
//! draw, paints a pattern fill and a pattern stroke separately — so an object
//! with both is drawn twice — and clears each from the residual draw, which
//! then does nothing.
//!
//! Four behaviours here are pixel-visible and none is obvious:
//!
//! - **A shading pattern's alpha is rounded, not truncated.** Every other
//!   alpha in the engine truncates (§1.3); `DrawShadingPattern` alone spells
//!   `FXSYS_roundf`, so `/ca 0.5` is 128 here and 127 everywhere else.
//! - **A pattern clips by the object's own geometry.** `ClipPattern` fills or
//!   strokes the path into the clip; an image clips by its transformed bbox;
//!   anything else drops the pattern entirely rather than painting it
//!   unclipped.
//! - **The tile stack is not a transparency group.** Every tile is blitted
//!   into one screen buffer and that buffer is composited with a plain normal
//!   blend at full alpha — so overlapping tiles overpaint each other rather
//!   than accumulating coverage, and the object's own alpha reaches the tiles
//!   through the cell's inherited general state, not through the blit.
//! - **A cell smaller than sixteen pixels is rendered at 8×8 and scaled
//!   down.** An antialiasing trick for tiny tiles, and visible on every one
//!   of them.
//!
//! The step, tile-range and cell-size ladders — every one of which silently
//! makes a pattern paint nothing — live in `pdfrum_page::TilingPattern`,
//! because they are decisions about the *pattern*, not about a device.

use kurbo::{Affine, BezPath, Rect, Shape};
use pdfrum_common::Diagnostics;
use pdfrum_page::{Pattern, TilingPattern};

use crate::clip;
use crate::color::Argb;
use crate::ctx::{RenderCaches, RenderCtx};
use crate::device::{ImageQuality, MAX_TARGET_DIMENSION, RasterBackend, RenderDevice};
use crate::path::{IntRect, is_available_matrix, outer_rect};
use crate::pixmap::{Pixmap, alpha_byte_rounding};
use crate::shading;

/// The cell area below which a tile is rendered at 8×8 and scaled down
/// (`cpdf_rendertiling.cpp:217-227`).
///
/// Strictly below: a 4×4 cell is exactly sixteen and renders at its own size.
pub const MIN_CELL_AREA: i64 = 16;

/// The size a sub-[`MIN_CELL_AREA`] cell is rendered at before being scaled
/// down to its real one.
pub const ENLARGED_CELL: u32 = 8;

/// The buffer size one cell is rasterized into, before it is scaled to its
/// real one.
///
/// A cell of fewer than [`MIN_CELL_AREA`] pixels is rendered at
/// [`ENLARGED_CELL`] square and scaled down, which is how the C++ gets
/// antialiasing out of a tile too small for a rasterizer to antialias. It is
/// an **area** test, not a per-axis one: a 1×20 cell has twenty pixels and is
/// rendered at its own size.
#[must_use]
pub fn render_size(width: u32, height: u32) -> (u32, u32) {
    if i64::from(width) * i64::from(height) < MIN_CELL_AREA {
        (ENLARGED_CELL, ENLARGED_CELL)
    } else {
        (width, height)
    }
}

/// What a pattern is painted through: the geometry that clips it.
///
/// `ClipPattern` (`cpdf_renderstatus.cpp:598-609`) accepts exactly two
/// shapes and **drops the pattern** for anything else, which is why this is
/// not an `Option<BezPath>` — the third case is a real one and it is not
/// "no clip".
#[derive(Debug, Clone)]
pub enum PatternClip<'a> {
    /// A path object, clipping by its own fill or stroke geometry.
    Path {
        /// The path in its own space.
        path: &'a BezPath,
        /// The matrix taking it to device space.
        to_device: Affine,
        /// Whether the pattern is the stroke's rather than the fill's, which
        /// selects `SetClip_PathStroke` over `SetClip_PathFill`.
        stroking: bool,
        /// The fill rule, for the fill case.
        rule: crate::device::FillRule,
        /// The stroke parameters, for the stroke case.
        stroke: &'a pdfrum_page::StrokeParams,
    },
    /// An image object, clipping by its transformed bounding box.
    Rect(Rect),
}

impl PatternClip<'_> {
    /// The device-space rectangle the pattern is confined to.
    fn device_bounds(&self) -> Rect {
        match self {
            Self::Path {
                path,
                to_device,
                stroking,
                stroke,
                ..
            } => {
                let mut b = to_device.transform_rect_bbox(path.bounding_box());
                if *stroking {
                    // A stroked clip is wider than the path by half the pen
                    // in each direction; the exact device width is the
                    // stroke module's, but the *bound* only has to contain
                    // it, and inflating by the untransformed width scaled by
                    // the matrix's larger axis always does.
                    let scale = to_device
                        .as_coeffs()
                        .iter()
                        .take(4)
                        .fold(0.0f64, |m, c| m.max(c.abs()));
                    let pad = f64::from(stroke.width).abs() * scale + 1.0;
                    b = b.inflate(pad, pad);
                }
                b
            }
            Self::Rect(r) => *r,
        }
    }

    /// Push the clip onto a device, returning how many `pop`s it owes.
    fn push(&self, device: &mut dyn RenderDevice) -> usize {
        match self {
            Self::Path {
                path,
                to_device,
                stroking,
                rule,
                stroke,
            } => {
                if *stroking {
                    // `SetClip_PathStroke`: the stroke's outline is the clip.
                    // Both rasterizers take a filled path, so the engine hands
                    // over the stroke geometry it already knows how to build.
                    let outline = crate::stroke::outline(path, *to_device, stroke);
                    device.push_clip(&outline, crate::device::FillRule::Winding);
                } else {
                    device.push_clip(&(*to_device * (*path).clone()), *rule);
                }
                1
            }
            Self::Rect(r) => {
                device.push_clip_rect(outer_rect(*r).to_rect());
                1
            }
        }
    }
}

/// Paint one pattern over one object's geometry.
///
/// This is `DrawPathWithPattern`: the dispatch on `/PatternType` plus the
/// clip both arms share. `alpha` is the object's own fill or stroke alpha,
/// still as a float — each arm rounds or truncates it its own way.
#[expect(
    clippy::too_many_arguments,
    reason = "painting a pattern needs the context, device, backend, caches, \
              the pattern, its clip geometry, the page transform and the \
              uncoloured colour"
)]
pub fn draw<B: RasterBackend>(
    ctx: &RenderCtx<'_>,
    device: &mut B::Device,
    backend: &B,
    caches: &mut RenderCaches,
    pattern: &Pattern,
    geometry: &PatternClip<'_>,
    to_device: Affine,
    device_box: Rect,
    alpha: f32,
    uncolored: Argb,
    diags: &mut Diagnostics,
) {
    if !ctx.may_recurse() {
        return;
    }
    let clip_rect = geometry.device_bounds().intersect(device_box);
    let rect = outer_rect(clip_rect);
    if !rect.is_valid() {
        return;
    }
    let pushed = geometry.push(device);
    match pattern {
        // A non-exhaustive enum from another crate needs a rest arm; a new
        // pattern type would be a spec change and would land here as "not
        // drawn" rather than as a silent wrong colour.
        Pattern::Shading(p) => {
            let matrix = to_device * p.matrix;
            if is_available_matrix(matrix) {
                // The shading's own `/BBox` bounds it in *shading* space, so
                // it intersects after the pattern matrix and before the
                // buffer is sized. Skipping it paints the shading's
                // `/Background` across the whole fill wherever the box does
                // not reach, which is the difference between a small patch of
                // gradient and a solid rectangle.
                let mut bounded = clip_rect;
                if let Some(b) = p.shading.bbox {
                    bounded = bounded.intersect(matrix.transform_rect_bbox(b));
                }
                let rect = outer_rect(bounded);
                if !rect.is_valid() {
                    clip::pop(device, pushed);
                    return;
                }
                // Rounded here and only here.
                let a = alpha_byte_rounding(alpha);
                if let Some(pixels) =
                    shading::draw_to_pixmap(&p.shading, rect, matrix, a, &ctx.opts)
                {
                    device.draw_image(
                        &pixels,
                        Affine::translate((f64::from(rect.left), f64::from(rect.top))),
                        ImageQuality::Nearest,
                        1.0,
                    );
                }
            }
        }
        Pattern::Tiling(p) => {
            draw_tiling(
                ctx, device, backend, caches, p, rect, to_device, uncolored, diags,
            );
        }
        _ => {}
    }
    clip::pop(device, pushed);
}

/// Tile one cell across a clip rectangle (`CPDF_RenderTiling::Draw`).
///
/// The cell is rendered once into its own buffer and blitted at every tile
/// position into a screen buffer the size of the clip; that buffer is then
/// composited with a plain normal blend. Rendering per tile instead would be
/// the C++'s slow path, which it takes only when the cell is larger than the
/// clip and which it documents as behaviorally identical.
#[expect(
    clippy::too_many_arguments,
    reason = "tiling needs the context, device, backend, caches, the pattern, \
              the clip rect, the page transform and the uncoloured colour"
)]
fn draw_tiling<B: RasterBackend>(
    ctx: &RenderCtx<'_>,
    device: &mut B::Device,
    backend: &B,
    caches: &mut RenderCaches,
    pattern: &TilingPattern,
    clip: IntRect,
    to_device: Affine,
    uncolored: Argb,
    diags: &mut Diagnostics,
) {
    let pattern_to_device = to_device * pattern.matrix;
    if !is_available_matrix(pattern_to_device) {
        return;
    }
    // Step one of the ladder: a cell whose size will not fit an `i32` aborts
    // the whole pattern, and a degenerate `/BBox` still yields a 1×1 tile.
    let Some((cell_w, cell_h)) = pattern.cell_size(to_device) else {
        return;
    };
    // Step four: the area guard, before anything is allocated.
    if i64::from(cell_h) > i64::from(i32::MAX) / i64::from(cell_w) {
        return;
    }
    // Steps two and three: the step validation and the tile index range,
    // both of which abort rather than clamp.
    let det = pattern_to_device.determinant();
    if det == 0.0 || !det.is_finite() {
        return;
    }
    let clip_in_pattern = pattern_to_device
        .inverse()
        .transform_rect_bbox(clip.to_rect());
    if !clip_in_pattern.x0.is_finite() || !clip_in_pattern.x1.is_finite() {
        return;
    }
    let Some(range) = pattern.tile_range(clip_in_pattern, diags) else {
        return;
    };
    let (Ok(clip_w), Ok(clip_h)) = (u32::try_from(clip.width()), u32::try_from(clip.height()))
    else {
        return;
    };
    if clip_w == 0 || clip_h == 0 || clip_w > MAX_TARGET_DIMENSION || clip_h > MAX_TARGET_DIMENSION
    {
        return;
    }

    let cell = render_cell(
        ctx,
        backend,
        caches,
        pattern,
        (cell_w, cell_h),
        to_device,
        uncolored,
        diags,
    );
    let Some(cell) = cell else {
        return;
    };

    // One screen buffer the size of the clip; every tile blits into it and
    // the whole thing composites once, at full alpha and a normal blend.
    let mut screen = Pixmap::new(clip_w, clip_h);
    let cell_bbox = pattern_to_device.transform_rect_bbox(pattern.bbox);
    let [_, _, _, _, e, f] = pattern_to_device.as_coeffs();
    let left_offset = cell_bbox.x0 - e;
    let top_offset = cell_bbox.y0 - f;
    for row in range.min_row..=range.max_row {
        for col in range.min_col..=range.max_col {
            let origin = pattern_to_device
                * kurbo::Point::new(
                    f64::from(col) * f64::from(pattern.x_step),
                    f64::from(row) * f64::from(pattern.y_step),
                );
            let (Some(x), Some(y)) = (
                checked_start(origin.x + left_offset, clip.left),
                checked_start(origin.y + top_offset, clip.top),
            ) else {
                // A tile position that will not fit an `i32` aborts the whole
                // pattern rather than skipping the tile.
                return;
            };
            blit(&mut screen, &cell, x, y);
        }
    }
    device.draw_image(
        &screen,
        Affine::translate((f64::from(clip.left), f64::from(clip.top))),
        ImageQuality::Nearest,
        1.0,
    );
}

/// One tile's device offset, through the C++'s checked `int` conversion.
fn checked_start(position: f64, clip_edge: i32) -> Option<i32> {
    let rounded = position.round();
    if !rounded.is_finite() || rounded < f64::from(i32::MIN) || rounded > f64::from(i32::MAX) {
        return None;
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the range check above is the C++'s checked conversion"
    )]
    let start = rounded as i32;
    start.checked_sub(clip_edge)
}

/// Render one tile into its own buffer.
///
/// An **uncoloured** cell renders in alpha colour mode and takes its colour
/// from the `scn` operands, so the cell's own drawing operations contribute
/// coverage rather than colour; a **coloured** one paints itself. Both force
/// `bForceHalftone` on.
#[expect(
    clippy::too_many_arguments,
    reason = "rendering a cell needs the context, backend, caches, the \
              pattern, its size, the page transform and the uncoloured colour"
)]
fn render_cell<B: RasterBackend>(
    ctx: &RenderCtx<'_>,
    backend: &B,
    caches: &mut RenderCaches,
    pattern: &TilingPattern,
    size: (i32, i32),
    to_device: Affine,
    uncolored: Argb,
    diags: &mut Diagnostics,
) -> Option<Pixmap> {
    let (Ok(w), Ok(h)) = (u32::try_from(size.0), u32::try_from(size.1)) else {
        return None;
    };
    if w == 0 || h == 0 || w > MAX_TARGET_DIMENSION || h > MAX_TARGET_DIMENSION {
        return None;
    }
    let (render_w, render_h) = render_size(w, h);
    let enlarged = (render_w, render_h) != (w, h);

    // The cell's own space maps the pattern's `/BBox` onto the whole buffer,
    // which is `mtAdjust.MatchRect(bitmap_rect, cell_bbox_in_device_space)`.
    let cell_bbox = (to_device * pattern.matrix).transform_rect_bbox(pattern.bbox);
    if !(cell_bbox.width() > 0.0 && cell_bbox.height() > 0.0) {
        return None;
    }
    let adjust = Affine::new([
        f64::from(render_w) / cell_bbox.width(),
        0.0,
        0.0,
        f64::from(render_h) / cell_bbox.height(),
        -cell_bbox.x0 * f64::from(render_w) / cell_bbox.width(),
        -cell_bbox.y0 * f64::from(render_h) / cell_bbox.height(),
    ]);

    let opts = if pattern.colored {
        crate::options::RenderOptions {
            force_halftone: true,
            ..ctx.opts.clone()
        }
    } else {
        ctx.opts.for_uncolored_tile()
    };
    let inner = RenderCtx {
        opts,
        // An uncoloured cell paints in the `scn` colour, imposed the way a
        // type-3 char proc's is: every uncoloured operation inside takes it.
        initial_fill: (!pattern.colored).then_some(uncolored),
        initial_stroke: (!pattern.colored).then_some(uncolored),
        std_cs: true,
        ..ctx.deeper()
    };
    let mut cell_device = backend.new_target(render_w, render_h, peniko::Color::TRANSPARENT);
    let target = Rect::new(0.0, 0.0, f64::from(render_w), f64::from(render_h));
    crate::walk::render_object_list(
        &inner,
        &mut cell_device,
        backend,
        caches,
        &pattern.objects,
        adjust * to_device,
        target,
        diags,
    );
    let rendered = backend.finish(cell_device);
    if !pattern.colored {
        // An uncoloured tile is a coverage mask painted in one colour: the
        // cell's alpha decides where, the `scn` colour decides what.
        return Some(recolor(&rendered, uncolored, w, h));
    }
    Some(if enlarged {
        scale_down(&rendered, w, h)
    } else {
        rendered
    })
}

/// Repaint a coverage cell in one colour, scaling it to its real size.
///
/// `CompositeMask(fill_argb)`: the cell's alpha is the coverage and every
/// pixel takes the same colour, which is what makes a `/PaintType 2` tile a
/// stencil rather than an image.
fn recolor(cell: &Pixmap, color: Argb, w: u32, h: u32) -> Pixmap {
    let scaled = if cell.width() == w && cell.height() == h {
        cell.clone()
    } else {
        scale_down(cell, w, h)
    };
    let mut out = Pixmap::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let Some(px) = scaled.pixel(x, y) else {
                continue;
            };
            let Some(&coverage) = px.get(3) else { continue };
            let a = crate::pixmap::mul255(color.a, coverage);
            out.set_pixel(
                x,
                y,
                [
                    crate::pixmap::mul255(color.r, a),
                    crate::pixmap::mul255(color.g, a),
                    crate::pixmap::mul255(color.b, a),
                    a,
                ],
            );
        }
    }
    out
}

/// Box-filter a cell down to its real size (`StretchTo` at the default
/// resample options).
fn scale_down(cell: &Pixmap, w: u32, h: u32) -> Pixmap {
    let mut out = Pixmap::new(w, h);
    if w == 0 || h == 0 || cell.width() == 0 || cell.height() == 0 {
        return out;
    }
    for y in 0..h {
        for x in 0..w {
            let (x0, x1) = span(x, w, cell.width());
            let (y0, y1) = span(y, h, cell.height());
            let mut acc = [0u32; 4];
            let mut n = 0u32;
            for sy in y0..y1 {
                for sx in x0..x1 {
                    let Some(px) = cell.pixel(sx, sy) else {
                        continue;
                    };
                    for (slot, &v) in acc.iter_mut().zip(px.iter()) {
                        *slot += u32::from(v);
                    }
                    n += 1;
                }
            }
            if n == 0 {
                continue;
            }
            let mut px = [0u8; 4];
            for (slot, &v) in px.iter_mut().zip(acc.iter()) {
                *slot = u8::try_from(v / n).unwrap_or(255);
            }
            out.set_pixel(x, y, px);
        }
    }
    out
}

/// The source span one destination pixel averages over.
fn span(index: u32, dest: u32, src: u32) -> (u32, u32) {
    let lo = u64::from(index) * u64::from(src) / u64::from(dest);
    let hi = (u64::from(index) + 1) * u64::from(src) / u64::from(dest);
    let lo = u32::try_from(lo).unwrap_or(0).min(src.saturating_sub(1));
    let hi = u32::try_from(hi).unwrap_or(src).clamp(lo + 1, src);
    (lo, hi)
}

/// Blit one tile into the screen buffer at a signed offset, source-over.
fn blit(screen: &mut Pixmap, cell: &Pixmap, x: i32, y: i32) {
    for sy in 0..cell.height() {
        for sx in 0..cell.width() {
            let (Ok(dx), Ok(dy)) = (
                u32::try_from(i64::from(x) + i64::from(sx)),
                u32::try_from(i64::from(y) + i64::from(sy)),
            ) else {
                continue;
            };
            if dx >= screen.width() || dy >= screen.height() {
                continue;
            }
            let (Some(src), Some(dst)) = (cell.pixel(sx, sy), screen.pixel(dx, dy)) else {
                continue;
            };
            let Some(&sa) = src.get(3) else { continue };
            if sa == 0 {
                continue;
            }
            let inv = 255 - sa;
            let mut px = [0u8; 4];
            for (slot, (&s, &d)) in px.iter_mut().zip(src.iter().zip(dst.iter())) {
                *slot = s.saturating_add(crate::pixmap::mul255(d, inv));
            }
            screen.set_pixel(dx, dy, px);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cell_under_sixteen_pixels_renders_enlarged() {
        // Fifteen pixels enlarges; exactly sixteen does not — the test is
        // `<`, so 4×4 lands on the boundary and renders at its own size.
        assert_eq!(render_size(3, 5), (ENLARGED_CELL, ENLARGED_CELL));
        assert_eq!(render_size(1, 1), (ENLARGED_CELL, ENLARGED_CELL));
        assert_eq!(render_size(4, 4), (4, 4));
        // And it is an area test, not a per-axis one: a tall thin cell has
        // twenty pixels and is not enlarged even though one axis is 1.
        assert_eq!(render_size(1, 20), (1, 20));
        assert_eq!(render_size(64, 64), (64, 64));
    }

    #[test]
    fn a_tile_offset_beyond_i32_aborts_rather_than_wrapping() {
        assert_eq!(checked_start(1e30, 0), None);
        assert_eq!(checked_start(f64::NAN, 0), None);
        // And an ordinary one rounds then subtracts the clip edge.
        assert_eq!(checked_start(10.6, 4), Some(7));
    }

    #[test]
    fn a_blit_clips_to_the_screen_rather_than_wrapping() {
        let mut screen = Pixmap::new(2, 2);
        let cell = Pixmap::filled(2, 2, peniko::Color::from_rgba8(255, 0, 0, 255));
        // Placed off the top left: only the bottom-right cell pixel lands.
        blit(&mut screen, &cell, -1, -1);
        assert_eq!(screen.pixel(0, 0), Some([255, 0, 0, 255]));
        assert_eq!(screen.pixel(1, 1), Some([0, 0, 0, 0]));
    }

    #[test]
    fn a_blit_composites_source_over_rather_than_replacing() {
        let mut screen = Pixmap::filled(1, 1, peniko::Color::from_rgba8(0, 0, 255, 255));
        let cell = Pixmap::filled(1, 1, peniko::Color::from_rgba8(255, 0, 0, 128));
        blit(&mut screen, &cell, 0, 0);
        let px = screen.pixel(0, 0).expect("a pixel");
        // The half-opaque red covers half the blue; neither channel is lost.
        assert!(px[0] > 100, "red arrived: {px:?}");
        assert!(px[2] > 50, "blue survived: {px:?}");
        assert_eq!(px[3], 255, "an opaque backdrop stays opaque");
    }

    #[test]
    fn an_uncolored_cell_takes_its_colour_from_the_operands() {
        // Coverage in, one colour out — the cell's own colours never survive.
        let cell = Pixmap::filled(1, 1, peniko::Color::from_rgba8(0, 255, 0, 128));
        let out = recolor(&cell, Argb::opaque(255, 0, 0), 1, 1);
        let px = out.pixel(0, 0).expect("a pixel");
        assert_eq!(px[3], 128, "the coverage is the cell's alpha");
        assert_eq!(px[1], 0, "the cell's own green is gone");
        assert!(px[0] > 100, "the operand colour is what paints: {px:?}");
    }

    #[test]
    fn scaling_down_averages_rather_than_dropping_samples() {
        // An 8x8 half-covered checkerboard averages to half coverage, which
        // is the whole point of the sub-sixteen enlargement.
        let mut cell = Pixmap::new(8, 8);
        for y in 0..8 {
            for x in 0..8 {
                if (x + y) % 2 == 0 {
                    cell.set_pixel(x, y, [255, 255, 255, 255]);
                }
            }
        }
        let out = scale_down(&cell, 2, 2);
        let px = out.pixel(0, 0).expect("a pixel");
        assert_eq!(px[3], 127, "eight of sixteen samples covered");
    }

    #[test]
    fn a_span_never_empties() {
        // Upscaling asks several destination pixels for the same source one;
        // each must still average at least one sample.
        for i in 0..8 {
            let (lo, hi) = span(i, 8, 3);
            assert!(hi > lo, "span {i} of 8 over 3 is empty");
            assert!(hi <= 3);
        }
    }
}
