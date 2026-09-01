//! Positions in a laid-out piece of text.
//!
//! # The caret invariant
//!
//! A [`Place`] names the position **after** word `word` of line `line` of
//! section `section`, and `word == -1` means *before the first word* of that
//! line. Reading it as "at word `word`" is the single easiest way to get this
//! module wrong: it is why an insert at the very start of a field targets
//! `word == -1`, why backspace removes the word the caret names, and why
//! forward delete has to step to the next place first.
//!
//! Ordering is lexicographic over the triple. A [`Range`] built from two
//! places normalizes, which is where a selection's direction is discarded —
//! so direction has to be preserved by whatever holds the two places, not by
//! the range.

/// A position in a laid-out text: after word `word` of line `line` of section
/// `section`.
///
/// `word == -1` means before that line's first word, which is the only
/// position an empty line has.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Place {
    /// Which paragraph.
    pub section: u32,
    /// Which line of it.
    pub line: u32,
    /// Which word of that line the caret sits after, or `-1` for before the
    /// first.
    pub word: i32,
}

impl Place {
    /// The start of a text: before the first word of the first line.
    pub const START: Place = Place {
        section: 0,
        line: 0,
        word: -1,
    };

    /// A place at the given coordinates.
    #[must_use]
    pub fn new(section: u32, line: u32, word: i32) -> Place {
        Place {
            section,
            line,
            word,
        }
    }

    /// Whether this place sits before its line's first word.
    #[must_use]
    pub fn at_line_start(self) -> bool {
        self.word < 0
    }

    /// Whether two places are on the same line, ignoring the word.
    ///
    /// The comparison movement and line selection need, and the one a derived
    /// `PartialEq` cannot give.
    #[must_use]
    pub fn same_line(self, other: Place) -> bool {
        self.section == other.section && self.line == other.line
    }
}

impl Default for Place {
    fn default() -> Place {
        Place::START
    }
}

/// An ordered span between two places, `begin <= end`.
///
/// Construction normalizes, so a `Range` can never be backwards. That is the
/// point: every consumer of a span wants it ordered, and the one thing that
/// wants direction — a selection — keeps its two places itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
    fn the_start_is_before_the_first_word() {
        assert!(Place::START.at_line_start());
        assert_eq!(Place::START.word, -1);
        assert!(!Place::new(0, 0, 0).at_line_start());
    }

    #[test]
    fn same_line_ignores_the_word() {
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
        assert!(Range::empty_at(Place::START).is_empty());
        assert!(!Range::new(Place::START, Place::new(0, 0, 0)).is_empty());
    }
}
