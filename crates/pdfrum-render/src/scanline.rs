//! The analytic cell rasterizer: exact per-pixel area and cover, in the style
//! the oracle's scan converter uses.
//!
//! # Why cells
//!
//! A scanline rasterizer has to answer one question per pixel — what fraction
//! of it the polygon covers — and there are two ways to answer it. A
//! supersampler asks a fixed grid of sample points and counts hits, which
//! quantises coverage to the number of samples it took. An *analytic*
//! rasterizer integrates the polygon's edges directly, so a pixel's coverage
//! is the real number the geometry implies, quantised only by the output byte.
//!
//! That difference is measurable against the oracle rather than aesthetic.
//! `tiny-skia` supersamples at four subsamples per axis, so a diagonal edge
//! has seventeen distinct coverage levels and a half-covered pixel lands on
//! 8/16 of the range; the oracle's own edge writes the exact half. Over the
//! corpus that shows up as a persistent few-count spread along every
//! non-axis-aligned edge — the population `docs/status/pdfrum-render.md`
//! names the coverage band.
//!
//! # The representation
//!
//! Each pixel the polygon's boundary crosses gets a [`Cell`] carrying two
//! integers:
//!
//! - **`cover`** — the net signed vertical distance the boundary travelled
//!   through this pixel, in [`SUBPIXEL_SCALE`]ths of a pixel. Summing `cover`
//!   left to right along a scanline gives the winding number, scaled, at every
//!   point to the right of the cell.
//! - **`area`** — twice the signed area the boundary swept *inside* this
//!   pixel, in the same units squared. It is the correction that turns the
//!   running `cover` into the exact coverage of the boundary pixel itself.
//!
//! A cell is therefore a *difference*: interior pixels between two boundaries
//! carry no cell at all, and their coverage falls out of the running sum. That
//! is what makes the sweep linear in the boundary rather than in the area.
//!
//! Both quantities are exact integers. There is no floating-point accumulation
//! anywhere in the sweep, so the same path always produces the same bytes on
//! every machine — the determinism property the conformance harness needs and
//! that a SIMD-dispatched rasterizer cannot promise for free.
//!
//! # Why it lives in the engine rather than in a backend
//!
//! It began as `pdfrum-raster-exact`'s private integrator, and the analytic
//! backend is still its largest consumer. It moved here when a *second*
//! consumer appeared that is not a backend at all: [`crate::glyph`] rasterizes
//! every small glyph into an alpha bitmap, and that bitmap must be identical
//! under all three rasterizers, because the oracle's own glyph bitmap is
//! produced by FreeType rather than by whatever draws the page's paths.
//!
//! That is the same argument [`crate::blend::composite_premultiplied`] already
//! makes: a decision the *engine* takes has one implementation the engine owns,
//! and Tier C's guarantee — that every engine decision is identical under both
//! gating backends — then holds by construction rather than by testing.

use store::CellStore;

mod store;

/// The subpixel grid the rasterizer works on: 256 steps per pixel per axis.
///
/// Coordinates arrive as `f64` device pixels and are scaled by this and
/// truncated, so the rasterizer's whole interior is integer arithmetic. 256 is
/// the oracle's own `poly_base_size` (`agg_rasterizer_scanline_aa.h:45-48`),
/// and it is load-bearing rather than a tunable: [`coverage_to_alpha`]'s
/// mapping is derived from this scale, and changing it would change every
/// antialiased edge byte in the corpus.
pub const SUBPIXEL_SCALE: i32 = 256;

/// `log2(SUBPIXEL_SCALE)`, the shift the coordinate conversion uses.
pub const SUBPIXEL_SHIFT: u32 = 8;

/// The low bits of a subpixel coordinate: its position within its pixel.
pub const SUBPIXEL_MASK: i32 = SUBPIXEL_SCALE - 1;

/// One pixel's accumulated boundary contribution.
///
/// `cover` and `area` are signed because a boundary crossing downward
/// contributes the negative of one crossing upward, which is what makes the
/// winding number fall out of a running sum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Cell {
    /// Pixel column.
    pub x: i32,
    /// Pixel row.
    pub y: i32,
    /// Net signed vertical travel through this pixel, in subpixel units.
    pub cover: i32,
    /// Twice the signed swept area inside this pixel, in subpixel units
    /// squared.
    pub area: i32,
}

impl Cell {
    /// A fresh, empty cell at a position.
    const fn at(x: i32, y: i32) -> Self {
        Self {
            x,
            y,
            cover: 0,
            area: 0,
        }
    }

    /// Whether this cell would contribute nothing to the sweep.
    const fn is_empty(self) -> bool {
        self.cover == 0 && self.area == 0
    }
}

/// Accumulates a path's boundary into cells.
///
/// The lifecycle is: [`Rasterizer::move_to`] and [`Rasterizer::line_to`] for
/// each flattened subpath, [`Rasterizer::close_polygon`] to close it, then
/// [`Rasterizer::sweep`] to turn the cells into spans. Curves are flattened by
/// the caller — this type sees only straight segments, which is also all the
/// oracle's scan converter sees.
#[derive(Debug, Default)]
pub struct Rasterizer {
    store: CellStore,
    /// The cell currently being accumulated into, held out of the store so a
    /// run of segments crossing one pixel costs no lookup.
    current: Option<Cell>,
    /// Where the pen is, in subpixel coordinates.
    x: i32,
    y: i32,
    /// Where the current subpath started, for `close_polygon`.
    start_x: i32,
    start_y: i32,
    /// Whether a subpath is open — a `line_to` without a preceding `move_to`
    /// is ignored rather than treated as starting at the origin.
    open: bool,
}

/// The largest device coordinate [`to_subpixel`] will carry, in pixels.
///
/// The subpixel grid must hold `coordinate * 256` without overflowing `i32`,
/// and the sweep additionally sums covers across a scanline, so the bound is
/// set well inside `i32::MAX / SUBPIXEL_SCALE` to leave headroom for both. The
/// engine's own ±32000 clamp is two orders of magnitude tighter, so this is
/// reached only by a coordinate that clamp did not see.
const COORDINATE_LIMIT: f64 = (1i32 << 22) as f64;

/// A device coordinate as a subpixel one.
///
/// Truncating toward zero, matching the oracle's `int(c * 256)`
/// (`agg_rasterizer_scanline_aa.h:50-53`) rather than rounding. The clamp
/// keeps a coordinate the engine's own ±32000 bound somehow missed from
/// overflowing the multiply; it can only be reached by a non-finite value,
/// which becomes zero.
#[must_use]
pub fn to_subpixel(v: f64) -> i32 {
    if !v.is_finite() {
        return 0;
    }
    let scaled = v.clamp(-COORDINATE_LIMIT, COORDINATE_LIMIT) * f64::from(SUBPIXEL_SCALE);
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the clamp bounds the product to +/-2^30; truncation toward \
                  zero is the ported conversion, not an accident"
    )]
    let out = scaled as i32;
    out
}

impl Rasterizer {
    /// A rasterizer with no cells.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Forget every cell, keeping the allocation.
    pub fn reset(&mut self) {
        self.store.clear();
        self.current = None;
        self.open = false;
    }

    /// Whether any boundary has been accumulated.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.store.is_empty() && self.current.is_none_or(Cell::is_empty)
    }

    /// Start a new subpath at a device-space point.
    pub fn move_to(&mut self, x: f64, y: f64) {
        self.close_polygon();
        let (x, y) = (to_subpixel(x), to_subpixel(y));
        self.set_current(x >> SUBPIXEL_SHIFT, y >> SUBPIXEL_SHIFT);
        self.x = x;
        self.y = y;
        self.start_x = x;
        self.start_y = y;
        self.open = true;
    }

    /// Extend the current subpath to a device-space point.
    pub fn line_to(&mut self, x: f64, y: f64) {
        if !self.open {
            return;
        }
        let (x, y) = (to_subpixel(x), to_subpixel(y));
        self.render_line(self.x, self.y, x, y);
        self.x = x;
        self.y = y;
    }

    /// Close the current subpath back to its start.
    ///
    /// A fill's boundary is closed by definition, so this runs implicitly
    /// before every `move_to` and before the sweep: an unclosed subpath in the
    /// input is filled as though the caller had closed it, which is what both
    /// PDF's `f` and the oracle's scan converter do.
    pub fn close_polygon(&mut self) {
        if !self.open {
            return;
        }
        self.render_line(self.x, self.y, self.start_x, self.start_y);
        self.x = self.start_x;
        self.y = self.start_y;
        self.open = false;
    }

    /// Add a whole flattened path, in device space.
    pub fn add_path(&mut self, path: &kurbo::BezPath, tolerance: f64) {
        // `flatten` emits only MoveTo/LineTo/ClosePath, so the match below is
        // total over what can actually arrive; the curve arms are unreachable
        // and say so rather than silently dropping geometry.
        kurbo::flatten(path.iter(), tolerance, |el| match el {
            kurbo::PathEl::MoveTo(p) => self.move_to(p.x, p.y),
            kurbo::PathEl::LineTo(p) => self.line_to(p.x, p.y),
            kurbo::PathEl::ClosePath => self.close_polygon(),
            kurbo::PathEl::QuadTo(..) | kurbo::PathEl::CurveTo(..) => {
                debug_assert!(false, "kurbo::flatten emits no curves");
            }
        });
        self.close_polygon();
    }

    /// Switch the cell being accumulated into, banking the old one.
    fn set_current(&mut self, x: i32, y: i32) {
        match self.current {
            Some(cell) if cell.x == x && cell.y == y => {}
            Some(cell) => {
                if !cell.is_empty() {
                    self.store.push(cell);
                }
                self.current = Some(Cell::at(x, y));
            }
            None => self.current = Some(Cell::at(x, y)),
        }
    }

    /// Add cover and area to the cell being accumulated into.
    fn add_cover(&mut self, cover: i32, area: i32) {
        if let Some(cell) = self.current.as_mut() {
            cell.cover = cell.cover.saturating_add(cover);
            cell.area = cell.area.saturating_add(area);
        }
    }

    /// Accumulate one segment that stays within a single scanline.
    ///
    /// `y1`/`y2` are *fractional* y within the row `ey`, so this integrates
    /// the segment's horizontal travel across whatever pixels it crosses.
    /// Splitting the general case into this makes the area integral a
    /// trapezoid per pixel, which is where the exactness comes from: each
    /// pixel's contribution is `(fx_in + fx_out) * dy`, twice the trapezoid's
    /// area, with no sampling anywhere.
    fn render_hline(&mut self, ey: i32, x1: i32, y1: i32, x2: i32, y2: i32) {
        let ex1 = x1 >> SUBPIXEL_SHIFT;
        let ex2 = x2 >> SUBPIXEL_SHIFT;
        let fx1 = x1 & SUBPIXEL_MASK;
        let fx2 = x2 & SUBPIXEL_MASK;

        // Horizontal: no vertical travel, so no cover and no area — but the
        // pen still moves, so the current cell follows it.
        if y1 == y2 {
            self.set_current(ex2, ey);
            return;
        }

        // Within one pixel: one trapezoid, closed form.
        if ex1 == ex2 {
            let delta = y2 - y1;
            self.add_cover(delta, (fx1 + fx2).saturating_mul(delta));
            return;
        }

        // Crossing pixels: split the segment at each vertical pixel boundary
        // and give each pixel its own trapezoid. The integer division carries
        // its remainder forward (`mod`/`rem`) so the pieces sum to exactly the
        // whole rather than drifting by a rounding step per pixel.
        let (p, first, incr, dx) = if x2 > x1 {
            (
                (SUBPIXEL_SCALE - fx1) * (y2 - y1),
                SUBPIXEL_SCALE,
                1,
                x2 - x1,
            )
        } else {
            (fx1 * (y2 - y1), 0, -1, x1 - x2)
        };
        if dx == 0 {
            return;
        }

        let mut delta = p / dx;
        let mut modulo = p % dx;
        if modulo < 0 {
            delta -= 1;
            modulo += dx;
        }
        self.add_cover(delta, (fx1 + first).saturating_mul(delta));

        let mut ex = ex1 + incr;
        self.set_current(ex, ey);
        let mut y = y1 + delta;

        if ex != ex2 {
            let step = SUBPIXEL_SCALE * (y2 - y + delta);
            let mut lift = step / dx;
            let mut rem = step % dx;
            if rem < 0 {
                lift -= 1;
                rem += dx;
            }
            modulo -= dx;
            // A malformed segment cannot make this unbounded: `ex` steps
            // toward `ex2` by one every iteration.
            while ex != ex2 {
                delta = lift;
                modulo += rem;
                if modulo >= 0 {
                    modulo -= dx;
                    delta += 1;
                }
                self.add_cover(delta, SUBPIXEL_SCALE.saturating_mul(delta));
                y += delta;
                ex += incr;
                self.set_current(ex, ey);
            }
        }

        delta = y2 - y;
        self.add_cover(delta, (fx2 + SUBPIXEL_SCALE - first).saturating_mul(delta));
    }

    /// Accumulate one straight segment in subpixel coordinates.
    ///
    /// Splits the segment at every horizontal pixel boundary and hands each
    /// piece to [`Rasterizer::render_hline`], so the whole integral is a sum
    /// of per-pixel trapezoids.
    fn render_line(&mut self, x1: i32, y1: i32, x2: i32, y2: i32) {
        // A segment long enough to overflow the area products is bisected
        // until it is not. The oracle does the same at the same threshold.
        const DX_LIMIT: i32 = 16384 << SUBPIXEL_SHIFT;
        let dx_total = x2.saturating_sub(x1);
        if dx_total >= DX_LIMIT || dx_total <= -DX_LIMIT {
            let cx = x1.saturating_add(x2) / 2;
            let cy = y1.saturating_add(y2) / 2;
            self.render_line(x1, y1, cx, cy);
            self.render_line(cx, cy, x2, y2);
            return;
        }

        let dy = y2 - y1;
        let ey1 = y1 >> SUBPIXEL_SHIFT;
        let ey2 = y2 >> SUBPIXEL_SHIFT;
        let fy1 = y1 & SUBPIXEL_MASK;
        let fy2 = y2 & SUBPIXEL_MASK;

        // Within one scanline: the whole segment is one horizontal pass.
        if ey1 == ey2 {
            self.render_hline(ey1, x1, fy1, x2, fy2);
            return;
        }

        // Exactly vertical: every scanline it crosses gets the same rectangle,
        // so the loop writes a constant rather than dividing per row.
        if dx_total == 0 {
            let ex = x1 >> SUBPIXEL_SHIFT;
            let two_fx = (x1 - (ex << SUBPIXEL_SHIFT)) << 1;
            let (first, incr) = if dy < 0 { (0, -1) } else { (SUBPIXEL_SCALE, 1) };

            let mut delta = first - fy1;
            self.add_cover(delta, two_fx.saturating_mul(delta));
            let mut ey = ey1 + incr;
            self.set_current(ex, ey);

            delta = first + first - SUBPIXEL_SCALE;
            let area = two_fx.saturating_mul(delta);
            while ey != ey2 {
                if let Some(cell) = self.current.as_mut() {
                    cell.cover = delta;
                    cell.area = area;
                }
                ey += incr;
                self.set_current(ex, ey);
            }
            delta = fy2 - SUBPIXEL_SCALE + first;
            self.add_cover(delta, two_fx.saturating_mul(delta));
            return;
        }

        // The general case: walk scanline by scanline, carrying the division
        // remainder so the x positions of the row crossings sum exactly.
        // The products are formed in `i64` because `(256 - fy) * dx` can
        // exceed `i32` on a long near-horizontal segment; the quotients are
        // bounded by `dx_total` and so fit back.
        let (p, first, incr, dy_abs) = if dy < 0 {
            (i64::from(fy1) * i64::from(dx_total), 0, -1, -dy)
        } else {
            (
                i64::from(SUBPIXEL_SCALE - fy1) * i64::from(dx_total),
                SUBPIXEL_SCALE,
                1,
                dy,
            )
        };
        if dy_abs == 0 {
            return;
        }
        let dy64 = i64::from(dy_abs);
        let narrow = |v: i64| -> i32 { i32::try_from(v).unwrap_or(0) };

        let mut delta = narrow(p / dy64);
        let mut modulo = narrow(p % dy64);
        if modulo < 0 {
            delta -= 1;
            modulo += dy_abs;
        }

        let mut x_from = x1.saturating_add(delta);
        self.render_hline(ey1, x1, fy1, x_from, first);
        let mut ey = ey1 + incr;
        self.set_current(x_from >> SUBPIXEL_SHIFT, ey);

        if ey != ey2 {
            let step = i64::from(SUBPIXEL_SCALE) * i64::from(dx_total);
            let mut lift = narrow(step / dy64);
            let mut rem = narrow(step % dy64);
            if rem < 0 {
                lift -= 1;
                rem += dy_abs;
            }
            modulo -= dy_abs;
            while ey != ey2 {
                delta = lift;
                modulo += rem;
                if modulo >= 0 {
                    modulo -= dy_abs;
                    delta += 1;
                }
                let x_to = x_from.saturating_add(delta);
                self.render_hline(ey, x_from, SUBPIXEL_SCALE - first, x_to, first);
                x_from = x_to;
                ey += incr;
                self.set_current(x_from >> SUBPIXEL_SHIFT, ey);
            }
        }
        self.render_hline(ey, x_from, SUBPIXEL_SCALE - first, x2, fy2);
    }

    /// Bank the in-progress cell and sort, readying the sweep.
    fn finish(&mut self) {
        self.close_polygon();
        if let Some(cell) = self.current.take()
            && !cell.is_empty()
        {
            self.store.push(cell);
        }
        self.store.sort();
    }

    /// Turn the accumulated cells into horizontal spans of coverage.
    ///
    /// `emit` receives `(x, len, alpha)` for each run of equal coverage, in
    /// increasing y then increasing x. Only non-zero alphas are emitted, so a
    /// consumer can blend unconditionally.
    pub fn sweep(&mut self, rule: FillRule, aa: bool, mut emit: impl FnMut(i32, i32, i32, u8)) {
        self.finish();
        for (y, row) in self.store.rows() {
            sweep_row(row, y, rule, aa, &mut emit);
        }
    }
}

/// Which winding rule decides a path's interior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FillRule {
    /// Non-zero winding: covered where the signed crossing count is not zero.
    #[default]
    NonZero,
    /// Even-odd: covered where the crossing count is odd.
    EvenOdd,
}

/// Sweep one scanline's cells into spans.
///
/// `row` is sorted by x. Cells sharing an x are merged; between two cells the
/// running `cover` is constant, which is the span.
fn sweep_row(
    row: &[Cell],
    y: i32,
    rule: FillRule,
    aa: bool,
    emit: &mut impl FnMut(i32, i32, i32, u8),
) {
    let mut cover = 0i32;
    let mut i = 0usize;
    while let Some(&first) = row.get(i) {
        let x = first.x;
        let mut area = first.area;
        cover = cover.saturating_add(first.cover);
        i += 1;
        // Merge every cell at this x.
        while let Some(&next) = row.get(i) {
            if next.x != x {
                break;
            }
            area = area.saturating_add(next.area);
            cover = cover.saturating_add(next.cover);
            i += 1;
        }

        // The boundary pixel itself: the running cover minus the area the
        // boundary swept inside it. `cover << (SHIFT + 1)` is the full-pixel
        // area in the same doubled units `area` is measured in.
        let mut next_x = x;
        if area != 0 {
            let alpha = coverage_to_alpha(
                (cover << (SUBPIXEL_SHIFT + 1)).saturating_sub(area),
                rule,
                aa,
            );
            if alpha != 0 {
                emit(x, 1, y, alpha);
            }
            next_x = x + 1;
        }

        // The interior run up to the next cell, at constant coverage.
        if let Some(&next) = row.get(i)
            && next.x > next_x
        {
            let alpha = coverage_to_alpha(cover << (SUBPIXEL_SHIFT + 1), rule, aa);
            if alpha != 0 {
                emit(next_x, next.x - next_x, y, alpha);
            }
        }
    }
}

/// The oracle's coverage-to-alpha mapping, measured and ported.
///
/// `area` is twice the covered area in subpixel units squared; shifting it
/// down by `2*SHIFT + 1 - 8` renormalises it to 0..=256, and the clamp at 255
/// is the only thing keeping 256 out. So the mapping is
///
/// ```text
/// alpha = min(255, floor(coverage * 256))
/// ```
///
/// — a **×256 scale clamped at the top**, not ×255, and truncating rather than
/// rounding. It was measured before it was ported: a shallow-slope fill
/// rendered through the oracle at 64x64 gives the edge ramp `223 159 95 31` on
/// grays whose true coverages are exactly ⅛, ⅜, ⅝ and ⅞, which is that formula
/// and no other. `calculate_alpha` (`agg_rasterizer_scanline_aa.h:283-297`) is
/// where it comes from, and there is **no gamma table anywhere on this path** —
/// `grep gamma` over the oracle's path driver has no hits, and two waves of the
/// burn-down separately confirmed that looking for one is wasted effort.
///
/// `aa = false` is the oracle's `aliased_path`, which does not turn the
/// rasterizer off but thresholds the same coverage at the midpoint. That is why
/// a hard-edged rect clip still goes through the identical integrator.
#[must_use]
pub fn coverage_to_alpha(area: i32, rule: FillRule, aa: bool) -> u8 {
    /// The renormalised coverage's full-cover value, 256.
    const COVER_FULL: i32 = 1 << 8;
    /// The largest alpha byte, 255 — the clamp that keeps 256 out.
    const COVER_MASK: i32 = COVER_FULL - 1;

    let mut cover = area >> (SUBPIXEL_SHIFT * 2 + 1 - 8);
    if cover < 0 {
        cover = cover.saturating_neg();
    }
    if rule == FillRule::EvenOdd {
        // Fold the winding count into 0..=256: an odd number of crossings is
        // covered, an even one is not, and the fold makes that continuous.
        cover &= (COVER_FULL * 2) - 1;
        if cover > COVER_FULL {
            cover = COVER_FULL * 2 - cover;
        }
    }
    if !aa {
        cover = if cover > COVER_MASK / 2 {
            COVER_MASK
        } else {
            0
        };
    }
    if cover > COVER_MASK {
        cover = COVER_MASK;
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the clamp above bounds cover to 0..=255"
    )]
    let byte = cover as u8;
    byte
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fill a path into a width x height coverage plane.
    fn coverage(path: &kurbo::BezPath, w: i32, h: i32, rule: FillRule, aa: bool) -> Vec<u8> {
        let cells = usize::try_from(w * h).expect("a test plane fits");
        let mut out = vec![0u8; cells];
        let mut raster = Rasterizer::new();
        raster.add_path(path, 0.1);
        raster.sweep(rule, aa, |x, len, y, alpha| {
            if y < 0 || y >= h {
                return;
            }
            for col in x.max(0)..(x + len).min(w) {
                let Ok(index) = usize::try_from(y * w + col) else {
                    continue;
                };
                if let Some(slot) = out.get_mut(index) {
                    *slot = alpha;
                }
            }
        });
        out
    }

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> kurbo::BezPath {
        let mut p = kurbo::BezPath::new();
        p.move_to((x0, y0));
        p.line_to((x1, y0));
        p.line_to((x1, y1));
        p.line_to((x0, y1));
        p.close_path();
        p
    }

    #[test]
    fn a_whole_pixel_rect_is_fully_covered() {
        let cov = coverage(&rect(1.0, 1.0, 3.0, 3.0), 4, 4, FillRule::NonZero, true);
        assert_eq!(cov.first().copied(), Some(0), "outside");
        assert_eq!(cov.get(5).copied(), Some(255), "inside (1,1)");
        assert_eq!(cov.get(10).copied(), Some(255), "inside (2,2)");
        assert_eq!(cov.get(15).copied(), Some(0), "outside (3,3)");
    }

    #[test]
    fn a_half_covered_pixel_is_exactly_half() {
        // The whole point of an analytic rasterizer: half a pixel is 128,
        // not the nearest of seventeen supersampled levels.
        let cov = coverage(&rect(0.0, 0.0, 0.5, 1.0), 1, 1, FillRule::NonZero, true);
        assert_eq!(cov.first().copied(), Some(128));
    }

    #[test]
    fn the_alpha_mapping_is_times_256_truncating() {
        // The measured ramp: coverages of 1/8, 3/8, 5/8, 7/8 over a pixel
        // give 32, 96, 160, 224 -- floor(cov * 256), not round(cov * 255).
        for (num, expected) in [(1, 32u8), (3, 96), (5, 160), (7, 224)] {
            let frac = f64::from(num) / 8.0;
            let cov = coverage(&rect(0.0, 0.0, frac, 1.0), 1, 1, FillRule::NonZero, true);
            assert_eq!(
                cov.first().copied(),
                Some(expected),
                "coverage {num}/8 must map to {expected}"
            );
        }
    }

    #[test]
    fn full_coverage_clamps_to_255_not_256() {
        // `floor(1.0 * 256)` is 256, which does not fit a byte; the clamp is
        // the only thing keeping it out, and it is why full cover is 255.
        assert_eq!(coverage_to_alpha(1 << 17, FillRule::NonZero, true), 255);
    }

    #[test]
    fn even_odd_punches_out_an_overlap() {
        // Two concentric squares: even-odd leaves the inner one empty where
        // non-zero would fill it.
        let mut p = rect(0.0, 0.0, 6.0, 6.0);
        p.extend(rect(2.0, 2.0, 4.0, 4.0).iter());
        let eo = coverage(&p, 6, 6, FillRule::EvenOdd, true);
        let nz = coverage(&p, 6, 6, FillRule::NonZero, true);
        // (3,3) is inside both squares.
        assert_eq!(eo.get(3 * 6 + 3).copied(), Some(0), "even-odd punches out");
        assert_eq!(nz.get(3 * 6 + 3).copied(), Some(255), "non-zero fills");
        // (1,1) is inside only the outer one, so both fill it.
        assert_eq!(eo.get(6 + 1).copied(), Some(255));
        assert_eq!(nz.get(6 + 1).copied(), Some(255));
    }

    #[test]
    fn aliasing_thresholds_at_the_midpoint() {
        // `aa = false` is the oracle's aliased_path: it thresholds the same
        // analytic coverage rather than turning the integrator off, so a
        // just-over-half pixel is solid and a just-under-half one is empty.
        let over = coverage(&rect(0.0, 0.0, 0.6, 1.0), 1, 1, FillRule::NonZero, false);
        let under = coverage(&rect(0.0, 0.0, 0.4, 1.0), 1, 1, FillRule::NonZero, false);
        assert_eq!(over.first().copied(), Some(255));
        assert_eq!(under.first().copied(), Some(0));
    }

    #[test]
    fn an_exact_45_degree_edge_halves_every_boundary_pixel() {
        // A diagonal through pixel corners cuts each boundary pixel into two
        // equal triangles, so 128 is the *only* partial level and any other
        // value would be a bug. This is the case a supersampler also gets
        // right, which is why it is not the one the next test measures.
        let mut p = kurbo::BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((32.0, 0.0));
        p.line_to((0.0, 32.0));
        p.close_path();
        let cov = coverage(&p, 32, 32, FillRule::NonZero, true);
        let mut levels: Vec<u8> = cov
            .iter()
            .copied()
            .filter(|&a| a != 0 && a != 255)
            .collect();
        levels.sort_unstable();
        levels.dedup();
        assert_eq!(levels, vec![128], "a 45 degree edge halves its pixels");
    }

    #[test]
    fn a_shallow_edge_has_more_than_seventeen_levels() {
        // The band this backend exists to close. `tiny-skia` supersamples at
        // four subsamples per axis, so a shallow edge can only take one of
        // seventeen coverages and a run of pixels along it steps in jumps of
        // sixteen counts. An analytic integrator produces the continuum the
        // geometry actually implies, which is what the oracle writes.
        let mut p = kurbo::BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((64.0, 0.0));
        p.line_to((64.0, 5.0));
        p.close_path();
        let cov = coverage(&p, 64, 8, FillRule::NonZero, true);
        let mut levels: Vec<u8> = cov
            .iter()
            .copied()
            .filter(|&a| a != 0 && a != 255)
            .collect();
        levels.sort_unstable();
        levels.dedup();
        assert!(
            levels.len() > 17,
            "only {} partial levels along a shallow edge",
            levels.len()
        );
    }

    #[test]
    fn winding_direction_does_not_change_coverage() {
        // A clockwise and a counter-clockwise square fill identically under
        // non-zero: the cover sum's sign is taken as magnitude.
        let cw = coverage(&rect(0.0, 0.0, 4.0, 4.0), 4, 4, FillRule::NonZero, true);
        let mut ccw = kurbo::BezPath::new();
        ccw.move_to((0.0, 0.0));
        ccw.line_to((0.0, 4.0));
        ccw.line_to((4.0, 4.0));
        ccw.line_to((4.0, 0.0));
        ccw.close_path();
        assert_eq!(cw, coverage(&ccw, 4, 4, FillRule::NonZero, true));
    }

    #[test]
    fn an_unclosed_subpath_fills_as_though_closed() {
        // PDF's `f` closes every open subpath before filling, and so does the
        // oracle's scan converter.
        let mut open = kurbo::BezPath::new();
        open.move_to((0.0, 0.0));
        open.line_to((4.0, 0.0));
        open.line_to((4.0, 4.0));
        open.line_to((0.0, 4.0));
        let closed = coverage(&rect(0.0, 0.0, 4.0, 4.0), 4, 4, FillRule::NonZero, true);
        assert_eq!(coverage(&open, 4, 4, FillRule::NonZero, true), closed);
    }

    #[test]
    fn coordinates_truncate_toward_zero() {
        // `int(c * 256)`, not `round`: the oracle's own conversion.
        assert_eq!(to_subpixel(1.0), 256);
        assert_eq!(to_subpixel(1.5), 384);
        assert_eq!(to_subpixel(-1.5), -384);
        // 0.999... truncates down rather than rounding up to the next pixel.
        assert_eq!(to_subpixel(0.999), 255);
    }

    #[test]
    fn a_non_finite_coordinate_becomes_zero_rather_than_panicking() {
        assert_eq!(to_subpixel(f64::NAN), 0);
        assert_eq!(to_subpixel(f64::INFINITY), 0);
    }

    #[test]
    fn an_empty_path_sweeps_nothing() {
        let mut r = Rasterizer::new();
        r.add_path(&kurbo::BezPath::new(), 0.1);
        let mut spans = 0;
        r.sweep(FillRule::NonZero, true, |_, _, _, _| spans += 1);
        assert_eq!(spans, 0);
    }

    #[test]
    fn a_line_to_without_a_move_to_is_ignored() {
        let mut r = Rasterizer::new();
        r.line_to(4.0, 4.0);
        assert!(r.is_empty());
    }

    #[test]
    fn total_coverage_matches_the_area_of_a_slanted_quad() {
        // The integral property an analytic rasterizer must have: summed
        // coverage equals the geometric area, to within the byte quantisation.
        let mut p = kurbo::BezPath::new();
        p.move_to((2.0, 1.0));
        p.line_to((14.0, 3.0));
        p.line_to((13.0, 14.0));
        p.line_to((1.0, 12.0));
        p.close_path();
        let cov = coverage(&p, 16, 16, FillRule::NonZero, true);
        let painted: f64 = cov.iter().map(|&a| f64::from(a) / 256.0).sum();
        // Shoelace over the four vertices.
        let pts = [(2.0, 1.0), (14.0, 3.0), (13.0, 14.0), (1.0, 12.0)];
        let mut area = 0.0f64;
        for i in 0..4 {
            let (Some(&(x0, y0)), Some(&(x1, y1))) = (pts.get(i), pts.get((i + 1) % 4)) else {
                continue;
            };
            area += x0 * y1 - x1 * y0;
        }
        let area = (area / 2.0).abs();
        assert!(
            (painted - area).abs() < 1.0,
            "painted {painted:.3} vs geometric {area:.3}"
        );
    }
}
