//! The four-way direction segmenter.
//!
//! **This is not the Unicode Bidirectional Algorithm.** PDFium buckets raw
//! bidi classes four ways, splits a line wherever the bucket changes, and
//! reverses the characters inside a right-to-left run. There are no embedding
//! levels, no paragraph resolution and no explicit-direction handling; the
//! only mirroring is a single table lookup, applied later. Feeding the same
//! text through a real UBA implementation gives different output, which is
//! why this crate does not use `unicode-bidi` (design brief D1).

use crate::unicode::{BidiClass, bidi_class};

/// One of the four buckets a character falls into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    /// The initial state, and everything the other three do not claim:
    /// `ON`, `S`, `WS`, `B` and the five explicit-direction controls.
    Neutral,
    /// Bidi class `L`.
    Left,
    /// Bidi classes `R` and `AL`.
    Right,
    /// The number-ish and mark classes: `AN`, `EN`, `NSM`, `CS`, `ES`, `ET`,
    /// `BN`. Weak enough that a run of them does not reset the overall
    /// direction.
    LeftWeak,
}

impl Direction {
    /// The bucket a code point falls into.
    #[must_use]
    pub fn of(code: u32) -> Self {
        match bidi_class(code) {
            BidiClass::L => Self::Left,
            BidiClass::An
            | BidiClass::En
            | BidiClass::Nsm
            | BidiClass::Cs
            | BidiClass::Es
            | BidiClass::Et
            | BidiClass::Bn => Self::LeftWeak,
            BidiClass::R | BidiClass::Al => Self::Right,
            BidiClass::On
            | BidiClass::S
            | BidiClass::Ws
            | BidiClass::B
            | BidiClass::Rlo
            | BidiClass::Rle
            | BidiClass::Lro
            | BidiClass::Lre
            | BidiClass::Pdf => Self::Neutral,
        }
    }
}

/// A maximal run of characters sharing a direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Segment {
    /// Index of the first character, into the string that was segmented.
    pub start: usize,
    /// How many characters the run holds. **May be zero**: see [`segments`].
    pub count: usize,
    /// The run's direction.
    pub direction: Direction,
}

/// A segmented line, with the overall direction the caller reads off it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BidiLine {
    segments: Vec<Segment>,
    overall: Direction,
}

impl BidiLine {
    /// The segments, in the order they are to be emitted.
    #[must_use]
    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }

    /// The line's overall direction, always [`Direction::Left`] or
    /// [`Direction::Right`].
    #[must_use]
    pub fn overall(&self) -> Direction {
        self.overall
    }

    /// Force the whole line right-to-left, **reversing the segment order**.
    ///
    /// This is the only thing `/ViewerPreferences /Direction (R2L)` does, and
    /// the only thing the auto-order heuristic does. Idempotent.
    pub fn set_right(&mut self) {
        if self.overall != Direction::Right {
            self.segments.reverse();
            self.overall = Direction::Right;
        }
    }
}

/// Segments a line of code points.
///
/// `auto_order` reproduces the C++'s heuristic: flip the whole line
/// right-to-left when right-directional segments **strictly outnumber**
/// left-directional ones — weak and neutral segments counting for neither.
/// `CloseTempLine` passes `false` (only the explicit R2L preference flips a
/// line) while `IsRightToLeft` passes `true`.
///
/// One structural quirk is reproduced deliberately: unless the first
/// character is itself neutral, the first segment emitted is a **zero-count
/// neutral segment at index 0**, because the segmenter's initial state is
/// neutral and it pushes the *completed* segment on every change. It produces
/// empty spans downstream and is a no-op there, but it is counted by the
/// auto-order heuristic, so dropping it would change which lines flip.
#[must_use]
pub fn segments(codes: &[u32], auto_order: bool) -> BidiLine {
    let mut out: Vec<Segment> = Vec::new();
    let mut current = Segment {
        start: 0,
        count: 0,
        direction: Direction::Neutral,
    };
    let start_new = |current: &mut Segment, direction: Direction| -> Segment {
        let completed = *current;
        current.start += current.count;
        current.count = 0;
        current.direction = direction;
        completed
    };
    for &code in codes {
        let direction = Direction::of(code);
        if direction != current.direction {
            out.push(start_new(&mut current, direction));
        }
        current.count += 1;
    }
    // `EndChar` flushes the final run, and only when it holds something.
    let last = start_new(&mut current, Direction::Neutral);
    if last.count > 0 {
        out.push(last);
    }

    let mut line = BidiLine {
        segments: out,
        overall: Direction::Left,
    };
    if auto_order {
        let count = |want: Direction| line.segments.iter().filter(|s| s.direction == want).count();
        if count(Direction::Right) > count(Direction::Left) {
            line.set_right();
        }
    }
    line
}

/// Whether a run of code points reads right-to-left overall
/// (`IsRightToLeft`).
///
/// Runs the auto-order heuristic, so this is "more right-directional segments
/// than left-directional ones" rather than any statement about the majority
/// of *characters*.
#[must_use]
pub fn is_right_to_left(codes: &[u32]) -> bool {
    segments(codes, true).overall() == Direction::Right
}

#[cfg(test)]
mod tests {
    // Test fixtures quote the oracle's own vectors, compare floats exactly
    // where the behaviour being pinned is exact, and index arrays whose
    // length the fixture itself fixes.
    #![allow(
        clippy::float_cmp,
        clippy::indexing_slicing,
        clippy::unreadable_literal,
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        reason = "test fixtures quote oracle vectors verbatim and compare exactly"
    )]

    use super::*;

    fn codes(text: &str) -> Vec<u32> {
        text.chars().map(u32::from).collect()
    }

    #[test]
    fn characters_land_in_the_four_buckets() {
        assert_eq!(Direction::of(u32::from('a')), Direction::Left);
        assert_eq!(Direction::of(0x05D0), Direction::Right); // HEBREW ALEF
        assert_eq!(Direction::of(0x0627), Direction::Right); // ARABIC ALEF, class AL
        assert_eq!(Direction::of(u32::from('1')), Direction::LeftWeak); // EN
        assert_eq!(Direction::of(0x0660), Direction::LeftWeak); // AN
        assert_eq!(Direction::of(0x0300), Direction::LeftWeak); // NSM
        assert_eq!(Direction::of(u32::from(' ')), Direction::Neutral); // WS
        assert_eq!(Direction::of(u32::from('(')), Direction::Neutral); // ON
        assert_eq!(Direction::of(0x202B), Direction::Neutral); // RLE
    }

    #[test]
    fn a_leading_zero_count_neutral_segment_is_emitted() {
        // Unless the first character is itself neutral, in which case the
        // initial state already matches and nothing is pushed.
        let line = segments(&codes("abc"), false);
        assert_eq!(line.segments()[0].count, 0);
        assert_eq!(line.segments()[0].direction, Direction::Neutral);

        let line = segments(&codes(" abc"), false);
        // A leading space *is* neutral, so the first segment is the space run.
        assert_eq!(line.segments().len(), 2);
        assert_eq!(line.segments()[0].count, 1);
        assert_eq!(line.segments()[0].direction, Direction::Neutral);
        assert_eq!(line.segments()[1].count, 3);
    }

    #[test]
    fn segment_starts_and_counts_tile_the_input() {
        let text = codes("ab \u{05D0}\u{05D1}1x");
        let line = segments(&text, false);
        let mut at = 0;
        for segment in line.segments() {
            assert_eq!(segment.start, at, "{segment:?}");
            at += segment.count;
        }
        assert_eq!(at, text.len());
    }

    #[test]
    fn an_empty_line_produces_no_segments() {
        let line = segments(&[], false);
        assert!(line.segments().is_empty());
        assert_eq!(line.overall(), Direction::Left);
    }

    #[test]
    fn auto_order_flips_only_on_a_strict_majority() {
        // Two right runs, one left run: flips.
        let text = codes("\u{05D0} a \u{05D1}");
        assert!(is_right_to_left(&text));
        // One right run, one left run: a tie does not flip.
        let text = codes("\u{05D0} a");
        assert!(!is_right_to_left(&text));
        // Pure Latin never flips.
        assert!(!is_right_to_left(&codes("hello")));
        // Weak and neutral segments count for neither side.
        assert!(!is_right_to_left(&codes("123 ... 456")));
    }

    #[test]
    fn flipping_reverses_the_segment_order_and_is_idempotent() {
        let mut line = segments(&codes("ab\u{05D0}"), false);
        let before: Vec<Segment> = line.segments().to_vec();
        line.set_right();
        let reversed: Vec<Segment> = line.segments().to_vec();
        assert_eq!(reversed, before.iter().rev().copied().collect::<Vec<_>>());
        // A second call changes nothing.
        line.set_right();
        assert_eq!(line.segments(), reversed.as_slice());
        assert_eq!(line.overall(), Direction::Right);
    }

    #[test]
    fn segmentation_splits_wherever_the_bucket_changes() {
        // Latin, space, Hebrew, digit: L, Neutral, Right, LeftWeak, plus the
        // leading zero-count neutral.
        let line = segments(&codes("a \u{05D0}1"), false);
        let kinds: Vec<Direction> = line.segments().iter().map(|s| s.direction).collect();
        assert_eq!(
            kinds,
            [
                Direction::Neutral,
                Direction::Left,
                Direction::Neutral,
                Direction::Right,
                Direction::LeftWeak,
            ]
        );
    }

    #[test]
    fn a_latin_line_resolves_left_with_a_leading_neutral_segment() {
        let latin: Vec<u32> = "abc".chars().map(u32::from).collect();
        let line = segments(&latin, false);
        assert_eq!(line.overall(), Direction::Left);
        // A leading zero-count neutral segment, then the run itself.
        assert_eq!(line.segments().len(), 2);
        assert_eq!(line.segments()[0].count, 0);
        assert_eq!(line.segments()[1].count, 3);
    }
}
