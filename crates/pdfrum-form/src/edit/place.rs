//! Positions in a laid-out text, and spans between them.
//!
//! # The caret invariant
//!
//! A [`Place`] names the position **after** character `word` of line `line`
//! of section `section`, and `word == -1` is the *line header* — the position
//! before that line's first character. Reading it as "at character `word`" is
//! the easiest way to get this module wrong: it is why an insert at the very
//! start of a field targets `word == -1`, why backspace removes the character
//! the caret names, and why a forward delete has to step to the next place
//! first.
//!
//! # One `Place`, not two
//!
//! The type itself is the layout engine's ([`pdfrum_doc::vt::hit::Place`]),
//! re-exported rather than mirrored. Every interesting operation on a place —
//! turning a click into one, turning one into a caret rectangle, counting
//! characters to it — belongs to the engine that owns the layout, so a second
//! structurally identical type here would buy nothing and cost a conversion
//! at every one of those calls. What this module adds is the vocabulary the
//! *editor* needs on top: the ordered [`Range`], and the small predicates in
//! [`PlaceExt`].

pub use pdfrum_doc::vt::hit::Place;

/// The editor's own questions about a place.
///
/// An extension trait because the type is the layout engine's: these are
/// conveniences for the editing machinery, not part of what a place *is*.
pub trait PlaceExt {
    /// The first place of any layout: the header of its first line.
    #[must_use]
    fn start() -> Place;

    /// Whether this place sits before its line's first character.
    #[must_use]
    fn at_line_start(self) -> bool;

    /// Whether two places are on the same line, ignoring the character.
    ///
    /// The comparison movement and line selection need, and the one a derived
    /// equality cannot give.
    #[must_use]
    fn same_line(self, other: Place) -> bool;
}

impl PlaceExt for Place {
    fn start() -> Place {
        Place::new(0, 0, -1)
    }

    fn at_line_start(self) -> bool {
        self.word < 0
    }

    fn same_line(self, other: Place) -> bool {
        self.section == other.section && self.line == other.line
    }
}

/// An ordered span between two places, `begin <= end`.
///
/// Construction normalizes, so a `Range` can never run backwards. That is the
/// point: every consumer of a span wants it ordered, and the one thing that
/// wants direction — a selection — keeps its two places itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Range {
    begin: Place,
    end: Place,
}

impl Range {
    /// The span between two places, in whichever order they arrive.
    #[must_use]
    pub fn new(a: Place, b: Place) -> Range {
        if a <= b {
            Range { begin: a, end: b }
        } else {
            Range { begin: b, end: a }
        }
    }

    /// An empty span at one place.
    #[must_use]
    pub fn empty_at(place: Place) -> Range {
        Range {
            begin: place,
            end: place,
        }
    }

    /// The earlier end.
    #[must_use]
    pub fn begin(self) -> Place {
        self.begin
    }

    /// The later end.
    #[must_use]
    pub fn end(self) -> Place {
        self.end
    }

    /// Whether the span covers nothing.
    #[must_use]
    pub fn is_empty(self) -> bool {
        self.begin == self.end
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn places_order_lexicographically_over_the_triple() {
        assert!(Place::new(0, 0, -1) < Place::new(0, 0, 0));
        assert!(Place::new(0, 0, 9) < Place::new(0, 1, -1));
        assert!(Place::new(0, 9, 9) < Place::new(1, 0, -1));
    }

    #[test]
    fn the_start_is_before_the_first_character() {
        assert!(Place::start().at_line_start());
        assert_eq!(Place::start().word, -1);
        assert!(!Place::new(0, 0, 0).at_line_start());
    }

    #[test]
    fn same_line_ignores_the_character() {
        assert!(Place::new(1, 2, 0).same_line(Place::new(1, 2, 7)));
        assert!(!Place::new(1, 2, 0).same_line(Place::new(1, 3, 0)));
        assert!(!Place::new(1, 2, 0).same_line(Place::new(2, 2, 0)));
    }

    #[test]
    fn a_range_normalizes_whichever_way_it_is_built() {
        let lo = Place::new(0, 0, 1);
        let hi = Place::new(0, 0, 5);
        assert_eq!(Range::new(lo, hi), Range::new(hi, lo));
        assert_eq!(Range::new(hi, lo).begin(), lo);
        assert_eq!(Range::new(hi, lo).end(), hi);
    }

    #[test]
    fn an_empty_range_covers_nothing() {
        assert!(Range::empty_at(Place::start()).is_empty());
        assert!(!Range::new(Place::start(), Place::new(0, 0, 0)).is_empty());
    }
}
