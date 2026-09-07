//! Tier C: two rasterizers against each other, over our own engine's
//! output.
//!
//! # Three backends, one gating pair
//!
//! There are three `RasterBackend` implementations, and this tier gates on
//! **`tiny-skia` against `vello_cpu`** only. That is not an oversight and not
//! a cost saving.
//!
//! The tier's whole value is that two *independent* implementations disagree
//! out loud: neither wrapped rasterizer knows anything about PDF, so anything
//! they agree on is the engine's decision and anything they differ on is a
//! rasterizer's own business. `pdfrum-raster-agg` does not have that
//! independence — it is ours, and it shares the engine's compositing
//! arithmetic, one `blend::composite_premultiplied` serving the engine and all
//! three backends. A disagreement between it and either wrapped backend
//! therefore tests *less* than a disagreement between the two wrapped ones,
//! because a shared bug cannot produce one.
//!
//! So it is measured and reported as a third column
//! ([`crate::run::TierCOutcome::agg_edge_rate`]) and never gated. If it ever
//! diverges widely from the pair, that is worth reading — but it is a
//! question, not a verdict.
//!
//! Tier B asks whether we match the oracle; Tier C asks whether the *engine*
//! is the thing that decided the answer. Every pixel falls into one of two
//! populations:
//!
//! - **Decisions belong to the engine and must be identical.** Geometry, the
//!   resolved colours, the shading rasterizers' per-pixel maths, layer
//!   structure, object visibility. A difference here is an engine bug —
//!   something read backend state it should not have — and no rounding
//!   budget covers it.
//! - **Edge coverage belongs to the rasterizer and may differ.** The two
//!   integrate an antialiased edge differently by construction: `tiny-skia`
//!   supersamples, `vello_cpu` computes analytic area. Curve flattening,
//!   stroke outlines at joins, and image filter kernels differ likewise.
//!
//! Without a device-call trace the harness cannot derive the edge mask from
//! the *inputs*, so this measures the two populations by a proxy that cannot
//! absorb a real disagreement: a pixel is called an **edge** pixel only when
//! at least one of its eight neighbours differs from it *within the same
//! image*. A flat interior region has no such neighbours, so a divergence
//! there is counted as interior however large the region is.
//!
//! # The proxy is dilated, because the contract's mask is
//!
//! The neighbourhood test alone marks the antialiased pixel and stops. The
//! contract's mask is `dilate(union of primitive edges, 1px)`, so a boundary
//! contributes its own ramp *and* the ring of pixels around it — and
//! [`edge_mask`] therefore applies a one-pixel dilation. Skipping it would
//! be a divergence from the contract, and it has a specific consequence.
//!
//! A tiling pattern's cell is rasterized once, antialiased like any other
//! content, and the finished cell is then blitted at every tile position
//! (`CPDF_RenderTiling`). The cell's own edge pixels arrive at the device
//! inside a composited image, a pixel or two in from anything the *page* has
//! a boundary at, and the two rasterizers distribute that cell's coverage
//! differently. Undilated, such a pixel sits in a locally flat neighbourhood
//! in both images and is scored as interior — a hard failure attributed to
//! the engine for a difference the rasterizers are entitled to.
//!
//! # Dilating from agreement, not from the difference
//!
//! Dilating the *union* of the two masks would be worse than not dilating at
//! all. A region where the images differ carries a boundary in one of them by
//! construction — the differing patch's own rim — so growing the union rolls
//! that rim inward over the difference and calls it an edge. The metric would
//! then absorb exactly what it exists to detect: a solid patch dropped into a
//! flat field would score clean.
//!
//! So the growth starts from the **intersection**: only pixels where both
//! backends independently found a boundary seed the dilation. That keeps it
//! anchored to structure the two agree is there. It can excuse a coverage
//! difference beside real geometry, and it cannot manufacture the licence for
//! a difference out of the difference itself.
//!
//! What that deliberately leaves as a hard failure: a divergence in open
//! space, at any size. A page one backend fills red and the other fills white
//! has no agreed boundary anywhere, so every pixel stays interior. That is
//! not hypothetical — it is `bug_554151`, and
//! `tests::a_whole_page_colour_divergence_survives_dilation` is it reduced
//! to sixteen pixels.
//!
//! Two absolute failures short-circuit the metric:
//!
//! - **One backend painted nothing where the other painted.** `tiny-skia`
//!   silently drops a path fill thinner than `1/4096`, which is a *silent*
//!   correctness inversion; making it loud is the whole point.
//! - **The images differ in size.** That is an engine decision, and there is
//!   no per-pixel story to tell about it.
//!
//! One budget is deliberately *not* implemented: the
//! three-count allowance for the four non-separable blend modes, whose
//! `ClipColor` rescaling amplifies a one-count rounding difference. Applying
//! it needs to know which pixels a non-separable blend touched, which needs
//! the device-call trace this comparison does not yet have. Until it does,
//! such a pixel is scored at the ordinary interior tolerance and would be
//! reported as a hard failure — strict in the safe direction, and a known
//! source of false positives on the `composite-*` cluster.

use crate::ssim::Image;

/// The share of *edge* pixels that may differ before Tier C calls a file
/// divergent.
///
/// # The denominator this budget was written for is not the one measured
///
/// The contract's edge mask is derived from the device-call trace and
/// **dilates every primitive edge by one pixel**, so a glyph stem contributes
/// its own antialiased column *and* the opaque columns on either side. The
/// neighbourhood proxy here contributes only the antialiased column itself:
/// on a text-heavy page it calls 2.9% of the page an edge where the dilated
/// mask would call several times that.
///
/// The two rasterizers disagree on roughly a sixth of *those* pixels — a
/// stem straddling a pixel boundary that `tiny-skia` splits 127/127 and
/// `vello_cpu` splits 104/150, same total ink, different distribution — so
/// the rate against this tighter denominator lands well above 1% while the
/// *interior* count, which is the population the contract actually gates on,
/// stays at zero.
///
/// The budget is therefore left at one percent and **the measured
/// rate is reported rather than enforced**: moving the number to fit the
/// proxy would be fitting the target to the instrument. The instrument
/// improves when the engine grows its device-call trace, which is what makes
/// the dilated mask computable from the *inputs* rather than from the
/// outputs.
pub const EDGE_BUDGET: f64 = 0.01;

/// The per-channel difference an edge pixel is allowed without being counted.
///
/// Eight counts, not one: an antialiased edge is a whole coverage ramp, and
/// two integrations of the same ramp land a good way apart at the steepest
/// point without either being wrong.
pub const EDGE_TOLERANCE: u8 = 8;

/// The per-channel difference an *interior* pixel is allowed.
///
/// One count, from the two compositing pipelines' rounding: `tiny-skia`
/// works in fixed-point `u16` lanes and `vello_cpu` in `f32`. A larger
/// difference at an interior pixel is a defect, not a rounding.
pub const INTERIOR_TOLERANCE: u8 = 1;

/// What comparing one page under both backends found.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Divergence {
    /// Pixels compared.
    pub pixels: u64,
    /// Pixels the neighbourhood test called an edge.
    pub edge_pixels: u64,
    /// Edge pixels differing by more than [`EDGE_TOLERANCE`].
    pub edge_differing: u64,
    /// Interior pixels differing by more than [`INTERIOR_TOLERANCE`].
    pub interior_differing: u64,
    /// The largest per-channel difference anywhere.
    pub max_channel_diff: u8,
    /// Whether exactly one backend painted at all.
    pub one_painted_nothing: bool,
}

impl Divergence {
    /// The fraction of edge pixels that differ — metric.
    #[must_use]
    pub fn edge_rate(&self) -> f64 {
        if self.edge_pixels == 0 {
            return 0.0;
        }
        #[expect(
            clippy::cast_precision_loss,
            reason = "a page's pixel count is far inside f64's exact integer range"
        )]
        let rate = self.edge_differing as f64 / self.edge_pixels as f64;
        rate
    }

    /// Whether this page fails the contract outright.
    ///
    /// An interior difference is a hard failure at any count, because the
    /// engine is what decided those pixels; the edge rate is the soft
    /// measure.
    #[must_use]
    pub fn hard_fail(&self) -> bool {
        self.one_painted_nothing || self.interior_differing > 0
    }

    /// Whether the page is within the soft budget as well.
    #[must_use]
    pub fn passes(&self) -> bool {
        !self.hard_fail() && self.edge_rate() < EDGE_BUDGET
    }
}

/// Compare one page rendered by each backend.
///
/// Returns `None` when the two images differ in size, which is an engine
/// decision rather than a rasterization difference and has no per-pixel
/// story.
#[must_use]
pub fn compare(a: &Image, b: &Image) -> Option<Divergence> {
    if a.width != b.width || a.height != b.height {
        return None;
    }
    let edges = edge_mask(a, b);
    let mut out = Divergence {
        pixels: u64::from(a.width) * u64::from(a.height),
        edge_pixels: 0,
        edge_differing: 0,
        interior_differing: 0,
        max_channel_diff: 0,
        one_painted_nothing: false,
    };
    let (mut a_painted, mut b_painted) = (false, false);

    for y in 0..a.height {
        for x in 0..a.width {
            let (Some(pa), Some(pb)) = (pixel(a, x, y), pixel(b, x, y)) else {
                continue;
            };
            a_painted |= pa[3] != 0;
            b_painted |= pb[3] != 0;
            let diff = pa
                .iter()
                .zip(pb.iter())
                .map(|(l, r)| l.abs_diff(*r))
                .max()
                .unwrap_or(0);
            out.max_channel_diff = out.max_channel_diff.max(diff);
            let index = (y as usize) * (a.width as usize) + (x as usize);
            if edges.get(index).copied().unwrap_or(false) {
                out.edge_pixels += 1;
                if diff > EDGE_TOLERANCE {
                    out.edge_differing += 1;
                }
            } else if diff > INTERIOR_TOLERANCE {
                out.interior_differing += 1;
            }
        }
    }
    out.one_painted_nothing = a_painted != b_painted;
    Some(out)
}

/// How far the agreed-boundary mask is grown before the interior is scored.
///
/// One pixel, which is the contract's `dilate(..., 1px)`, and it is
/// deliberately not tuned past it. Radius two was measured over the full
/// store: it clears no further *file*, shrinking only the pixel counts inside
/// files that fail anyway, while moving 32 files under the soft edge budget
/// by enlarging their denominator. That is fitting the target to the
/// instrument, which the [`EDGE_BUDGET`] note above rejects for the same
/// reason.
const EDGE_DILATION: u32 = 1;

/// Which pixels sit on an edge in *either* image.
///
/// A pixel is an edge when any of its eight neighbours differs from it by
/// more than [`INTERIOR_TOLERANCE`] within the same image. Derived from each
/// image separately and unioned, so a soft boundary that only one backend
/// produced still counts as an edge — the conservative direction for the
/// *edge* population, and therefore the strict direction for the interior
/// one, which is the population the contract actually gates on.
///
/// # Dilating only from what both backends drew
///
/// The union is then grown by [`EDGE_DILATION`] — but the growth starts from
/// the **intersection** of the two per-image masks, not from the union, and
/// that distinction is the whole design.
///
/// A region where the two images *differ* has a boundary in one of them by
/// construction: the differing patch's own rim. Dilating the union would
/// therefore grow that rim inward over the difference and mark it an edge,
/// so the metric would absorb precisely the divergences it exists to find —
/// a 4x4 patch in the middle of a flat field would score clean. Growing only
/// from pixels where *both* backends independently found a boundary keeps
/// the dilation anchored to structure the two agree is there, so it can
/// excuse a coverage difference beside real geometry (a blitted pattern
/// cell's seam) and can never manufacture the licence for a difference out
/// of the difference itself.
fn edge_mask(a: &Image, b: &Image) -> Vec<bool> {
    let (w, h) = (a.width as usize, a.height as usize);
    let size = w.saturating_mul(h);
    let per_image = [boundaries(a), boundaries(b)];
    let mut union = vec![false; size];
    let mut both = vec![false; size];
    for index in 0..size {
        let in_a = per_image[0].get(index).copied().unwrap_or(false);
        let in_b = per_image[1].get(index).copied().unwrap_or(false);
        if let Some(slot) = union.get_mut(index) {
            *slot = in_a || in_b;
        }
        if let Some(slot) = both.get_mut(index) {
            *slot = in_a && in_b;
        }
    }

    let grown = dilate(&both, a.width, a.height, EDGE_DILATION);
    for index in 0..size {
        if grown.get(index).copied().unwrap_or(false)
            && let Some(slot) = union.get_mut(index)
        {
            *slot = true;
        }
    }
    union
}

/// Where one image has a boundary: a pixel with a neighbour unlike itself.
fn boundaries(image: &Image) -> Vec<bool> {
    let (w, h) = (image.width as usize, image.height as usize);
    let mut mask = vec![false; w.saturating_mul(h)];
    for y in 0..image.height {
        for x in 0..image.width {
            let Some(centre) = pixel(image, x, y) else {
                continue;
            };
            let index = (y as usize) * w + (x as usize);
            let mut is_edge = false;
            for dy in -1i64..=1 {
                for dx in -1i64..=1 {
                    if dx == 0 && dy == 0 {
                        continue;
                    }
                    let (Ok(nx), Ok(ny)) = (
                        u32::try_from(i64::from(x) + dx),
                        u32::try_from(i64::from(y) + dy),
                    ) else {
                        continue;
                    };
                    let Some(n) = pixel(image, nx, ny) else {
                        continue;
                    };
                    if centre
                        .iter()
                        .zip(n.iter())
                        .any(|(c, v)| c.abs_diff(*v) > INTERIOR_TOLERANCE)
                    {
                        is_edge = true;
                    }
                }
            }
            if is_edge && let Some(slot) = mask.get_mut(index) {
                *slot = true;
            }
        }
    }
    mask
}

/// Grow a mask by `radius` pixels in all eight directions.
///
/// Each round marks every pixel with a marked neighbour, so `radius` rounds
/// reach `radius` pixels out. Reading from the previous round rather than in
/// place is what keeps that true: growing a mask in place would let a pixel
/// marked earlier in the same scan seed the next one and smear the mask
/// across the row.
fn dilate(mask: &[bool], width: u32, height: u32, radius: u32) -> Vec<bool> {
    let w = width as usize;
    let mut current = mask.to_vec();
    for _ in 0..radius {
        let previous = current.clone();
        for y in 0..height {
            for x in 0..width {
                let index = (y as usize) * w + (x as usize);
                if previous.get(index).copied().unwrap_or(false) {
                    continue;
                }
                let mut touched = false;
                for dy in -1i64..=1 {
                    for dx in -1i64..=1 {
                        let (Ok(nx), Ok(ny)) = (
                            u32::try_from(i64::from(x) + dx),
                            u32::try_from(i64::from(y) + dy),
                        ) else {
                            continue;
                        };
                        if nx >= width || ny >= height {
                            continue;
                        }
                        let n = (ny as usize) * w + (nx as usize);
                        if previous.get(n).copied().unwrap_or(false) {
                            touched = true;
                        }
                    }
                }
                if touched && let Some(slot) = current.get_mut(index) {
                    *slot = true;
                }
            }
        }
    }
    current
}

fn pixel(image: &Image, x: u32, y: u32) -> Option<[u8; 4]> {
    if x >= image.width || y >= image.height {
        return None;
    }
    let i = ((y as usize) * (image.width as usize) + (x as usize)).checked_mul(4)?;
    let px = image.rgba.get(i..i + 4)?;
    Some([*px.first()?, *px.get(1)?, *px.get(2)?, *px.get(3)?])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(width: u32, height: u32, fill: [u8; 4]) -> Image {
        Image {
            width,
            height,
            rgba: fill
                .iter()
                .copied()
                .cycle()
                .take((width * height * 4) as usize)
                .collect(),
        }
    }

    fn set(image: &mut Image, x: u32, y: u32, px: [u8; 4]) {
        let i = ((y as usize) * (image.width as usize) + (x as usize)) * 4;
        if let Some(slot) = image.rgba.get_mut(i..i + 4) {
            slot.copy_from_slice(&px);
        }
    }

    #[test]
    fn identical_images_diverge_not_at_all() {
        let a = image(8, 8, [10, 20, 30, 255]);
        let d = compare(&a, &a).expect("same size");
        assert!(d.passes());
        assert_eq!(d.edge_differing, 0);
        assert_eq!(d.interior_differing, 0);
        assert_eq!(d.max_channel_diff, 0);
    }

    #[test]
    fn a_size_mismatch_has_no_per_pixel_story() {
        let a = image(8, 8, [0, 0, 0, 255]);
        let b = image(9, 8, [0, 0, 0, 255]);
        assert_eq!(compare(&a, &b), None);
    }

    #[test]
    fn one_backend_painting_nothing_is_a_hard_failure() {
        // B3's guard: tiny-skia silently drops a thin fill, so a page it
        // left blank while vello painted must be loud rather than a 1% drift.
        let painted = image(4, 4, [255, 0, 0, 255]);
        let blank = image(4, 4, [0, 0, 0, 0]);
        let d = compare(&painted, &blank).expect("same size");
        assert!(d.one_painted_nothing);
        assert!(d.hard_fail());
        assert!(!d.passes());
    }

    #[test]
    fn an_interior_difference_is_a_hard_failure_at_any_size() {
        // A large flat region has no neighbourhood variation, so a
        // difference in the middle of it cannot be excused as an edge.
        let a = image(16, 16, [100, 100, 100, 255]);
        let mut b = a.clone();
        for y in 6..10 {
            for x in 6..10 {
                set(&mut b, x, y, [140, 100, 100, 255]);
            }
        }
        let d = compare(&a, &b).expect("same size");
        assert!(d.interior_differing > 0, "the flat centre is interior");
        assert!(d.hard_fail());
    }

    #[test]
    fn a_one_count_interior_difference_is_within_the_rounding_budget() {
        // The two compositing pipelines round differently by exactly this
        // much; a contract that failed on it would fail on every file.
        let a = image(8, 8, [100, 100, 100, 255]);
        let mut b = a.clone();
        for y in 0..8 {
            for x in 0..8 {
                set(&mut b, x, y, [101, 100, 100, 255]);
            }
        }
        let d = compare(&a, &b).expect("same size");
        assert_eq!(d.interior_differing, 0);
        assert!(d.passes());
        assert_eq!(d.max_channel_diff, 1);
    }

    #[test]
    fn edge_pixels_absorb_a_coverage_difference() {
        // A hard boundary makes its own neighbours edge pixels, and a
        // coverage difference there is what the two rasterizers are
        // *expected* to produce.
        let mut a = image(16, 16, [255, 255, 255, 255]);
        for y in 0..16 {
            for x in 0..8 {
                set(&mut a, x, y, [0, 0, 0, 255]);
            }
        }
        let mut b = a.clone();
        // Soften the boundary column on one side only.
        for y in 0..16 {
            set(&mut b, 7, y, [40, 40, 40, 255]);
        }
        let d = compare(&a, &b).expect("same size");
        assert_eq!(
            d.interior_differing, 0,
            "the difference is all on the boundary"
        );
        assert!(d.edge_pixels > 0);
    }

    #[test]
    fn a_whole_page_colour_divergence_survives_dilation() {
        // `bug_554151.pdf` in miniature: one backend fills the page red and
        // the other fills it white. There is no boundary anywhere for the
        // dilation to grow out of, so every pixel stays interior and the
        // file stays a hard failure. This is the property that keeps the
        // contract non-vacuous — if a future radius ever absorbed this, the
        // interior guarantee would be gone.
        let a = image(16, 16, [255, 0, 0, 255]);
        let b = image(16, 16, [255, 255, 255, 255]);
        let d = compare(&a, &b).expect("same size");
        assert_eq!(d.edge_pixels, 0, "a flat page has no edges to dilate");
        assert_eq!(d.interior_differing, d.pixels);
        assert!(d.hard_fail());
    }

    #[test]
    fn a_blit_seam_one_pixel_off_a_boundary_is_an_edge() {
        // The tiling case (`bug_1288_2`, `xfermodes3`): a pattern cell is
        // rasterized with antialiasing and then blitted, so the cell's own
        // coverage difference lands just inside a boundary rather than on
        // it. The undilated neighbourhood test called that interior and
        // reported it as an engine bug; one pixel of dilation reaches it.
        let mut a = image(16, 16, [255, 255, 255, 255]);
        for y in 0..16 {
            for x in 0..8 {
                set(&mut a, x, y, [0, 0, 0, 255]);
            }
        }
        let mut b = a.clone();
        // Differ one column *inside* the dark side, not on the seam itself.
        for y in 0..16 {
            set(&mut b, 6, y, [40, 40, 40, 255]);
        }
        let d = compare(&a, &b).expect("same size");
        assert_eq!(
            d.interior_differing, 0,
            "a pixel one step off a real boundary is the rasterizer's business"
        );
        assert!(!d.hard_fail(), "and so not a hard failure");
    }

    #[test]
    fn dilation_does_not_reach_across_open_space() {
        // A boundary on the left and a solid differing block well away from
        // it. The block's own rim makes its outline an edge either way — a
        // one-pixel-wide difference is indistinguishable from a coverage
        // ramp and always was — but its *filled centre* is what the interior
        // population is, and no amount of structure elsewhere may excuse it.
        let mut a = image(20, 20, [255, 255, 255, 255]);
        for y in 0..20 {
            for x in 0..4 {
                set(&mut a, x, y, [0, 0, 0, 255]);
            }
        }
        let mut b = a.clone();
        for y in 10..16 {
            for x in 12..18 {
                set(&mut b, x, y, [200, 200, 200, 255]);
            }
        }
        let d = compare(&a, &b).expect("same size");
        assert!(
            d.interior_differing > 0,
            "the block's centre is open space, and stays the engine's"
        );
        assert!(d.hard_fail());
    }

    #[test]
    fn the_edge_rate_is_measured_against_edge_pixels_only() {
        let a = image(10, 10, [0, 0, 0, 255]);
        let d = compare(&a, &a).expect("same size");
        assert!(
            d.edge_rate().abs() < f64::EPSILON,
            "a flat image has no edges to rate"
        );
        assert_eq!(d.edge_pixels, 0);
    }
}
