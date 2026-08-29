//! Types 6 and 7, Coons and tensor patch meshes (`DrawCoonPatchMeshes` and
//! `PatchDrawer`, `cpdf_rendershading.cpp:588-1003`).
//!
//! Unlike the other five rasterizers this one draws *through* a path
//! rasterizer: it subdivides a patch until each cell is either smaller than
//! two device units or flat enough in colour, then fills the cell's twelve
//! outer control points as a closed path.
//!
//! # `full_cover`, and why the scratch pixmap is not optional
//!
//! AGG fills those cells with a `full_cover` flag that bypasses coverage
//! entirely, so abutting cells overpaint each other's antialiased edges to
//! full opacity and the patch shows no seams. It is [`AntiAlias::FullCover`],
//! and it is emphatically **not** hard-edging: `full_cover` keeps the
//! rasterizer's choice of which pixels a span covers and discards only their
//! coverage *value*, where hard-edging thresholds that value at the midpoint.
//! A pixel two abutting cells each cover by 40% is painted by both under the
//! first rule and dropped by both under the second — a white pin-hole through
//! every internal seam, which is what this rasterizer used to draw.
//!
//! The cells still go into a scratch buffer at **alpha 1.0**, and the
//! shading's alpha is applied exactly once, when that buffer is blitted. That
//! is a second and separate requirement: `vello_cpu` binarizes each path's
//! *own* coverage, so a pixel claimed by two cells is written at full alpha
//! **twice**, which is harmless only when both writes are the same opaque
//! value.
//!
//! # The depth cap
//!
//! Upstream has none: termination relies solely on the two-device-unit bbox
//! test and the colour threshold, so a patch with non-finite control points —
//! reachable from a crafted mesh stream — never terminates. SPEC §8 accepts a
//! cap of 32 plus a non-finite check as additive safety; each level halves
//! the patch, so 32 levels covers any patch up to 2^32 device units.

use kurbo::{Affine, BezPath, Point, Rect, Shape};
use pdfrum_page::Rgb;
use pdfrum_page::shading::Patch;

use crate::color::Argb;
use crate::device::{AntiAlias, Brush, FillRule, RenderDevice};
use crate::shading::steps::{ColorSteps, component_to_shading_index};

/// The maximum per-component colour delta before a cell is flat-filled
/// (`kCoonColorThreshold`, `cpdf_rendershading.cpp:739`).
pub const COLOR_THRESHOLD: i32 = 4;

/// A patch smaller than this in both axes is filled rather than subdivided
/// (`cpdf_rendershading.cpp:591-595`).
pub const SMALL_PATCH: f64 = 2.0;

/// The subdivision depth cap (SPEC §8, render brief Q2). Additive safety over
/// a non-terminating recursion, not a fidelity change.
pub const MAX_DEPTH: u32 = 32;

/// An integer colour triple, the space the patch interpolator works in.
type IntColor = [i32; 3];

/// A corner colour in the interpolator's integer space.
///
/// The two cases are genuinely different quantities, which is why upstream
/// writes them as two branches rather than one conversion:
///
/// - Without a ramp the mesh carries a real colour, and each channel is
///   `(int32_t)(c * 255)` — a **truncation**, the C++ cast.
/// - With one, the mesh carries a single parametric value in the red slot and
///   the other two are dead. `ComponentToShadingIndex` maps it into the
///   ramp's 0..255 across the mesh's own decode range, which is `[1, 2]` or
///   `[0, 255]` as readily as the unit interval.
fn to_int_color(c: Rgb, range: Option<[f32; 2]>) -> IntColor {
    if let Some([lo, hi]) = range {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "the C++ takes `static_cast<int32_t>` of the same value; \
                      the index is clamped into 0..=255 where it is used as a \
                      ramp subscript"
        )]
        let index = component_to_shading_index(c.r, lo, hi) as i32;
        [index, 0, 0]
    } else {
        let [r, g, b] = c.to_bytes_truncating();
        [i32::from(r), i32::from(g), i32::from(b)]
    }
}

/// `Interpolate` (`cpdf_rendershading.cpp:686-696`): integer linear
/// interpolation with overflow detection.
///
/// Any overflow aborts the whole cell — which is upstream's own guard and the
/// only thing standing between a crafted mesh and a runaway subdivision.
fn interpolate(c0: i32, c1: i32, delta1: i32, delta2: i32) -> Option<i32> {
    if delta2 == 0 {
        return Some(c0);
    }
    c1.checked_sub(c0)?
        .checked_mul(delta1)?
        .checked_div(delta2)?
        .checked_add(c0)
}

/// The bilinear blend of a patch's four corner colours at a cell position.
fn bilinear(
    colors: &[IntColor; 4],
    left: i32,
    bottom: i32,
    x_scale: i32,
    y_scale: i32,
) -> Option<IntColor> {
    let mut out = [0i32; 3];
    for i in 0..3 {
        let (Some(&c0), Some(&c1), Some(&c2), Some(&c3)) = (
            colors.first().and_then(|c| c.get(i)),
            colors.get(1).and_then(|c| c.get(i)),
            colors.get(2).and_then(|c| c.get(i)),
            colors.get(3).and_then(|c| c.get(i)),
        ) else {
            return None;
        };
        let bottom_edge = interpolate(c0, c3, left, x_scale)?;
        let top_edge = interpolate(c1, c2, left, x_scale)?;
        let v = interpolate(bottom_edge, top_edge, bottom, y_scale)?;
        *out.get_mut(i)? = v;
    }
    Some(out)
}

/// `Distance`: the maximum per-component absolute difference.
fn distance(a: IntColor, b: IntColor) -> i32 {
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| (x - y).abs())
        .max()
        .unwrap_or(0)
}

/// A patch's sixteen (or twelve) control points, as the subdivider handles
/// them: four rows of four, the outer twelve being the boundary.
#[derive(Debug, Clone, Copy)]
struct Points {
    /// Row-major 4x4. A Coons patch's four interior points are derived.
    grid: [[Point; 4]; 4],
}

/// De Casteljau at `t = 0.5` on one cubic, returning both halves.
#[expect(
    clippy::manual_midpoint,
    reason = "`(a + b) / 2.0` is the De Casteljau step as PDFium writes it. \
              `f64::midpoint` is not the same function — it is correctly \
              rounded where this rounds twice — and swapping it in would move \
              subdivided patch cells off the oracle's pixels. The overflow \
              the lint warns about needs a coordinate near f64::MAX, which \
              `all_finite` on the resulting points already rejects."
)]
#[expect(
    clippy::many_single_char_names,
    reason = "a..f are De Casteljau's intermediate points in the order the \
              construction names them; p0..p3 are the input control points"
)]
fn split_cubic(p: [Point; 4]) -> ([Point; 4], [Point; 4]) {
    let mid = |a: Point, b: Point| Point::new((a.x + b.x) / 2.0, (a.y + b.y) / 2.0);
    let (p0, p1, p2, p3) = (p[0], p[1], p[2], p[3]);
    let a = mid(p0, p1);
    let b = mid(p1, p2);
    let c = mid(p2, p3);
    let d = mid(a, b);
    let e = mid(b, c);
    let f = mid(d, e);
    ([p0, a, d, f], [f, e, c, p3])
}

impl Points {
    /// The twelve boundary control points, in the order the mesh stream
    /// gives them, plus the four derived interior ones for a Coons patch.
    fn from_boundary(boundary: &[Point]) -> Option<Self> {
        if boundary.len() < 12 {
            return None;
        }
        let g = |i: usize| boundary.get(i).copied();
        // The ISO 32000-2 §8.7.4.5.7 boundary walk: p1..p12 counterclockwise
        // from the lower-left corner. Laid into a 4x4 grid whose corners are
        // the patch corners and whose edges are the four cubics.
        // `coons_interior` returns the four derived points in the formula's
        // own order — p11, p12, p21, p22 — and each lands in the grid slot it
        // is named for. A tensor patch fills the same four slots from the
        // stream, which is what makes the two patch types one surface.
        let interior = pdfrum_page::shading::coons_interior(boundary);
        let grid = [
            [g(0)?, g(1)?, g(2)?, g(3)?],
            [g(11)?, interior[0], interior[1], g(4)?],
            [g(10)?, interior[2], interior[3], g(5)?],
            [g(9)?, g(8)?, g(7)?, g(6)?],
        ];
        Some(Self { grid })
    }

    fn from_tensor(points: &[Point]) -> Option<Self> {
        if points.len() < 16 {
            return Self::from_boundary(points);
        }
        let g = |i: usize| points.get(i).copied();
        let grid = [
            [g(0)?, g(1)?, g(2)?, g(3)?],
            [g(11)?, g(12)?, g(13)?, g(4)?],
            [g(10)?, g(15)?, g(14)?, g(5)?],
            [g(9)?, g(8)?, g(7)?, g(6)?],
        ];
        Some(Self { grid })
    }

    fn all_finite(&self) -> bool {
        self.grid
            .iter()
            .flatten()
            .all(|p| p.x.is_finite() && p.y.is_finite())
    }

    fn bbox(&self) -> Rect {
        let mut r: Option<Rect> = None;
        for p in self.grid.iter().flatten() {
            let cell = Rect::new(p.x, p.y, p.x, p.y);
            r = Some(match r {
                Some(acc) => acc.union(cell),
                None => cell,
            });
        }
        r.unwrap_or(Rect::ZERO)
    }

    /// `IsSmall`: under two device units in both axes.
    fn is_small(&self) -> bool {
        let b = self.bbox();
        b.width() < SMALL_PATCH && b.height() < SMALL_PATCH
    }

    /// Split every row at `t = 0.5`, halving the patch along its **second**
    /// grid index.
    ///
    /// This is the split the `bottom`/`y_scale` half of the colour lattice
    /// tracks: `colors[0]` sits at grid corner `(0, 0)` and `colors[1]` at
    /// `(0, 3)`, so walking the second index is walking `c0 → c1`.
    fn split_along_columns(&self) -> (Self, Self) {
        let mut first = self.grid;
        let mut second = self.grid;
        for (i, row) in self.grid.iter().enumerate() {
            let (l, r) = split_cubic(*row);
            if let (Some(ls), Some(rs)) = (first.get_mut(i), second.get_mut(i)) {
                *ls = l;
                *rs = r;
            }
        }
        (Self { grid: first }, Self { grid: second })
    }

    /// Split every column at `t = 0.5`, halving the patch along its **first**
    /// grid index — the axis `left`/`x_scale` tracks, `c0 → c3`.
    fn split_along_rows(&self) -> (Self, Self) {
        let mut bottom = self.grid;
        let mut top = self.grid;
        for col in 0..4 {
            let column = [
                self.grid
                    .first()
                    .and_then(|r| r.get(col))
                    .copied()
                    .unwrap_or(Point::ZERO),
                self.grid
                    .get(1)
                    .and_then(|r| r.get(col))
                    .copied()
                    .unwrap_or(Point::ZERO),
                self.grid
                    .get(2)
                    .and_then(|r| r.get(col))
                    .copied()
                    .unwrap_or(Point::ZERO),
                self.grid
                    .get(3)
                    .and_then(|r| r.get(col))
                    .copied()
                    .unwrap_or(Point::ZERO),
            ];
            let (b, t) = split_cubic(column);
            for i in 0..4 {
                if let (Some(slot), Some(&v)) =
                    (bottom.get_mut(i).and_then(|r| r.get_mut(col)), b.get(i))
                {
                    *slot = v;
                }
                if let (Some(slot), Some(&v)) =
                    (top.get_mut(i).and_then(|r| r.get_mut(col)), t.get(i))
                {
                    *slot = v;
                }
            }
        }
        (Self { grid: bottom }, Self { grid: top })
    }

    /// The closed path of the twelve outer control points — the boundary,
    /// never the interior ones.
    fn boundary_path(&self) -> BezPath {
        let g = |r: usize, c: usize| {
            self.grid
                .get(r)
                .and_then(|row| row.get(c))
                .copied()
                .unwrap_or(Point::ZERO)
        };
        let mut p = BezPath::new();
        p.move_to(g(0, 0));
        p.curve_to(g(0, 1), g(0, 2), g(0, 3));
        p.curve_to(g(1, 3), g(2, 3), g(3, 3));
        p.curve_to(g(3, 2), g(3, 1), g(3, 0));
        p.curve_to(g(2, 0), g(1, 0), g(0, 0));
        p.close_path();
        p
    }
}

/// Subdivide one patch and fill its cells into `dest`.
///
/// `dest` is the scratch device: every cell is drawn at **alpha 1.0** with
/// antialiasing off, and the shading's alpha is applied by the caller when
/// the scratch is blitted.
#[expect(
    clippy::too_many_arguments,
    reason = "the eight after `dest` are the recursion's own state: the four \
              corner colours plus the (left, bottom, x_scale, y_scale) \
              lattice position each half inherits. Bundling them into a \
              struct would add a construction at every one of the four \
              recursive calls without removing a single value being threaded."
)]
#[expect(
    clippy::too_many_lines,
    reason = "the flatness test, the four-way split and the cell fill share \
              the same subdivision state and each recursive call reads all of \
              it; splitting them would turn locals into a parameter list as \
              long as the body"
)]
#[expect(
    clippy::cast_sign_loss,
    reason = "every colour component is clamped to 0..=255 immediately before \
              its cast, so no negative value reaches one"
)]
fn subdivide(
    dest: &mut dyn RenderDevice,
    points: Points,
    colors: &[IntColor; 4],
    steps: Option<&ColorSteps>,
    x_scale: i32,
    y_scale: i32,
    left: i32,
    bottom: i32,
    depth: u32,
) {
    if !points.all_finite() {
        return; // Additive: a crafted mesh cannot spin the recursion forever.
    }
    let small = points.is_small();
    let Some(c0) = bilinear(colors, left, bottom, x_scale, y_scale) else {
        return;
    };

    let flat = small || depth >= MAX_DEPTH || {
        let (Some(c1), Some(c2), Some(c3)) = (
            bilinear(colors, left, bottom.saturating_add(1), x_scale, y_scale),
            bilinear(
                colors,
                left.saturating_add(1),
                bottom.saturating_add(1),
                x_scale,
                y_scale,
            ),
            bilinear(colors, left.saturating_add(1), bottom, x_scale, y_scale),
        ) else {
            return;
        };
        let d_bottom = distance(c3, c0);
        let d_left = distance(c1, c0);
        let d_top = distance(c1, c2);
        let d_right = distance(c2, c3);
        if d_bottom < COLOR_THRESHOLD
            && d_left < COLOR_THRESHOLD
            && d_top < COLOR_THRESHOLD
            && d_right < COLOR_THRESHOLD
        {
            true
        } else {
            // Subdivide along whichever axis still varies.
            let vertical_only = d_bottom < COLOR_THRESHOLD && d_top < COLOR_THRESHOLD;
            let horizontal_only = d_left < COLOR_THRESHOLD && d_right < COLOR_THRESHOLD;
            let next = depth.saturating_add(1);
            if vertical_only {
                let (b, t) = points.split_along_columns();
                let ys = y_scale.saturating_mul(2);
                let bb = bottom.saturating_mul(2);
                subdivide(dest, b, colors, steps, x_scale, ys, left, bb, next);
                subdivide(
                    dest,
                    t,
                    colors,
                    steps,
                    x_scale,
                    ys,
                    left,
                    bb.saturating_add(1),
                    next,
                );
            } else if horizontal_only {
                let (l, r) = points.split_along_rows();
                let xs = x_scale.saturating_mul(2);
                let ll = left.saturating_mul(2);
                subdivide(dest, l, colors, steps, xs, y_scale, ll, bottom, next);
                subdivide(
                    dest,
                    r,
                    colors,
                    steps,
                    xs,
                    y_scale,
                    ll.saturating_add(1),
                    bottom,
                    next,
                );
            } else {
                // Both axes vary: halve along the columns, then halve each
                // half along the rows, so the four cells inherit the lattice
                // position each of their two splits assigns.
                let (near, far) = points.split_along_columns();
                let xs = x_scale.saturating_mul(2);
                let ys = y_scale.saturating_mul(2);
                let ll = left.saturating_mul(2);
                let bb = bottom.saturating_mul(2);
                for (half, by) in [(near, bb), (far, bb.saturating_add(1))] {
                    let (lo, hi) = half.split_along_rows();
                    subdivide(dest, lo, colors, steps, xs, ys, ll, by, next);
                    subdivide(
                        dest,
                        hi,
                        colors,
                        steps,
                        xs,
                        ys,
                        ll.saturating_add(1),
                        by,
                        next,
                    );
                }
            }
            return;
        }
    };

    if !flat {
        return;
    }
    let color = match steps {
        Some(ramp) => {
            let index = c0.first().copied().unwrap_or(0).clamp(0, 255) as usize;
            match ramp.entry(index) {
                Some(c) => c.with_alpha(255),
                None => return,
            }
        }
        None => Argb {
            a: 255,
            r: c0.first().copied().unwrap_or(0).clamp(0, 255) as u8,
            g: c0.get(1).copied().unwrap_or(0).clamp(0, 255) as u8,
            b: c0.get(2).copied().unwrap_or(0).clamp(0, 255) as u8,
        },
    };
    dest.fill_path(
        &points.boundary_path(),
        Affine::IDENTITY,
        &Brush::Solid(color.to_peniko()),
        FillRule::Winding,
        // `full_cover`: every pixel the cell touches at all, at full alpha.
        // Thresholding instead would drop a pixel two abutting cells each
        // half-cover, which is a white pin-hole along every internal seam.
        AntiAlias::FullCover,
    );
}

/// Fill one patch's cells into a scratch device.
///
/// The patch's control points must already be in the scratch's pixel space.
pub fn draw_patch(
    dest: &mut dyn RenderDevice,
    patch: &Patch,
    steps: Option<&ColorSteps>,
    component_range: [f32; 2],
    to_bitmap: Affine,
    tensor: bool,
) {
    let transformed: Vec<Point> = patch.points.iter().map(|&p| to_bitmap * p).collect();
    let Some(points) = (if tensor {
        Points::from_tensor(&transformed)
    } else {
        Points::from_boundary(&transformed)
    }) else {
        return;
    };
    if !points.all_finite() {
        return;
    }
    // A patch entirely outside the destination is skipped before any
    // subdivision, as upstream's bbox reject does.
    let bbox = points.bbox();
    if bbox.x1 <= 0.0 || bbox.y1 <= 0.0 {
        return;
    }
    // A ramp is present exactly when the mesh carried parametric values, so
    // it is also what says which of `to_int_color`'s two conversions applies.
    let range = steps.map(|_| component_range);
    let colors = patch.colors.map(|c| to_int_color(c, range));
    subdivide(dest, points, &colors, steps, 1, 1, 0, 0, 0);
}

/// Whether a patch's device-space bbox lies wholly outside a target.
#[must_use]
pub fn patch_is_offscreen(patch: &Patch, to_bitmap: Affine, width: u32, height: u32) -> bool {
    let mut bbox: Option<Rect> = None;
    for &p in &patch.points {
        let q = to_bitmap * p;
        let cell = Rect::new(q.x, q.y, q.x, q.y);
        bbox = Some(match bbox {
            Some(acc) => acc.union(cell),
            None => cell,
        });
    }
    let Some(b) = bbox else { return true };
    b.x1 <= 0.0 || b.x0 >= f64::from(width) || b.y1 <= 0.0 || b.y0 >= f64::from(height)
}

/// The device-space bounding box of a whole mesh's patches.
#[must_use]
pub fn patches_bbox(patches: &[Patch], to_bitmap: Affine) -> Option<Rect> {
    let mut bbox: Option<Rect> = None;
    for patch in patches {
        for &p in &patch.points {
            let q = to_bitmap * p;
            if !q.x.is_finite() || !q.y.is_finite() {
                continue;
            }
            let cell = Rect::new(q.x, q.y, q.x, q.y);
            bbox = Some(match bbox {
                Some(acc) => acc.union(cell),
                None => cell,
            });
        }
    }
    bbox
}

/// A closed path over a patch's twelve outer control points, for callers that
/// need the silhouette (a clip, say) rather than the filled cells.
#[must_use]
pub fn patch_outline(patch: &Patch, to_bitmap: Affine) -> BezPath {
    let transformed: Vec<Point> = patch.points.iter().map(|&p| to_bitmap * p).collect();
    Points::from_boundary(&transformed)
        .map(|p| p.boundary_path())
        .unwrap_or_default()
}

/// The area a patch outline encloses, for tests.
#[must_use]
pub fn outline_area(path: &BezPath) -> f64 {
    path.area().abs()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A corner colour with no ramp behind it — the direct-colour conversion.
    fn to_int_color_plain(c: Rgb) -> IntColor {
        to_int_color(c, None)
    }

    fn square_patch(size: f64, colors: [Rgb; 4]) -> Patch {
        // A square whose edges are straight cubics: 12 boundary points
        // counterclockwise from the lower-left.
        let s = size;
        let t = s / 3.0;
        let pts = vec![
            Point::new(0.0, 0.0),
            Point::new(0.0, t),
            Point::new(0.0, 2.0 * t),
            Point::new(0.0, s),
            Point::new(t, s),
            Point::new(2.0 * t, s),
            Point::new(s, s),
            Point::new(s, 2.0 * t),
            Point::new(s, t),
            Point::new(s, 0.0),
            Point::new(2.0 * t, 0.0),
            Point::new(t, 0.0),
        ];
        Patch {
            points: pts.into_boxed_slice(),
            colors,
        }
    }

    /// The lattice axis each split advances, pinned against the grid.
    ///
    /// This is the one relationship the whole rasterizer rests on and the one
    /// nothing else catches: swapping the two splits leaves every cell the
    /// right shape and the right size, and reflects the *colour* field across
    /// the patch's anti-diagonal. On `2_shading_type_6_00` that was 98% of
    /// pixels differing at a mean of 41 counts, with the geometry perfect.
    ///
    /// `colors` are indexed `[c0, c1, c2, c3]` and `bilinear` reads them as
    /// `c0 -> c3` along `left`/`x_scale` and `c0 -> c1` along
    /// `bottom`/`y_scale`. So `left` must advance with the split that walks
    /// the grid's **first** index and `bottom` with the **second**.
    #[test]
    fn each_split_advances_the_lattice_axis_it_walks() {
        let p = Points::from_boundary(&square_patch(12.0, [Rgb::BLACK; 4]).points).expect("built");
        // Corner (0, 0) is grid[0][0] and stays put under both splits' first
        // half; the far corner grid[3][3] is what moves.
        let (rows_lo, rows_hi) = p.split_along_rows();
        let (cols_lo, cols_hi) = p.split_along_columns();

        // Splitting along the rows halves the first index, so the halves
        // differ in grid[3][*] and agree on grid[0][*].
        assert_eq!(rows_lo.grid[0], p.grid[0], "the first row is untouched");
        assert_eq!(rows_hi.grid[3], p.grid[3], "and so is the last");
        assert_ne!(rows_lo.grid[3], rows_hi.grid[3]);

        // Splitting along the columns halves the second index instead, so
        // every row's *ends* move and the two halves meet in the middle.
        assert_eq!(cols_lo.grid[0][0], p.grid[0][0]);
        assert_eq!(cols_hi.grid[0][3], p.grid[0][3]);
        assert_eq!(
            cols_lo.grid[0][3], cols_hi.grid[0][0],
            "the halves share the split point"
        );
        assert_eq!(
            rows_lo.grid[3][0], rows_hi.grid[0][0],
            "and so do the other axis's"
        );
    }

    /// A Coons patch's four derived points land in the slots a tensor patch
    /// reads from the stream, in the same order.
    ///
    /// The two patch types are one surface, and that identity *is* the
    /// derivation: feeding a tensor patch the interiors a Coons patch would
    /// derive must give the same grid. Transposing the middle two — which is
    /// the easy mistake, since `p12` and `p21` are mirror images of one
    /// formula — shears the surface in a way only a curved patch shows.
    #[test]
    fn coons_interiors_land_in_the_tensor_slots() {
        // A deliberately asymmetric boundary, so p12 and p21 differ.
        let pts: Vec<Point> = [
            (0.0, 0.0),
            (1.0, 4.0),
            (2.0, 8.0),
            (3.0, 12.0),
            (7.0, 13.0),
            (11.0, 14.0),
            (15.0, 15.0),
            (14.0, 11.0),
            (13.0, 7.0),
            (12.0, 3.0),
            (8.0, 2.0),
            (4.0, 1.0),
        ]
        .into_iter()
        .map(|(x, y)| Point::new(x, y))
        .collect();
        let coons = Points::from_boundary(&pts).expect("built");
        let interior = pdfrum_page::shading::coons_interior(&pts);
        assert_ne!(
            interior[1], interior[2],
            "the fixture separates p12 and p21"
        );

        // The same twelve points plus those four interiors, in the stream's
        // tensor order: c12 = p11, c13 = p12, c14 = p22, c15 = p21.
        let mut tensor_pts = pts;
        tensor_pts.extend([interior[0], interior[1], interior[3], interior[2]]);
        let tensor = Points::from_tensor(&tensor_pts).expect("built");
        assert_eq!(coons.grid, tensor.grid);
    }

    /// A parametric corner colour is mapped into the ramp across the mesh's
    /// own decode range, not the unit interval.
    #[test]
    fn a_ramp_corner_maps_across_the_meshs_decode_range() {
        let t = |v: f32| Rgb {
            r: v,
            g: 0.0,
            b: 0.0,
        };
        // The unit interval is the easy case and the one that hid the bug.
        assert_eq!(to_int_color(t(0.0), Some([0.0, 1.0])), [0, 0, 0]);
        assert_eq!(to_int_color(t(1.0), Some([0.0, 1.0])), [255, 0, 0]);
        // `[1, 2]` and `[0, 255]` both occur in the corpus, and reading them
        // as `[0, 1]` pins every corner to one end of the ramp.
        assert_eq!(to_int_color(t(1.0), Some([1.0, 2.0])), [0, 0, 0]);
        assert_eq!(to_int_color(t(2.0), Some([1.0, 2.0])), [255, 0, 0]);
        assert_eq!(to_int_color(t(1.5), Some([1.0, 2.0])), [127, 0, 0]);
        assert_eq!(to_int_color(t(255.0), Some([0.0, 255.0])), [255, 0, 0]);
        // A degenerate range is upstream's divide-by-zero guard.
        assert_eq!(to_int_color(t(9.0), Some([3.0, 3.0])), [0, 0, 0]);
        // Without a ramp the value is a real colour, truncated as the C++
        // cast truncates.
        assert_eq!(
            to_int_color(
                Rgb {
                    r: 0.5,
                    g: 1.0,
                    b: 0.0
                },
                None
            ),
            [127, 255, 0]
        );
    }

    #[test]
    fn is_small_is_a_two_device_unit_bbox() {
        let p = Points::from_boundary(&square_patch(1.5, [Rgb::BLACK; 4]).points).expect("built");
        assert!(p.is_small());
        let p = Points::from_boundary(&square_patch(3.0, [Rgb::BLACK; 4]).points).expect("built");
        assert!(!p.is_small());
    }

    #[test]
    fn color_threshold_stops_subdivision() {
        // Four corner colours within three counts of each other never
        // subdivide, whatever the patch's size.
        let near = [
            Rgb {
                r: 0.0,
                g: 0.0,
                b: 0.0,
            },
            Rgb {
                r: 1.0 / 255.0,
                g: 0.0,
                b: 0.0,
            },
            Rgb {
                r: 2.0 / 255.0,
                g: 0.0,
                b: 0.0,
            },
            Rgb {
                r: 3.0 / 255.0,
                g: 0.0,
                b: 0.0,
            },
        ];
        let colors = near.map(|c| to_int_color(c, None));
        let d = distance(colors[0], colors[3]);
        assert!(d < COLOR_THRESHOLD, "delta {d} must be under the threshold");
    }

    #[test]
    fn integer_interpolate_detects_overflow() {
        assert_eq!(interpolate(0, 10, 1, 2), Some(5));
        assert_eq!(
            interpolate(7, 7, 5, 0),
            Some(7),
            "a zero span keeps the endpoint"
        );
        assert_eq!(
            interpolate(0, i32::MAX, i32::MAX, 1),
            None,
            "overflow aborts the cell"
        );
    }

    #[test]
    fn distance_is_the_max_component_delta() {
        assert_eq!(distance([0, 0, 0], [3, 9, 1]), 9);
        assert_eq!(distance([10, 10, 10], [10, 10, 10]), 0);
    }

    #[test]
    fn subdivision_axis_choice_follows_the_varying_edges() {
        // Colours varying only bottom-to-top subdivide vertically; only
        // left-to-right, horizontally. Verified through `bilinear`'s deltas,
        // which is what the choice reads.
        let vertical = [
            to_int_color_plain(Rgb {
                r: 0.0,
                g: 0.0,
                b: 0.0,
            }),
            to_int_color_plain(Rgb {
                r: 1.0,
                g: 1.0,
                b: 1.0,
            }),
            to_int_color_plain(Rgb {
                r: 1.0,
                g: 1.0,
                b: 1.0,
            }),
            to_int_color_plain(Rgb {
                r: 0.0,
                g: 0.0,
                b: 0.0,
            }),
        ];
        let c0 = bilinear(&vertical, 0, 0, 1, 1).expect("interpolates");
        let c1 = bilinear(&vertical, 0, 1, 1, 1).expect("interpolates");
        let c3 = bilinear(&vertical, 1, 0, 1, 1).expect("interpolates");
        assert!(
            distance(c3, c0) < COLOR_THRESHOLD,
            "the bottom edge is flat"
        );
        assert!(distance(c1, c0) >= COLOR_THRESHOLD, "the left edge is not");
    }

    #[test]
    fn non_finite_control_points_drop_the_patch() {
        let mut patch = square_patch(10.0, [Rgb::BLACK; 4]);
        #[expect(
            clippy::indexing_slicing,
            reason = "the fixture is a square patch with all twelve boundary \
                      points present, so index 3 exists by construction"
        )]
        {
            patch.points[3] = Point::new(f64::NAN, 0.0);
        }
        let outline = patch_outline(&patch, Affine::IDENTITY);
        // The outline still exists as a path, but the subdivider declines it.
        assert!(!outline.elements().is_empty());
        let points = Points::from_boundary(&patch.points).expect("built");
        assert!(!points.all_finite());
    }

    #[test]
    fn a_patch_left_of_the_target_is_offscreen() {
        let patch = square_patch(4.0, [Rgb::BLACK; 4]);
        assert!(patch_is_offscreen(
            &patch,
            Affine::translate((-50.0, 0.0)),
            20,
            20
        ));
        assert!(!patch_is_offscreen(&patch, Affine::IDENTITY, 20, 20));
    }

    #[test]
    fn split_cubic_halves_a_straight_line_at_its_midpoint() {
        let line = [
            Point::new(0.0, 0.0),
            Point::new(1.0, 0.0),
            Point::new(2.0, 0.0),
            Point::new(3.0, 0.0),
        ];
        let (l, r) = split_cubic(line);
        assert!((l[3].x - 1.5).abs() < 1e-9);
        assert!((r[0].x - 1.5).abs() < 1e-9);
        assert!((r[3].x - 3.0).abs() < 1e-9);
    }

    #[test]
    fn boundary_path_uses_the_twelve_outer_points_only() {
        let patch = square_patch(9.0, [Rgb::BLACK; 4]);
        let outline = patch_outline(&patch, Affine::IDENTITY);
        let bbox = outline.bounding_box();
        assert!((bbox.x0 - 0.0).abs() < 1e-9);
        assert!((bbox.x1 - 9.0).abs() < 1e-9);
        assert!(outline_area(&outline) > 0.0);
    }
}
