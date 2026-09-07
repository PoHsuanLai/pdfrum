//! The clipping path (ISO 32000-1 §8.5.4).
//!
//! Two things live here that a naive stack would not have:
//!
//! - **An auto-merge.** When the previously pushed clip is a rectangle that
//!   *contains* the new path's bounding box, the old entry is popped before
//!   the new one is pushed. It changes no pixels, but it changes clip counts
//!   in a dump, so it is not an optimization to skip.
//! - **A text-clip batch is all-or-nothing at 1024 objects.** Adding a batch
//!   that would take the total past the cap **drops the whole batch**
//!   silently, rather than truncating it.

use kurbo::{BezPath, Rect, Shape};
use std::sync::Arc;

/// The most text objects a clipping path may accumulate.
pub const MAX_TEXT_OBJECTS: usize = 1024;

/// A text-clip batch that would take the clip past [`MAX_TEXT_OBJECTS`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("text clip batch of {adding} would exceed the {limit}-object cap ({have} already held)")]
pub struct TextClipLimit {
    /// How many text objects the stack already holds.
    pub have: usize,
    /// How many the refused batch wanted to add.
    pub adding: usize,
    /// The cap, [`MAX_TEXT_OBJECTS`].
    pub limit: usize,
}

/// Which rule decides a clipping path's interior (ISO 32000-1 §8.5.4).
///
/// Distinct from [`FillRule`](crate::FillRule), which has a third state for
/// "does not fill at all": a clip always has an interior, so `W` and `W*` are
/// the only two answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub enum ClipRule {
    /// Nonzero winding number rule (`W`).
    #[default]
    Winding,
    /// Even-odd rule (`W*`).
    EvenOdd,
}

/// One clipping contribution.
#[derive(Debug, Clone, PartialEq)]
pub enum ClipEntry {
    /// A path, with the rule that decides its interior.
    Path {
        /// The path in device-ish space; the drawn path keeps its own matrix.
        path: BezPath,
        /// The rule deciding the path's interior.
        rule: ClipRule,
    },
    /// One batch of text objects shown in a clipping render mode, between a
    /// `BT` and the `ET` that closed it.
    ///
    /// The *objects*, not their outlines, because turning a run into glyph
    /// outlines needs the placement arithmetic — advances, kerning, word and
    /// character spacing, the substituted-font width solve — that lives in the
    /// renderer beside the code that draws the same run normally. The oracle
    /// holds text objects here for the same reason and lays each one out at
    /// clip time; deriving them twice, once for painting and once for
    /// clipping, is how the two would drift apart.
    Text {
        /// The runs, in the order they were shown.
        runs: Vec<TextClipRun>,
    },
}

/// One text run held for clipping, with the state its placement needs.
///
/// Character and word spacing are graphics state rather than properties of the
/// object, and a `Tc` or `Tw` between two runs applies to the second only, so
/// each run carries the values that were live when it was shown.
#[derive(Debug, Clone, PartialEq)]
pub struct TextClipRun {
    /// The run itself.
    pub object: crate::TextObject,
    /// `Tc` when the run was shown.
    pub char_space: f32,
    /// `Tw` when the run was shown.
    pub word_space: f32,
}

/// The clipping state: an ordered list of contributions to intersect.
///
/// The entries sit behind an `Arc`, so the clone every emitted object takes
/// of its graphics state shares them: thousands of consecutive paths under
/// one clip cost one reference count each, and a push copies the vector
/// only when it is shared. With the vector owned, the state clone was 9.6%
/// of `vector_paths_1751`'s build.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ClipStack {
    entries: Arc<Vec<ClipEntry>>,
    /// How many text objects the whole stack holds, for the 1024 cap.
    text_objects: usize,
}

impl ClipStack {
    /// An unclipped state.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The contributions, outermost first.
    #[must_use]
    pub fn entries(&self) -> &[ClipEntry] {
        &self.entries
    }

    /// How many contributions there are.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether nothing clips.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Add a path, merging it with a containing rectangle above it.
    ///
    /// The merge is what keeps a `re W n` inside a larger `re W n` from
    /// producing two entries.
    pub fn push_path(&mut self, path: BezPath, rule: ClipRule) {
        let incoming = path.bounding_box();
        if let Some(ClipEntry::Path {
            path: previous,
            rule: _,
        }) = self.entries.last()
            && let Some(rect) = as_rectangle(previous)
            && contains_rect(rect, incoming)
        {
            Arc::make_mut(&mut self.entries).pop();
        }
        Arc::make_mut(&mut self.entries).push(ClipEntry::Path { path, rule });
    }

    /// Add a batch of clipping text runs.
    ///
    /// A batch taking the total past [`MAX_TEXT_OBJECTS`] is **dropped
    /// whole**, not truncated — no prefix of it clips.
    ///
    /// # Errors
    ///
    /// [`TextClipLimit`] when the batch would take the stack past the cap.
    /// The stack is left unchanged; a refused batch does not re-offer itself
    /// at the next `ET`.
    pub fn push_text(&mut self, runs: Vec<TextClipRun>) -> Result<(), TextClipLimit> {
        let adding = runs.len();
        if self.text_objects + adding > MAX_TEXT_OBJECTS {
            return Err(TextClipLimit {
                have: self.text_objects,
                adding,
                limit: MAX_TEXT_OBJECTS,
            });
        }
        self.text_objects += adding;
        Arc::make_mut(&mut self.entries).push(ClipEntry::Text { runs });
        Ok(())
    }

    /// Add an empty clip, which blanks everything after it.
    ///
    /// This is what a single-point path with a pending clip produces: a
    /// degenerate rectangle at the origin, whose interior is nothing.
    pub fn push_empty(&mut self) {
        Arc::make_mut(&mut self.entries).push(ClipEntry::Path {
            path: Rect::ZERO.to_path(0.1),
            rule: ClipRule::Winding,
        });
    }

    /// The intersection of every contribution's bounding box, or `None` when
    /// nothing clips.
    #[must_use]
    pub fn bounds(&self) -> Option<Rect> {
        let mut result: Option<Rect> = None;
        for entry in self.entries.iter() {
            let rect = match entry {
                ClipEntry::Path { path, .. } => path.bounding_box(),
                // A text layer's contribution is the *union* of its runs; the
                // layers then intersect.
                //
                // A run contributes only its **origin**, because that is the
                // only geometry an unplaced run has: the glyph outlines come
                // into existence when a renderer places them, and this entry
                // deliberately holds the run instead. So the box under-reports
                // a text clip, and the renderer must not use it to cull —
                // which it does not: `clip::resolve` reads `entries()` and
                // nothing in `pdfrum-render` calls this at all. It answers the
                // page-graph question "where does this clip start", and a
                // caller wanting the ink must place the runs.
                ClipEntry::Text { runs } => {
                    let mut union: Option<Rect> = None;
                    for run in runs {
                        let p = run.object.position;
                        let b = Rect::new(p.x, p.y, p.x, p.y);
                        union = Some(union.map_or(b, |u| u.union(b)));
                    }
                    union?
                }
            };
            result = Some(result.map_or(rect, |r| r.intersect(rect)));
        }
        result
    }
}

/// Whether a path is exactly a rectangle, and which one.
///
/// PDFium tests the path's own shape rather than its bounding box, and
/// builds the rectangle from points 0 and 2.
fn as_rectangle(path: &BezPath) -> Option<Rect> {
    let points: Vec<_> = path
        .elements()
        .iter()
        .filter_map(|el| match el {
            kurbo::PathEl::MoveTo(p) | kurbo::PathEl::LineTo(p) => Some(*p),
            _ => None,
        })
        .collect();
    // A rectangle is five points with the last closing back onto the first,
    // or four with an explicit close.
    if !(4..=5).contains(&points.len()) {
        return None;
    }
    let (p0, p2) = (points.first()?, points.get(2)?);
    let rect = Rect::from_points(*p0, *p2);
    // Every point must sit on the rectangle's boundary for it to be one.
    let on_edge = |p: &kurbo::Point| {
        let x_edge = (p.x - rect.x0).abs() < 1e-9 || (p.x - rect.x1).abs() < 1e-9;
        let y_edge = (p.y - rect.y0).abs() < 1e-9 || (p.y - rect.y1).abs() < 1e-9;
        x_edge && y_edge
    };
    points.iter().all(on_edge).then_some(rect)
}

/// Whether `outer` contains `inner`, inclusively.
fn contains_rect(outer: Rect, inner: Rect) -> bool {
    outer.x0 <= inner.x0 && outer.y0 <= inner.y0 && outer.x1 >= inner.x1 && outer.y1 >= inner.y1
}

#[cfg(test)]
mod tests {
    // Test fixtures quote the oracle's own vectors, compare floats exactly
    // where the behaviour being pinned is exact, and index arrays whose
    // length the fixture itself fixes.
    #![allow(
        clippy::unreadable_literal,
        clippy::float_cmp,
        clippy::indexing_slicing,
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        reason = "test fixtures quote oracle vectors verbatim and compare exactly"
    )]

    use super::{ClipRule, ClipStack, MAX_TEXT_OBJECTS, TextClipLimit, TextClipRun};
    use kurbo::{BezPath, Rect, Shape};

    fn rect_path(x0: f64, y0: f64, x1: f64, y1: f64) -> BezPath {
        Rect::new(x0, y0, x1, y1).to_path(0.1)
    }

    /// A clipping run that shows nothing, at `(x, y)`.
    ///
    /// The cap and the bounds are the only things these tests ask of a run,
    /// and both read its position rather than its glyphs.
    fn run_at(x: f64, y: f64) -> TextClipRun {
        TextClipRun {
            object: crate::TextObject {
                segments: Box::new([]),
                position: kurbo::Point::new(x, y),
                matrix: kurbo::Affine::IDENTITY,
                font: None,
                font_source: None,
                render_mode: crate::ops::TextRenderMode::Clip,
                type3_metrics: std::collections::BTreeMap::new(),
            },
            char_space: 0.0,
            word_space: 0.0,
        }
    }

    #[test]
    fn a_contained_rectangle_replaces_its_container() {
        let mut stack = ClipStack::new();
        stack.push_path(rect_path(0.0, 0.0, 100.0, 100.0), ClipRule::Winding);
        assert_eq!(stack.len(), 1);
        // A smaller rectangle inside the first merges rather than stacking.
        stack.push_path(rect_path(10.0, 10.0, 50.0, 50.0), ClipRule::Winding);
        assert_eq!(stack.len(), 1);
        let bounds = stack.bounds().expect("bounds");
        assert!((bounds.width() - 40.0).abs() < 1.0);
    }

    #[test]
    fn an_overlapping_rectangle_does_not_merge() {
        let mut stack = ClipStack::new();
        stack.push_path(rect_path(0.0, 0.0, 100.0, 100.0), ClipRule::Winding);
        // Sticking out to the right: not contained, so both stay.
        stack.push_path(rect_path(50.0, 50.0, 150.0, 150.0), ClipRule::Winding);
        assert_eq!(stack.len(), 2);
    }

    #[test]
    fn a_text_batch_past_the_cap_is_dropped_whole() {
        let mut stack = ClipStack::new();
        let run = || run_at(0.0, 0.0);
        // Exactly the cap fits.
        let batch: Vec<_> = std::iter::repeat_with(run).take(MAX_TEXT_OBJECTS).collect();
        assert!(stack.push_text(batch).is_ok());
        assert_eq!(stack.len(), 1);
        // One more is refused, and nothing is truncated in.
        assert_eq!(
            stack.push_text(vec![run()]),
            Err(TextClipLimit {
                have: MAX_TEXT_OBJECTS,
                adding: 1,
                limit: MAX_TEXT_OBJECTS,
            })
        );
        assert_eq!(stack.len(), 1);
    }

    #[test]
    fn a_batch_that_would_overflow_is_refused_before_any_of_it_lands() {
        let mut stack = ClipStack::new();
        let run = || run_at(0.0, 0.0);
        let batch: Vec<_> = std::iter::repeat_with(run)
            .take(MAX_TEXT_OBJECTS - 1)
            .collect();
        assert!(stack.push_text(batch).is_ok());
        // Two more would make 1025: the whole batch is dropped.
        assert_eq!(
            stack.push_text(vec![run(), run()]),
            Err(TextClipLimit {
                have: MAX_TEXT_OBJECTS - 1,
                adding: 2,
                limit: MAX_TEXT_OBJECTS,
            })
        );
        assert_eq!(stack.len(), 1);
    }

    #[test]
    fn an_empty_clip_blanks_everything() {
        let mut stack = ClipStack::new();
        stack.push_path(rect_path(0.0, 0.0, 100.0, 100.0), ClipRule::Winding);
        stack.push_empty();
        let bounds = stack.bounds().expect("bounds");
        assert!(bounds.area() < 1e-6, "got {bounds:?}");
    }

    #[test]
    fn an_unclipped_stack_has_no_bounds() {
        assert!(ClipStack::new().bounds().is_none());
        assert!(ClipStack::new().is_empty());
    }

    #[test]
    fn text_layers_union_within_and_intersect_between() {
        let mut stack = ClipStack::new();
        // One layer covering two far-apart runs unions to a wide box.
        assert!(
            stack
                .push_text(vec![run_at(0.0, 0.0), run_at(100.0, 0.0)])
                .is_ok()
        );
        let bounds = stack.bounds().expect("bounds");
        assert!((bounds.width() - 100.0).abs() < 1.0);
        // A second layer intersects with the first.
        assert!(
            stack
                .push_text(vec![run_at(0.0, 0.0), run_at(20.0, 0.0)])
                .is_ok()
        );
        let bounds = stack.bounds().expect("bounds");
        assert!(bounds.width() <= 21.0);
    }
}
