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
use pdfrum_font::GlyphCache;
use pdfrum_page::ClipStack;
use pdfrum_page::{ClipEntry, ClipRule, TextClipRun};

use crate::options::RenderOptions;

use crate::device::{FillRule, RenderDevice};
use crate::path::{outer_rect, path_rect};

/// The rectangle an empty clip path becomes: one pixel entirely off the top
/// left of any device, i.e. "clip everything out".
///
/// **Private on purpose** (`docs/design/idiomatic-api.md` §C, Tier 1 item 5).
/// It was public as `EMPTY_CLIP_RECT`, an inverted off-device rectangle
/// standing for "nothing" — a magic value a caller had to recognise, and one
/// whose own test had to assert that a rect meaning *empty* measures one unit
/// wide. [`Clip::Empty`] says it in the type instead. The rectangle itself is
/// unchanged and still what reaches the device, because it interacts with the
/// outer-rect rounding of the rect fast path downstream.
const EMPTY_CLIP_RECT: Rect = Rect::new(-1.0, -1.0, 0.0, 0.0);

/// One clip the engine will push, already reduced to what the device takes.
#[derive(Debug, Clone, PartialEq)]
pub enum Clip {
    /// An integer-snapped, hard-edged rectangle.
    Rect(Rect),
    /// An antialiased path.
    Path(BezPath, FillRule),
    /// A clip that lets **nothing** through.
    ///
    /// Distinct from `Rect` of a zero-extent rectangle: this reaches the
    /// device as a deliberately off-canvas rectangle rather than as an empty
    /// region, because `tiny-skia` *drops* a fill or clip thinner than
    /// `1/4096` and would turn an empty clip into a no-op one.
    Empty,
}

/// Whether a path encloses nothing at all, in either axis.
///
/// A clip that lets nothing through must become [`Clip::Empty`] rather
/// than reaching a rasterizer: `tiny-skia` *drops* a fill or clip thinner
/// than `1/4096`, which turns an empty clip into a no-op clip — a
/// correctness inversion, and one this guard is what prevents.
fn is_degenerate(path: &BezPath) -> bool {
    let b = path.bounding_box();
    !b.width().is_finite() || !b.height().is_finite() || b.width() <= 0.0 || b.height() <= 0.0
}

/// One clipping run's glyph outlines, in device space.
///
/// Placed through the same [`crate::text::place_glyphs`] that draws the run,
/// which is the whole point of holding the run rather than its outlines: the
/// advances, the kerning, the word and character spacing and the substituted
/// font's width solve are one implementation, so the shape that clips is the
/// shape that would have been painted.
///
/// The placement is **fractional**. `ProcessText`'s snapping gate is
/// `if (is_clip || is_stroke)`, and `is_clip` is true for exactly this caller:
/// `ProcessClipPath` passes a `clipping_path` where the painting pass passes
/// `nullptr` (`cpdf_renderstatus.cpp:573-581` against `:312`). So a `Tr 4`
/// run's *painted* glyphs snap to the blit grid and the *same* run's clipping
/// glyphs do not, and asking for `subpixel_text_positioning` here is how that
/// is spelled.
fn text_clip_glyphs(
    run: &TextClipRun,
    glyphs: &mut GlyphCache,
    opts: &RenderOptions,
    to_device: Affine,
) -> Vec<BezPath> {
    let state = pdfrum_page::GraphicsState {
        text: pdfrum_page::TextState {
            char_space: run.char_space,
            word_space: run.word_space,
            ..pdfrum_page::TextState::default()
        },
        ..pdfrum_page::GraphicsState::default()
    };
    let opts = RenderOptions {
        subpixel_text_positioning: true,
        ..opts.clone()
    };
    // `clip: true` with no fill and no stroke: the run contributes its
    // outlines and paints nothing, which is what this pass is.
    let kinds = crate::text::TextPaintKinds {
        fill: false,
        stroke: false,
        clip: true,
    };
    crate::text::place_glyphs(&run.object, &state, glyphs, to_device, &opts, kinds)
        .iter()
        .map(crate::text::PlacedGlyph::device_path)
        .collect()
}

/// Reduce a clip stack to the device calls it becomes.
///
/// Entries are outermost first and intersect; the device's own stack does the
/// intersecting, so this is a straight translation. Every returned clip must
/// be matched by one [`RenderDevice::pop`].
///
/// The glyph cache is taken because a text clip holds *runs*: turning one into
/// the outlines it clips with is the same placement the painting pass runs,
/// and it wants the same cache.
#[must_use]
pub fn resolve(
    clip: &ClipStack,
    to_device: Affine,
    glyphs: &mut GlyphCache,
    opts: &RenderOptions,
) -> Vec<Clip> {
    let mut out = Vec::with_capacity(clip.len());
    if !clip.is_empty() {
        crate::walkprofile::alloc_items(
            crate::walkprofile::Site::ClipVec,
            clip.len(),
            core::mem::size_of::<Clip>(),
        );
    }
    for entry in clip.entries() {
        match entry {
            ClipEntry::Path {
                path,
                rule: clip_rule,
            } => {
                if path.elements().is_empty() || is_degenerate(path) {
                    // The empty-path case: an off-canvas rect, not an empty
                    // region. Reproduced literally, because that rectangle
                    // interacts with the outer-rect rounding downstream.
                    //
                    // `pdfrum-page` spells an empty clip as a zero-extent
                    // path rather than as no path at all, so both shapes
                    // land here.
                    out.push(Clip::Empty);
                    continue;
                }
                let rule = match clip_rule {
                    ClipRule::EvenOdd => FillRule::EvenOdd,
                    ClipRule::Winding => FillRule::Winding,
                };
                // The rect fast path: snapped outward to whole pixels and
                // applied with no antialiasing at all. Everything else keeps
                // its curves and is transformed into device space, which is
                // the clip's one allocation.
                if let Some(r) = path_rect(path, to_device) {
                    out.push(Clip::Rect(outer_rect(r).to_rect()));
                } else {
                    crate::walkprofile::alloc_items(
                        crate::walkprofile::Site::ClipPath,
                        path.elements().len(),
                        core::mem::size_of::<kurbo::PathEl>(),
                    );
                    out.push(Clip::Path(to_device * path.clone(), rule));
                }
            }
            ClipEntry::Text { runs } => {
                // Every glyph of every run in the batch unions into one path,
                // filled non-zero and applied once — `ProcessClipPath`
                // accumulates into a single `CFX_Path` across the batch and
                // calls `SetClip_PathFill` when the terminator arrives
                // (`cpdf_renderstatus.cpp:573-595`).
                let mut union = BezPath::new();
                for run in runs {
                    for glyph in text_clip_glyphs(run, glyphs, opts, to_device) {
                        union.extend(glyph);
                    }
                }
                if union.elements().is_empty() {
                    // A text clip that produced no outlines still clips: an
                    // empty text-clipping path paints nothing through.
                    out.push(Clip::Empty);
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
            Clip::Empty => device.push_clip_rect(EMPTY_CLIP_RECT),
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
#[allow(
    dead_code,
    reason = "exercised only by this module's own tests; the library builds once without `cfg(test)`"
)]
pub fn device_bounds(clips: &[Clip]) -> Option<Rect> {
    let mut acc: Option<Rect> = None;
    for clip in clips {
        let r = match clip {
            Clip::Rect(r) => *r,
            Clip::Path(p, _) => p.bounding_box(),
            Clip::Empty => EMPTY_CLIP_RECT,
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
    fn empty_path_is_the_empty_clip() {
        let mut stack = ClipStack::new();
        stack.push_empty();
        let clips = resolve_bare(&stack);
        assert_eq!(clips.as_slice(), &[Clip::Empty]);
        // What it means is now the variant, not the rectangle's coordinates.
        // The rectangle survives as the private shape the device is handed,
        // and it is still a real 1x1 off the top left rather than an empty
        // one -- which is the awkwardness that made the constant a poor
        // public surface, and is now nobody's business but this module's.
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
        stack.push_path(rect_path(1.2, 2.7, 5.4, 8.1), ClipRule::Winding);
        let clips = resolve_bare(&stack);
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
        stack.push_path(curved, ClipRule::EvenOdd);
        let clips = resolve_bare(&stack);
        assert!(matches!(
            clips.first(),
            Some(Clip::Path(_, FillRule::EvenOdd))
        ));
    }

    /// A clipping run showing `text` at `x`, in stock Helvetica at 20 pt.
    fn text_run(text: &[u8], x: f64) -> TextClipRun {
        let font = std::sync::Arc::new(pdfrum_font::Font::load_standard(
            pdfrum_font::StandardFont::Helvetica,
            &pdfrum_font::FontCache::default(),
        ));
        TextClipRun {
            object: pdfrum_page::TextObject {
                segments: Box::new([pdfrum_page::TextSegment {
                    codes: text.to_vec().into_boxed_slice(),
                    kerning: 0.0,
                }]),
                position: kurbo::Point::new(x, 0.0),
                matrix: Affine::IDENTITY,
                font: Some((font, 20.0)),
                font_source: None,
                render_mode: pdfrum_page::TextRenderMode::Clip,
                type3_metrics: std::collections::BTreeMap::new(),
            },
            char_space: 0.0,
            word_space: 0.0,
        }
    }

    fn resolve_bare(stack: &ClipStack) -> Vec<Clip> {
        let mut glyphs = pdfrum_font::GlyphCache::default();
        resolve(
            stack,
            Affine::IDENTITY,
            &mut glyphs,
            &RenderOptions::default(),
        )
    }

    #[test]
    fn text_clips_union_every_run_in_the_batch_into_one_path() {
        let mut stack = ClipStack::new();
        assert!(stack.push_text(vec![text_run(b"H", 0.0), text_run(b"H", 100.0)]));
        let clips = resolve_bare(&stack);
        let Some(Clip::Path(p, rule)) = clips.first() else {
            panic!("expected a path clip, got {clips:?}")
        };
        assert_eq!(*rule, FillRule::Winding);
        let b = p.bounding_box();
        // One path spanning both runs: the far one is 100 units away, so the
        // union is far wider than either glyph.
        assert!(b.x0 < 5.0 && b.x1 > 100.0, "one path over both runs: {b:?}");
        assert_eq!(clips.len(), 1, "the batch is one clip, not one per run");
    }

    /// A run that places no glyph at all still clips, and clips *everything*
    /// out: an empty text-clipping path is not an absent one.
    #[test]
    fn an_empty_text_clip_still_clips_everything_out() {
        let mut stack = ClipStack::new();
        assert!(stack.push_text(vec![text_run(b"", 0.0)]));
        let clips = resolve_bare(&stack);
        assert_eq!(clips.as_slice(), &[Clip::Empty]);
    }

    #[test]
    fn clips_intersect_for_the_cull_bounds() {
        let mut stack = ClipStack::new();
        stack.push_path(rect_path(0.0, 0.0, 10.0, 10.0), ClipRule::Winding);
        stack.push_path(rect_path(5.0, 5.0, 20.0, 20.0), ClipRule::Winding);
        let clips = resolve_bare(&stack);
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
        stack.push_path(rect_path(0.0, 0.0, 4.0, 2.0), ClipRule::Winding);
        // A 90-degree turn keeps it axis-aligned, so it stays a rect clip.
        let quarter = Affine::new([0.0, 1.0, -1.0, 0.0, 0.0, 0.0]);
        assert!(matches!(
            resolve(
                &stack,
                quarter,
                &mut pdfrum_font::GlyphCache::default(),
                &RenderOptions::default()
            )
            .first(),
            Some(Clip::Rect(_))
        ));
        // A 45-degree one does not.
        let eighth = Affine::rotate(std::f64::consts::FRAC_PI_4);
        assert!(matches!(
            resolve(
                &stack,
                eighth,
                &mut pdfrum_font::GlyphCache::default(),
                &RenderOptions::default()
            )
            .first(),
            Some(Clip::Path(..))
        ));
    }
}
