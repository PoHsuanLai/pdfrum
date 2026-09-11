//! Painting a path or a glyph run with a pattern colour.
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
//!   alpha in the engine truncates; `DrawShadingPattern` alone spells
//!   rounds, so `/ca 0.5` is 128 here and 127 everywhere else.
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
use crate::pixmap::{AlphaMask, Pixmap, alpha_byte_rounding};

/// The cell area below which a tile is rendered at 8×8 and scaled down.
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
/// `ClipPattern` accepts exactly two shapes and **drops the pattern** for
/// anything else, which is why this is not an `Option<BezPath>` — the third
/// case is a real one and it is not "no clip".
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
                // The same rasterizer `sh` reaches, including the scratch
                // device types 6 and 7 need. Calling the buffer-only entry
                // here made a Coons or tensor pattern paint nothing at all.
                crate::walk::draw_shading_into(ctx, device, backend, &p.shading, rect, matrix, a);
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

/// Tile one cell across a clip rectangle.
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

    // The C++ chooses between two ways of painting the tiles, and the choice
    // is only *usually* a performance one. A cell bigger than the clip — or
    // one whose area exceeds the clip's — is drawn tile by tile straight to
    // the device, because caching it would mean allocating a buffer larger
    // than the region it is about to be cropped into. `bug_1693`'s cell is
    // 102400x12800 device pixels behind a 200x200 clip, so the cached path
    // cannot allocate it at all and the whole pattern silently paints
    // nothing.
    if tiles_one_at_a_time((cell_w, cell_h), (clip.width(), clip.height())) {
        draw_tiling_per_tile(
            ctx,
            device,
            backend,
            caches,
            pattern,
            &range,
            pattern_to_device,
            uncolored,
            clip,
            diags,
        );
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
    //
    // The buffer is **straight**-alpha, not premultiplied, because
    // `pScreen->Create(..., kBgra)` is and because the difference is visible:
    // an uncoloured tile composites through `CompositeMask`, which carries one
    // flat colour and accumulates only alpha, so two overlapping tiles must
    // leave the colour untouched. Premultiplying first and blitting would
    // instead blend the colour against itself once per overlap, and the
    // truncation in each blend biases the result upward by a count or two —
    // which is exactly the 127-vs-129/130 spread `bug_1288_2` showed.
    let mut screen = Screen::new(clip_w, clip_h);
    let place = TilePlace::new(pattern, pattern_to_device, clip, cell_w, cell_h);
    let Some(range) = place.range(range) else {
        return;
    };
    for row in range.min_row..=range.max_row {
        for col in range.min_col..=range.max_col {
            let Some((x, y)) = place.origin(col, row) else {
                // A tile position that will not fit an `i32` aborts the whole
                // pattern rather than skipping the tile.
                return;
            };
            screen.blit(&cell, x, y);
        }
    }
    device.draw_image(
        &screen.into_pixmap(),
        Affine::translate((f64::from(clip.left), f64::from(clip.top))),
        ImageQuality::Nearest,
        1.0,
    );
}

/// Where each cached tile is blitted: integer grid when `bAligned`, else
/// the float-step origin rounded into the clip.
struct TilePlace {
    aligned: bool,
    orig_x: i32,
    orig_y: i32,
    cell_w: i32,
    cell_h: i32,
    clip: IntRect,
    pattern_to_device: Affine,
    x_step: f32,
    y_step: f32,
    left_offset: f64,
    top_offset: f64,
}

impl TilePlace {
    fn new(
        pattern: &TilingPattern,
        pattern_to_device: Affine,
        clip: IntRect,
        cell_w: i32,
        cell_h: i32,
    ) -> Self {
        let cell_bbox = pattern_to_device.transform_rect_bbox(pattern.bbox);
        let [_, _, _, _, e, f] = pattern_to_device.as_coeffs();
        #[expect(
            clippy::cast_possible_truncation,
            reason = "a pattern origin is a device pixel; round-to-nearest is the C++ `FXSYS_roundf`"
        )]
        let (orig_x, orig_y) = (e.round() as i32, f.round() as i32);
        Self {
            aligned: tiling_is_aligned(pattern, pattern_to_device),
            orig_x,
            orig_y,
            cell_w,
            cell_h,
            clip,
            pattern_to_device,
            x_step: pattern.x_step,
            y_step: pattern.y_step,
            left_offset: cell_bbox.x0 - e,
            top_offset: cell_bbox.y0 - f,
        }
    }

    /// `bAligned` rewrites the index range onto the integer grid so a scaled
    /// identity cell does not accumulate a float step (`2_uncolor_tiling`).
    fn range(&self, fallback: pdfrum_page::TileRange) -> Option<pdfrum_page::TileRange> {
        if self.aligned {
            aligned_tile_range(
                self.clip,
                self.orig_x,
                self.orig_y,
                self.cell_w,
                self.cell_h,
            )
        } else {
            Some(fallback)
        }
    }

    fn origin(&self, col: i32, row: i32) -> Option<(i32, i32)> {
        if self.aligned {
            let x = col
                .checked_mul(self.cell_w)
                .and_then(|off| self.orig_x.checked_add(off))
                .and_then(|v| v.checked_sub(self.clip.left))?;
            let y = row
                .checked_mul(self.cell_h)
                .and_then(|off| self.orig_y.checked_add(off))
                .and_then(|v| v.checked_sub(self.clip.top))?;
            Some((x, y))
        } else {
            let origin = self.pattern_to_device
                * kurbo::Point::new(
                    f64::from(col) * f64::from(self.x_step),
                    f64::from(row) * f64::from(self.y_step),
                );
            let x = checked_start(origin.x + self.left_offset, self.clip.left)?;
            let y = checked_start(origin.y + self.top_offset, self.clip.top)?;
            Some((x, y))
        }
    }
}

/// Whether the cell is a unit step on an axis-aligned (or 90°-rotated)
/// device grid, so tile origins can be integer multiples of the cell size
/// (`cpdf_rendertiling.cpp`'s `bAligned`).
///
/// `/BBox` must be exactly `(0, 0, XStep, YStep)` and the pattern-to-device
/// matrix must be a scale (`|b|*1000 < |a|` and `|c|*1000 < |d|`) or a
/// 90° rotation (`|a|*1000 < |b|` and `|d|*1000 < |c|`): a shear or a bbox
/// that does not fill the step still walks the float grid.
fn tiling_is_aligned(pattern: &TilingPattern, pattern_to_device: Affine) -> bool {
    // C++ compares the float bbox to the step with `==`.
    #[expect(
        clippy::float_cmp,
        reason = "bAligned is an exact float identity, not a tolerance"
    )]
    let bbox_is_cell = pattern.bbox.x0 == 0.0
        && pattern.bbox.y0 == 0.0
        && pattern.bbox.x1 == f64::from(pattern.x_step)
        && pattern.bbox.y1 == f64::from(pattern.y_step);
    if !bbox_is_cell {
        return false;
    }
    let [a, b, c, d, _, _] = pattern_to_device.as_coeffs();
    // `CFX_Matrix::IsScaled`: `|b| * 1000 < |a| && |c| * 1000 < |d|`.
    let scaled = b.abs() * 1000.0 < a.abs() && c.abs() * 1000.0 < d.abs();
    // `CFX_Matrix::Is90Rotated`: `|a| * 1000 < |b| && |d| * 1000 < |c|`.
    let rotated = a.abs() * 1000.0 < b.abs() && d.abs() * 1000.0 < c.abs();
    scaled || rotated
}

/// The inclusive tile-index range `bAligned` walks, in device pixels.
///
/// C++ integer division is toward zero, then `min` decrements when the clip
/// edge is strictly left/above the origin and `max` decrements when it is
/// left/above *or equal*. An index that will not fit `i32` aborts the
/// pattern, matching the unaligned overflow guard.
fn aligned_tile_range(
    clip: IntRect,
    orig_x: i32,
    orig_y: i32,
    width: i32,
    height: i32,
) -> Option<pdfrum_page::TileRange> {
    if width == 0 || height == 0 {
        return None;
    }
    let index = |edge: i32, orig: i32, step: i32, decrement: bool| -> Option<i32> {
        let mut idx = (i64::from(edge) - i64::from(orig)) / i64::from(step);
        if decrement {
            idx -= 1;
        }
        i32::try_from(idx).ok()
    };
    Some(pdfrum_page::TileRange {
        min_col: index(clip.left, orig_x, width, clip.left < orig_x)?,
        max_col: index(clip.right, orig_x, width, clip.right <= orig_x)?,
        min_row: index(clip.top, orig_y, height, clip.top < orig_y)?,
        max_row: index(clip.bottom, orig_y, height, clip.bottom <= orig_y)?,
    })
}

/// Whether the tiles are drawn one at a time rather than through a cached
/// cell.
///
/// Either axis larger than the clip's, or a larger area, and the cell is not
/// worth caching — in the extreme it cannot even be allocated.
///
/// The C++'s third arm is **redundant** and is kept only because it is what
/// the source says: if neither axis exceeds the clip's then neither does the
/// product, so the area test can never be the one that fires. Reproducing the
/// spelling costs nothing and keeps the predicate diffable against the C++.
#[must_use]
fn tiles_one_at_a_time(cell: (i32, i32), clip: (i32, i32)) -> bool {
    cell.0 > clip.0
        || cell.1 > clip.1
        || i64::from(cell.0) * i64::from(cell.1) > i64::from(clip.0) * i64::from(clip.1)
}

/// Draw every tile straight to the device, one object list per position.
///
/// The slow path: there is no cell buffer and nothing is composited at the
/// end. Each tile is a translated render of the pattern's
/// own objects, clipped to the region the pattern is filling. That makes it
/// the only way to paint a cell larger than the clip, where allocating the
/// cell is either impossible or wasteful.
///
/// An uncoloured pattern imposes its `scn` colour on every operation inside,
/// exactly as the cached path does — `CloneObjStates` sets *both* the fill and
/// the stroke ref to the chosen colour, so a tile's strokes take it too.
#[expect(
    clippy::too_many_arguments,
    reason = "the per-tile path needs everything the cached one does, minus \
              the cell buffer and plus the tile range"
)]
fn draw_tiling_per_tile<B: RasterBackend>(
    ctx: &RenderCtx<'_>,
    device: &mut B::Device,
    backend: &B,
    caches: &mut RenderCaches,
    pattern: &TilingPattern,
    range: &pdfrum_page::TileRange,
    pattern_to_device: Affine,
    uncolored: Argb,
    clip: IntRect,
    diags: &mut Diagnostics,
) {
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
        initial_fill: (!pattern.colored).then_some(uncolored),
        initial_stroke: (!pattern.colored).then_some(uncolored),
        ..ctx.deeper()
    };
    let target = clip.to_rect();
    for row in range.min_row..=range.max_row {
        for col in range.min_col..=range.max_col {
            // The tile's offset is a translation in *pattern* space, so it
            // rides through `pattern.matrix` with the rest of the cell and
            // needs no rounding of its own — the per-tile path never lands on
            // an integer pixel grid the way the blitted one does.
            let offset = Affine::translate((
                f64::from(col) * f64::from(pattern.x_step),
                f64::from(row) * f64::from(pattern.y_step),
            ));
            crate::walk::render_object_list(
                &inner,
                device,
                backend,
                caches,
                &pattern.objects,
                // A pattern's cell is its own object list, outside the page
                // graph the visibility pre-pass walked, so nothing in it is
                // hidden by index. An `/OC` inside a cell would need the
                // pre-pass to reach patterns too — the oracle does not, and
                // no corpus file asks.
                &pdfrum_page::Visibility::all_visible(),
                pattern_to_device * offset,
                target,
                diags,
            );
        }
    }
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
) -> Option<Cell> {
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
        // As above: a cell's objects are not the page's, so the page's
        // visibility tree does not describe them.
        &pdfrum_page::Visibility::all_visible(),
        adjust * to_device,
        target,
        diags,
    );
    let rendered = backend.finish(cell_device);
    if !pattern.colored {
        // An uncoloured tile stays a *coverage mask*. `CPDF_RenderTiling`
        // hands `DrawPatternBitmap`'s 8bpp result straight to `CompositeMask`,
        // which carries the fill colour separately; recolouring it into RGBA
        // here would make every overlap blend the colour against itself.
        return Some(Cell::Mask {
            coverage: coverage_of(&rendered, w, h),
            color: uncolored,
        });
    }
    Some(Cell::Colored(if enlarged {
        scale_down(&rendered, w, h)
    } else {
        rendered
    }))
}

/// One rendered tile, in the shape the tiler blits it in.
enum Cell {
    /// A `/PaintType 1` cell: its own premultiplied pixels.
    Colored(Pixmap),
    /// A `/PaintType 2` cell: an 8-bit coverage plane plus the one flat colour
    /// every one of its pixels takes, `CompositeMask`.
    Mask { coverage: AlphaMask, color: Argb },
}

/// A cell's alpha channel as a coverage plane, scaled to its real size.
fn coverage_of(cell: &Pixmap, w: u32, h: u32) -> AlphaMask {
    let scaled = if cell.width() == w && cell.height() == h {
        cell.clone()
    } else {
        scale_down(cell, w, h)
    };
    let mut out = AlphaMask::new(w, h);
    let plane = out.data_mut();
    for y in 0..h {
        for x in 0..w {
            if let Some(px) = scaled.pixel(x, y)
                && let Some(&coverage) = px.get(3)
                && let Some(slot) = plane.get_mut((y as usize) * (w as usize) + x as usize)
            {
                *slot = coverage;
            }
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

/// The tile screen buffer: **straight**-alpha BGRA, as `pScreen->Create` makes
/// it, held as RGBA in our own byte order.
///
/// It is a distinct type from [`Pixmap`] precisely because it is not
/// premultiplied. `CompositeRow_ByteMask2Bgra` and `CompositeRow_Bgra2Bgra`
/// both work on straight alpha, and the `dest.alpha == 0` early-out that makes
/// a first touch lossless only exists there.
struct Screen {
    width: u32,
    height: u32,
    /// Straight-alpha RGBA, four bytes per pixel.
    data: Vec<u8>,
}

impl Screen {
    fn new(width: u32, height: u32) -> Self {
        let len = (width as usize)
            .saturating_mul(height as usize)
            .saturating_mul(4);
        Self {
            width,
            height,
            data: vec![0; len],
        }
    }

    fn at(&mut self, x: u32, y: u32) -> Option<&mut [u8]> {
        let i = (y as usize)
            .checked_mul(self.width as usize)?
            .checked_add(x as usize)?
            .checked_mul(4)?;
        self.data.get_mut(i..i.checked_add(4)?)
    }

    /// Blit one tile at a signed offset, by the C++'s two composite rows.
    fn blit(&mut self, cell: &Cell, x: i32, y: i32) {
        let (cell_w, cell_h) = match cell {
            Cell::Colored(pixels) => (pixels.width(), pixels.height()),
            Cell::Mask { coverage, .. } => (coverage.width(), coverage.height()),
        };
        for sy in 0..cell_h {
            for sx in 0..cell_w {
                let (Ok(dx), Ok(dy)) = (
                    u32::try_from(i64::from(x) + i64::from(sx)),
                    u32::try_from(i64::from(y) + i64::from(sy)),
                ) else {
                    continue;
                };
                if dx >= self.width || dy >= self.height {
                    continue;
                }
                // The source's straight colour and its alpha. A coloured cell
                // arrives premultiplied from the rasterizer and is undone
                // here; an uncoloured one never had a colour to premultiply.
                let (src, src_alpha) = match cell {
                    Cell::Colored(pixels) => {
                        let Some([red, green, blue, alpha]) = pixels.pixel(sx, sy) else {
                            continue;
                        };
                        (
                            crate::pixmap::unpremultiply_rgb(red, green, blue, alpha),
                            alpha,
                        )
                    }
                    Cell::Mask { coverage, color } => {
                        let cov = coverage
                            .data()
                            .get((sy as usize) * (coverage.width() as usize) + sx as usize)
                            .copied()
                            .unwrap_or(0);
                        // `GetAlphaWithSrc`: `mask.alpha * src_scan[col] / 255`
                        // with no clip plane in play.
                        (
                            [color.r, color.g, color.b],
                            crate::pixmap::mul255(color.a, cov),
                        )
                    }
                };
                let Some(dest) = self.at(dx, dy) else {
                    continue;
                };
                let back_alpha = dest.get(3).copied().unwrap_or(0);
                // The first touch of a pixel is a plain copy: no arithmetic at
                // all, and so no rounding. Every tile position in a
                // non-overlapping tiling takes this arm, which is why the
                // oracle's uncoloured tilings come out at one exact value.
                if back_alpha == 0 {
                    dest.copy_from_slice(&[src[0], src[1], src[2], src_alpha]);
                    continue;
                }
                if src_alpha == 0 {
                    continue;
                }
                let dest_alpha = crate::pixmap::alpha_union(back_alpha, src_alpha);
                // `src_alpha * 255 / dest_alpha`, and `AlphaUnion`'s
                // definition puts `src_alpha` no higher than `dest_alpha`, so
                // the ratio is a byte.
                let ratio = u8::try_from((u32::from(src_alpha) * 255) / u32::from(dest_alpha))
                    .unwrap_or(u8::MAX);
                for (channel, &value) in src.iter().enumerate() {
                    if let Some(slot) = dest.get_mut(channel) {
                        *slot = crate::pixmap::alpha_merge(*slot, value, ratio);
                    }
                }
                if let Some(a) = dest.get_mut(3) {
                    *a = dest_alpha;
                }
            }
        }
    }

    /// The finished screen as the premultiplied pixmap `draw_image` wants.
    fn into_pixmap(self) -> Pixmap {
        let mut out = Pixmap::new(self.width, self.height);
        for y in 0..self.height {
            for x in 0..self.width {
                let base = ((y as usize) * (self.width as usize) + x as usize) * 4;
                let Some(&[red, green, blue, alpha]) = self
                    .data
                    .get(base..base + 4)
                    .and_then(|bytes| <&[u8; 4]>::try_from(bytes).ok())
                else {
                    continue;
                };
                out.set_pixel(
                    x,
                    y,
                    [
                        crate::pixmap::mul255(red, alpha),
                        crate::pixmap::mul255(green, alpha),
                        crate::pixmap::mul255(blue, alpha),
                        alpha,
                    ],
                );
            }
        }
        out
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
    fn a_cell_bigger_than_its_clip_is_tiled_one_at_a_time() {
        // A cell that fits inside the clip on both axes and in area takes the
        // cached path.
        assert!(!tiles_one_at_a_time((100, 100), (200, 200)));
        assert!(!tiles_one_at_a_time((200, 200), (200, 200)));
        // Either axis over is enough.
        assert!(tiles_one_at_a_time((201, 10), (200, 200)));
        assert!(tiles_one_at_a_time((10, 201), (200, 200)));
        // The area arm can never be the one that fires: both axes fitting
        // implies the product does. Pinned so that a future edit that makes
        // the axis tests looser has to face the question.
        assert!(!tiles_one_at_a_time((400, 30), (400, 30)));
        assert!(!tiles_one_at_a_time((399, 29), (400, 30)));
        // `bug_1693`'s cell, which cannot be allocated at all.
        assert!(tiles_one_at_a_time((102_400, 12_800), (200, 200)));
    }

    #[test]
    fn an_identity_cell_filling_its_step_is_aligned() {
        let pattern = TilingPattern {
            colored: true,
            x_step: 10.0,
            y_step: 10.0,
            bbox: Rect::new(0.0, 0.0, 10.0, 10.0),
            matrix: Affine::IDENTITY,
            resources: None,
            content: pdfrum_object::ByteSpan::from(Vec::<u8>::new()),
            objects: Vec::new(),
        };
        assert!(tiling_is_aligned(&pattern, Affine::IDENTITY));
        // A shear is not `IsScaled` and not `Is90Rotated`.
        assert!(!tiling_is_aligned(
            &pattern,
            Affine::new([1.0, 0.5, 0.0, 1.0, 0.0, 0.0])
        ));
        // A bbox that does not fill the step stays on the float grid.
        let mut inset = pattern.clone();
        inset.bbox = Rect::new(0.0, 0.0, 8.0, 10.0);
        assert!(!tiling_is_aligned(&inset, Affine::IDENTITY));
        // A 90° rotation (`a=d=0`, `b=-s`, `c=s`) is aligned.
        assert!(tiling_is_aligned(
            &pattern,
            Affine::new([0.0, -1.0, 1.0, 0.0, 0.0, 0.0])
        ));
    }

    #[test]
    fn an_aligned_range_uses_toward_zero_then_decrements() {
        // orig at (0, 0), 10×10 cells, clip [0, 0, 20, 20): tiles 0, 1, and
        // the one that starts on the exclusive edge (C++ still includes it).
        let clip = IntRect {
            left: 0,
            top: 0,
            right: 20,
            bottom: 20,
        };
        let range = aligned_tile_range(clip, 0, 0, 10, 10).expect("fits i32");
        assert_eq!(range.min_col, 0);
        assert_eq!(range.max_col, 2);
        assert_eq!(range.min_row, 0);
        assert_eq!(range.max_row, 2);
        // Clip left of the origin: toward-zero of a negative, then decrement.
        let clip = IntRect {
            left: -1,
            top: -1,
            right: 10,
            bottom: 10,
        };
        let range = aligned_tile_range(clip, 0, 0, 10, 10).expect("fits i32");
        assert_eq!(range.min_col, -1);
        assert_eq!(range.max_col, 1);
        assert_eq!(range.min_row, -1);
        assert_eq!(range.max_row, 1);
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
        let mut screen = Screen::new(2, 2);
        let cell = Cell::Colored(Pixmap::filled(
            2,
            2,
            peniko::Color::from_rgba8(255, 0, 0, 255),
        ));
        // Placed off the top left: only the bottom-right cell pixel lands.
        screen.blit(&cell, -1, -1);
        let out = screen.into_pixmap();
        assert_eq!(out.pixel(0, 0), Some([255, 0, 0, 255]));
        assert_eq!(out.pixel(1, 1), Some([0, 0, 0, 0]));
    }

    #[test]
    fn a_blit_composites_source_over_rather_than_replacing() {
        let mut screen = Screen::new(1, 1);
        screen.blit(
            &Cell::Colored(Pixmap::filled(
                1,
                1,
                peniko::Color::from_rgba8(0, 0, 255, 255),
            )),
            0,
            0,
        );
        screen.blit(
            &Cell::Colored(Pixmap::filled(
                1,
                1,
                peniko::Color::from_rgba8(255, 0, 0, 128),
            )),
            0,
            0,
        );
        let px = screen.into_pixmap().pixel(0, 0).expect("a pixel");
        // The half-opaque red covers half the blue; neither channel is lost.
        assert!(px[0] > 100, "red arrived: {px:?}");
        assert!(px[2] > 50, "blue survived: {px:?}");
        assert_eq!(px[3], 255, "an opaque backdrop stays opaque");
    }

    #[test]
    fn an_uncolored_cell_takes_its_colour_from_the_operands() {
        // Coverage in, one colour out — the cell's own colours never survive.
        let cell = Pixmap::filled(1, 1, peniko::Color::from_rgba8(0, 255, 0, 128));
        let Some(Cell::Mask { coverage, color }) = Some(Cell::Mask {
            coverage: coverage_of(&cell, 1, 1),
            color: Argb::opaque(255, 0, 0),
        }) else {
            unreachable!()
        };
        assert_eq!(coverage.data(), &[128], "the coverage is the cell's alpha");
        assert_eq!(color.g, 0, "the cell's own green never reaches the blit");
        let mut screen = Screen::new(1, 1);
        screen.blit(&Cell::Mask { coverage, color }, 0, 0);
        let px = screen.into_pixmap().pixel(0, 0).expect("a pixel");
        assert_eq!(px[3], 128, "the coverage becomes the alpha");
        assert!(px[0] > 100, "the operand colour is what paints: {px:?}");
    }

    #[test]
    fn overlapping_uncolored_tiles_keep_one_flat_colour() {
        // `bug_1288_2` in miniature. An uncoloured pattern composites through
        // `CompositeMask`, which carries one flat colour and merges only
        // alpha, so however many tiles overlap a pixel its colour is the fill
        // colour exactly — never a blend of the colour with itself, whose
        // truncation would drift upward by a count per overlap.
        let color = Argb {
            r: 0,
            g: 0,
            b: 255,
            a: 255,
        };
        let mut screen = Screen::new(1, 1);
        for _ in 0..8 {
            screen.blit(
                &Cell::Mask {
                    coverage: AlphaMask::filled(1, 1, 128),
                    color,
                },
                0,
                0,
            );
        }
        let straight = screen.data.clone();
        assert_eq!(
            straight.get(..3),
            Some(&[0u8, 0, 255][..]),
            "eight overlaps and the colour has not moved a count"
        );
        // Only the alpha accumulated, by `AlphaUnion` at each step.
        let mut a = 128u8;
        for _ in 1..8 {
            a = crate::pixmap::alpha_union(a, 128);
        }
        assert_eq!(straight.get(3), Some(&a));
    }

    #[test]
    fn a_single_uncolored_tile_lands_on_the_oracles_exact_value() {
        // The first touch of a pixel is a copy, not a merge — the arm that
        // makes a non-overlapping tiling land on one exact value. Half
        // coverage of blue over white must give the oracle's 127, which is
        // `AlphaMerge(255, 0, 128)`, and not the 128 a premultiplied
        // source-over blit produces.
        let mut screen = Screen::new(1, 1);
        screen.blit(
            &Cell::Mask {
                coverage: AlphaMask::filled(1, 1, 128),
                color: Argb {
                    r: 0,
                    g: 0,
                    b: 255,
                    a: 255,
                },
            },
            0,
            0,
        );
        assert_eq!(screen.data, vec![0, 0, 255, 128]);
        // Composited over white by the same `AlphaMerge` the device uses.
        assert_eq!(crate::pixmap::alpha_merge(255, 0, 128), 127);
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
