//! Applying a page object's clip stack to a device
//! (`ProcessClipPath`, `cpdf_renderstatus.cpp:534-596`).
//!
//! Three contracts survive the translation from PDFium's 8-bit coverage
//! region to our two-method trait:
//!
//! - **An empty clip path clips everything out**, and it does so as the
//!   deliberately off-canvas rectangle `(-1, -1, 0, 0)` rather than as an
//!   empty region. The distinction matters because it interacts with the
//!   outer-rect rounding of the rect fast path.
//! - **An axis-aligned rectangle clips hard-edged**, through
//!   [`RenderDevice::push_clip_rect`], never as an antialiased path.
//! - **A text clip is the union of its glyph outlines**, accumulated across
//!   the whole text object and intersected once.

use kurbo::{Affine, BezPath, Rect, Shape};
use pdfrum_page::ClipStack;
use pdfrum_page::state::ClipEntry;

use crate::device::{FillRule, RenderDevice};
use crate::path::{outer_rect, path_rect};

/// The rectangle an empty clip path becomes: one pixel entirely off the top
/// left of any device, i.e. "clip everything out".
pub const EMPTY_CLIP_RECT: Rect = Rect::new(-1.0, -1.0, 0.0, 0.0);

/// One clip the engine will push, already reduced to what the device takes.
#[derive(Debug, Clone, PartialEq)]
pub enum Clip {
    /// An integer-snapped, hard-edged rectangle.
    Rect(Rect),
    /// An antialiased path.
    Path(BezPath, FillRule),
}

/// Whether a path encloses nothing at all, in either axis.
///
/// A clip that lets nothing through must become [`EMPTY_CLIP_RECT`] rather
/// than reaching a rasterizer: `tiny-skia` *drops* a fill or clip thinner
/// than `1/4096`, which turns an empty clip into a no-op clip — a
/// correctness inversion, and one this guard is what prevents.
fn is_degenerate(path: &BezPath) -> bool {
    let b = path.bounding_box();
    !b.width().is_finite() || !b.height().is_finite() || b.width() <= 0.0 || b.height() <= 0.0
}

/// Reduce a clip stack to the device calls it becomes.
///
/// Entries are outermost first and intersect; the device's own stack does the
/// intersecting, so this is a straight translation. Every returned clip must
/// be matched by one [`RenderDevice::pop`].
#[must_use]
pub fn resolve(clip: &ClipStack, to_device: Affine) -> Vec<Clip> {
    let mut out = Vec::with_capacity(clip.len());
    for entry in clip.entries() {
        match entry {
            ClipEntry::Path { path, even_odd } => {
                if path.elements().is_empty() || is_degenerate(path) {
                    // The empty-path case: an off-canvas rect, not an empty
                    // region. Reproduced literally, because that rectangle
                    // interacts with the outer-rect rounding downstream.
                    //
                    // `pdfrum-page` spells an empty clip as a zero-extent
                    // path rather than as no path at all, so both shapes
                    // land here.
                    out.push(Clip::Rect(EMPTY_CLIP_RECT));
                    continue;
                }
                let rule = if *even_odd {
                    FillRule::EvenOdd
                } else {
                    FillRule::Winding
                };
                match path_rect(path, to_device) {
                    // The rect fast path: snapped outward to whole pixels
                    // and applied with no antialiasing at all.
                    Some(r) => out.push(Clip::Rect(outer_rect(r).to_rect())),
                    None => out.push(Clip::Path(to_device * path.clone(), rule)),
                }
            }
            ClipEntry::Text { glyphs } => {
                // The glyph outlines union within one entry: a single path
                // carrying every glyph, filled non-zero.
                let mut union = BezPath::new();
                for g in glyphs {
                    union.extend(to_device * g.clone());
                }
                if union.elements().is_empty() {
                    // A text clip that produced no outlines still clips: an
                    // empty text-clipping path paints nothing through.
                    out.push(Clip::Rect(EMPTY_CLIP_RECT));
                } else {
                    out.push(Clip::Path(union, FillRule::Winding));
                }
            }
        }
    }
    out
}

/// Push a resolved clip stack onto a device, returning how many `pop`s it
/// owes.
pub fn push(device: &mut dyn RenderDevice, clips: &[Clip]) -> usize {
    for clip in clips {
        match clip {
            Clip::Rect(r) => device.push_clip_rect(*r),
            Clip::Path(p, rule) => device.push_clip(p, *rule),
        }
    }
    clips.len()
}

/// Pop `n` clip levels.
pub fn pop(device: &mut dyn RenderDevice, n: usize) {
    for _ in 0..n {
        device.pop();
    }
}

/// The device-space rectangle a clip stack confines drawing to, or the whole
/// device when it confines nothing.
///
/// Used for the per-object cull test and to size a transparency group's
/// offscreen buffer.
#[must_use]
pub fn device_bounds(clips: &[Clip]) -> Option<Rect> {
    let mut acc: Option<Rect> = None;
    for clip in clips {
        let r = match clip {
            Clip::Rect(r) => *r,
            Clip::Path(p, _) => p.bounding_box(),
        };
        acc = Some(match acc {
            Some(a) => a.intersect(r),
            None => r,
        });
    }
    acc
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect_path(x0: f64, y0: f64, x1: f64, y1: f64) -> BezPath {
        let mut p = BezPath::new();
        p.move_to((x0, y0));
        p.line_to((x1, y0));
        p.line_to((x1, y1));
        p.line_to((x0, y1));
        p.close_path();
        p
    }

    #[test]
    fn empty_path_is_the_offscreen_rect() {
        let mut stack = ClipStack::new();
        stack.push_empty();
        let clips = resolve(&stack, Affine::IDENTITY);
        assert_eq!(clips.as_slice(), &[Clip::Rect(EMPTY_CLIP_RECT)]);
        // It is a real 1x1 rectangle off the top left, not an empty one.
        #[expect(
            clippy::float_cmp,
            reason = "the rect is a literal const; an exact width is what is being pinned"
        )]
        {
            assert_eq!(EMPTY_CLIP_RECT.width(), 1.0);
        }
        // Constant by construction, so it is pinned at compile time.
        const { assert!(EMPTY_CLIP_RECT.x1 <= 0.0 && EMPTY_CLIP_RECT.y1 <= 0.0) };
    }

    #[test]
    fn an_axis_aligned_rect_clips_hard_edged_and_snapped() {
        let mut stack = ClipStack::new();
        stack.push_path(rect_path(1.2, 2.7, 5.4, 8.1), false);
        let clips = resolve(&stack, Affine::IDENTITY);
        // Snapped to the *outer* integer rect, and a Rect rather than a Path
        // so the device takes its hard-edged method.
        assert_eq!(
            clips.as_slice(),
            &[Clip::Rect(Rect::new(1.0, 2.0, 6.0, 9.0))]
        );
    }

    #[test]
    fn a_non_rect_clip_stays_a_path() {
        let mut curved = BezPath::new();
        curved.move_to((0.0, 0.0));
        curved.curve_to((5.0, 0.0), (5.0, 5.0), (0.0, 5.0));
        curved.close_path();
        let mut stack = ClipStack::new();
        stack.push_path(curved, true);
        let clips = resolve(&stack, Affine::IDENTITY);
        assert!(matches!(
            clips.first(),
            Some(Clip::Path(_, FillRule::EvenOdd))
        ));
    }

    #[test]
    fn text_clips_union_their_glyphs() {
        let mut stack = ClipStack::new();
        assert!(stack.push_text(vec![
            rect_path(0.0, 0.0, 2.0, 2.0),
            rect_path(8.0, 0.0, 10.0, 2.0)
        ]));
        let clips = resolve(&stack, Affine::IDENTITY);
        let Some(Clip::Path(p, rule)) = clips.first() else {
            panic!("expected a path clip")
        };
        assert_eq!(*rule, FillRule::Winding);
        let b = p.bounding_box();
        assert_eq!((b.x0, b.x1), (0.0, 10.0), "both glyphs are in one path");
    }

    #[test]
    fn an_empty_text_clip_still_clips_everything_out() {
        let mut stack = ClipStack::new();
        assert!(stack.push_text(vec![BezPath::new()]));
        let clips = resolve(&stack, Affine::IDENTITY);
        assert_eq!(clips.as_slice(), &[Clip::Rect(EMPTY_CLIP_RECT)]);
    }

    #[test]
    fn clips_intersect_for_the_cull_bounds() {
        let mut stack = ClipStack::new();
        stack.push_path(rect_path(0.0, 0.0, 10.0, 10.0), false);
        stack.push_path(rect_path(5.0, 5.0, 20.0, 20.0), false);
        let clips = resolve(&stack, Affine::IDENTITY);
        let b = device_bounds(&clips).expect("bounded");
        assert_eq!((b.x0, b.y0, b.x1, b.y1), (5.0, 5.0, 10.0, 10.0));
    }

    #[test]
    fn an_unclipped_stack_bounds_nothing() {
        assert_eq!(device_bounds(&[]), None);
    }

    #[test]
    fn the_transform_is_applied_before_the_rect_test() {
        let mut stack = ClipStack::new();
        stack.push_path(rect_path(0.0, 0.0, 4.0, 2.0), false);
        // A 90-degree turn keeps it axis-aligned, so it stays a rect clip.
        let quarter = Affine::new([0.0, 1.0, -1.0, 0.0, 0.0, 0.0]);
        assert!(matches!(
            resolve(&stack, quarter).first(),
            Some(Clip::Rect(_))
        ));
        // A 45-degree one does not.
        let eighth = Affine::rotate(std::f64::consts::FRAC_PI_4);
        assert!(matches!(
            resolve(&stack, eighth).first(),
            Some(Clip::Path(..))
        ));
    }
}
