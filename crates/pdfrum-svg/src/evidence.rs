//! How a pixel region's cause is worked out without a cause channel.
//!
//! `RenderDevice` hands a device a pixmap and a transform, and the trait
//! stays unchanged, so the cause has to come from somewhere else. It comes
//! from the engine's *own* behaviour, which
//! [`RasterBackend`](pdfrum_render::RasterBackend) makes visible for free:
//! the engine renders every composited subtree into an offscreen target and
//! blits the result, so the offscreen targets opened and finished since the
//! last root draw are the evidence for what that draw is.
//!
//! Each offscreen target records what was drawn *into* it, as a
//! [`Fingerprint`]; the targets that closed before a root draw are its
//! [`Witness`]es, and [`classify`] reads them.

use crate::RasterCause;

/// How an offscreen target's pixels started.
///
/// An enum rather than a `from_backdrop: bool`, because the two are genuinely
/// different origins with different meanings and only one of them decides a
/// cause.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Seed {
    /// Cleared — an isolated group, a pattern cell, a glyph procedure, a
    /// mask buffer.
    #[default]
    Clear,
    /// Seeded from the page's existing pixels: the backdrop only a
    /// non-isolated transparency group asks for, and the one the engine
    /// afterwards subtracts back out.
    Backdrop,
}

/// Which primitives reached a target.
///
/// A set of three flags rather than three fields, so the questions asked of
/// it below — "only a fill", "an image and nothing else" — are one comparison
/// each instead of a conjunction of negations.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Drew {
    /// A fill reached the target.
    pub fills: bool,
    /// A stroke reached the target.
    pub strokes: bool,
    /// An image reached the target.
    pub images: bool,
}

/// What one offscreen target saw, reduced to the facts that separate causes.
///
/// A small record rather than the call log itself: the classification is a
/// closed question, and keeping the log would invite it to grow into a
/// heuristic.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Fingerprint {
    /// How its pixels started.
    pub seed: Seed,
    /// Something was filled with
    /// [`AntiAlias::FullCover`](pdfrum_render::AntiAlias), which in this
    /// engine only the mesh-patch rasterizer does.
    pub full_cover_fill: bool,
    /// Which primitives reached it.
    pub drew: Drew,
    /// How many drawing calls of any kind it took.
    pub draws: usize,
}

impl Fingerprint {
    /// A target holding one image and nothing else.
    ///
    /// The shape of a coverage pass: a stencil, an `/SMask` plane or a
    /// luminosity buffer's single blit, each rendered alone so it can be
    /// multiplied into another pixmap's alpha channel.
    fn is_lone_image(self) -> bool {
        self.draws == 1
            && self.drew
                == Drew {
                    images: true,
                    ..Drew::default()
                }
    }

    /// A target holding fills and no strokes — the first half of the
    /// fill+stroke knockout buffer.
    fn is_fill_only(self) -> bool {
        self.drew.fills && !self.drew.strokes
    }

    /// A target holding strokes and no fills — the second half.
    fn is_stroke_only(self) -> bool {
        self.drew.strokes && !self.drew.fills
    }
}

/// One closed offscreen target, waiting to explain a root draw.
pub type Witness = Fingerprint;

/// Why the root draw that follows `witnesses` is pixels.
///
/// `witnesses` are the offscreen targets that closed since the previous root
/// draw, in the order they closed.
#[must_use]
pub fn classify(witnesses: &[Witness]) -> RasterCause {
    // No offscreen work at all: a sampled image, an axial or radial shading's
    // evaluated buffer, or a glyph bitmap. Nothing was composited.
    let Some(last) = witnesses.last() else {
        return RasterCause::SampledSource;
    };

    // A backdrop-seeded target is unambiguous: only `needs_backdrop` asks for
    // one, and only a non-isolated group makes `needs_backdrop` true.
    if witnesses.iter().any(|w| matches!(w.seed, Seed::Backdrop)) {
        return RasterCause::NonIsolatedGroup;
    }
    // Full-cover fills exist for exactly one reason in this engine.
    if witnesses.iter().any(|w| w.full_cover_fill) {
        return RasterCause::MeshShading;
    }
    // The fill+stroke knockout buffer: two sibling targets, the first holding
    // only a fill and the second only a stroke, combined by `knockout_replace`
    // before the blit. Nothing else in the engine has that shape.
    if witnesses.len() == 2
        && witnesses.first().is_some_and(|f| f.is_fill_only())
        && last.is_stroke_only()
    {
        return RasterCause::Knockout;
    }
    // A lone coverage plane, blitted alone into its own target so it can be
    // multiplied into an alpha channel: a soft mask, a stencil mask, or an
    // image's `/SMask`. The *last* witness is the one that decides, because
    // the mask pass always closes after the content it masks.
    if last.is_lone_image() {
        return RasterCause::SoftMask;
    }
    // Everything else that went through an offscreen target: an isolated
    // group, a knockout group, a type-3 glyph procedure, a tiling-pattern
    // cell. The region is exact and the cause is this family.
    RasterCause::CompositedGroup
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drew(n: usize) -> Fingerprint {
        Fingerprint {
            drew: Drew {
                fills: true,
                ..Drew::default()
            },
            draws: n,
            ..Fingerprint::default()
        }
    }

    #[test]
    fn no_offscreen_work_is_a_sampled_source() {
        assert_eq!(classify(&[]), RasterCause::SampledSource);
    }

    #[test]
    fn a_backdrop_seeded_target_is_a_non_isolated_group() {
        let w = Fingerprint {
            seed: Seed::Backdrop,
            ..drew(3)
        };
        assert_eq!(classify(&[w]), RasterCause::NonIsolatedGroup);
    }

    #[test]
    fn full_cover_fills_are_a_mesh_shading() {
        let w = Fingerprint {
            full_cover_fill: true,
            ..drew(900)
        };
        assert_eq!(classify(&[w]), RasterCause::MeshShading);
    }

    #[test]
    fn a_fill_target_then_a_stroke_target_is_a_knockout() {
        let fill = drew(1);
        let stroke = Fingerprint {
            drew: Drew {
                strokes: true,
                ..Drew::default()
            },
            draws: 1,
            ..Fingerprint::default()
        };
        assert_eq!(classify(&[fill, stroke]), RasterCause::Knockout);
    }

    #[test]
    fn a_lone_image_target_is_a_mask_pass() {
        let content = drew(5);
        let mask = Fingerprint {
            drew: Drew {
                images: true,
                ..Drew::default()
            },
            draws: 1,
            ..Fingerprint::default()
        };
        assert_eq!(classify(&[content, mask]), RasterCause::SoftMask);
    }

    #[test]
    fn an_ordinary_subtree_is_a_composited_group() {
        assert_eq!(classify(&[drew(7)]), RasterCause::CompositedGroup);
    }

    #[test]
    fn the_backdrop_wins_over_every_other_signal() {
        // A non-isolated group containing a mesh: the group is the outer
        // fact, and it is the one whose arithmetic has no vector meaning.
        let mesh = Fingerprint {
            full_cover_fill: true,
            ..drew(2)
        };
        let group = Fingerprint {
            seed: Seed::Backdrop,
            ..drew(1)
        };
        assert_eq!(classify(&[mesh, group]), RasterCause::NonIsolatedGroup);
    }
}
