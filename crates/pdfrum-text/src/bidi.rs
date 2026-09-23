//! The four-way direction segmenter, and the reordering that replaces it on
//! a line with right-to-left text in it.
//!
//! **Neither is the Unicode Bidirectional Algorithm.** PDFium buckets raw
//! bidi classes four ways, splits a line wherever the bucket changes, and
//! reverses the characters inside a right-to-left run. There are no embedding
//! levels, no paragraph resolution and no explicit-direction handling; the
//! only mirroring is a single table lookup, applied later. That is still how
//! a line with no right-to-left letter is read, so every such line comes out
//! as the oracle's. A line with one is put in logical order by
//! [`logical_order`], which resolves levels the way UAX #9 does but on text
//! already laid out — the reverse of the problem `unicode-bidi` solves, which
//! is why this crate does not use it.

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

/// How a character takes part in reordering a line read back from the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Strength {
    Left,
    Right,
    /// A run of digits, with the separators and terminators that belong to
    /// it: `1,234`, `12.5`, `50%`.
    Number,
    Neutral,
}

/// A line's characters in logical order, from the visual order the page
/// placed them in — `None` when the line holds no right-to-left letter, and
/// the segmenter's order is left alone. A line with no letter of either
/// direction — `3/5.`, the end of a sentence wrapped alone — follows the one
/// before it, which `after_right` says was right-to-left.
///
/// Each entry is an index into `codes` and whether that character sits in a
/// right-to-left run; the flag beside them is whether the line itself reads
/// right to left, for the next line to follow. `joins_previous[i]` says
/// character `i` is part of the same unit as `i - 1`: an `/ActualText`
/// string, already logical, which moves as one piece and is never turned
/// round inside.
///
/// `[oracle-bug]` PDFium's segmenter reverses each right-to-left run in
/// place but keeps the runs in visual order, so a Hebrew line's words come
/// out last word first — `2024 זה חוק` for `חוק זה 2024` — and it turns an
/// `/ActualText` run forwards only when its *first* character came from one,
/// which Chrome, marking each Arabic letter separately, defeats on nearly
/// every word. This is the reordering of UAX #9 rule L2 run backwards. The
/// line's direction is right-to-left when `rtl` says so or when it holds more
/// right-to-left letters than left-to-right ones; a neutral takes the
/// direction its neighbours share, else the line's; a number is laid out
/// left to right inside right-to-left text. The levels are resolved on the
/// visual order, which is what the page gives, rather than on a logical one
/// nobody has: an embedding the page does not show cannot be recovered, and
/// a number between Latin and Hebrew text reads with the Latin before it.
#[must_use]
pub fn logical_order(
    codes: &[u32],
    joins_previous: &[bool],
    rtl: bool,
    after_right: bool,
) -> Option<(Vec<(usize, bool)>, bool)> {
    let mut strengths: Vec<Strength> = codes
        .iter()
        .map(|&code| match bidi_class(code) {
            BidiClass::L => Strength::Left,
            BidiClass::R | BidiClass::Al => Strength::Right,
            BidiClass::En | BidiClass::An => Strength::Number,
            _ => Strength::Neutral,
        })
        .collect();
    let rights = strengths.iter().filter(|s| **s == Strength::Right).count();
    let lefts = strengths.iter().filter(|s| **s == Strength::Left).count();
    if rights == 0 && !(lefts == 0 && after_right) {
        return None;
    }
    let line = if rtl || rights > lefts || lefts == 0 {
        Strength::Right
    } else {
        Strength::Left
    };
    absorb_number_punctuation(codes, &mut strengths);

    let resolved = resolve(&strengths, line);
    let base: u8 = u8::from(line == Strength::Right);
    let levels: Vec<u8> = resolved
        .iter()
        .map(|s| match s {
            Strength::Left => base * 2,
            Strength::Right => 1,
            Strength::Number => 2,
            Strength::Neutral => base,
        })
        .collect();

    Some((reorder(&levels, joins_previous), line == Strength::Right))
}

/// Each character's direction, numbers and neutrals settled against their
/// neighbours and the line's direction `line`.
fn resolve(strengths: &[Strength], line: Strength) -> Vec<Strength> {
    // In a left-to-right line a number read after Latin text is part of it;
    // one beside Hebrew or Arabic, or at the start of a line that goes on
    // right to left, is laid out inside that text.
    let nearest = |from: usize, step: isize| -> Option<Strength> {
        let mut at = from.checked_add_signed(step)?;
        loop {
            match strengths.get(at)? {
                Strength::Left => return Some(Strength::Left),
                Strength::Right => return Some(Strength::Right),
                _ => at = at.checked_add_signed(step)?,
            }
        }
    };
    let numbers: Vec<Strength> = (0..strengths.len())
        .map(|i| match strengths.get(i) {
            Some(Strength::Number) if line == Strength::Left => {
                match (nearest(i, -1), nearest(i, 1)) {
                    (Some(Strength::Left), _) | (None, None | Some(Strength::Left)) => {
                        Strength::Left
                    }
                    _ => Strength::Number,
                }
            }
            Some(s) => *s,
            None => Strength::Neutral,
        })
        .collect();

    // A neutral between two runs of one direction takes it, a number
    // counting as right-to-left; anything else takes the line's.
    let as_strong = |s: Strength| match s {
        Strength::Number => Some(Strength::Right),
        Strength::Neutral => None,
        s => Some(s),
    };
    let mut resolved = numbers.clone();
    let mut i = 0;
    while i < numbers.len() {
        if numbers.get(i) != Some(&Strength::Neutral) {
            i += 1;
            continue;
        }
        let start = i;
        while numbers.get(i) == Some(&Strength::Neutral) {
            i += 1;
        }
        let before = start
            .checked_sub(1)
            .and_then(|j| numbers.get(j).copied())
            .and_then(as_strong)
            .unwrap_or(line);
        let after = numbers.get(i).copied().and_then(as_strong).unwrap_or(line);
        let direction = if before == after { before } else { line };
        for slot in resolved.get_mut(start..i).into_iter().flatten() {
            *slot = direction;
        }
    }
    resolved
}

/// Rule L2 on `levels`, each `/ActualText` string one unit: the characters'
/// indices in the new order, each with whether it sits at an odd level.
fn reorder(levels: &[u8], joins_previous: &[bool]) -> Vec<(usize, bool)> {
    // The units, each a range of characters and the level of its first.
    let mut units: Vec<(usize, usize, u8)> = Vec::new();
    for (i, level) in levels.iter().enumerate() {
        match units.last_mut() {
            Some(unit) if joins_previous.get(i).copied().unwrap_or(false) => unit.1 = i + 1,
            _ => units.push((i, i + 1, *level)),
        }
    }
    // Rule L2 is an involution on the levels it reverses: from the highest
    // level down to one, every maximal run at or above it is turned round.
    let highest = units.iter().map(|unit| unit.2).max().unwrap_or(0);
    for level in (1..=highest).rev() {
        let mut i = 0;
        while i < units.len() {
            if units.get(i).is_some_and(|unit| unit.2 >= level) {
                let start = i;
                while units.get(i).is_some_and(|unit| unit.2 >= level) {
                    i += 1;
                }
                if let Some(run) = units.get_mut(start..i) {
                    run.reverse();
                }
            } else {
                i += 1;
            }
        }
    }
    units
        .into_iter()
        .flat_map(|(start, end, level)| (start..end).map(move |i| (i, level % 2 == 1)))
        .collect()
}

/// Folds into a number what belongs to it (rules W4 and W5): a single
/// separator between two digits, and a run of terminators beside one.
fn absorb_number_punctuation(codes: &[u32], strengths: &mut [Strength]) {
    let class = |i: usize| codes.get(i).map(|&code| bidi_class(code));
    let is_number = |strengths: &[Strength], i: Option<usize>| {
        i.and_then(|i| strengths.get(i)) == Some(&Strength::Number)
    };
    for i in 0..codes.len() {
        if matches!(class(i), Some(BidiClass::Cs | BidiClass::Es))
            && is_number(strengths, i.checked_sub(1))
            && is_number(strengths, Some(i + 1))
            && let Some(slot) = strengths.get_mut(i)
        {
            *slot = Strength::Number;
        }
    }
    let mut i = 0;
    while i < codes.len() {
        if class(i) != Some(BidiClass::Et) {
            i += 1;
            continue;
        }
        let start = i;
        while class(i) == Some(BidiClass::Et) {
            i += 1;
        }
        if is_number(strengths, start.checked_sub(1)) || is_number(strengths, Some(i)) {
            for slot in strengths.get_mut(start..i).into_iter().flatten() {
                *slot = Strength::Number;
            }
        }
    }
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
    fn reordering_the_reordered_line_gives_back_the_original() {
        // What lets a line laid out by L2 be read back by L2: the levels
        // travel with their characters, and reversing by them again undoes
        // the first pass.
        let joins = [false; 9];
        for levels in [
            vec![0, 0, 1, 1, 1, 0, 0],
            vec![1, 1, 2, 2, 1, 1, 1],
            vec![0, 1, 2, 2, 1, 0, 1, 2, 1],
            vec![2, 1, 0, 1, 2],
            vec![1],
        ] {
            let there: Vec<usize> = reorder(&levels, &joins).iter().map(|(i, _)| *i).collect();
            let laid_out: Vec<u8> = there.iter().map(|&i| levels[i]).collect();
            let back: Vec<usize> = reorder(&laid_out, &joins)
                .iter()
                .map(|(i, _)| there[*i])
                .collect();
            assert_eq!(back, (0..levels.len()).collect::<Vec<_>>(), "{levels:?}");
        }
    }

    #[test]
    fn a_line_with_no_right_to_left_letter_is_left_to_the_segmenter() {
        let joins = [false; 16];
        assert!(logical_order(&codes("plain text 2024"), &joins, false, false).is_none());
        // The document's R2L preference does not change which path a Latin
        // line takes; the segmenter answers it, as the oracle does.
        assert!(logical_order(&codes("plain text"), &joins, true, false).is_none());
        // Digits alone follow only a right-to-left line.
        assert!(logical_order(&codes("3/5."), &joins, false, false).is_none());
        assert!(logical_order(&codes("3/5."), &joins, false, true).is_some());
    }

    #[test]
    fn a_number_keeps_its_separators_and_its_sign() {
        let text = codes("20% 1,250.50");
        let mut strengths: Vec<Strength> = text
            .iter()
            .map(|&code| match bidi_class(code) {
                BidiClass::En | BidiClass::An => Strength::Number,
                _ => Strength::Neutral,
            })
            .collect();
        absorb_number_punctuation(&text, &mut strengths);
        let numbers: String = text
            .iter()
            .zip(&strengths)
            .map(|(&c, s)| {
                if *s == Strength::Number {
                    char::from_u32(c).unwrap_or('?')
                } else {
                    '_'
                }
            })
            .collect();
        assert_eq!(numbers, "20%_1,250.50");
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
