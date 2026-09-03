//! Which annotation is under a point, and in what order they are considered.
//!
//! # There are two different lookups, and the asymmetry is deliberate
//!
//! **Hovering** uses plain rectangle containment over **every** annotation
//! subtype except pop-ups. That is what makes a bare pointer move over a
//! highlight annotation raise its pop-up — six pixel fixtures in the corpus
//! do nothing else — and it is why hover cannot simply reuse the click test.
//!
//! **Clicking** goes through a widget hit test, which additionally rejects
//! signature widgets, invisible widgets, **read-only widgets**, and, for
//! anything that is not a push button, a document whose permissions grant
//! neither form filling nor annotation modification.
//!
//! Read-only deserves a second look because it is the rule most likely to be
//! read as a bug: a read-only widget is not clickable *at all*. That is not
//! in tension with a read-only check box consuming a Return — there the
//! widget already had focus and the event arrived from the keyboard, which
//! never consults this test.
//!
//! # Layout order is not annotation order
//!
//! Annotations are considered in a stable sort by layout band — pop-ups
//! first, then widgets, then everything else — which preserves file order
//! within each band. The focused annotation is then moved: to the **front**
//! for hit testing, so it wins an overlap tie, and to the **end** for
//! drawing, so it paints on top. Same list, two arrangements, opposite ends.

use crate::session::AnnotId;
use crate::tab::Rect;

/// How far a focused widget's box is grown beyond its rectangle.
///
/// A focused widget draws a focus ring, so its clickable box is one unit
/// larger on every side than its `/Rect`.
pub const FOCUS_INFLATION: f32 = 1.0;

/// Which layout band an annotation sorts into.
///
/// The three values are the oracle's own, and the gaps in them are its own
/// too: everything that is neither a pop-up nor a widget shares the last
/// band.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LayoutBand {
    /// A pop-up note. Drawn under everything.
    Popup = 1,
    /// A form widget.
    Widget = 2,
    /// Every other subtype.
    Other = 5,
}

/// What the hit test needs to know about one annotation.
///
/// # Build these from the raw `/Annots` array
///
/// A candidate's `id` carries a **raw** `/Annots` index, pop-ups counted —
/// see [`AnnotId`]. Pop-ups appear here as ordinary candidates in the
/// [`LayoutBand::Popup`] band rather than being filtered out, precisely so
/// that a caller can walk the array once and index it directly. Filtering
/// them out on the way in and then reporting positions in the filtered list
/// is the mistake this type is shaped to prevent: the appearance overlay is
/// keyed by the raw index, so every widget after a pop-up would be off by
/// one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Candidate {
    /// Which annotation, by its raw `/Annots` index.
    pub(crate) id: AnnotId,
    /// Its rectangle, as the file wrote it. Need not be normalized:
    /// [`contains`] normalizes before comparing.
    pub(crate) rect: Rect,
    /// Which band it sorts into.
    pub(crate) band: LayoutBand,
    /// Whether it is a widget the click test may accept.
    pub(crate) widget: Option<WidgetHit>,
}

/// The widget-only half of a candidate.
///
/// Four independent gates, each read from a different place in the file — two
/// from the annotation, two from the field — and each one a hard rejection on
/// its own. Collapsing them into a mask would lose which gate rejected a
/// click, which is exactly the question a reader of a failing hit test asks.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WidgetHit {
    /// Whether the widget is a signature, which is never hit-testable.
    pub signature: bool,
    /// Whether any of the invisible, hidden or no-view flags is set.
    pub hidden: bool,
    /// Whether the field refuses edits.
    pub read_only: bool,
    /// Whether the field is a push button, which is hit-testable regardless
    /// of the document's permissions.
    pub push_button: bool,
}

impl WidgetHit {
    /// Whether this widget accepts a click, given the document's permissions.
    ///
    /// The order matters only for readability — every gate is a hard `false`
    /// — but it is the oracle's order, so a reader comparing the two sees the
    /// same sequence.
    #[must_use]
    pub fn accepts_click(self, permissions: Permissions) -> bool {
        if self.signature || self.hidden || self.read_only {
            return false;
        }
        self.push_button || permissions.may_interact()
    }
}

/// The document permission bits this crate consults.
///
/// Only two of them matter, and **either one suffices**: a document that
/// grants form filling *or* annotation modification is interactive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Permissions {
    /// Whether the document permits filling in form fields.
    pub fill_form: bool,
    /// Whether it permits modifying annotations.
    pub modify_annotation: bool,
}

impl Permissions {
    /// A document that permits everything, which is what an unencrypted one
    /// and an owner-authenticated one both amount to.
    pub const ALL: Permissions = Permissions {
        fill_form: true,
        modify_annotation: true,
    };

    /// A document that permits neither.
    pub const NONE: Permissions = Permissions {
        fill_form: false,
        modify_annotation: false,
    };

    /// Whether a non-push-button widget may be clicked.
    #[must_use]
    pub fn may_interact(self) -> bool {
        self.fill_form || self.modify_annotation
    }
}

/// Whether a point lies inside a rectangle.
///
/// Inclusive on every edge, which is what makes a click exactly on a widget's
/// boundary land in it.
///
/// **Normalizes first**, so a rectangle a file wrote inside out is still
/// hit-testable — which is what the oracle's own containment does, by
/// copying and normalizing before it compares. Leaving this as a
/// precondition on the caller would make an inverted `/Rect` silently
/// unclickable, and real files contain them.
pub(crate) fn contains(rect: Rect, x: f32, y: f32) -> bool {
    let rect = crate::geom::normalize(rect);
    x >= rect.left && x <= rect.right && y >= rect.bottom && y <= rect.top
}

/// Grows a rectangle by one unit on every side.
pub(crate) fn inflate(rect: Rect, by: f32) -> Rect {
    Rect::new(
        rect.left - by,
        rect.bottom - by,
        rect.right + by,
        rect.top + by,
    )
}

/// Orders candidates for hit testing: by band, then file order, with the
/// focused annotation moved to the front so it wins an overlap tie.
pub(crate) fn hit_order(candidates: &[Candidate], focused: Option<AnnotId>) -> Vec<Candidate> {
    let mut ordered = band_sorted(candidates);
    if let Some(focused) = focused
        && let Some(at) = ordered.iter().position(|c| c.id == focused)
    {
        let moved = ordered.remove(at);
        ordered.insert(0, moved);
    }
    ordered
}

/// Orders candidates for drawing: the same band sort, with the focused
/// annotation moved to the **end** so it paints on top.
///
/// The painting half of the band sort — [`hit_order`] is the hit-testing half.
/// Only the crate's own tests call this one, and it lives under `cfg(test)`
/// for that reason; the pair is the invariant.
#[cfg(test)]
pub(crate) fn draw_order(candidates: &[Candidate], focused: Option<AnnotId>) -> Vec<Candidate> {
    let mut ordered = band_sorted(candidates);
    if let Some(focused) = focused
        && let Some(at) = ordered.iter().position(|c| c.id == focused)
    {
        let moved = ordered.remove(at);
        ordered.push(moved);
    }
    ordered
}

/// The shared stable sort by band, preserving file order within each.
fn band_sorted(candidates: &[Candidate]) -> Vec<Candidate> {
    let mut ordered = candidates.to_vec();
    ordered.sort_by_key(|c| c.band);
    ordered
}

/// The annotation under a point for **hover** purposes.
///
/// Rectangle containment over every subtype but pop-ups. This is what raises
/// a highlight annotation's pop-up on a bare pointer move, and it is
/// deliberately more permissive than [`widget_at_point`].
pub(crate) fn annot_at_point(
    candidates: &[Candidate],
    focused: Option<AnnotId>,
    x: f32,
    y: f32,
) -> Option<AnnotId> {
    hit_order(candidates, focused)
        .into_iter()
        .find(|c| c.band != LayoutBand::Popup && contains(c.rect, x, y))
        .map(|c| c.id)
}

/// The widget under a point for **click** purposes.
///
/// Every gate of [`WidgetHit::accepts_click`] applies, and the box tested is
/// grown by [`FOCUS_INFLATION`] for the focused widget, which therefore has a
/// slightly larger target than its neighbours.
pub(crate) fn widget_at_point(
    candidates: &[Candidate],
    focused: Option<AnnotId>,
    permissions: Permissions,
    x: f32,
    y: f32,
) -> Option<AnnotId> {
    hit_order(candidates, focused)
        .into_iter()
        .find(|c| {
            let Some(widget) = c.widget else {
                return false;
            };
            if !widget.accepts_click(permissions) {
                return false;
            }
            let box_ = if Some(c.id) == focused {
                inflate(c.rect, FOCUS_INFLATION)
            } else {
                c.rect
            };
            contains(box_, x, y)
        })
        .map(|c| c.id)
}

/// The z-order index of the widget under a point, or `None` when there is
/// none.
///
/// The index is into the band-sorted list, which is what the oracle reports.
///
/// The z-ordered form of [`widget_at_point`], under `cfg(test)` for the same
/// reason as [`draw_order`].
#[cfg(test)]
pub(crate) fn widget_z_order_at_point(
    candidates: &[Candidate],
    permissions: Permissions,
    x: f32,
    y: f32,
) -> Option<usize> {
    band_sorted(candidates).into_iter().position(|c| {
        c.widget.is_some_and(|w| w.accepts_click(permissions)) && contains(c.rect, x, y)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain_widget() -> WidgetHit {
        WidgetHit {
            signature: false,
            hidden: false,
            read_only: false,
            push_button: false,
        }
    }

    fn widget(index: u32, rect: Rect) -> Candidate {
        Candidate {
            id: AnnotId::new(0, index),
            rect,
            band: LayoutBand::Widget,
            widget: Some(plain_widget()),
        }
    }

    fn other(index: u32, rect: Rect) -> Candidate {
        Candidate {
            id: AnnotId::new(0, index),
            rect,
            band: LayoutBand::Other,
            widget: None,
        }
    }

    fn popup(index: u32, rect: Rect) -> Candidate {
        Candidate {
            id: AnnotId::new(0, index),
            rect,
            band: LayoutBand::Popup,
            widget: None,
        }
    }

    fn box_at(left: f32, bottom: f32) -> Rect {
        Rect::new(left, bottom, left + 100.0, bottom + 50.0)
    }

    /// A file may write a rectangle inside out, and the widget is still
    /// clickable — which is what the oracle does and what this crate would
    /// otherwise get silently wrong, since every comparison fails when the
    /// edges are swapped.
    #[test]
    fn an_inside_out_rectangle_is_still_hit_testable() {
        // The shape a real corpus field has: top written below bottom.
        let inverted = Rect::new(100.0, 100.0, 200.0, -130.0);
        assert!(contains(inverted, 150.0, 0.0));
        assert!(contains(inverted, 150.0, -100.0));
        assert!(!contains(inverted, 150.0, 200.0));

        let mut candidate = widget(0, inverted);
        candidate.rect = inverted;
        assert_eq!(
            widget_at_point(&[candidate], None, Permissions::ALL, 150.0, 0.0),
            Some(AnnotId::new(0, 0)),
            "an inverted rect must not make a widget unclickable"
        );
    }

    #[test]
    fn containment_includes_every_edge() {
        let rect = Rect::new(10.0, 20.0, 30.0, 40.0);
        assert!(contains(rect, 20.0, 30.0));
        assert!(contains(rect, 10.0, 20.0), "the corner is inside");
        assert!(contains(rect, 30.0, 40.0), "the far corner is inside");
        assert!(!contains(rect, 9.9, 30.0));
        assert!(!contains(rect, 20.0, 40.1));
    }

    /// Hover matches any subtype but a pop-up, which is what raises a
    /// highlight annotation's pop-up on a bare pointer move.
    #[test]
    fn hover_matches_a_non_widget_annotation() {
        let candidates = [other(0, box_at(100.0, 700.0))];
        assert_eq!(
            annot_at_point(&candidates, None, 128.0, 713.0),
            Some(AnnotId::new(0, 0))
        );
        // …while the click test finds nothing there at all.
        assert_eq!(
            widget_at_point(&candidates, None, Permissions::ALL, 128.0, 713.0),
            None
        );
    }

    #[test]
    fn hover_skips_popups() {
        let candidates = [popup(0, box_at(0.0, 0.0))];
        assert_eq!(annot_at_point(&candidates, None, 50.0, 25.0), None);
    }

    /// A read-only widget is not clickable at all — a separate rule from a
    /// read-only control consuming a keystroke, which never reaches here.
    #[test]
    fn a_read_only_widget_is_not_clickable() {
        let mut candidate = widget(0, box_at(0.0, 0.0));
        candidate.widget = Some(WidgetHit {
            read_only: true,
            ..plain_widget()
        });
        assert_eq!(
            widget_at_point(&[candidate], None, Permissions::ALL, 50.0, 25.0),
            None
        );
    }

    #[test]
    fn a_signature_or_hidden_widget_is_not_clickable() {
        for hit in [
            WidgetHit {
                signature: true,
                ..plain_widget()
            },
            WidgetHit {
                hidden: true,
                ..plain_widget()
            },
        ] {
            let mut candidate = widget(0, box_at(0.0, 0.0));
            candidate.widget = Some(hit);
            assert_eq!(
                widget_at_point(&[candidate], None, Permissions::ALL, 50.0, 25.0),
                None
            );
        }
    }

    /// Either permission suffices, and a push button needs neither.
    #[test]
    fn permissions_gate_everything_but_a_push_button() {
        let ordinary = plain_widget();
        assert!(!ordinary.accepts_click(Permissions::NONE));
        assert!(ordinary.accepts_click(Permissions::ALL));
        assert!(ordinary.accepts_click(Permissions {
            fill_form: true,
            modify_annotation: false
        }));
        assert!(ordinary.accepts_click(Permissions {
            fill_form: false,
            modify_annotation: true
        }));

        let button = WidgetHit {
            push_button: true,
            ..plain_widget()
        };
        assert!(button.accepts_click(Permissions::NONE));
    }

    /// The focused widget wins an overlap tie, because it is moved to the
    /// front of the hit order.
    #[test]
    fn the_focused_widget_wins_an_overlap() {
        let candidates = [widget(0, box_at(0.0, 0.0)), widget(1, box_at(0.0, 0.0))];

        // With nothing focused, file order decides.
        assert_eq!(
            widget_at_point(&candidates, None, Permissions::ALL, 50.0, 25.0),
            Some(AnnotId::new(0, 0))
        );

        // Focusing the second one moves it in front.
        assert_eq!(
            widget_at_point(
                &candidates,
                Some(AnnotId::new(0, 1)),
                Permissions::ALL,
                50.0,
                25.0
            ),
            Some(AnnotId::new(0, 1))
        );
    }

    /// Hit order and draw order move the focused annotation to opposite ends
    /// of the same list.
    #[test]
    fn hit_order_and_draw_order_are_mirror_images() {
        let candidates = [
            widget(0, box_at(0.0, 0.0)),
            widget(1, box_at(0.0, 0.0)),
            widget(2, box_at(0.0, 0.0)),
        ];
        let focused = Some(AnnotId::new(0, 1));

        let hit: Vec<u32> = hit_order(&candidates, focused)
            .iter()
            .map(|c| c.id.index)
            .collect();
        let draw: Vec<u32> = draw_order(&candidates, focused)
            .iter()
            .map(|c| c.id.index)
            .collect();

        assert_eq!(hit, vec![1, 0, 2], "focused first for hit testing");
        assert_eq!(draw, vec![0, 2, 1], "focused last for drawing");
    }

    /// The bands sort pop-up, widget, then everything else, and file order
    /// survives within each.
    #[test]
    fn the_band_sort_is_stable_within_each_band() {
        let candidates = [
            other(0, box_at(0.0, 0.0)),
            widget(1, box_at(0.0, 0.0)),
            popup(2, box_at(0.0, 0.0)),
            widget(3, box_at(0.0, 0.0)),
            other(4, box_at(0.0, 0.0)),
        ];
        let ordered: Vec<u32> = hit_order(&candidates, None)
            .iter()
            .map(|c| c.id.index)
            .collect();
        assert_eq!(ordered, vec![2, 1, 3, 0, 4]);
    }

    /// A focused widget's target is one unit larger on every side, so a click
    /// just outside its rectangle still lands in it.
    #[test]
    fn a_focused_widget_has_a_slightly_larger_target() {
        let candidates = [widget(0, Rect::new(10.0, 10.0, 20.0, 20.0))];
        let just_outside = (20.5, 15.0);

        assert_eq!(
            widget_at_point(
                &candidates,
                None,
                Permissions::ALL,
                just_outside.0,
                just_outside.1
            ),
            None
        );
        assert_eq!(
            widget_at_point(
                &candidates,
                Some(AnnotId::new(0, 0)),
                Permissions::ALL,
                just_outside.0,
                just_outside.1
            ),
            Some(AnnotId::new(0, 0))
        );
    }

    /// A point over nothing is a miss, not a panic and not a default.
    #[test]
    fn a_point_over_nothing_hits_nothing() {
        let candidates = [widget(0, box_at(100.0, 100.0))];
        assert_eq!(
            widget_at_point(&candidates, None, Permissions::ALL, 1.0, 1.0),
            None
        );
        assert_eq!(annot_at_point(&candidates, None, 1.0, 1.0), None);
        assert_eq!(
            widget_z_order_at_point(&candidates, Permissions::ALL, 1.0, 1.0),
            None
        );
    }

    #[test]
    fn an_empty_page_hits_nothing() {
        assert_eq!(widget_at_point(&[], None, Permissions::ALL, 0.0, 0.0), None);
        assert_eq!(annot_at_point(&[], None, 0.0, 0.0), None);
    }

    /// The case the whole index-space contract exists for: a page whose
    /// first `/Annots` entry is a pop-up. A hit on the widget at raw index 1
    /// must report **1**, not the 0 it would occupy in a pop-up-filtered
    /// list — the appearance overlay is keyed by the raw index, so reporting
    /// the filtered position would draw this widget's appearance onto the
    /// pop-up.
    #[test]
    fn a_page_with_a_popup_reports_raw_annots_indices() {
        // /Annots = [ popup, widget, widget ]
        let candidates = [
            popup(0, box_at(0.0, 600.0)),
            widget(1, box_at(100.0, 400.0)),
            widget(2, box_at(100.0, 200.0)),
        ];

        // The first widget is at raw index 1 even though it is the list's
        // first *widget*.
        assert_eq!(
            widget_at_point(&candidates, None, Permissions::ALL, 150.0, 425.0),
            Some(AnnotId::new(0, 1))
        );
        assert_eq!(
            widget_at_point(&candidates, None, Permissions::ALL, 150.0, 225.0),
            Some(AnnotId::new(0, 2))
        );

        // Hover skips the pop-up, and still answers in the raw index space.
        assert_eq!(annot_at_point(&candidates, None, 50.0, 625.0), None);
        assert_eq!(
            annot_at_point(&candidates, None, 150.0, 425.0),
            Some(AnnotId::new(0, 1))
        );
    }

    /// Sorting into bands must not renumber anything: the pop-up moves to the
    /// front of the order while every id keeps the raw index it arrived with.
    #[test]
    fn the_band_sort_reorders_without_renumbering() {
        let candidates = [
            widget(0, box_at(0.0, 0.0)),
            popup(1, box_at(0.0, 0.0)),
            widget(2, box_at(0.0, 0.0)),
        ];
        let ordered: Vec<u32> = hit_order(&candidates, None)
            .iter()
            .map(|c| c.id.index)
            .collect();

        // The pop-up sorts first, but it is still annotation 1.
        assert_eq!(ordered, vec![1, 0, 2]);
    }

    #[test]
    fn z_order_counts_from_the_band_sorted_list() {
        let candidates = [
            other(0, box_at(0.0, 0.0)),
            widget(1, box_at(0.0, 0.0)),
            popup(2, box_at(0.0, 0.0)),
        ];
        // Band order is popup(2), widget(1), other(0); the widget is index 1.
        assert_eq!(
            widget_z_order_at_point(&candidates, Permissions::ALL, 50.0, 25.0),
            Some(1)
        );
    }
}
