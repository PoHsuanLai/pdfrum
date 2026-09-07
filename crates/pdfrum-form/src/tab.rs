//! Focus traversal order: which annotation Tab moves to next.
//!
//! This is **not** the order annotations are drawn or hit-tested in. Those
//! sort by a layout band and put the focused annotation first or last; this
//! one reads the page's own `/Tabs` entry and produces a ring that focus walks
//! and never wraps.
//!
//! # Three orders, and only two of them sort anything
//!
//! - **Structure** — plain annotation order, with no sorting at all. This is
//!   the default, and it is what *anything other than* `R` or `C` selects.
//!   The `S` spelling the specification defines is not special-cased: it
//!   falls through to the same branch that an absent entry does.
//! - **Row** — reading order across the page: repeatedly take the topmost
//!   remaining annotation, then take everything whose vertical centre falls
//!   strictly inside its band, left to right.
//! - **Column** — the same idea rotated: leftmost first, banding on
//!   horizontal centres.
//!
//! # Why this terminates and the original does not
//!
//! The C++'s row and column loops both contain `if (index < 0) continue;`
//! inside a `while (!list.empty())` that erases nothing on that path — so a
//! page whose every remaining annotation has a non-positive top spins
//! forever. It is not reachable from any file in the corpus, and no test
//! asserts it, which is why it has survived.
//!
//! This banding is a fold that consumes its input: when no candidate is
//! found, the remainder is appended in index order and the caller is told.
//! **A library that can hang on input is a bug regardless of what the oracle
//! does**, and this is the one place in the crate where the behaviour is
//! deliberately better rather than identical.

// The banding arithmetic mirrors the oracle's bit for bit: sums halved rather
// than `midpoint`, and comparisons left strict. Both lints below would suggest
// changes that move annotations between bands.
#![allow(clippy::manual_midpoint, clippy::float_cmp)]

use crate::session::AnnotId;

/// A rectangle in page space, as a focusable annotation's `/Rect`, in this
/// crate's own `f32`.
///
/// The raw rectangle, deliberately: the ring is built from `/Rect` and not
/// from the inflated box a focused widget draws into.
///
/// **Private.** The public vocabulary is [`kurbo::Rect`]; `page::to_rect`
/// narrows into this one on the way in and `PopupGeometry` widens back out
/// on the way out. It stays `f32` because [`Rect::center_y`]'s banding
/// midpoint is compared *strictly* against other `f32` edges: widening it
/// would move annotations between bands, and it is fed only by widget
/// `/Rect`s, never by an event point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Rect {
    /// Left edge.
    pub(crate) left: f32,
    /// Bottom edge.
    pub(crate) bottom: f32,
    /// Right edge.
    pub(crate) right: f32,
    /// Top edge.
    pub(crate) top: f32,
}

impl Rect {
    /// A rectangle from its four edges.
    pub(crate) fn new(left: f32, bottom: f32, right: f32, top: f32) -> Rect {
        Rect {
            left,
            bottom,
            right,
            top,
        }
    }

    /// The vertical midpoint, which row banding tests.
    ///
    /// Written as the oracle writes it — sum then halve — rather than as
    /// `f32::midpoint`, which rounds differently at the extremes. The banding
    /// comparisons are strict, so a midpoint that differs in the last bit
    /// moves an annotation between bands.
    pub(crate) fn center_y(self) -> f32 {
        (self.top + self.bottom) / 2.0
    }

    /// The horizontal midpoint, which column banding tests.
    ///
    /// Sum then halve, for the reason [`Rect::center_y`] gives.
    pub(crate) fn center_x(self) -> f32 {
        (self.left + self.right) / 2.0
    }
}

/// Which traversal order a page asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TabOrder {
    /// Annotation order, unsorted. The default, and what any unrecognized
    /// `/Tabs` value means.
    #[default]
    Structure,
    /// Reading order across rows.
    Row,
    /// Reading order down columns.
    Column,
}

impl TabOrder {
    /// Reads the order from a page's `/Tabs` value.
    ///
    /// Only `R` and `C` are recognized. Everything else — including the `S`
    /// the specification defines for structure order, and an absent entry —
    /// is structure order, because that is the branch the oracle falls
    /// through to.
    #[must_use]
    pub fn from_tabs(tabs: Option<&[u8]>) -> TabOrder {
        match tabs {
            Some(b"R") => TabOrder::Row,
            Some(b"C") => TabOrder::Column,
            _ => TabOrder::Structure,
        }
    }
}

/// One annotation eligible for focus.
///
/// The `id` carries a **raw** `/Annots` index, pop-ups counted — see
/// [`AnnotId`]. Unlike hit testing, pop-ups never appear here at all: they
/// are not a focusable subtype. That makes it tempting to number the ring
/// from its own positions, and it would be wrong for the same reason —
/// whatever the ring hands back is used to key an appearance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Focusable {
    /// Which annotation, by its raw `/Annots` index.
    pub(crate) id: AnnotId,
    /// Its rectangle, from the raw `/Rect` rather than a focused widget's
    /// inflated box.
    pub(crate) rect: Rect,
}

/// The focus ring for one page, in traversal order.
///
/// `degenerate` reports that the banding could not make progress and the
/// remainder was appended in index order — the recovery that replaces the
/// oracle's hang.
#[derive(Debug, Clone, PartialEq)]
pub struct FocusRing {
    /// The annotations, in traversal order.
    pub order: Vec<AnnotId>,
    /// Whether the banding degenerated.
    pub degenerate: bool,
}

impl FocusRing {
    /// Builds the ring for a page.
    ///
    /// `annots` arrives in annotation order and must already be filtered to
    /// the focusable subtypes — signature widgets excluded, which the caller
    /// does because only it knows which widgets are signatures.
    pub(crate) fn build(annots: &[Focusable], order: TabOrder) -> FocusRing {
        match order {
            TabOrder::Structure => FocusRing {
                order: annots.iter().map(|a| a.id).collect(),
                degenerate: false,
            },
            TabOrder::Row => band(annots, Axis::Row),
            TabOrder::Column => band(annots, Axis::Column),
        }
    }

    /// The annotation after `current`, or `None` at the end.
    ///
    /// Focus does not wrap: the last annotation has no next, which is what
    /// makes a fifth Tab across four fields report the event unconsumed.
    #[must_use]
    pub fn next(&self, current: AnnotId) -> Option<AnnotId> {
        let at = self.order.iter().position(|a| *a == current)?;
        self.order.get(at + 1).copied()
    }

    /// The annotation before `current`, or `None` at the start.
    #[must_use]
    pub fn prev(&self, current: AnnotId) -> Option<AnnotId> {
        let at = self.order.iter().position(|a| *a == current)?;
        self.order.get(at.checked_sub(1)?).copied()
    }

    /// The first annotation, which a forward Tab from nothing lands on.
    #[must_use]
    pub fn first(&self) -> Option<AnnotId> {
        self.order.first().copied()
    }

    /// The last annotation, which a backward Tab from nothing lands on.
    ///
    /// That the two differ is the whole reason both exist: with nothing
    /// focused, forward and backward Tab land on *different* annotations,
    /// because the cursor starts between the ends rather than before them.
    #[must_use]
    pub fn last(&self) -> Option<AnnotId> {
        self.order.last().copied()
    }

    /// How many annotations are in the ring.
    #[must_use]
    pub fn len(&self) -> usize {
        self.order.len()
    }

    /// Whether nothing is focusable.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }
}

/// Which axis a banding pass runs along.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Axis {
    Row,
    Column,
}

/// The banding fold, shared by the row and column orders.
///
/// Each pass picks a seed — the topmost remaining for rows, the leftmost for
/// columns — emits it, then emits every remaining annotation whose centre on
/// the cross axis lies **strictly** inside the seed's extent, in index order.
/// The comparisons are strict at both ends, so an annotation exactly level
/// with a band's edge starts a new band rather than joining that one.
//
// [oracle-bug] cpdfsdk_annotiterator.cpp:137-147 cannot terminate when no
// candidate remains. The row pass seeds `float fTop = 0.0f;` (:137) and tests
// `rcAnnot.top > fTop` (:140), so a page whose remaining annotations all have
// a non-positive top leaves `nLeftTopIndex` at -1; the `continue` at :146 then
// re-enters `while (!sa.empty())` (:135) without erasing anything, so the loop
// state is bit-identical on every pass and the iterator spins forever. A
// non-positive top is ordinary: §12.5.5 places `/Tabs R` order in the page's
// own coordinate space, which a `/MediaBox` with a negative or zero origin
// puts entirely at or below zero. pdf.js implements no `/Tabs` order at all —
// every widget gets `tabIndex = 0` (annotation_layer.js:412) and the DOM
// decides — so it cannot hang here either. We terminate instead: with no
// candidate, the remainder is appended in index order and the ring records
// `degenerate`. A library that hangs on input its own spec admits is a bug
// whatever the oracle does.
fn band(annots: &[Focusable], axis: Axis) -> FocusRing {
    // Sort by the axis's primary key, keeping annotation order within ties.
    let mut remaining: Vec<Focusable> = annots.to_vec();
    match axis {
        Axis::Row => remaining.sort_by(|a, b| {
            a.rect
                .left
                .partial_cmp(&b.rect.left)
                .unwrap_or(std::cmp::Ordering::Equal)
        }),
        Axis::Column => remaining.sort_by(|a, b| {
            b.rect
                .top
                .partial_cmp(&a.rect.top)
                .unwrap_or(std::cmp::Ordering::Equal)
        }),
    }

    let mut order = Vec::with_capacity(remaining.len());
    let mut degenerate = false;

    while !remaining.is_empty() {
        let Some(seed) = seed_index(&remaining, axis) else {
            // No candidate: this is the `[oracle-bug]` above, at the line
            // where it bites — `cpdfsdk_annotiterator.cpp:145-147` spins
            // forever here. Append what is left in index order and say so.
            degenerate = true;
            order.extend(remaining.iter().map(|a| a.id));
            break;
        };

        let Some(head) = remaining.get(seed).copied() else {
            degenerate = true;
            order.extend(remaining.iter().map(|a| a.id));
            break;
        };
        remaining.remove(seed);
        order.push(head.id);

        // Everything whose cross-axis centre is strictly inside the seed's
        // extent joins this band, in index order.
        let mut kept = Vec::with_capacity(remaining.len());
        for annot in remaining.drain(..) {
            let joins = match axis {
                Axis::Row => {
                    annot.rect.center_y() > head.rect.bottom
                        && annot.rect.center_y() < head.rect.top
                }
                Axis::Column => {
                    annot.rect.center_x() > head.rect.left
                        && annot.rect.center_x() < head.rect.right
                }
            };
            if joins {
                order.push(annot.id);
            } else {
                kept.push(annot);
            }
        }
        remaining = kept;
    }

    FocusRing { order, degenerate }
}

/// Picks the next band's seed, or `None` when nothing qualifies.
///
/// The scan runs **downward** with a strict comparison, so a tie never
/// displaces the running best — and the running best when a tie is met was
/// set by a *higher* index. So the winner among ties is the **highest** index,
/// which after the left-ascending sort is the **rightmost** of the tied
/// annotations for a row pass.
///
/// "Lowest index, i.e. leftmost" is the natural guess and it is wrong;
/// `row_order_seeds_each_band_with_the_rightmost_of_the_topmost` is the test
/// that fails if this is changed to it.
fn seed_index(remaining: &[Focusable], axis: Axis) -> Option<usize> {
    match axis {
        Axis::Row => {
            let mut best: Option<usize> = None;
            let mut top = 0.0f32;
            for (i, annot) in remaining.iter().enumerate().rev() {
                if annot.rect.top > top {
                    best = Some(i);
                    top = annot.rect.top;
                }
            }
            best
        }
        Axis::Column => {
            // Two upstream quirks are reproduced here rather than tidied,
            // because both are reachable and both change the order.
            //
            // The guard is `left < 0`, which reads as "nothing chosen yet"
            // only while coordinates are positive. A page whose annotations
            // sit at negative x re-enters it on later iterations, and each
            // time it does it seeds **index zero** rather than the index it
            // is looking at. Neither is what the row pass does, and the
            // difference is why the two passes are not one function with a
            // flipped axis.
            let mut best: Option<usize> = None;
            let mut left = -1.0f32;
            for (i, annot) in remaining.iter().enumerate().rev() {
                if left < 0.0 {
                    best = Some(0);
                    left = annot.rect.left;
                } else if annot.rect.left < left {
                    best = Some(i);
                    left = annot.rect.left;
                }
            }
            best
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn annot(index: u32, left: f32, bottom: f32, right: f32, top: f32) -> Focusable {
        Focusable {
            id: AnnotId::new(0, index),
            rect: Rect::new(left, bottom, right, top),
        }
    }

    fn ids(ring: &FocusRing) -> Vec<u32> {
        ring.order.iter().map(|a| a.index).collect()
    }

    /// Only `R` and `C` are recognized; `S` is not special-cased and lands in
    /// the same branch as an absent entry.
    #[test]
    fn the_tabs_entry_recognizes_exactly_two_spellings() {
        assert_eq!(TabOrder::from_tabs(Some(b"R")), TabOrder::Row);
        assert_eq!(TabOrder::from_tabs(Some(b"C")), TabOrder::Column);
        assert_eq!(TabOrder::from_tabs(Some(b"S")), TabOrder::Structure);
        assert_eq!(TabOrder::from_tabs(Some(b"r")), TabOrder::Structure);
        assert_eq!(TabOrder::from_tabs(Some(b"")), TabOrder::Structure);
        assert_eq!(TabOrder::from_tabs(None), TabOrder::Structure);
    }

    #[test]
    fn structure_order_sorts_nothing() {
        let annots = [
            annot(0, 500.0, 500.0, 600.0, 600.0),
            annot(1, 0.0, 0.0, 100.0, 100.0),
            annot(2, 200.0, 700.0, 300.0, 800.0),
        ];
        let ring = FocusRing::build(&annots, TabOrder::Structure);
        assert_eq!(ids(&ring), vec![0, 1, 2]);
        assert!(!ring.degenerate);
    }

    /// Focus does not wrap in either direction.
    #[test]
    fn the_ring_has_two_ends_and_no_wrap() {
        let annots = [
            annot(0, 0.0, 0.0, 10.0, 10.0),
            annot(1, 0.0, 20.0, 10.0, 30.0),
            annot(2, 0.0, 40.0, 10.0, 50.0),
        ];
        let ring = FocusRing::build(&annots, TabOrder::Structure);

        assert_eq!(ring.next(AnnotId::new(0, 0)), Some(AnnotId::new(0, 1)));
        assert_eq!(ring.next(AnnotId::new(0, 2)), None);
        assert_eq!(ring.prev(AnnotId::new(0, 2)), Some(AnnotId::new(0, 1)));
        assert_eq!(ring.prev(AnnotId::new(0, 0)), None);
    }

    /// With nothing focused, forward and backward Tab land on different
    /// annotations — the cursor starts between the ends, not before them.
    #[test]
    fn the_two_ends_are_different_annotations() {
        let annots = [
            annot(0, 0.0, 0.0, 10.0, 10.0),
            annot(1, 0.0, 20.0, 10.0, 30.0),
        ];
        let ring = FocusRing::build(&annots, TabOrder::Structure);
        assert_eq!(ring.first(), Some(AnnotId::new(0, 0)));
        assert_eq!(ring.last(), Some(AnnotId::new(0, 1)));
        assert_ne!(ring.first(), ring.last());
    }

    /// Row order reads across the page, one band at a time — but the band's
    /// *seed* is the rightmost of the annotations tied for topmost, not the
    /// leftmost, and the reading order that follows is relative to it.
    ///
    /// Two rows of two, indices deliberately not in reading order, gives
    /// `3, 1, 0, 2` rather than the `1, 3, 2, 0` a reader expects. The seed
    /// scan runs from the highest index down with a strict comparison, so an
    /// annotation only displaces the running best by being *strictly* higher;
    /// the tied one visited first — which after the left-ascending sort is
    /// the rightmost — keeps the seat. This is the oracle's order and it is
    /// what the goldens contain.
    #[test]
    fn row_order_seeds_each_band_with_the_rightmost_of_the_topmost() {
        let annots = [
            annot(0, 300.0, 100.0, 400.0, 150.0), // bottom row, right
            annot(1, 100.0, 500.0, 200.0, 550.0), // top row, left
            annot(2, 100.0, 100.0, 200.0, 150.0), // bottom row, left
            annot(3, 300.0, 500.0, 400.0, 550.0), // top row, right
        ];
        let ring = FocusRing::build(&annots, TabOrder::Row);
        assert_eq!(ids(&ring), vec![3, 1, 0, 2]);
        assert!(!ring.degenerate);
    }

    /// With no tie the seed is simply the topmost, and the band that follows
    /// it reads left to right.
    #[test]
    fn an_untied_row_band_reads_left_to_right() {
        let annots = [
            annot(0, 300.0, 500.0, 400.0, 540.0), // top row, right, lower top
            annot(1, 100.0, 500.0, 200.0, 550.0), // top row, left, highest
            annot(2, 100.0, 100.0, 200.0, 150.0), // bottom row
        ];
        let ring = FocusRing::build(&annots, TabOrder::Row);
        assert_eq!(ids(&ring), vec![1, 0, 2]);
    }

    /// Column order bands on horizontal centres, and its seed rule is the
    /// quirky one: the guard reads "nothing chosen yet" as `left < 0`, and
    /// when it fires it seeds index **zero** rather than the index being
    /// looked at. Both are reproduced, so this order is what the oracle
    /// produces rather than what a rotated row pass would.
    #[test]
    fn column_order_reads_down_bands() {
        let annots = [
            annot(0, 300.0, 100.0, 400.0, 150.0), // right column, bottom
            annot(1, 100.0, 500.0, 200.0, 550.0), // left column, top
            annot(2, 100.0, 100.0, 200.0, 150.0), // left column, bottom
            annot(3, 300.0, 500.0, 400.0, 550.0), // right column, top
        ];
        let ring = FocusRing::build(&annots, TabOrder::Column);
        assert_eq!(ids(&ring), vec![1, 2, 3, 0]);
        assert!(!ring.degenerate);
    }

    /// The banding comparison is strict at both ends, so an annotation whose
    /// centre sits exactly on a band edge starts its own band.
    #[test]
    fn a_centre_exactly_on_a_band_edge_does_not_join_it() {
        // The second annotation's centre y is exactly the first's bottom.
        let annots = [
            annot(0, 0.0, 100.0, 50.0, 200.0),
            annot(1, 100.0, 50.0, 150.0, 150.0),
        ];
        let ring = FocusRing::build(&annots, TabOrder::Row);
        assert_eq!(annots[1].rect.center_y(), annots[0].rect.bottom);
        // Both are still emitted, but as two bands rather than one.
        assert_eq!(ids(&ring), vec![0, 1]);
    }

    /// The case the oracle spins on: every annotation's top is non-positive,
    /// so its downward scan never finds a candidate and its loop erases
    /// nothing. Here the remainder is appended and the caller is told.
    #[test]
    fn banding_terminates_where_the_oracle_would_hang() {
        let annots = [
            annot(0, 10.0, -200.0, 20.0, -100.0),
            annot(1, 30.0, -400.0, 40.0, -300.0),
        ];
        let ring = FocusRing::build(&annots, TabOrder::Row);

        assert!(ring.degenerate, "the recovery should be reported");
        assert_eq!(ring.len(), 2, "no annotation may be dropped");
        assert_eq!(ids(&ring), vec![0, 1]);
    }

    /// A top of exactly zero is the boundary case, and it degenerates too:
    /// the comparison is strict.
    #[test]
    fn a_zero_top_is_on_the_hanging_side_of_the_comparison() {
        let annots = [annot(0, 0.0, -10.0, 10.0, 0.0)];
        let ring = FocusRing::build(&annots, TabOrder::Row);
        assert!(ring.degenerate);
        assert_eq!(ids(&ring), vec![0]);
    }

    /// Whatever the geometry, every annotation comes out exactly once and the
    /// pass returns. This is the property the oracle cannot state.
    #[test]
    fn banding_always_terminates_and_preserves_every_annotation() {
        for seed in 0..200u32 {
            let mut bits = seed.wrapping_mul(2_654_435_761);
            let mut annots = Vec::new();
            for i in 0..6u32 {
                let mut next = || {
                    bits = bits.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                    // Bounded to 0..1000, so the conversion is exact.
                    f32::from(u16::try_from((bits >> 16) % 1000).unwrap_or(0)) - 500.0
                };
                let (x, y) = (next(), next());
                annots.push(annot(i, x, y, x + 20.0, y + 20.0));
            }

            for order in [TabOrder::Row, TabOrder::Column, TabOrder::Structure] {
                let ring = FocusRing::build(&annots, order);
                assert_eq!(ring.len(), 6, "annotation lost at seed {seed}");

                let mut seen: Vec<u32> = ring.order.iter().map(|a| a.index).collect();
                seen.sort_unstable();
                assert_eq!(seen, vec![0, 1, 2, 3, 4, 5], "duplicate at seed {seed}");
            }
        }
    }

    /// The ring is built only from focusable annotations, but it reports the
    /// raw `/Annots` indices those annotations have — so a page whose
    /// pop-ups and non-widgets leave gaps in the numbering keeps the gaps.
    #[test]
    fn the_ring_keeps_the_gaps_that_unfocusable_annotations_leave() {
        // /Annots = [ popup, widget, highlight, widget ]; only 1 and 3 are
        // focusable, so those are the indices the ring must carry.
        let annots = [
            annot(1, 100.0, 400.0, 200.0, 450.0),
            annot(3, 100.0, 200.0, 200.0, 250.0),
        ];
        let ring = FocusRing::build(&annots, TabOrder::Structure);

        assert_eq!(ids(&ring), vec![1, 3], "the ring is not renumbered");
        assert_eq!(ring.first(), Some(AnnotId::new(0, 1)));
        assert_eq!(ring.next(AnnotId::new(0, 1)), Some(AnnotId::new(0, 3)));
        assert_eq!(ring.next(AnnotId::new(0, 3)), None);
    }

    #[test]
    fn an_empty_page_has_an_empty_ring() {
        let ring = FocusRing::build(&[], TabOrder::Row);
        assert!(ring.is_empty());
        assert_eq!(ring.first(), None);
        assert_eq!(ring.last(), None);
        assert!(!ring.degenerate);
    }

    /// An annotation the ring does not contain has neither a next nor a
    /// previous, rather than defaulting to an end.
    #[test]
    fn an_unknown_annotation_has_no_neighbours() {
        let annots = [annot(0, 0.0, 0.0, 10.0, 10.0)];
        let ring = FocusRing::build(&annots, TabOrder::Structure);
        assert_eq!(ring.next(AnnotId::new(0, 99)), None);
        assert_eq!(ring.prev(AnnotId::new(0, 99)), None);
    }

    mod annotiter {
        //! Ported focus-traversal assertions.
        //!
        //! In-crate rather than in `tests/`, because the fixture is built from
        //! [`Focusable`] and [`Rect`] — the crate's own `f32` geometry, not its
        //! public vocabulary. Nothing else about them moved.
        //!
        //! The fixture is the oracle's own `annotiter.pdf`, transcribed rather than
        //! parsed: one field with four widget kids, on three pages that differ only
        //! in the traversal order they ask for. Its geometry is the point — the four
        //! widgets sit at the corners of a square, in an annotation order that is
        //! none of the three traversal orders, so each order produces a different
        //! answer and a wrong implementation cannot accidentally agree.
        //!
        //! ```text
        //!   (201,400) LeftTop  #2        #1 RightTop  (401,401)
        //!
        //!   (200,200) LeftBottom #0      #3 RightBottom (400,201)
        //! ```

        use super::*;

        /// The four widgets of `annotiter.pdf`, in the order its `/Annots` array
        /// lists them.
        fn annotiter_widgets() -> Vec<Focusable> {
            [
                (0, 200.0, 200.0, 220.0, 220.0), // Sub_LeftBottom
                (1, 401.0, 401.0, 421.0, 421.0), // Sub_RightTop
                (2, 201.0, 400.0, 221.0, 420.0), // Sub_LeftTop
                (3, 400.0, 201.0, 420.0, 221.0), // Sub_RightBottom
            ]
            .into_iter()
            .map(|(index, left, bottom, right, top)| Focusable {
                id: AnnotId::new(0, index),
                rect: Rect::new(left, bottom, right, top),
            })
            .collect()
        }

        fn row_ring() -> FocusRing {
            FocusRing::build(&annotiter_widgets(), TabOrder::Row)
        }

        fn indices(ring: &FocusRing) -> Vec<u32> {
            ring.order.iter().map(|a| a.index).collect()
        }

        /// `FormFillFirstTab`: a forward tab with nothing focused lands on annot 1.
        #[test]
        fn first_tab_lands_on_annot_one() {
            assert_eq!(row_ring().first(), Some(AnnotId::new(0, 1)));
        }

        /// `FormFillFirstShiftTab`: a backward tab with nothing focused lands on
        /// annot 0 — a *different* annotation, because the cursor starts between the
        /// ends rather than before them.
        #[test]
        fn first_shift_tab_lands_on_annot_zero() {
            assert_eq!(row_ring().last(), Some(AnnotId::new(0, 0)));
        }

        /// `FormFillContinuousTab`: four forward tabs visit 1, 2, 3, 0 and the fifth
        /// is not handled, because focus does not wrap.
        #[test]
        fn continuous_tab_visits_one_two_three_zero_then_stops() {
            let ring = row_ring();
            let mut at = ring.first().expect("the ring is not empty");
            assert_eq!(at, AnnotId::new(0, 1));

            let mut visited = vec![at.index];
            for _ in 0..3 {
                at = ring.next(at).expect("a next annotation");
                visited.push(at.index);
            }
            assert_eq!(visited, vec![1, 2, 3, 0]);

            assert_eq!(ring.next(at), None, "the fifth tab is not handled");
        }

        /// `FormFillContinuousShiftTab`: four backward tabs visit 0, 3, 2, 1 and the
        /// fifth is not handled.
        #[test]
        fn continuous_shift_tab_visits_zero_three_two_one_then_stops() {
            let ring = row_ring();
            let mut at = ring.last().expect("the ring is not empty");
            assert_eq!(at, AnnotId::new(0, 0));

            let mut visited = vec![at.index];
            for _ in 0..3 {
                at = ring.prev(at).expect("a previous annotation");
                visited.push(at.index);
            }
            assert_eq!(visited, vec![0, 3, 2, 1]);

            assert_eq!(ring.prev(at), None, "the fifth shift-tab is not handled");
        }

        /// The forward and backward walks are exact reverses of one another over this
        /// fixture, which is what makes the two "first tab" answers differ.
        #[test]
        fn the_backward_walk_reverses_the_forward_one() {
            let ring = row_ring();
            let forward = indices(&ring);
            let mut backward = forward.clone();
            backward.reverse();

            let mut walked = vec![ring.last().expect("a last").index];
            let mut at = ring.last().expect("a last");
            while let Some(prev) = ring.prev(at) {
                walked.push(prev.index);
                at = prev;
            }
            assert_eq!(walked, backward);
        }

        /// The three pages of the fixture differ only in the order they ask for, and
        /// each produces a different answer over the same four widgets — which is
        /// what makes the fixture able to tell the orders apart at all.
        #[test]
        fn the_three_orders_disagree_over_this_fixture() {
            let widgets = annotiter_widgets();
            let row = indices(&FocusRing::build(&widgets, TabOrder::Row));
            let column = indices(&FocusRing::build(&widgets, TabOrder::Column));
            let structure = indices(&FocusRing::build(&widgets, TabOrder::Structure));

            assert_eq!(structure, vec![0, 1, 2, 3], "structure order sorts nothing");
            assert_eq!(row, vec![1, 2, 3, 0]);
            assert_ne!(row, column);
            assert_ne!(row, structure);
            assert_ne!(column, structure);
        }

        /// Every order is a permutation of the annotations: none is dropped and none
        /// is visited twice, whichever way the page asks for.
        #[test]
        fn every_order_is_a_permutation() {
            let widgets = annotiter_widgets();
            for order in [TabOrder::Row, TabOrder::Column, TabOrder::Structure] {
                let ring = FocusRing::build(&widgets, order);
                let mut seen = indices(&ring);
                seen.sort_unstable();
                assert_eq!(seen, vec![0, 1, 2, 3], "{order:?} is not a permutation");
                assert!(!ring.degenerate, "{order:?} should not degenerate");
            }
        }
    }

    mod never_panics {
        //! The property that outranks every behavioural one, for the geometry
        //! this crate keeps private: **no input panics.**
        //!
        //! In-crate rather than in `tests/never_panics.rs`, because these three
        //! generate [`Plate`]s, [`Candidate`]s and [`Focusable`]s out of
        //! `f32` [`Rect`]s — the crate's own geometry, not its public
        //! vocabulary. The rest of that file's properties are over public
        //! types and stayed where they were.

        use crate::event::Point;
        use crate::geom::{Plate, Rotation};
        use crate::hit::{Candidate, LayoutBand, Permissions, WidgetHit, widget_at_point};
        use crate::session::AnnotId;
        use crate::tab::{FocusRing, Focusable, Rect, TabOrder};

        /// A small deterministic generator: a counter run through a mixing
        /// step. Enough spread to reach the awkward cases, and reproducible
        /// when one fails.
        struct Gen(u32);

        impl Gen {
            fn next(&mut self) -> u32 {
                self.0 = self.0.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                self.0
            }

            fn coord(&mut self) -> f32 {
                // Bounded to -1000..1000, so the conversion is exact.
                let raw = i16::try_from(self.next() % 2000).unwrap_or(0) - 1000;
                f32::from(raw) / 2.0
            }

            fn below(&mut self, n: u32) -> u32 {
                if n == 0 { 0 } else { self.next() % n }
            }
        }

        /// Rectangles that would break a naive implementation: inverted, degenerate,
        /// negative, and a one-by-one box a real corpus file contains.
        fn awkward_rects() -> Vec<Rect> {
            vec![
                Rect::new(0.0, 0.0, 0.0, 0.0),
                Rect::new(10.0, 10.0, 10.0, 10.0),
                Rect::new(1.0, 1.0, 2.0, 2.0),
                // Written inside out, as bug_889099's field is.
                Rect::new(100.0, 100.0, 200.0, -130.0),
                Rect::new(200.0, 200.0, 100.0, 100.0),
                Rect::new(-500.0, -500.0, -400.0, -400.0),
                Rect::new(0.0, 0.0, 1e6, 1e6),
            ]
        }

        /// The plate transform survives every awkward rectangle and every rotation,
        /// and never produces a value that is not a number.
        #[test]
        fn the_plate_transform_never_panics_or_produces_nonsense() {
            let mut rng = Gen(1);
            for rect in awkward_rects() {
                for rotation in [
                    Rotation::None,
                    Rotation::Quarter,
                    Rotation::Half,
                    Rotation::ThreeQuarter,
                ] {
                    let plate = Plate::new(rect, rotation);
                    for _ in 0..20 {
                        let at = Point::new(rng.coord(), rng.coord());
                        let there = plate.to_plate(at);
                        let back = plate.to_page(there);
                        assert!(there.x.is_finite() && there.y.is_finite());
                        assert!(back.x.is_finite() && back.y.is_finite());
                    }
                    assert!(plate.width().is_finite());
                    assert!(plate.height().is_finite());
                    assert!(
                        plate.width() >= 0.0,
                        "a normalized box has no negative width"
                    );
                    assert!(plate.height() >= 0.0);
                }
            }
        }

        /// Hit testing over generated geometry returns, and never names an
        /// annotation that is not in the list.
        #[test]
        fn hit_testing_never_panics_and_never_invents_an_annotation() {
            let mut rng = Gen(7);
            for _ in 0..200 {
                let count = rng.below(6);
                let candidates: Vec<Candidate> = (0..count)
                    .map(|i| {
                        let (x, y) = (rng.coord(), rng.coord());
                        Candidate {
                            id: AnnotId::new(rng.below(3), i),
                            rect: Rect::new(x, y, x + rng.coord(), y + rng.coord()),
                            band: match rng.below(3) {
                                0 => LayoutBand::Popup,
                                1 => LayoutBand::Widget,
                                _ => LayoutBand::Other,
                            },
                            widget: (rng.below(2) == 0).then(|| WidgetHit {
                                signature: rng.below(2) == 0,
                                hidden: rng.below(2) == 0,
                                read_only: rng.below(2) == 0,
                                push_button: rng.below(2) == 0,
                            }),
                        }
                    })
                    .collect();

                let focused = candidates.first().map(|c| c.id);
                for permissions in [Permissions::ALL, Permissions::NONE] {
                    let hit = widget_at_point(
                        &candidates,
                        focused,
                        permissions,
                        rng.coord(),
                        rng.coord(),
                    );
                    if let Some(hit) = hit {
                        assert!(
                            candidates.iter().any(|c| c.id == hit),
                            "hit test named an annotation that is not in the list"
                        );
                    }
                }
            }
        }

        /// The tab-order banding terminates over any geometry, in every order, and
        /// emits each annotation exactly once. The upstream loop hangs here.
        #[test]
        fn the_focus_ring_always_terminates_and_is_always_a_permutation() {
            let mut rng = Gen(13);
            for _ in 0..300 {
                let count = rng.below(8);
                let annots: Vec<Focusable> = (0..count)
                    .map(|i| {
                        let (x, y) = (rng.coord(), rng.coord());
                        Focusable {
                            id: AnnotId::new(0, i),
                            // Deliberately including zero-area and inverted boxes.
                            rect: Rect::new(x, y, x + rng.coord(), y + rng.coord()),
                        }
                    })
                    .collect();

                for order in [TabOrder::Row, TabOrder::Column, TabOrder::Structure] {
                    let built = FocusRing::build(&annots, order);
                    assert_eq!(built.len(), annots.len(), "{order:?} lost an annotation");

                    let mut seen: Vec<u32> = built.order.iter().map(|a| a.index).collect();
                    seen.sort_unstable();
                    let expected: Vec<u32> = (0..count).collect();
                    assert_eq!(seen, expected, "{order:?} is not a permutation");

                    // Walking the ring from either end terminates.
                    if let Some(mut at) = built.first() {
                        let mut steps = 0;
                        while let Some(next) = built.next(at) {
                            at = next;
                            steps += 1;
                            assert!(
                                steps <= count as usize,
                                "the forward walk did not terminate"
                            );
                        }
                    }
                }
            }
        }
    }
}
