//! What the conversion could not say in vectors, and why.
//!
//! A silent raster fallback is the failure mode this crate exists to avoid,
//! so every pixel region that reaches the document is recorded with a cause
//! and the caller reads the list back. The causes are an enum rather than a
//! string: a caller that wants to reject mesh shadings and
//! accept plain images matches on the variant, and adding a cause makes every
//! such match fail to compile.
//!
//! # What the causes are inferred from
//!
//! `RenderDevice` carries no cause channel — the engine hands a device a
//! pixmap and a transform, not a reason — and M24 explicitly keeps the trait
//! unchanged. The classification therefore reads the *shape of the engine's
//! own behaviour* around each draw, which is observable through
//! [`RasterBackend`](pdfrum_render::RasterBackend) without changing anything:
//! the engine renders each composited subtree into an offscreen target of its
//! own and blits the result, so the offscreen work that happened between one
//! root draw and the previous one is the evidence. Three of the causes have
//! an exact fingerprint and two are a residue; [`RasterCause`] says which is
//! which on each variant, and §4 states the limit in
//! full.

use kurbo::Rect;

/// Why one region of the page is pixels rather than vectors.
///
/// Ordered from the most specific evidence to the least.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum RasterCause {
    /// A type 4-7 mesh shading (ISO 32000 §8.7.4.5.5-8).
    ///
    /// **Exact.** `pdfrum_render`'s patch rasterizer is the only thing in the
    /// engine that fills with
    /// [`AntiAlias::FullCover`](pdfrum_render::AntiAlias) — the mode exists so
    /// abutting patch cells do not show seams — so a subtree whose fills are
    /// full-cover is a mesh and nothing else is.
    MeshShading,
    /// A non-isolated transparency group (ISO 32000 §11.4.6).
    ///
    /// **Exact.** It is the only thing that asks the backend for a target
    /// seeded with the page's existing pixels, and the engine then subtracts
    /// that backdrop back out arithmetically — the operation that has no
    /// vector meaning at all.
    NonIsolatedGroup,
    /// A knockout composite: a fill and a stroke of one path that must not
    /// see each other, or a knockout group's objects.
    ///
    /// **Exact for the fill+stroke case**, which is the only place the engine
    /// renders two sibling targets and combines them; a knockout *group*
    /// reaches the document through the same signature.
    Knockout,
    /// An isolated transparency group: `/Group` with `/I true`, a group
    /// `ca` below one, or a non-Normal blend on a form.
    ///
    /// The residue of "one offscreen subtree, ordinary fills inside" — which
    /// is also what a type-3 glyph procedure and a tiling-pattern cell look
    /// like from the backend. The region is right and the cause is a family.
    CompositedGroup,
    /// A soft mask, a stencil-masked image, or an image with an `/SMask`.
    ///
    /// **Exact.** Each renders a coverage plane into its own target and then
    /// multiplies it into the alpha channel of another pixmap — a per-pixel
    /// product SVG's `<mask>` could express only if the masked content were
    /// itself vector, which by then it is not.
    SoftMask,
    /// A pixel region with no offscreen work behind it: a sampled image, an
    /// axial or radial shading's evaluated buffer, or a hinted glyph bitmap.
    ///
    /// Not a *fallback* — an image is pixels in the PDF too, and embedding it
    /// as a PNG is the faithful mapping rather than an approximation. It is
    /// reported because the report's contract is *every* region delivered as
    /// pixels, and because an axial shading in this bucket genuinely could
    /// have been an SVG gradient.
    SampledSource,
}

impl RasterCause {
    /// A short, stable name for logs and for the `data-cause` attribute the
    /// document tags each rasterized region with.
    ///
    /// ```
    /// use pdfrum_svg::RasterCause;
    ///
    /// assert_eq!(RasterCause::MeshShading.name(), "mesh-shading");
    /// ```
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::MeshShading => "mesh-shading",
            Self::NonIsolatedGroup => "non-isolated-group",
            Self::Knockout => "knockout",
            Self::CompositedGroup => "composited-group",
            Self::SoftMask => "soft-mask",
            Self::SampledSource => "sampled-source",
        }
    }

    /// Whether the cause is one the engine composited in the pixel domain,
    /// as opposed to content that was samples to begin with.
    ///
    /// The distinction a caller usually wants: everything but
    /// [`Self::SampledSource`] is a region SVG *could* in principle have held
    /// as vectors and this pipeline does not.
    ///
    /// ```
    /// use pdfrum_svg::RasterCause;
    ///
    /// assert!(RasterCause::NonIsolatedGroup.is_composite());
    /// assert!(!RasterCause::SampledSource.is_composite());
    /// ```
    #[must_use]
    pub const fn is_composite(self) -> bool {
        !matches!(self, Self::SampledSource)
    }
}

/// One rasterized region: where it landed and why.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RasterRegion {
    /// The device-space rectangle the pixels cover, in the same coordinates
    /// as the document's `viewBox`.
    pub bounds: Rect,
    /// Why it is pixels.
    pub cause: RasterCause,
}

/// Every rasterized region of one converted page, in the order the walk
/// produced them.
///
/// Empty is the meaningful answer for a page of paths and text: the whole
/// page is vectors. A caller that only wants the summary reads
/// [`Self::counts`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RasterReport {
    regions: Vec<RasterRegion>,
}

impl RasterReport {
    /// The regions, in walk order.
    ///
    /// ```
    /// # use kurbo::Rect;
    /// # use pdfrum_svg::{RasterCause, RasterRegion, RasterReport};
    /// let mut report = RasterReport::default();
    /// assert!(report.regions().is_empty());
    ///
    /// let bounds = Rect::new(0.0, 0.0, 8.0, 8.0);
    /// report.push(RasterRegion { bounds, cause: RasterCause::SoftMask });
    /// assert_eq!(report.regions().first().map(|r| r.bounds), Some(bounds));
    /// ```
    #[must_use]
    pub fn regions(&self) -> &[RasterRegion] {
        &self.regions
    }

    /// Whether the page converted entirely to vectors.
    ///
    /// ```
    /// # use kurbo::Rect;
    /// # use pdfrum_svg::{RasterCause, RasterRegion, RasterReport};
    /// let mut report = RasterReport::default();
    /// assert!(report.is_empty(), "nothing recorded is all vectors");
    ///
    /// report.push(RasterRegion {
    ///     bounds: Rect::new(0.0, 0.0, 1.0, 1.0),
    ///     cause: RasterCause::MeshShading,
    /// });
    /// assert!(!report.is_empty());
    /// ```
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }

    /// How many regions each cause accounts for, in [`RasterCause`] order.
    ///
    /// ```
    /// use kurbo::Rect;
    /// use pdfrum_svg::{RasterCause, RasterRegion, RasterReport};
    ///
    /// let mut report = RasterReport::default();
    /// report.push(RasterRegion {
    ///     bounds: Rect::new(0.0, 0.0, 4.0, 4.0),
    ///     cause: RasterCause::MeshShading,
    /// });
    /// assert_eq!(report.counts(), vec![(RasterCause::MeshShading, 1)]);
    /// ```
    #[must_use]
    pub fn counts(&self) -> Vec<(RasterCause, usize)> {
        let mut out: Vec<(RasterCause, usize)> = Vec::new();
        for region in &self.regions {
            match out.iter_mut().find(|(cause, _)| *cause == region.cause) {
                Some((_, n)) => *n += 1,
                None => out.push((region.cause, 1)),
            }
        }
        out.sort_unstable();
        out
    }

    /// Record one region.
    ///
    /// Public because a caller composing several pages into one document
    /// merges their reports.
    ///
    /// ```
    /// # use kurbo::Rect;
    /// # use pdfrum_svg::{RasterCause, RasterRegion, RasterReport};
    /// let mut report = RasterReport::default();
    /// report.push(RasterRegion {
    ///     bounds: Rect::new(2.0, 2.0, 6.0, 6.0),
    ///     cause: RasterCause::Knockout,
    /// });
    /// assert_eq!(report.regions().len(), 1);
    /// ```
    pub fn push(&mut self, region: RasterRegion) {
        self.regions.push(region);
    }

    /// Absorb another page's regions.
    ///
    /// ```
    /// # use kurbo::Rect;
    /// # use pdfrum_svg::{RasterCause, RasterRegion, RasterReport};
    /// let region = RasterRegion {
    ///     bounds: Rect::new(0.0, 0.0, 4.0, 4.0),
    ///     cause: RasterCause::SampledSource,
    /// };
    /// let mut first = RasterReport::default();
    /// first.push(region);
    /// let mut second = RasterReport::default();
    /// second.push(region);
    ///
    /// first.extend(&second);
    /// assert_eq!(first.counts(), vec![(RasterCause::SampledSource, 2)]);
    /// ```
    pub fn extend(&mut self, other: &Self) {
        self.regions.extend_from_slice(&other.regions);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region(cause: RasterCause) -> RasterRegion {
        RasterRegion {
            bounds: Rect::new(0.0, 0.0, 1.0, 1.0),
            cause,
        }
    }

    #[test]
    fn an_empty_report_is_the_all_vector_answer() {
        assert!(RasterReport::default().is_empty());
        assert!(RasterReport::default().counts().is_empty());
    }

    #[test]
    fn counts_group_and_sort_by_cause() {
        let mut r = RasterReport::default();
        r.push(region(RasterCause::SampledSource));
        r.push(region(RasterCause::MeshShading));
        r.push(region(RasterCause::SampledSource));
        assert_eq!(
            r.counts(),
            vec![
                (RasterCause::MeshShading, 1),
                (RasterCause::SampledSource, 2)
            ]
        );
    }

    #[test]
    fn every_cause_has_a_distinct_name() {
        let all = [
            RasterCause::MeshShading,
            RasterCause::NonIsolatedGroup,
            RasterCause::Knockout,
            RasterCause::CompositedGroup,
            RasterCause::SoftMask,
            RasterCause::SampledSource,
        ];
        let mut names: Vec<_> = all.iter().map(|c| c.name()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), all.len());
    }

    #[test]
    fn only_a_sampled_source_is_not_a_composite() {
        assert!(!RasterCause::SampledSource.is_composite());
        for cause in [
            RasterCause::MeshShading,
            RasterCause::NonIsolatedGroup,
            RasterCause::Knockout,
            RasterCause::CompositedGroup,
            RasterCause::SoftMask,
        ] {
            assert!(cause.is_composite(), "{}", cause.name());
        }
    }
}
