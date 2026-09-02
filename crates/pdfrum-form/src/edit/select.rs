//! The text selection: two places, stored directional.
//!
//! `begin` is the **anchor**, the end that stays put; `end` is the **active**
//! end, the one the caret drags around. Nothing orders them on the way in, so
//! a backwards selection is a legal, reachable state — a shift-left from the
//! middle of a field produces one, and replacing it has to put the caret in
//! the same place a forwards selection of the same span would.
//!
//! # The distinction that is easy to miss
//!
//! *Collapsed* and *reset* are different states and the difference is
//! observable. After every mutation and after a mouse-down, the selection is
//! set to `(caret, caret)`: it selects nothing, but it is a **live anchor**,
//! so the next shift-arrow or drag extends from exactly there. A reset
//! selection has no anchor at all, and a shift-arrow starting from one has to
//! anchor on the caret's *previous* position instead. Both report
//! `is_empty()`; only [`Selection::is_reset`] tells them apart.

use super::place::{Place, Range};

/// A selection: an anchor and an active end, in that order, unordered by
/// value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    /// The fixed end.
    pub begin: Place,
    /// The end that moves with the caret.
    pub end: Place,
}

impl Selection {
    /// A collapsed selection at the start of the text.
    #[must_use]
    pub fn empty() -> Selection {
        Selection {
            begin: Place::start(),
            end: Place::start(),
        }
    }

    /// A selection anchored at `begin`, active at `end`.
    #[must_use]
    pub fn new(begin: Place, end: Place) -> Selection {
        Selection { begin, end }
    }

    /// A live anchor at one place, selecting nothing.
    ///
    /// What a mutation or a mouse-down leaves behind, and what a following
    /// shift-move extends from.
    #[must_use]
    pub fn collapsed_at(place: Place) -> Selection {
        Selection {
            begin: place,
            end: place,
        }
    }

    /// No selection and no anchor.
    ///
    /// Distinct from a collapsed selection: a shift-move from here anchors on
    /// the caret's previous position rather than on this one.
    #[must_use]
    pub fn reset() -> Selection {
        Selection {
            begin: Place {
                section: u32::MAX,
                line: u32::MAX,
                word: None,
            },
            end: Place {
                section: u32::MAX,
                line: u32::MAX,
                word: None,
            },
        }
    }

    /// Whether the selection covers no text.
    #[must_use]
    pub fn is_empty(self) -> bool {
        self.begin == self.end
    }

    /// Whether the selection has no anchor at all.
    #[must_use]
    pub fn is_reset(self) -> bool {
        self.begin
            == Place {
                section: u32::MAX,
                line: u32::MAX,
                word: None,
            }
            && self.end
                == Place {
                    section: u32::MAX,
                    line: u32::MAX,
                    word: None,
                }
    }

    /// Whether the active end lies before the anchor.
    #[must_use]
    pub fn is_backwards(self) -> bool {
        self.end < self.begin
    }

    /// The span, ordered. Direction is discarded here and nowhere else.
    #[must_use]
    pub fn range(self) -> Range {
        Range::new(self.begin, self.end)
    }

    /// Moves the active end, keeping the anchor. What a shift-move does once
    /// an anchor exists.
    pub fn set_active(&mut self, place: Place) {
        self.end = place;
    }
}

impl Default for Selection {
    fn default() -> Selection {
        Selection::empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_selection_keeps_the_direction_it_was_built_with() {
        let lo = Place::new(0, 0, Some(1));
        let hi = Place::new(0, 0, Some(5));
        let forwards = Selection::new(lo, hi);
        let backwards = Selection::new(hi, lo);

        assert!(!forwards.is_backwards());
        assert!(backwards.is_backwards());
        assert_ne!(forwards, backwards);
        // …but they cover the same text.
        assert_eq!(forwards.range(), backwards.range());
    }

    /// Both report empty; only one has an anchor to extend from.
    #[test]
    fn collapsed_and_reset_are_different_states() {
        let here = Place::new(0, 0, Some(3));
        let collapsed = Selection::collapsed_at(here);
        let reset = Selection::reset();

        assert!(collapsed.is_empty());
        assert!(reset.is_empty());

        assert!(!collapsed.is_reset());
        assert!(reset.is_reset());
        assert_ne!(collapsed, reset);
    }

    #[test]
    fn extending_moves_only_the_active_end() {
        let anchor = Place::new(0, 0, Some(1));
        let mut sel = Selection::collapsed_at(anchor);
        sel.set_active(Place::new(0, 0, Some(4)));

        assert_eq!(sel.begin, anchor);
        assert_eq!(sel.end, Place::new(0, 0, Some(4)));
        assert!(!sel.is_empty());
    }

    #[test]
    fn the_range_is_always_ordered() {
        let sel = Selection::new(Place::new(0, 0, Some(7)), Place::new(0, 0, Some(2)));
        assert!(sel.range().begin() <= sel.range().end());
        assert_eq!(sel.range().begin(), Place::new(0, 0, Some(2)));
    }
}
