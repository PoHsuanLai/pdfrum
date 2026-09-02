//! Searching a page's text (`docs/design/pdfrum-text.md` §1.14).
//!
//! Searches run over the **search-facing text**, not over the character
//! stream `--txt` emits, so a match's offsets are text offsets and a caller
//! wanting character indices goes through [`IndexMap`](crate::index::IndexMap).
//!
//! # The needle is split, not matched whole
//!
//! A needle is first split into sub-needles at spaces *and* at every
//! character that is a "standalone searchable unit" — which is every
//! character outside Latin-1, the Arabic and Cyrillic blocks and General
//! Punctuation, so every CJK ideograph and every Devanagari letter becomes
//! its own sub-needle. The sub-needles must then appear in order, separated
//! in the text only by line breaks, spaces or non-breaking spaces. This is
//! what lets a search find text that reflowed across a line break, and what
//! lets a CJK needle match without the spaces a Latin one would need.

use crate::unicode::{is_decimal_digit, lower_string};
use std::ops::Range;

/// A non-breaking space, which counts as a separator between sub-needles.
const NON_BREAKING_SPACE: char = '\u{00A0}';

/// The soft hyphen the text buffer carries where a word was hyphenated across
/// a line break.
///
/// `U+00AD`, not the `U+FFFE` noncharacter `cpdf_textpage.cpp:1361`
/// (`AppendChar(0xfffe)`) writes: audit **A41**'s buffer half repairs it at
/// the source, in `pipeline`'s `SOFT_HYPHEN`. Dropping it from the haystack is
/// **A42**, and the two are independent — A42 would still be needed if the
/// buffer carried a plain `-`, because a query for the un-hyphenated word has
/// to match across the break either way.
const HYPHEN_SENTINEL: char = '\u{00AD}';

/// How a search behaves.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FindOptions {
    /// Compare case-sensitively. The default is **insensitive**, which is
    /// the C++'s default too.
    pub match_case: bool,
    /// Reject a match whose neighbours make it part of a longer word.
    pub match_whole_word: bool,
    /// Step forward by one character after a match rather than past it, so
    /// overlapping matches are all reported.
    pub consecutive: bool,
}

/// Whether a character is a standalone searchable unit that the needle is
/// split around (`IsIgnoreSpaceCharacter`).
///
/// The name in the C++ reads backwards: returning true means "split here".
/// It is false — do not split — for Latin-1 and a handful of script blocks
/// whose text is written with spaces between words, and true for everything
/// else.
#[must_use]
pub fn splits_needle(ch: char) -> bool {
    let code = u32::from(ch);
    // Note `< 255`, not `<= 255`.
    !(code < 255
        || (0x0600..=0x06FF).contains(&code)   // Arabic
        || (0xFE70..=0xFEFF).contains(&code)   // Arabic Presentation Forms-B
        || (0xFB50..=0xFDFF).contains(&code)   // Arabic Presentation Forms-A
        || (0x0400..=0x04FF).contains(&code)   // Cyrillic
        || (0x0500..=0x052F).contains(&code)   // Cyrillic Supplement
        || (0xA640..=0xA69F).contains(&code)   // Cyrillic Extended-B
        || (0x2DE0..=0x2DFF).contains(&code)   // Cyrillic Extended-A
        || code == 0x2113                      // SCRIPT SMALL L
        || (0x2000..=0x206F).contains(&code)) // General Punctuation
}

/// Whether a character can separate two sub-needles in the text.
fn is_separator(ch: char) -> bool {
    ch == '\n' || ch == ' ' || ch == '\r' || ch == NON_BREAKING_SPACE
}

/// The `iSubString`-th space-delimited token of a needle
/// (`ExtractSubString`).
///
/// Runs of spaces after a token are skipped, so `"a  b"` has `"b"` at index
/// one. A trailing space leaves an **empty** token at the next index, and the
/// index after that has none at all — which is what makes a needle ending in
/// a space behave differently from one that does not.
fn sub_string(needle: &[char], index: usize) -> Option<Vec<char>> {
    let mut at = 0usize;
    for _ in 0..index {
        // `wcschr` failing is what ends the walk.
        let space = needle.get(at..)?.iter().position(|ch| *ch == ' ')?;
        at += space + 1;
        while needle.get(at) == Some(&' ') {
            at += 1;
        }
    }
    let rest = needle.get(at..)?;
    let end = rest.iter().position(|ch| *ch == ' ').unwrap_or(rest.len());
    rest.get(..end).map(<[char]>::to_vec)
}

/// Splits a needle into the sub-needles a match must find in order
/// (`ExtractFindWhat`).
///
/// A needle that is entirely spaces (or empty) is *not* split: it comes back
/// as one element, which is what makes searching for `" "` mean "find a
/// space" rather than "find nothing".
#[must_use]
pub fn split_needle(needle: &str) -> Vec<String> {
    let chars: Vec<char> = needle.chars().collect();
    if chars.iter().all(|ch| *ch == ' ') {
        return vec![needle.to_owned()];
    }
    let mut out: Vec<String> = Vec::new();
    let mut index = 0usize;
    // The C++'s `while (true)` ends when `ExtractSubString` runs out; the
    // bound is belt-and-braces against a needle that somehow never does.
    while index <= chars.len() {
        let Some(mut word) = sub_string(&chars, index) else {
            break;
        };
        if word.is_empty() {
            out.push(String::new());
            index += 1;
            continue;
        }
        let mut pos = 0usize;
        while pos < word.len() {
            let Some(&current) = word.get(pos) else { break };
            if splits_needle(current) {
                // A right single quotation mark inside a word is an
                // apostrophe, not a split point.
                if pos > 0 && current == '\u{2019}' {
                    pos += 1;
                    continue;
                }
                if pos > 0 {
                    out.push(word.get(..pos).unwrap_or_default().iter().collect());
                }
                out.push(current.to_string());
                if pos == word.len() - 1 {
                    word.clear();
                    break;
                }
                word = word.get(pos + 1..).unwrap_or_default().to_vec();
                pos = 0;
                continue;
            }
            pos += 1;
        }
        if !word.is_empty() {
            out.push(word.iter().collect());
        }
        index += 1;
    }
    out
}

/// Whether a match's neighbours leave it standing as a whole word
/// (`IsMatchWholeWord`).
///
/// Two overlapping tests, both transcribed: the first uses **exclusive**
/// bounds, so `'A'`, `'a'`, `'z'`, `U+FB00` and `U+FB06` pass it and `'Z'`
/// does not; the second then rejects any ASCII letter properly. The redundancy
/// is harmless and the exclusive ligature band appears only in the first.
#[must_use]
pub fn is_whole_word(text: &[char], start: usize, end: usize) -> bool {
    if start > end {
        return false;
    }
    let count = end - start + 1;
    if count == 1
        && text
            .get(start)
            .copied()
            .is_some_and(|ch| u32::from(ch) > 255)
    {
        return true;
    }
    let at = |index: usize| -> u32 { text.get(index).copied().map_or(0, u32::from) };
    let left = if start >= 1 { at(start - 1) } else { 0 };
    let right = if start + count < text.len() {
        at(start + count)
    } else {
        0
    };
    let letterish = |ch: u32| {
        (ch > u32::from(b'A') && ch < u32::from(b'a'))
            || (ch > u32::from(b'a') && ch < u32::from(b'z'))
            || (ch > 0xFB00 && ch < 0xFB06)
            || is_decimal_digit(ch)
    };
    if letterish(left) || letterish(right) {
        return false;
    }
    let outside_ascii_letters = |ch: u32| {
        (u32::from(b'A') > ch || ch > u32::from(b'Z'))
            && (u32::from(b'a') > ch || ch > u32::from(b'z'))
    };
    if !(outside_ascii_letters(left) && outside_ascii_letters(right)) {
        return false;
    }
    if is_decimal_digit(left) && is_decimal_digit(at(start)) {
        return false;
    }
    if is_decimal_digit(right) && is_decimal_digit(at(end)) {
        return false;
    }
    true
}

/// A search in progress over one page's text.
///
/// Yields text-offset ranges. `Iterator` rather than the C++'s
/// find-next/find-previous pair: "previous" is a reverse walk over the same
/// sequence, so the second engine the C++ constructs is unnecessary (design
/// brief D7).
#[derive(Debug, Clone)]
pub struct Search<'a> {
    /// The haystack, case-folded when the search is insensitive, with the
    /// soft-hyphen sentinels removed — see [`search`].
    haystack: Vec<char>,
    /// `[oracle-bug]` For each haystack index, the text index it came from.
    /// Empty when nothing was removed, in which case the two spaces coincide.
    origins: Vec<usize>,
    /// The sub-needles, case-folded the same way.
    needles: Vec<Vec<char>>,
    options: FindOptions,
    /// Where the next scan starts, or `None` when the search is finished.
    next_start: Option<usize>,
    marker: std::marker::PhantomData<&'a ()>,
}

/// Builds a search over `text`.
///
/// `[oracle-bug]` **A word split across a line break is searched joined.**
/// `cpdf_textpage.cpp:1360-1361` writes `U+FFFE` into the text buffer at a
/// soft hyphen, and `cpdf_textpagefind.cpp:209-211`/`:262` search that buffer
/// with a plain `Find`, so `"note-\nbook"` can never match `"notebook"`
/// (`crbug.com/431824298`). What makes it a bug rather than a trade-off is the
/// **asymmetry**: `cpdf_linkextract.cpp:154-155` repairs the very same
/// sentinel (`Replace(L"\xfffe", L"-")`) for link detection and find does not.
/// pdf.js joins across the break and keeps a reversible index map so the
/// caller still gets offsets into the original text
/// (`pdf_find_controller.js:131`, `:290-307`, whose `p5.slice(0, -2)` drops
/// the hyphen *and* the newline). The same shape is used here: the sentinel is
/// dropped from the haystack and `origins` maps every haystack index back to
/// its text index, so the yielded ranges are still text offsets.
#[must_use]
pub fn search<'a>(text: &str, needle: &str, options: FindOptions) -> Search<'a> {
    let fold = |value: &str| -> String {
        if options.match_case {
            value.to_owned()
        } else {
            lower_string(value)
        }
    };
    let folded: Vec<char> = fold(text).chars().collect();
    let mut haystack: Vec<char> = Vec::with_capacity(folded.len());
    let mut origins: Vec<usize> = Vec::with_capacity(folded.len());
    let mut dropped = false;
    for (at, &ch) in folded.iter().enumerate() {
        if ch == HYPHEN_SENTINEL {
            dropped = true;
            continue;
        }
        haystack.push(ch);
        origins.push(at);
    }
    if !dropped {
        origins.clear();
    }
    let needles: Vec<Vec<char>> = split_needle(&fold(needle))
        .into_iter()
        .map(|word| word.chars().collect())
        .collect();
    Search {
        // An empty haystack never starts, which the C++ expresses by leaving
        // both cursors unset.
        next_start: (!haystack.is_empty()).then_some(0),
        haystack,
        origins,
        needles,
        options,
        marker: std::marker::PhantomData,
    }
}

impl Iterator for Search<'_> {
    type Item = Range<usize>;

    fn next(&mut self) -> Option<Range<usize>> {
        let start = self.next_start?;
        let (result_start, result_end) = self.scan(start)?;
        self.next_start = Some(if self.options.consecutive {
            result_start + 1
        } else {
            result_end + 1
        });
        // `[oracle-bug]` Back into text offsets. The end is inclusive here, so
        // the exclusive bound is its origin plus one — which is what keeps a
        // match that *spans* a dropped sentinel covering it in the text.
        Some(self.origin(result_start)..self.origin(result_end) + 1)
    }
}

impl Search<'_> {
    /// The text offset a haystack index came from.
    ///
    /// The identity when nothing was dropped, which is every page without a
    /// hyphenated line break.
    fn origin(&self, index: usize) -> usize {
        self.origins.get(index).copied().unwrap_or(index)
    }

    /// One scan, from `from`, returning the inclusive `(start, end)` of a
    /// match (`FindNext`).
    ///
    /// The restart is the awkward part: when a sub-needle matches in the
    /// wrong place the C++ sets its loop counter to `-1` so the increment
    /// makes it zero, restarting the whole walk from a start position that has
    /// advanced. Termination rests entirely on that advance, which happens
    /// because the first sub-needle has already matched once and set the
    /// result start. Reproduced with an explicit loop plus the same advance
    /// rule, and bounded so a needle whose first element is empty and whose
    /// second never matches cannot spin.
    fn scan(&mut self, from: usize) -> Option<(usize, usize)> {
        let length = self.haystack.len();
        if self.haystack.is_empty() || self.needles.is_empty() || from >= length {
            self.next_start = None;
            return None;
        }
        let mut start = from;
        let mut result_pos = 0usize;
        let mut result_start = 0usize;
        let mut space_start = false;
        let mut word = 0usize;
        // Every restart advances `start`, so `length + 1` restarts is more
        // than the walk can possibly need.
        let mut restarts_left = length + 1;

        while word < self.needles.len() {
            let Some(needle) = self.needles.get(word) else {
                break;
            };
            if needle.is_empty() {
                if word == self.needles.len() - 1 {
                    // A trailing empty sub-needle matches one separator.
                    let Some(&ch) = self.haystack.get(start) else {
                        self.next_start = None;
                        return None;
                    };
                    if is_separator(ch) {
                        result_pos = start + 1;
                        break;
                    }
                    // Restart, which cannot make progress here — the start
                    // has not moved — so the bound is what ends it.
                    restarts_left = restarts_left.checked_sub(1)?;
                    word = 0;
                    continue;
                }
                if word == 0 {
                    space_start = true;
                }
                word += 1;
                continue;
            }
            let Some(found) = find_from(&self.haystack, needle, start) else {
                self.next_start = None;
                return None;
            };
            result_pos = found;
            let end_index = found + needle.len() - 1;
            if word == 0 {
                result_start = found;
            }
            let mut matched = true;
            if word != 0 && !space_start {
                let current = needle.first().copied().unwrap_or('\0');
                let last = self
                    .needles
                    .get(word - 1)
                    .and_then(|previous| previous.last().copied())
                    .unwrap_or('\0');
                // Two sub-needles butted together are only a match when one
                // of the joining characters is a standalone unit — which is
                // what lets CJK match without spaces and makes Latin need one.
                if start == found && !(splits_needle(last) || splits_needle(current)) {
                    matched = false;
                }
                for offset in start..found {
                    if !self.haystack.get(offset).copied().is_some_and(is_separator) {
                        matched = false;
                        break;
                    }
                }
            } else if space_start && found > 0 {
                let before = self.haystack.get(found - 1).copied().unwrap_or('\0');
                if is_separator(before) {
                    // The leading empty sub-needle consumed the separator, so
                    // the match starts one earlier.
                    result_start = found - 1;
                } else {
                    matched = false;
                    result_start = found;
                }
            }
            if self.options.match_whole_word && matched {
                matched = is_whole_word(&self.haystack, found, end_index);
            }
            if matched {
                start = end_index + 1;
                word += 1;
            } else {
                restarts_left = restarts_left.checked_sub(1)?;
                let index = usize::from(space_start);
                let advance = self.needles.get(index).map_or(0, Vec::len);
                start = result_start + advance;
                if start >= length {
                    self.next_start = None;
                    return None;
                }
                word = 0;
            }
        }

        let last_len = self.needles.last().map_or(0, Vec::len);
        // The end is derived from the *last* scan position plus the last
        // sub-needle's length, which on the trailing-empty-needle path makes
        // the match one character long.
        let result_end = result_pos + last_len;
        let result_end = result_end.checked_sub(1)?;
        Some((result_start, result_end))
    }
}

/// The first occurrence of `needle` in `haystack` at or after `from`.
fn find_from(haystack: &[char], needle: &[char], from: usize) -> Option<usize> {
    if needle.is_empty() || from > haystack.len() {
        return None;
    }
    let last = haystack.len().checked_sub(needle.len())?;
    (from..=last).find(|start| haystack.get(*start..start + needle.len()) == Some(needle))
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

    fn ranges(text: &str, needle: &str, options: FindOptions) -> Vec<Range<usize>> {
        search(text, needle, options).collect()
    }

    const HELLO: &str = "Hello, world!\r\nGoodbye, world!";

    /// Audit item **A42**. `cpdf_textpage.cpp:1360-1361` writes `U+FFFE` into
    /// the text buffer at a soft hyphen and `cpdf_textpagefind.cpp:262`
    /// searches that buffer verbatim, so a word split across a line break can
    /// never be found (crbug.com/431824298). We drop the hyphen from the
    /// haystack and map back, so the word is found and the range is still a
    /// text offset — which is the property the fix has to keep.
    ///
    /// The character dropped is `U+00AD` rather than `U+FFFE` since audit
    /// **A41**'s buffer half; A42 is unaffected by that, because what it
    /// needs is that *something* stands between the two halves of the word
    /// and is not itself part of either.
    #[test]
    fn a_word_split_across_a_line_break_is_found_joined() {
        // "a note-\nbook here", as the pipeline writes it: the hyphen and the
        // break collapse to the one soft hyphen.
        let text = "a note\u{00AD}book here";
        let hits: Vec<_> = search(text, "notebook", FindOptions::default()).collect();
        assert_eq!(hits.len(), 1, "the joined word is found");

        // The range is a *text* offset, and it spans the soft hyphen, so
        // slicing the original text by it recovers the split spelling.
        let chars: Vec<char> = text.chars().collect();
        let hit = hits[0].clone();
        let slice: String = chars[hit.clone()].iter().collect();
        assert_eq!(slice, "note\u{00AD}book");
        assert_eq!(hit, 2..11);

        // And the soft hyphen is not itself a space: dropping it must not
        // splice two words into a spelling the page does not contain.
        assert_eq!(
            search(text, "note book", FindOptions::default()).count(),
            0,
            "the sentinel is not a space"
        );
    }

    #[test]
    fn splitting_is_by_script_not_by_alphabet() {
        // Latin-1 and the named blocks stay whole.
        assert!(!splits_needle('a'));
        assert!(!splits_needle('\u{00FE}'));
        assert!(!splits_needle('\u{0410}')); // Cyrillic
        assert!(!splits_needle('\u{0627}')); // Arabic
        assert!(!splits_needle('\u{2019}')); // General Punctuation
        assert!(!splits_needle('\u{2113}')); // SCRIPT SMALL L
        // Everything else is its own unit.
        assert!(splits_needle('\u{4E00}')); // CJK
        assert!(splits_needle('\u{AC00}')); // Hangul
        assert!(splits_needle('\u{0905}')); // Devanagari
        // The bound is `< 255`, so U+00FF itself splits.
        assert!(splits_needle('\u{00FF}'));
    }

    #[test]
    fn an_all_space_needle_is_not_split() {
        assert_eq!(split_needle(" "), [" "]);
        assert_eq!(split_needle("   "), ["   "]);
        assert_eq!(split_needle(""), [""]);
    }

    #[test]
    fn a_needle_splits_at_spaces_and_at_standalone_units() {
        assert_eq!(split_needle("ab cd"), ["ab", "cd"]);
        // Runs of spaces are skipped, not turned into empty tokens.
        assert_eq!(split_needle("ab  cd"), ["ab", "cd"]);
        // A CJK character splits the token around it.
        assert_eq!(split_needle("a\u{4E00}b"), ["a", "\u{4E00}", "b"]);
        assert_eq!(split_needle("\u{4E00}\u{4E8C}"), ["\u{4E00}", "\u{4E8C}"]);
        // An apostrophe inside a word is not a split point.
        assert_eq!(split_needle("don\u{2019}t"), ["don\u{2019}t"]);
    }

    #[test]
    fn a_trailing_space_leaves_an_empty_sub_needle() {
        assert_eq!(split_needle("ld! "), ["ld!", ""]);
        // And a leading one puts the empty element first.
        assert_eq!(split_needle(" Good"), ["", "Good"]);
    }

    #[test]
    fn substring_extraction_walks_space_delimited_tokens() {
        let chars: Vec<char> = "a b".chars().collect();
        assert_eq!(sub_string(&chars, 0), Some(vec!['a']));
        assert_eq!(sub_string(&chars, 1), Some(vec!['b']));
        assert_eq!(sub_string(&chars, 2), None);
        // A run of spaces is skipped as one separator.
        let chars: Vec<char> = "a  b".chars().collect();
        assert_eq!(sub_string(&chars, 1), Some(vec!['b']));
        // A trailing space leaves an empty token, then nothing.
        let chars: Vec<char> = "a ".chars().collect();
        assert_eq!(sub_string(&chars, 1), Some(vec![]));
        assert_eq!(sub_string(&chars, 2), None);
    }

    #[test]
    fn searching_finds_every_occurrence() {
        assert_eq!(ranges(HELLO, "nope", FindOptions::default()), []);
        assert_eq!(
            ranges(HELLO, "world", FindOptions::default()),
            [7..12, 24..29]
        );
    }

    #[test]
    fn the_default_is_case_insensitive() {
        assert_eq!(
            ranges(HELLO, "WORLD", FindOptions::default()),
            [7..12, 24..29]
        );
        let cased = FindOptions {
            match_case: true,
            ..FindOptions::default()
        };
        assert_eq!(ranges(HELLO, "WORLD", cased), []);
        assert_eq!(ranges(HELLO, "world", cased), [7..12, 24..29]);
    }

    #[test]
    fn whole_word_rejects_a_substring_match() {
        let whole = FindOptions {
            match_whole_word: true,
            ..FindOptions::default()
        };
        // "orld" matches as a substring but not as a word.
        assert_eq!(
            ranges(HELLO, "orld", FindOptions::default()),
            [8..12, 25..29]
        );
        assert_eq!(ranges(HELLO, "orld", whole), []);
        assert_eq!(ranges(HELLO, "world", whole), [7..12, 24..29]);
    }

    #[test]
    fn consecutive_reports_overlapping_matches() {
        let text = "aaaaaaaaaa";
        assert_eq!(ranges(text, "aaaa", FindOptions::default()), [0..4, 4..8]);
        let consecutive = FindOptions {
            consecutive: true,
            ..FindOptions::default()
        };
        assert_eq!(
            ranges(text, "aaaa", consecutive),
            [0..4, 1..5, 2..6, 3..7, 4..8, 5..9, 6..10]
        );
    }

    #[test]
    fn a_needle_spanning_a_line_break_matches_through_it() {
        // A single space in the needle spans the two-character CRLF run.
        assert_eq!(ranges(HELLO, "ld! G", FindOptions::default()), vec![10..16]);
    }

    #[test]
    fn a_leading_space_in_the_needle_matches_the_separator_before_the_word() {
        // The match starts on the '\n' at 14, not on the 'G' at 15.
        assert_eq!(ranges(HELLO, " Good", FindOptions::default()), vec![14..19]);
    }

    #[test]
    fn a_trailing_space_in_the_needle_matches_the_separator_after_the_word() {
        // "ld! " matches "ld!" plus the '\r' at 13.
        assert_eq!(ranges(HELLO, "ld! ", FindOptions::default()), vec![10..14]);
    }

    #[test]
    fn searching_an_empty_page_finds_nothing() {
        assert_eq!(ranges("", "anything", FindOptions::default()), []);
        assert_eq!(ranges("text", "", FindOptions::default()), []);
    }

    #[test]
    fn whole_word_boundaries_use_the_two_overlapping_tests() {
        // An ASCII letter on either side is caught by the *second* test even
        // when the first lets it through: 'a' passes the exclusive band and
        // is then rejected outright.
        let text: Vec<char> = "a-b".chars().collect();
        assert!(!is_whole_word(&text, 1, 1));
        // Non-letter neighbours leave the match standing.
        let spaced: Vec<char> = " - ".chars().collect();
        assert!(is_whole_word(&spaced, 1, 1));
        // A digit beside a digit is not.
        let digits: Vec<char> = "12".chars().collect();
        assert!(!is_whole_word(&digits, 1, 1));
        // 'Z' is caught only by the second test, and still rejects.
        let capital: Vec<char> = "Zx".chars().collect();
        assert!(!is_whole_word(&capital, 1, 1));
        // An inverted range never matches.
        assert!(!is_whole_word(&digits, 1, 0));
        // A single non-Latin-1 character is a whole word whatever surrounds it.
        let cjk: Vec<char> = "a\u{4E00}b".chars().collect();
        assert!(is_whole_word(&cjk, 1, 1));
    }

    #[test]
    fn a_restarting_search_terminates() {
        // A needle whose first sub-needle is empty and whose second never
        // matches must stop rather than spin.
        let found = ranges("aaaa", " zzz", FindOptions::default());
        assert!(found.is_empty());
        // And one whose only sub-needle is empty against a haystack with no
        // separator at all.
        let found = ranges("aaaa", " ", FindOptions::default());
        assert!(found.is_empty(), "{found:?}");
    }
}
