//! Positions in a laid-out text, and spans between them.
//!
//! **The caret invariant.** A [`Place`] names the position **after**
//! character `word` of line `line` of section `section`, and `word == -1` is
//! the *line header*, before that line's first character. Reading it as "at
//! character `word`" is the easiest way to get this module wrong.
//!
//! The type is the layout engine's ([`pdfrum_doc::vt::hit::Place`]),
//! re-exported rather than mirrored. What this module adds is the vocabulary
//! the *editor* needs on top: the ordered [`Range`] and the predicates in
//! [`PlaceExt`].
//!
//! ```
//! use pdfrum_form::edit::{Place, PlaceExt, Range};
//!
//! // The header of the first line: before its first character, not at it.
//! assert!(Place::start().at_line_start());
//! assert_eq!(Place::start().word, None);
//!
//! // `word == Some(0)` is *after* the first character, and sorts later.
//! assert!(Place::start() < Place::new(0, 0, Some(0)));
//!
//! // A span between two places is ordered however it is built.
//! let span = Range::new(Place::new(0, 0, Some(5)), Place::new(0, 0, Some(1)));
//! assert_eq!(span.begin(), Place::new(0, 0, Some(1)));
//! ```

pub use pdfrum_doc::vt::hit::Place;

/// The editor's own questions about a place.
///
/// An extension trait because the type is the layout engine's: these are
/// conveniences for the editing machinery, not part of what a place *is*.
///
/// ```
/// use pdfrum_form::edit::{Place, PlaceExt};
///
/// // Places order lexicographically over (section, line, word), and the
/// // header's `None` sorts before every character on its line.
/// assert!(Place::new(0, 0, None) < Place::new(0, 0, Some(0)));
/// assert!(Place::new(0, 0, Some(9)) < Place::new(0, 1, None));
/// assert!(Place::new(0, 9, Some(9)) < Place::new(1, 0, None));
/// ```
pub trait PlaceExt {
    /// The first place of any layout: the header of its first line.
    ///
    /// ```
    /// use pdfrum_form::edit::{Place, PlaceExt};
    ///
    /// assert_eq!(Place::start(), Place::new(0, 0, None));
    /// assert!(Place::start().at_line_start());
    /// ```
    #[must_use]
    fn start() -> Place;

    /// Whether this place sits before its line's first character.
    ///
    /// ```
    /// use pdfrum_form::edit::{Place, PlaceExt};
    ///
    /// assert!(Place::new(0, 0, None).at_line_start());
    /// // `Some(0)` is *after* the first character, so this is not the header.
    /// assert!(!Place::new(0, 0, Some(0)).at_line_start());
    /// ```
    #[must_use]
    fn at_line_start(self) -> bool;

    /// Whether two places are on the same line, ignoring the character.
    ///
    /// The comparison movement and line selection need, and the one a derived
    /// equality cannot give.
    ///
    /// ```
    /// use pdfrum_form::edit::{Place, PlaceExt};
    ///
    /// assert!(Place::new(1, 2, Some(0)).same_line(Place::new(1, 2, Some(7))));
    /// assert!(!Place::new(1, 2, Some(0)).same_line(Place::new(1, 3, Some(0))));
    /// assert!(!Place::new(1, 2, Some(0)).same_line(Place::new(2, 2, Some(0))));
    /// ```
    #[must_use]
    fn same_line(self, other: Place) -> bool;
}

impl PlaceExt for Place {
    fn start() -> Place {
        Place::new(0, 0, None)
    }

    fn at_line_start(self) -> bool {
        self.word.is_none()
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
///
/// ```
/// use pdfrum_form::edit::{Place, Range};
///
/// let lo = Place::new(0, 0, Some(1));
/// let hi = Place::new(0, 0, Some(5));
/// // Whichever order the two arrive in, the span is the same.
/// assert_eq!(Range::new(lo, hi), Range::new(hi, lo));
/// assert_eq!(Range::new(hi, lo).begin(), lo);
/// assert_eq!(Range::new(hi, lo).end(), hi);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Range {
    begin: Place,
    end: Place,
}

impl Range {
    /// The span between two places, in whichever order they arrive.
    ///
    /// ```
    /// use pdfrum_form::edit::{Place, Range};
    ///
    /// let backwards = Range::new(Place::new(0, 0, Some(7)), Place::new(0, 0, Some(2)));
    /// assert!(backwards.begin() <= backwards.end());
    /// assert_eq!(backwards.begin(), Place::new(0, 0, Some(2)));
    /// ```
    #[must_use]
    pub fn new(a: Place, b: Place) -> Range {
        if a <= b {
            Range { begin: a, end: b }
        } else {
            Range { begin: b, end: a }
        }
    }

    /// An empty span at one place.
    ///
    /// ```
    /// use pdfrum_form::edit::{Place, PlaceExt, Range};
    ///
    /// assert!(Range::empty_at(Place::start()).is_empty());
    /// ```
    #[must_use]
    pub fn empty_at(place: Place) -> Range {
        Range {
            begin: place,
            end: place,
        }
    }

    /// The earlier end.
    ///
    /// ```
    /// use pdfrum_form::edit::{Place, Range};
    ///
    /// let span = Range::new(Place::new(0, 0, Some(5)), Place::new(0, 0, Some(1)));
    /// assert_eq!(span.begin(), Place::new(0, 0, Some(1)));
    /// ```
    #[must_use]
    pub fn begin(self) -> Place {
        self.begin
    }

    /// The later end.
    ///
    /// ```
    /// use pdfrum_form::edit::{Place, Range};
    ///
    /// let span = Range::new(Place::new(0, 0, Some(5)), Place::new(0, 0, Some(1)));
    /// assert_eq!(span.end(), Place::new(0, 0, Some(5)));
    /// ```
    #[must_use]
    pub fn end(self) -> Place {
        self.end
    }

    /// Whether the span covers nothing.
    ///
    /// ```
    /// use pdfrum_form::edit::{Place, PlaceExt, Range};
    ///
    /// assert!(Range::empty_at(Place::start()).is_empty());
    /// assert!(!Range::new(Place::start(), Place::new(0, 0, Some(0))).is_empty());
    /// ```
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
        // `None < Some(0)` is what the old `word: -1` bought by being
        // negative, and `Option`'s derived `Ord` gives it for free — which is
        // the whole argument for the replacement.
        assert!(Place::new(0, 0, None) < Place::new(0, 0, Some(0)));
        assert!(Place::new(0, 0, Some(9)) < Place::new(0, 1, None));
        assert!(Place::new(0, 9, Some(9)) < Place::new(1, 0, None));
    }

    #[test]
    fn the_start_is_before_the_first_character() {
        assert!(Place::start().at_line_start());
        assert_eq!(Place::start().word, None);
        assert!(!Place::new(0, 0, Some(0)).at_line_start());
    }

    #[test]
    fn same_line_ignores_the_character() {
        assert!(Place::new(1, 2, Some(0)).same_line(Place::new(1, 2, Some(7))));
        assert!(!Place::new(1, 2, Some(0)).same_line(Place::new(1, 3, Some(0))));
        assert!(!Place::new(1, 2, Some(0)).same_line(Place::new(2, 2, Some(0))));
    }

    #[test]
    fn a_range_normalizes_whichever_way_it_is_built() {
        let lo = Place::new(0, 0, Some(1));
        let hi = Place::new(0, 0, Some(5));
        assert_eq!(Range::new(lo, hi), Range::new(hi, lo));
        assert_eq!(Range::new(hi, lo).begin(), lo);
        assert_eq!(Range::new(hi, lo).end(), hi);
    }

    #[test]
    fn an_empty_range_covers_nothing() {
        assert!(Range::empty_at(Place::start()).is_empty());
        assert!(!Range::new(Place::start(), Place::new(0, 0, Some(0))).is_empty());
    }
}
