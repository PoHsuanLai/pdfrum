//! The analytic cell rasterizer: exact per-pixel area and cover, in the style
//! the oracle's scan converter uses.
//!
//! **Part of the backend seam.** A backend that owns its pixels rasterizes
//! with [`Rasterizer`] rather than with a rasterizer of its own, because the
//! coverage a page's edges get is exactly what the cross-backend equality
//! property compares. `pdfrum-raster-agg` is the in-tree caller.
//!
//! Coverage is *integrated* from the polygon's edges, not sampled: a pixel's
//! coverage is the real number the geometry implies, quantised only by the
//! output byte. The sweep is all-integer, so the same path always produces
//! the same bytes on every machine.

// A supersampler asks a fixed grid of sample points and counts hits, which
// quantises coverage to the number of samples it took. `tiny-skia`
// supersamples at four subsamples per axis, so a diagonal edge has seventeen
// distinct coverage levels and a half-covered pixel lands on 8/16 of the
// range, where the oracle's own edge writes the exact half. Over the corpus
// that is a persistent few-count spread along every non-axis-aligned edge.
//
// # The representation
//
// Each pixel the polygon's boundary crosses gets a `Cell` carrying two
// integers:
//
// - **`cover`** — the net signed vertical distance the boundary travelled
//   through this pixel, in `SUBPIXEL_SCALE`ths of a pixel. Summing `cover`
//   left to right along a scanline gives the winding number, scaled, at every
//   point to the right of the cell.
// - **`area`** — twice the signed area the boundary swept *inside* this
//   pixel, in the same units squared. It is the correction that turns the
//   running `cover` into the exact coverage of the boundary pixel itself.
//
// A cell is therefore a *difference*: interior pixels between two boundaries
// carry no cell at all, and their coverage falls out of the running sum. That
// is what makes the sweep linear in the boundary rather than in the area.
//
// It lives in the engine rather than in a backend because `crate::glyph`
// rasterizes every small glyph into an alpha bitmap that must be identical
// under all three rasterizers — the oracle's own glyph bitmap comes from
// FreeType rather than from whatever draws the page's paths. Same argument as
// `crate::blend::composite_premultiplied`: a decision the *engine* takes has
// one implementation the engine owns.

use core::ops::Range;

use store::CellStore;

mod store;

/// The subpixel grid the rasterizer works on: 256 steps per pixel per axis.
///
/// Coordinates arrive as `f64` device pixels and are scaled by this and
/// truncated, so the rasterizer's whole interior is integer arithmetic. 256
/// is the oracle's own `poly_base_size`, and it is load-bearing rather than a
/// tunable: [`coverage_to_alpha`]'s mapping is derived from this scale, and
/// changing it would change every antialiased edge byte in the corpus.
pub(crate) const SUBPIXEL_SCALE: i32 = 256;

/// `log2(SUBPIXEL_SCALE)`, the shift the coordinate conversion uses.
pub(crate) const SUBPIXEL_SHIFT: u32 = 8;

/// The low bits of a subpixel coordinate: its position within its pixel.
pub(crate) const SUBPIXEL_MASK: i32 = SUBPIXEL_SCALE - 1;

/// One pixel's accumulated boundary contribution.
///
/// `cover` and `area` are signed because a boundary crossing downward
/// contributes the negative of one crossing upward, which is what makes the
/// winding number fall out of a running sum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Cell {
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
///
/// A caller that will throw away the spans outside a band of rows should say
/// which band with [`Rasterizer::keep_rows`], and pay for the rows it keeps
/// rather than for the rows the path reaches.
///
/// ```
/// use pdfrum_render::scanline::{Coverage, FillRule, Rasterizer};
///
/// let mut rasterizer = Rasterizer::new();
/// rasterizer.move_to(0.0, 0.0);
/// rasterizer.line_to(4.0, 0.0);
/// rasterizer.line_to(4.0, 2.0);
/// rasterizer.line_to(0.0, 2.0);
/// rasterizer.close_polygon();
///
/// let mut spans = Vec::new();
/// rasterizer.sweep(FillRule::Winding, Coverage::Exact, |x, len, y, alpha| {
///     spans.push((x, y, len, alpha));
/// });
///
/// // A 4x2 rectangle: two full rows, no partial coverage anywhere.
/// assert_eq!(spans, [(0, 0, 4, 255), (0, 1, 4, 255)]);
/// ```
#[derive(Debug, Default)]
pub struct Rasterizer {
    store: CellStore,
    /// The cell currently being accumulated into, held out of the store so a
    /// run of segments crossing one pixel costs no lookup.
    current: Option<Cell>,
    /// The rows the caller keeps, as a half-open range of pixel rows; `None`
    /// keeps every row. See [`Rasterizer::keep_rows`].
    keep: Option<Range<i32>>,
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
/// Truncating toward zero, matching the oracle's `int(c * 256)` rather than
/// rounding. The clamp keeps a coordinate the engine's own ±32000 bound
/// somehow missed from overflowing the multiply; it can only be reached by a
/// non-finite value, which becomes zero.
#[must_use]
pub(crate) fn to_subpixel(v: f64) -> i32 {
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
    ///
    /// ```
    /// use pdfrum_render::scanline::Rasterizer;
    ///
    /// assert!(Rasterizer::new().is_empty());
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Forget every cell, keeping the allocation and the kept row range.
    ///
    /// ```
    /// use pdfrum_render::scanline::{Coverage, FillRule, Rasterizer};
    ///
    /// let mut rasterizer = Rasterizer::new();
    /// rasterizer.move_to(0.0, 0.0);
    /// rasterizer.line_to(4.0, 0.0);
    /// rasterizer.line_to(4.0, 2.0);
    /// rasterizer.line_to(0.0, 2.0);
    /// rasterizer.close_polygon();
    ///
    /// let mut spans = Vec::new();
    /// rasterizer.sweep(FillRule::Winding, Coverage::Exact, |x, len, y, alpha| {
    ///     spans.push((x, y, len, alpha));
    /// });
    ///
    /// // The allocation and the kept row range survive; the cells do not.
    /// rasterizer.reset();
    /// assert!(rasterizer.is_empty());
    /// ```
    pub fn reset(&mut self) {
        self.store.clear();
        self.current = None;
        self.open = false;
    }

    /// Keep only the cells on rows in `rows`, a half-open range of pixel rows.
    ///
    /// This is a promise about the *caller*, not a change to the geometry: it
    /// says the caller will discard every span outside `rows` anyway, so the
    /// rasterizer may drop those cells rather than sort and sweep them. The
    /// spans that do come out are byte-for-byte the ones an unrestricted
    /// rasterizer emits, because [`Rasterizer::sweep`] resolves each row from
    /// that row's cells alone — the running cover is reset at every row
    /// boundary, so a dropped row can change no other.
    ///
    /// It is worth setting whenever the path may extend far outside the
    /// target: a path whose bounding box is tens of thousands of rows tall on
    /// an 842-row page otherwise pays for every row it crosses.
    ///
    /// Only rows are restricted. A span too far left or right still costs its
    /// cells, because the horizontal extent is bounded by the path's own
    /// segment count rather than by the rows it crosses.
    ///
    /// ```
    /// use pdfrum_render::scanline::{Coverage, FillRule, Rasterizer};
    ///
    /// // A promise about the caller: it will discard every span outside
    /// // row 0 anyway, so those cells need not be sorted or swept.
    /// let mut rasterizer = Rasterizer::new();
    /// rasterizer.keep_rows(0..1);
    /// rasterizer.move_to(0.0, 0.0);
    /// rasterizer.line_to(4.0, 0.0);
    /// rasterizer.line_to(4.0, 2.0);
    /// rasterizer.line_to(0.0, 2.0);
    /// rasterizer.close_polygon();
    ///
    /// let mut rows = Vec::new();
    /// rasterizer.sweep(FillRule::Winding, Coverage::Exact, |_, _, y, _| rows.push(y));
    /// // Byte-for-byte the spans an unrestricted rasterizer emits for row 0.
    /// assert_eq!(rows, [0]);
    /// ```
    pub fn keep_rows(&mut self, rows: Range<i32>) {
        self.keep = Some(rows);
    }

    /// Whether a row would survive [`Rasterizer::keep_rows`].
    fn kept(&self, y: i32) -> bool {
        self.keep.as_ref().is_none_or(|rows| rows.contains(&y))
    }

    /// Bank a cell, unless its row is one the caller discards.
    fn bank(&mut self, cell: Cell) {
        if !cell.is_empty() && self.kept(cell.y) {
            self.store.push(cell);
        }
    }

    /// Whether any boundary has been accumulated.
    ///
    /// A path entirely outside [`Rasterizer::keep_rows`]'s range is empty by
    /// this test once it has been swept, which is what it means for the caller
    /// to have said those rows do not matter.
    ///
    /// ```
    /// use pdfrum_render::scanline::Rasterizer;
    ///
    /// let mut rasterizer = Rasterizer::new();
    /// assert!(rasterizer.is_empty());
    /// rasterizer.move_to(0.0, 0.0);
    /// rasterizer.line_to(4.0, 2.0);
    /// rasterizer.close_polygon();
    /// assert!(!rasterizer.is_empty());
    /// ```
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.store.is_empty() && self.current.is_none_or(Cell::is_empty)
    }

    /// Start a new subpath at a device-space point.
    ///
    /// ```
    /// use pdfrum_render::scanline::{Coverage, FillRule, Rasterizer};
    ///
    /// let mut rasterizer = Rasterizer::new();
    /// rasterizer.move_to(0.0, 0.0);
    /// rasterizer.line_to(4.0, 0.0);
    /// rasterizer.line_to(4.0, 2.0);
    /// rasterizer.line_to(0.0, 2.0);
    /// rasterizer.close_polygon();
    ///
    /// let mut spans = Vec::new();
    /// rasterizer.sweep(FillRule::Winding, Coverage::Exact, |x, len, y, alpha| {
    ///     spans.push((x, y, len, alpha));
    /// });
    ///
    /// assert_eq!(spans.len(), 2);
    /// ```
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
    ///
    /// ```
    /// use pdfrum_render::scanline::Rasterizer;
    ///
    /// // A `line_to` with no `move_to` before it is ignored rather than
    /// // treated as starting at the origin.
    /// let mut rasterizer = Rasterizer::new();
    /// rasterizer.line_to(4.0, 2.0);
    /// assert!(rasterizer.is_empty());
    /// ```
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
    ///
    /// ```
    /// use pdfrum_render::scanline::{Coverage, FillRule, Rasterizer};
    ///
    /// // An unclosed subpath fills as though the caller had closed it,
    /// // which is what `f` and the oracle both do -- so closing explicitly
    /// // and leaving it open give the same spans.
    /// let build = |close: bool| {
    ///     let mut rasterizer = Rasterizer::new();
    ///     rasterizer.move_to(0.0, 0.0);
    ///     rasterizer.line_to(4.0, 0.0);
    ///     rasterizer.line_to(4.0, 2.0);
    ///     rasterizer.line_to(0.0, 2.0);
    ///     if close {
    ///         rasterizer.close_polygon();
    ///     }
    ///     let mut spans = Vec::new();
    ///     rasterizer.sweep(FillRule::Winding, Coverage::Exact, |x, len, y, a| {
    ///         spans.push((x, y, len, a));
    ///     });
    ///     spans
    /// };
    /// assert_eq!(build(true), build(false));
    /// ```
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
    ///
    /// ```
    /// use pdfrum_render::scanline::{Coverage, FillRule, Rasterizer};
    ///
    /// // Curves are flattened here; the rasterizer sees only segments.
    /// use kurbo::Shape;
    ///
    /// let path = kurbo::Rect::new(0.0, 0.0, 4.0, 2.0).to_path(0.1);
    /// let mut rasterizer = Rasterizer::new();
    /// rasterizer.add_path(&path, 0.25);
    ///
    /// let mut spans = Vec::new();
    /// rasterizer.sweep(FillRule::Winding, Coverage::Exact, |x, len, y, a| {
    ///     spans.push((x, y, len, a));
    /// });
    /// assert_eq!(spans, [(0, 0, 4, 255), (0, 1, 4, 255)]);
    /// ```
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
                self.bank(cell);
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
    /// pixel's contribution is `(fx_in + fx_out) * dy`, twice the
    /// trapezoid's area, with no sampling anywhere.
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
        if let Some(cell) = self.current.take() {
            self.bank(cell);
        }
        self.store.sort();
    }

    /// Turn the accumulated cells into horizontal spans of coverage.
    ///
    /// `emit` receives `(x, len, alpha)` for each run of equal coverage, in
    /// increasing y then increasing x. Only non-zero alphas are emitted, so a
    /// consumer can blend unconditionally.
    ///
    /// Each row is resolved from its own cells: the running cover starts at
    /// zero on every row and is not carried into the next, because a closed
    /// boundary crosses each scanline an even number of times and so returns
    /// the winding count to zero by the row's end. That is what makes
    /// [`Rasterizer::keep_rows`] exact rather than approximate.
    ///
    /// ```
    /// use pdfrum_render::scanline::{Coverage, FillRule, Rasterizer};
    ///
    /// let mut rasterizer = Rasterizer::new();
    /// rasterizer.move_to(0.0, 0.0);
    /// rasterizer.line_to(4.0, 0.0);
    /// rasterizer.line_to(4.0, 2.0);
    /// rasterizer.line_to(0.0, 2.0);
    /// rasterizer.close_polygon();
    ///
    /// let mut spans = Vec::new();
    /// rasterizer.sweep(FillRule::Winding, Coverage::Exact, |x, len, y, alpha| {
    ///     spans.push((x, y, len, alpha));
    /// });
    ///
    /// // Increasing y then increasing x, and only non-zero alphas, so a
    /// // consumer can blend unconditionally.
    /// assert_eq!(spans, [(0, 0, 4, 255), (0, 1, 4, 255)]);
    /// ```
    pub fn sweep(
        &mut self,
        rule: FillRule,
        coverage: Coverage,
        mut emit: impl FnMut(i32, i32, i32, u8),
    ) {
        self.finish();
        for (y, row) in self.store.rows() {
            sweep_row(row, y, rule, coverage, &mut emit);
        }
    }
}

/// How an integrated coverage becomes an alpha byte — AGG's three modes.
///
/// See [`AntiAlias`](crate::device::AntiAlias), whose three variants these
/// mirror one for one; this is the integrator's own spelling of the same
/// choice, so `scanline` does not depend on the device vocabulary.
///
/// ```
/// use pdfrum_render::scanline::{Coverage, FillRule, Rasterizer};
///
/// // A half-covered pixel: `Exact` keeps the partial alpha, `Full`
/// // pushes any touched pixel to 255.
/// let alpha = |coverage| {
///     let mut rasterizer = Rasterizer::new();
///     rasterizer.move_to(0.0, 0.0);
///     rasterizer.line_to(0.5, 0.0);
///     rasterizer.line_to(0.5, 1.0);
///     rasterizer.line_to(0.0, 1.0);
///     rasterizer.close_polygon();
///     let mut out = 0u8;
///     rasterizer.sweep(FillRule::Winding, coverage, |_, _, _, a| out = a);
///     out
/// };
/// assert!(alpha(Coverage::Exact) < 255);
/// assert_eq!(alpha(Coverage::Full), 255);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Coverage {
    /// Coverage becomes alpha: `min(255, floor(cov * 256))`.
    #[default]
    Exact,
    /// `aliased_path`: the same coverage thresholded at its midpoint.
    Thresholded,
    /// `full_cover`: any touched pixel at 255, whatever its coverage.
    Full,
}

/// The fill rule [`Rasterizer::sweep`] takes — the same
/// [`FillRule`] the backend trait uses.
///
/// ```
/// use pdfrum_render::scanline::{Coverage, FillRule, Rasterizer};
///
/// // Two nested squares wound the same way: winding fills the hole,
/// // even-odd leaves it clear.
/// let covered = |rule| {
///     let mut rasterizer = Rasterizer::new();
///     for (lo, hi) in [(0.0, 6.0), (2.0, 4.0)] {
///         rasterizer.move_to(lo, lo);
///         rasterizer.line_to(hi, lo);
///         rasterizer.line_to(hi, hi);
///         rasterizer.line_to(lo, hi);
///         rasterizer.close_polygon();
///     }
///     let mut len = 0;
///     rasterizer.sweep(rule, Coverage::Exact, |_, l, y, _| {
///         if y == 3 {
///             len += l;
///         }
///     });
///     len
/// };
/// assert_eq!(covered(FillRule::Winding), 6);
/// assert_eq!(covered(FillRule::EvenOdd), 4);
/// ```
pub use crate::FillRule;

/// Sweep one scanline's cells into spans.
///
/// `row` is sorted by x. Cells sharing an x are merged; between two cells the
/// running `cover` is constant, which is the span.
fn sweep_row(
    row: &[Cell],
    y: i32,
    rule: FillRule,
    coverage: Coverage,
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
                coverage,
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
            let alpha = coverage_to_alpha(cover << (SUBPIXEL_SHIFT + 1), rule, coverage);
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
/// grays whose true coverages are exactly ⅛, ⅜, ⅝ and ⅞, which is that
/// formula and no other. There is **no gamma table anywhere on this path**;
/// the text path's is not reachable from here.
///
/// [`Coverage::Thresholded`] is the oracle's `aliased_path`, which does not
/// turn the rasterizer off but thresholds the same coverage at the midpoint.
/// That is why a hard-edged rect clip still goes through the identical
/// integrator. [`Coverage::Full`] is `full_cover`, which keeps the same choice
/// of covered pixels and discards the value: every pixel this integrator
/// reaches at all is written opaque.
#[must_use]
pub(crate) fn coverage_to_alpha(area: i32, rule: FillRule, coverage: Coverage) -> u8 {
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
    match coverage {
        Coverage::Exact => {}
        Coverage::Thresholded => {
            cover = if cover > COVER_MASK / 2 {
                COVER_MASK
            } else {
                0
            };
        }
        // Not a threshold: the test is against zero, so a pixel the span
        // touches at all is opaque. Two cells that each half-cover a pixel
        // both paint it, which is the seam suppression `full_cover` exists
        // for; thresholding would drop it from both.
        Coverage::Full => {
            if cover > 0 {
                cover = COVER_MASK;
            }
        }
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
    ///
    /// This is the *specification*: the rasterizer records every row the path
    /// crosses and the callback discards the ones outside the plane, which is
    /// what every consumer did before `keep_rows` existed.
    /// [`banded_coverage`] is the same plane taken the cheap way, and
    /// [`the_band_reproduces_the_unbanded_plane`] is what pins them together.
    fn coverage(path: &kurbo::BezPath, w: i32, h: i32, rule: FillRule, mode: Coverage) -> Vec<u8> {
        let cells = usize::try_from(w * h).expect("a test plane fits");
        let mut out = vec![0u8; cells];
        let mut raster = Rasterizer::new();
        raster.add_path(path, 0.1);
        raster.sweep(rule, mode, |x, len, y, alpha| {
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

    /// The same plane as [`coverage`], with the rasterizer told the rows.
    ///
    /// The callback is byte-for-byte [`coverage`]'s, kept rather than
    /// simplified: the point of the comparison is that the *only* difference
    /// between the two is where the discard happens.
    fn banded_coverage(
        path: &kurbo::BezPath,
        w: i32,
        h: i32,
        rule: FillRule,
        mode: Coverage,
    ) -> Vec<u8> {
        let cells = usize::try_from(w * h).expect("a test plane fits");
        let mut out = vec![0u8; cells];
        let mut raster = Rasterizer::new();
        raster.keep_rows(0..h);
        raster.add_path(path, 0.1);
        raster.sweep(rule, mode, |x, len, y, alpha| {
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

    /// How many cells a path banks under a row range — the cost the band cuts.
    fn banked(path: &kurbo::BezPath, rows: Option<core::ops::Range<i32>>) -> usize {
        let mut raster = Rasterizer::new();
        if let Some(rows) = rows {
            raster.keep_rows(rows);
        }
        raster.add_path(path, 0.1);
        let mut cells = 0usize;
        raster.sweep(FillRule::Winding, Coverage::Exact, |_, _, _, _| {});
        for (_, row) in raster.store.rows() {
            cells += row.len();
        }
        cells
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
        let cov = coverage(
            &rect(1.0, 1.0, 3.0, 3.0),
            4,
            4,
            FillRule::Winding,
            Coverage::Exact,
        );
        assert_eq!(cov.first().copied(), Some(0), "outside");
        assert_eq!(cov.get(5).copied(), Some(255), "inside (1,1)");
        assert_eq!(cov.get(10).copied(), Some(255), "inside (2,2)");
        assert_eq!(cov.get(15).copied(), Some(0), "outside (3,3)");
    }

    #[test]
    fn a_half_covered_pixel_is_exactly_half() {
        // The whole point of an analytic rasterizer: half a pixel is 128,
        // not the nearest of seventeen supersampled levels.
        let cov = coverage(
            &rect(0.0, 0.0, 0.5, 1.0),
            1,
            1,
            FillRule::Winding,
            Coverage::Exact,
        );
        assert_eq!(cov.first().copied(), Some(128));
    }

    #[test]
    fn the_alpha_mapping_is_times_256_truncating() {
        // The measured ramp: coverages of 1/8, 3/8, 5/8, 7/8 over a pixel
        // give 32, 96, 160, 224 -- floor(cov * 256), not round(cov * 255).
        for (num, expected) in [(1, 32u8), (3, 96), (5, 160), (7, 224)] {
            let frac = f64::from(num) / 8.0;
            let cov = coverage(
                &rect(0.0, 0.0, frac, 1.0),
                1,
                1,
                FillRule::Winding,
                Coverage::Exact,
            );
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
        assert_eq!(
            coverage_to_alpha(1 << 17, FillRule::Winding, Coverage::Exact),
            255
        );
    }

    #[test]
    fn even_odd_punches_out_an_overlap() {
        // Two concentric squares: even-odd leaves the inner one empty where
        // non-zero would fill it.
        let mut p = rect(0.0, 0.0, 6.0, 6.0);
        p.extend(rect(2.0, 2.0, 4.0, 4.0).iter());
        let eo = coverage(&p, 6, 6, FillRule::EvenOdd, Coverage::Exact);
        let nz = coverage(&p, 6, 6, FillRule::Winding, Coverage::Exact);
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
        let over = coverage(
            &rect(0.0, 0.0, 0.6, 1.0),
            1,
            1,
            FillRule::Winding,
            Coverage::Thresholded,
        );
        let under = coverage(
            &rect(0.0, 0.0, 0.4, 1.0),
            1,
            1,
            FillRule::Winding,
            Coverage::Thresholded,
        );
        assert_eq!(over.first().copied(), Some(255));
        assert_eq!(under.first().copied(), Some(0));
    }

    #[test]
    fn full_cover_tests_against_zero_rather_than_the_midpoint() {
        // `full_cover` keeps the integrator's choice of covered pixels and
        // discards the coverage *value*, so the test is `> 0` and not
        // `> 127`. The distinction is the whole point: two Coons cells that
        // each cover a shared pixel by 40% both paint it here, where
        // thresholding drops it from both and leaves a white pin-hole along
        // every internal seam of a subdivided patch.
        for frac in [0.4, 0.6, 0.05] {
            let cov = coverage(
                &rect(0.0, 0.0, frac, 1.0),
                1,
                1,
                FillRule::Winding,
                Coverage::Full,
            );
            assert_eq!(
                cov.first().copied(),
                Some(255),
                "coverage {frac} is non-zero, so full_cover writes it opaque"
            );
        }
        // A pixel the path does not reach at all stays empty — the mode does
        // not flood, it only flattens.
        let miss = coverage(
            &rect(2.0, 2.0, 3.0, 3.0),
            1,
            1,
            FillRule::Winding,
            Coverage::Full,
        );
        assert_eq!(miss.first().copied(), Some(0));
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
        let cov = coverage(&p, 32, 32, FillRule::Winding, Coverage::Exact);
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
        let cov = coverage(&p, 64, 8, FillRule::Winding, Coverage::Exact);
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
        let cw = coverage(
            &rect(0.0, 0.0, 4.0, 4.0),
            4,
            4,
            FillRule::Winding,
            Coverage::Exact,
        );
        let mut ccw = kurbo::BezPath::new();
        ccw.move_to((0.0, 0.0));
        ccw.line_to((0.0, 4.0));
        ccw.line_to((4.0, 4.0));
        ccw.line_to((4.0, 0.0));
        ccw.close_path();
        assert_eq!(cw, coverage(&ccw, 4, 4, FillRule::Winding, Coverage::Exact));
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
        let closed = coverage(
            &rect(0.0, 0.0, 4.0, 4.0),
            4,
            4,
            FillRule::Winding,
            Coverage::Exact,
        );
        assert_eq!(
            coverage(&open, 4, 4, FillRule::Winding, Coverage::Exact),
            closed
        );
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
        r.sweep(FillRule::Winding, Coverage::Exact, |_, _, _, _| spans += 1);
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
        let cov = coverage(&p, 16, 16, FillRule::Winding, Coverage::Exact);
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

    /// The paths a clip on a real page produces when its geometry leaves the
    /// target: above it, below it, both, and out to the engine's own clamp.
    ///
    /// The last is the case §18.3 measured — `hard_clip`'s +/-32000 is a
    /// deliberate artefact of the oracle's 16-bit truncation, so a path
    /// carrying 32000 device units of height reaches the integrator by design
    /// and must be answered rather than avoided.
    fn off_target_paths() -> Vec<(&'static str, kurbo::BezPath)> {
        vec![
            ("above", rect(2.0, -900.0, 6.0, 5.0)),
            ("below", rect(2.0, 3.0, 6.0, 900.0)),
            ("both", rect(2.0, -900.0, 6.0, 900.0)),
            ("clamped", rect(2.0, -32000.0, 6.0, 32000.0)),
            ("clamped-slanted", {
                let mut p = kurbo::BezPath::new();
                p.move_to((1.5, -32000.0));
                p.line_to((6.5, 32000.0));
                p.line_to((7.5, 32000.0));
                p.line_to((2.5, -32000.0));
                p.close_path();
                p
            }),
            ("left", rect(-32000.0, 2.0, 3.5, 6.0)),
            ("right", rect(4.5, 2.0, 32000.0, 6.0)),
            ("every-side", rect(-32000.0, -32000.0, 32000.0, 32000.0)),
        ]
    }

    #[test]
    fn the_band_reproduces_the_unbanded_plane() {
        // The invariant the whole change rests on: telling the rasterizer
        // which rows survive changes no byte on a row that does. Both fill
        // rules and all three coverage modes, because the band is upstream of
        // every one of them.
        for (name, path) in off_target_paths() {
            for rule in [FillRule::Winding, FillRule::EvenOdd] {
                for mode in [Coverage::Exact, Coverage::Thresholded, Coverage::Full] {
                    let spec = coverage(&path, 8, 8, rule, mode);
                    let banded = banded_coverage(&path, 8, 8, rule, mode);
                    assert_eq!(spec, banded, "{name} under {rule:?}/{mode:?}");
                }
            }
        }
    }

    #[test]
    fn a_path_that_stays_inside_the_band_is_untouched_by_it() {
        // The control for the test above: a path with nothing to discard must
        // still agree, or the band would be hiding a difference behind the
        // rows it drops.
        let path = rect(1.25, 1.75, 6.5, 5.5);
        for rule in [FillRule::Winding, FillRule::EvenOdd] {
            for mode in [Coverage::Exact, Coverage::Thresholded, Coverage::Full] {
                assert_eq!(
                    coverage(&path, 8, 8, rule, mode),
                    banded_coverage(&path, 8, 8, rule, mode),
                    "{rule:?}/{mode:?}"
                );
            }
        }
    }

    #[test]
    fn the_band_keeps_the_targets_last_row() {
        // A half-open range read as closed, or closed as half-open, moves the
        // boundary by one and the last row is where that shows. The path is
        // painted across rows 6 and 7 of an eight-row plane, so row 7 is both
        // the last kept row and the one an off-by-one drops.
        let path = rect(1.0, 6.0, 7.0, 8.0);
        let banded = banded_coverage(&path, 8, 8, FillRule::Winding, Coverage::Exact);
        assert_eq!(banded.get(8 * 7 + 3).copied(), Some(255), "the last row");
        assert_eq!(
            coverage(&path, 8, 8, FillRule::Winding, Coverage::Exact),
            banded
        );
    }

    #[test]
    fn the_band_keeps_the_targets_first_row() {
        // The other end, for the same reason.
        let path = rect(1.0, -4.0, 7.0, 1.0);
        let banded = banded_coverage(&path, 8, 8, FillRule::Winding, Coverage::Exact);
        assert_eq!(banded.first().copied(), Some(0), "column 0 is outside");
        assert_eq!(banded.get(3).copied(), Some(255), "the first row");
        assert_eq!(
            coverage(&path, 8, 8, FillRule::Winding, Coverage::Exact),
            banded
        );
    }

    #[test]
    fn the_band_drops_the_rows_it_says_it_drops() {
        // The measurement the change exists for. A path 64 000 rows tall over
        // an eight-row target banks tens of thousands of cells unbanded and a
        // handful banded, which is the 530 073 rows of a real render in
        // miniature.
        let path = rect(2.0, -32000.0, 6.0, 32000.0);
        let unbanded = banked(&path, None);
        let banded = banked(&path, Some(0..8));
        assert!(
            unbanded > 60_000,
            "the unbanded store holds a cell per crossed row, got {unbanded}"
        );
        assert!(
            banded <= 32,
            "the banded store holds only the target's rows, got {banded}"
        );
    }

    #[test]
    fn a_band_is_only_about_rows() {
        // Stated as a test because it is a limit of the fix rather than an
        // oversight: a path 64 000 columns wide costs its cells either way,
        // because the cells a segment banks are bounded by the pixels it
        // crosses and a horizontal one crosses them all on a single row.
        let path = rect(-32000.0, 2.0, 32000.0, 6.0);
        assert_eq!(banked(&path, None), banked(&path, Some(0..8)));
    }

    #[test]
    fn a_band_outside_the_path_leaves_nothing() {
        // The degenerate end of the range: a target the path misses entirely
        // paints nothing, which is what the unbanded spelling's callback did
        // by returning on every row.
        let path = rect(2.0, -900.0, 6.0, -100.0);
        assert_eq!(
            banded_coverage(&path, 8, 8, FillRule::Winding, Coverage::Exact),
            vec![0u8; 64]
        );
        assert_eq!(banked(&path, Some(0..8)), 0);
    }

    #[test]
    fn every_rows_cover_returns_to_zero_by_its_end() {
        // The proof `keep_rows` rests on, checked rather than argued: the
        // sweep starts each row's running cover at zero because a closed
        // boundary crosses a scanline an even number of times, so the cover
        // one row leaves behind is nothing the next one needs. If it were
        // not, dropping a row would corrupt the rows below it.
        for (name, path) in off_target_paths() {
            let mut raster = Rasterizer::new();
            raster.add_path(&path, 0.1);
            raster.sweep(FillRule::Winding, Coverage::Exact, |_, _, _, _| {});
            for (y, row) in raster.store.rows() {
                let cover: i32 = row.iter().map(|c| c.cover).sum();
                assert_eq!(cover, 0, "{name} leaves cover on row {y}");
            }
        }
    }

    #[test]
    fn a_reset_keeps_the_band_the_caller_set() {
        // `AggDevice` sets the band once per sweep, but `reset` is what a
        // reused rasterizer calls between paths and it must not quietly widen
        // the range back to everything.
        let mut raster = Rasterizer::new();
        raster.keep_rows(0..8);
        raster.add_path(&rect(2.0, -900.0, 6.0, 900.0), 0.1);
        raster.sweep(FillRule::Winding, Coverage::Exact, |_, _, _, _| {});
        raster.reset();
        raster.add_path(&rect(2.0, -900.0, 6.0, 900.0), 0.1);
        raster.sweep(FillRule::Winding, Coverage::Exact, |_, _, _, _| {});
        let cells: usize = raster.store.rows().map(|(_, row)| row.len()).sum();
        assert!(cells <= 32, "the band survives a reset, got {cells}");
    }
}
