//! Line buffer accumulating characters before flushing to output.
//!
//! Handles space collapsing, right-to-left segment reversal, and character
//! normalization.

// # Why a paired buffer is a type
//
// Extraction keeps two parallel outputs — a character list and a text
// buffer — and they hold **different characters**. Within a line they stay
// in lockstep, one text unit per character record; it is only at the moment
// a line closes that they diverge, because a character the text buffer drops
// is still pushed to the character list. In the C++ that lockstep is a
// convention: three sites push to both containers, two pop both, one
// reverses both. `Line` makes it an invariant instead, so the two cannot
// drift apart by accident. (`docs/design/pdfrum-text.md` §1.10, §1.10b.)
//
// The staging text is `Vec<u32>`, not a `String`: it legitimately holds the
// `0xFFFE` charcode-zero placeholder and lone zeroes that the final string
// will not contain. That placeholder is private and dies here — its record is
// never `normal`, so it is dropped before the buffer a caller reads, which is
// the shape §C.1 of `docs/design/idiomatic-api.md` permits a sentinel to keep.

use crate::bidi::{self, Direction};
use crate::charinfo::{CharBox, CharType};
use crate::unicode::{mirror_char, normalize, normalize_space};

/// The two staging buffers, kept one-to-one.
#[derive(Debug, Clone, Default)]
pub struct Line {
    text: Vec<u32>,
    chars: Vec<CharBox>,
}

impl Line {
    /// How many characters the line holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.chars.len()
    }

    /// Whether the line is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.chars.is_empty()
    }

    /// The staged text units, which are **not** the staged characters'
    /// unicodes: a character-code-zero placeholder stages `U+FFFE` while its
    /// record keeps zero, and the hyphen path deliberately writes different
    /// values into the two. Only the hyphen's reaches a caller; the
    /// placeholder is filtered out when the line closes.
    #[must_use]
    pub fn text(&self) -> &[u32] {
        &self.text
    }

    /// The last text unit, if any.
    #[must_use]
    pub fn last_unit(&self) -> Option<u32> {
        self.text.last().copied()
    }

    /// The last character record, if any.
    #[must_use]
    pub fn last_char(&self) -> Option<&CharBox> {
        self.chars.last()
    }

    /// The last character record, mutably.
    pub fn last_char_mut(&mut self) -> Option<&mut CharBox> {
        self.chars.last_mut()
    }

    /// Appends one character to both buffers.
    pub fn push(&mut self, unit: u32, info: CharBox) {
        self.text.push(unit);
        self.chars.push(info);
    }

    /// Removes the last character from both buffers.
    pub fn pop(&mut self) {
        self.text.pop();
        self.chars.pop();
    }

    /// Replaces the last text unit without touching its character record.
    ///
    /// The one place the two are *meant* to disagree: the hyphen path writes
    /// `0x0002` into the record and `U+00AD` into the text, and both readings
    /// of that position are observable.
    pub fn set_last_unit(&mut self, unit: u32) {
        if let Some(last) = self.text.last_mut() {
            *last = unit;
        }
    }

    /// Reverses everything from `index` onwards, in both buffers together.
    ///
    /// This is what a right-to-left text object does to its own characters
    /// after emitting them (`ReverseTempTextBufs`). A no-op when the object
    /// emitted nothing.
    pub fn reverse_from(&mut self, index: usize) {
        if let Some(tail) = self.text.get_mut(index..) {
            tail.reverse();
        }
        if let Some(tail) = self.chars.get_mut(index..) {
            tail.reverse();
        }
    }

    /// Collapses each run of spaces to its first, in both buffers.
    ///
    /// Runs on the *staging* buffers only, so a run of spaces split across a
    /// line boundary is not collapsed — which is exactly the C++'s behaviour
    /// and is observable wherever a line break lands between two spaces.
    pub fn collapse_spaces(&mut self) {
        let mut previous_was_space = false;
        let mut index = 0;
        while index < self.text.len() {
            let is_space = self.text.get(index) == Some(&u32::from(b' '));
            if !is_space {
                previous_was_space = false;
                index += 1;
                continue;
            }
            if previous_was_space {
                self.text.remove(index);
                self.chars.remove(index);
                // Stay put: the next character has slid into this slot.
                continue;
            }
            previous_was_space = true;
            index += 1;
        }
    }

    /// Empties the line, returning what it held.
    pub fn take(&mut self) -> (Vec<u32>, Vec<CharBox>) {
        (
            std::mem::take(&mut self.text),
            std::mem::take(&mut self.chars),
        )
    }
}

/// Where a closed line's characters go.
#[derive(Debug, Default)]
pub struct Output {
    /// The character list — **the `--txt` stream**.
    pub chars: Vec<CharBox>,
    /// The search-facing text, which holds a different sequence.
    pub text: Vec<u32>,
}

/// Closes a line: collapse spaces, segment by direction, and move every
/// character into the final output (`CloseTempLine`).
///
/// Right-to-left segments are emitted **backwards**, so the characters land
/// in logical order in a buffer that was built in visual order. A segment
/// whose first character came from `/ActualText` is the exception: it is
/// emitted forwards, because the `/ActualText` string was already logical.
///
/// `rtl` is the document's `/ViewerPreferences /Direction (R2L)` flag, and
/// is the *only* thing that flips a whole line — the auto-order heuristic is
/// deliberately not run here.
pub fn close(line: &mut Line, out: &mut Output, rtl: bool) {
    if line.is_empty() {
        return;
    }
    line.collapse_spaces();
    let (text, chars) = line.take();

    let mut segmented = bidi::segments(&text, false);
    if rtl {
        segmented.set_right();
    }
    let mut current = segmented.overall();

    for segment in segmented.segments() {
        let range = segment.start..segment.start.saturating_add(segment.count);
        let (Some(units), Some(infos)) = (text.get(range.clone()), chars.get(range)) else {
            continue;
        };
        let is_right = segment.direction == Direction::Right
            || (segment.direction == Direction::Neutral && current == Direction::Right);
        if is_right {
            current = Direction::Right;
            // An `/ActualText` run is already in logical order; reversing it
            // would undo the very thing the mark was there to state.
            let actual_text = infos
                .first()
                .is_some_and(|info| info.char_type == CharType::ActualText);
            if actual_text {
                for (unit, info) in units.iter().zip(infos) {
                    add(*unit, *info, true, out);
                }
            } else {
                for (unit, info) in units.iter().zip(infos).rev() {
                    add(*unit, *info, true, out);
                }
            }
        } else {
            // A weak segment does not reset the direction; a real left one
            // does.
            if segment.direction != Direction::LeftWeak {
                current = Direction::Left;
            }
            for (unit, info) in units.iter().zip(infos) {
                add(*unit, *info, false, out);
            }
        }
    }
}

/// Pushes one character into the final output (`AddCharInfo`).
///
/// This is where the two outputs part company, three ways:
///
/// 1. A character the text buffer does not want — a control character, or the
///    charcode-zero placeholder — still lands in the character list, and so
///    still appears in `--txt`.
/// 2. Only a right-to-left character has its record's unicode rewritten from
///    the mirrored, normalized text; a left-to-right one keeps whatever it
///    was constructed with. That is what carries the hyphen sentinel's
///    `0x0002` through while the text buffer receives `U+00AD`. The one
///    exception is `[oracle-bug]` A40b's space normalization, which rewrites
///    the record in either direction precisely so the two outputs *cannot*
///    disagree about a space.
/// 3. Normalization multiplies one character record into several, all sharing
///    one box, origin and matrix, and retypes them as pieces.
fn add(unit: u32, info: CharBox, is_rtl: bool, out: &mut Output) {
    if !info.is_normal() {
        out.chars.push(info);
        return;
    }
    let unit = if is_rtl { mirror_char(unit) } else { unit };
    // `[oracle-bug]` The NFKC space normalization, applied to **every**
    // character rather than only inside a right-to-left run. `AddCharInfo`
    // (`cpdf_textpage.cpp:793-795`) consults `GetUnicodeNormalization` — whose
    // table maps `U+00A0` to `U+0020` — only when `is_rtl` or the code point
    // is in the `U+FB00..=U+FB06` band, so the same NO-BREAK SPACE comes out
    // as a plain space in a Hebrew run and as `U+00A0` in a Latin one. pdf.js
    // normalises every extracted chunk (`src/shared/util.js:1050-1065`,
    // applied at `src/core/evaluator.js:2685-2689` with
    // `disableNormalization` defaulting to `false` at `:2403`), so both
    // implementations emit `U+0020`; only the route differs. See
    // [`normalize_space`](crate::unicode::normalize_space) for why this is the
    // thirteen space code points and not PDFium's whole normalization table,
    // which is not NFKC and would strip every accent on the page.
    let normalized_unit = normalize_space(unit);
    let space_normalized = normalized_unit != unit;
    let unit = normalized_unit;
    // Latin ligatures decompose unconditionally; everything else only inside
    // a right-to-left run.
    let normalized = if is_rtl || (0xFB00..=0xFB06).contains(&unit) {
        normalize(unit)
    } else {
        Vec::new()
    };

    let mut modified = info;
    // Empty means "normalization was not consulted at all". A consulted
    // lookup always answers with at least the character itself, so a
    // right-to-left character is retyped as a piece even when nothing about
    // it changed — which is why `CharType::Piece` shows up on ordinary Hebrew
    // letters and why the hyphen look-back accepts it.
    if normalized.is_empty() {
        out.text.push(unit);
        // `[oracle-bug]` The character *record* carries the normalized space
        // too. `--txt` writes the char list, not the search-facing text
        // (`cpdf_textpage.cpp:797-800` sets `modified_info.set_unicode` only
        // under `is_rtl`), so normalizing the text alone would leave the two
        // outputs disagreeing about the same character — which is the very
        // split this item is closing.
        if is_rtl || space_normalized {
            modified.unicode = unit;
        }
        out.chars.push(modified);
        return;
    }
    modified.char_type = CharType::Piece;
    for piece in normalized {
        modified.unicode = piece;
        out.text.push(piece);
        out.chars.push(modified);
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
    use kurbo::{Affine, Point, Rect};

    fn info(unicode: u32) -> CharBox {
        CharBox {
            char_type: CharType::Normal,
            unicode,
            code: Some(pdfrum_font::CharCode(unicode)),
            origin: Point::ZERO,
            char_box: Rect::ZERO,
            loose_char_box: Rect::ZERO,
            matrix: Affine::IDENTITY,
            object: None,
            font_size: 1.0,
            angle: 0.0,
        }
    }

    fn staged(text: &str) -> Line {
        let mut line = Line::default();
        for ch in text.chars() {
            line.push(u32::from(ch), info(u32::from(ch)));
        }
        line
    }

    fn rendered(out: &Output) -> String {
        out.text.iter().filter_map(|u| char::from_u32(*u)).collect()
    }

    fn char_units(out: &Output) -> Vec<u32> {
        out.chars.iter().map(|c| c.unicode).collect()
    }

    #[test]
    fn space_runs_collapse_to_their_first() {
        let mut line = staged("a  b   c");
        line.collapse_spaces();
        let (text, chars) = line.take();
        assert_eq!(
            text.iter()
                .filter_map(|u| char::from_u32(*u))
                .collect::<String>(),
            "a b c"
        );
        // Both buffers shrink together.
        assert_eq!(chars.len(), text.len());
    }

    #[test]
    fn a_leading_or_trailing_space_run_collapses_too() {
        let mut line = staged("   a   ");
        line.collapse_spaces();
        let (text, _) = line.take();
        assert_eq!(
            text.iter()
                .filter_map(|u| char::from_u32(*u))
                .collect::<String>(),
            " a "
        );
    }

    #[test]
    fn a_space_run_split_across_a_line_boundary_is_not_collapsed() {
        // The collapse runs on the staging buffers, so two lines each ending
        // and starting with a space keep both.
        let mut out = Output::default();
        let mut line = staged("a ");
        close(&mut line, &mut out, false);
        let mut line = staged(" b");
        close(&mut line, &mut out, false);
        assert_eq!(rendered(&out), "a  b");
    }

    #[test]
    fn reversing_from_an_index_moves_both_buffers_together() {
        let mut line = staged("abcde");
        line.reverse_from(2);
        let (text, chars) = line.take();
        let as_string: String = text.iter().filter_map(|u| char::from_u32(*u)).collect();
        assert_eq!(as_string, "abedc");
        let from_chars: String = chars
            .iter()
            .filter_map(|c| char::from_u32(c.unicode))
            .collect();
        assert_eq!(from_chars, as_string);
        // Past the end is a no-op, not a panic.
        let mut line = staged("ab");
        line.reverse_from(9);
        assert_eq!(line.len(), 2);
    }

    #[test]
    fn a_left_to_right_line_passes_straight_through() {
        let mut out = Output::default();
        close(&mut staged("hello"), &mut out, false);
        assert_eq!(rendered(&out), "hello");
        assert_eq!(out.chars.len(), 5);
    }

    #[test]
    fn a_right_to_left_segment_comes_out_in_logical_order() {
        // Hebrew, built in visual order, read back logically.
        let mut out = Output::default();
        close(&mut staged("\u{05D0}\u{05D1}\u{05D2}"), &mut out, false);
        assert_eq!(rendered(&out), "\u{05D2}\u{05D1}\u{05D0}");
    }

    #[test]
    fn a_right_to_left_segment_mirrors_its_brackets() {
        let mut out = Output::default();
        // A Hebrew letter then a bracket. The bracket is a *neutral* segment
        // following a right-directional one, so it inherits the direction and
        // mirrors -- but the two are separate segments, and segment order is
        // not reversed here, only the characters within each one. So the
        // letter still comes first and the bracket comes back mirrored.
        close(&mut staged("\u{05D0}("), &mut out, false);
        assert_eq!(rendered(&out), "\u{05D0})");
    }

    #[test]
    fn a_control_character_reaches_the_char_list_but_not_the_text() {
        let mut line = Line::default();
        line.push(u32::from('a'), info(u32::from('a')));
        line.push(0x03, info(0x03));
        line.push(u32::from('b'), info(u32::from('b')));
        let mut out = Output::default();
        close(&mut line, &mut out, false);
        assert_eq!(rendered(&out), "ab");
        assert_eq!(char_units(&out), [u32::from('a'), 0x03, u32::from('b')]);
    }

    #[test]
    fn the_hyphen_splits_the_two_outputs() {
        // What ProcessGenerateCharacter leaves behind: the record says 0x2,
        // the staged text says U+00AD. The asymmetry is the point, and after
        // audit A41's buffer half it is an asymmetry of our own making — the
        // record keeps PDFium's 0x2, the buffer carries a real soft hyphen
        // where PDFium writes the U+FFFE noncharacter.
        let mut line = Line::default();
        line.push(u32::from('a'), info(u32::from('a')));
        let mut hyphen = info(0x02);
        hyphen.char_type = CharType::Hyphen;
        line.push(0x00AD, hyphen);
        line.push(u32::from('s'), info(u32::from('s')));

        let mut out = Output::default();
        close(&mut line, &mut out, false);
        // The text keeps the soft hyphen; the character list keeps 0x2.
        assert_eq!(out.text, [u32::from('a'), 0x00AD, u32::from('s')]);
        assert_eq!(char_units(&out), [u32::from('a'), 0x02, u32::from('s')]);
        // And what lands in the buffer is a real character, which is the
        // whole of A41's buffer half.
        assert!(char::from_u32(0x00AD).is_some());
    }

    #[test]
    fn a_latin_ligature_normalizes_even_left_to_right() {
        let mut out = Output::default();
        close(&mut staged("a\u{FB01}b"), &mut out, false);
        assert_eq!(rendered(&out), "afib");
        // The two pieces share one record's geometry and are retyped.
        assert_eq!(out.chars.len(), 4);
        assert_eq!(out.chars[1].char_type, CharType::Piece);
        assert_eq!(out.chars[2].char_type, CharType::Piece);
    }

    // Audit item **A40b**. This asserted `"a\u{00A0}b"`, reproducing
    // `cpdf_textpage.cpp:793-795`'s gate: `GetUnicodeNormalization` maps
    // `U+00A0` to `U+0020` but is consulted only inside a right-to-left run,
    // so the same character came out two ways on the same page. pdf.js
    // NFKC-normalises every chunk, so the space is a space in either
    // direction now.
    #[test]
    fn a_no_break_space_normalizes_in_either_direction() {
        let mut out = Output::default();
        close(&mut staged("a\u{00A0}b"), &mut out, false);
        assert_eq!(rendered(&out), "a b");
        // Not retyped as a piece: the space normalization is a substitution,
        // not a decomposition, so the record stays a normal character.
        assert_eq!(out.chars[1].char_type, CharType::Normal);
        // And the character *record* carries it too — `--txt` writes the char
        // list, not the search-facing text.
        assert_eq!(out.chars[1].unicode, 0x0020);
    }

    /// Audit item **A40b**, the other half: this is not PDFium's whole
    /// normalization table, which is not NFKC and would strip the accent.
    #[test]
    fn an_accented_letter_is_not_normalized_left_to_right() {
        let mut out = Output::default();
        close(&mut staged("a\u{00C0}b"), &mut out, false);
        assert_eq!(rendered(&out), "a\u{00C0}b");
        assert_eq!(out.chars[1].unicode, 0x00C0);
    }

    #[test]
    fn the_r2l_preference_flips_a_whole_line() {
        let mut out = Output::default();
        close(&mut staged("ab"), &mut out, true);
        // A pure-Latin line under an R2L preference has its one left segment
        // emitted as-is, but the overall direction starts Right, so the
        // leading zero-count neutral segment counts as right-directional.
        assert_eq!(rendered(&out), "ab");
    }

    #[test]
    fn an_empty_line_closes_to_nothing() {
        let mut out = Output::default();
        close(&mut Line::default(), &mut out, false);
        assert!(out.chars.is_empty() && out.text.is_empty());
    }
}
