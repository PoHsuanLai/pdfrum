//! Mapping between character-list and search-facing text index spaces.
//!
//! Bridges [`CharIndex`] (positions in [`TextPage::chars`]) and [`TextIndex`]
//! (positions in [`TextPage::search_text`]).
//!
//! [`TextPage::chars`]: crate::TextPage::chars
//! [`TextPage::search_text`]: crate::TextPage::search_text

// The character list and the text string are different sequences: a control
// character, a character-code-zero placeholder or an unmapped code is in the
// first and not the second, while normalization puts several entries in the
// second for one in the first. So a caller holding a character index cannot
// use it as a text offset, and vice versa.
// (`docs/design/pdfrum-text.md` §1.13.)
//
// The bridge is a table of segments, each saying "text runs on from here for
// this many characters". Building it is one pass; reading it is a walk.
// **`--txt` uses none of this** — it reads the character list straight
// through.

use crate::charinfo::{CharBox, CharType};
use std::fmt;

/// A position in the character list ([`TextPage::chars`]) — the sequence a
/// `--txt` dump emits.
///
/// Distinct from [`TextIndex`] on purpose: the two sequences disagree. A
/// control character or an unmapped code is in this one and not the other,
/// and one character here can become several there. The compiler is asked to
/// notice instead of the caller.
///
/// [`TextPage::chars`]: crate::TextPage::chars
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct CharIndex(usize);

/// A position in the search-facing text ([`TextPage::search_text`]) — what a
/// search matches and a selection copies.
///
/// Distinct from [`CharIndex`]; see there for why.
///
/// [`TextPage::search_text`]: crate::TextPage::search_text
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct TextIndex(usize);

macro_rules! index_newtype {
    ($name:ident, $what:literal) => {
        impl $name {
            #[doc = concat!("The ", $what, " at this position.")]
            #[must_use]
            pub const fn new(index: usize) -> Self {
                Self(index)
            }

            /// The position as a plain number.
            #[must_use]
            pub const fn get(self) -> usize {
                self.0
            }
        }

        impl From<usize> for $name {
            fn from(index: usize) -> Self {
                Self(index)
            }
        }

        impl From<$name> for usize {
            fn from(index: $name) -> Self {
                index.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}

index_newtype!(CharIndex, "character-list position");
index_newtype!(TextIndex, "text position");

/// One run of characters that made it into the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CharSegment {
    /// The character index the run starts at.
    pub index: u32,
    /// How many characters it holds.
    pub count: u32,
}

/// The map between the two index spaces of
/// [`TextPage`](crate::TextPage).
///
/// A table of segments, not an index: it converts a character index into a
/// text offset and back, so a caller never has to guess which of the two
/// sequences a number counts in.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IndexMap {
    segments: Vec<CharSegment>,
}

/// Builds the table from a character list.
///
/// A character counts when it is *generated* — every generated character is
/// in both outputs — or when it is normal. Anything else breaks the run: the
/// next counting character starts a new segment if the current one has
/// anything in it, and otherwise just slides the current segment's start
/// forward.
#[must_use]
pub(crate) fn build(chars: &[CharBox]) -> IndexMap {
    let mut segments: Vec<CharSegment> = Vec::new();
    if !chars.is_empty() {
        segments.push(CharSegment { index: 0, count: 0 });
    }
    // True once the current segment holds at least one counting character.
    let mut started = false;
    for (position, info) in chars.iter().enumerate() {
        let counts = info.char_type == CharType::Generated || info.is_normal();
        let next = u32::try_from(position.saturating_add(1)).unwrap_or(u32::MAX);
        if counts {
            if let Some(last) = segments.last_mut() {
                last.count = last.count.saturating_add(1);
            }
            started = true;
        } else if started {
            segments.push(CharSegment {
                index: next,
                count: 0,
            });
            started = false;
        } else if let Some(last) = segments.last_mut() {
            last.index = next;
        }
    }
    IndexMap { segments }
}

impl IndexMap {
    /// The segments, oldest first.
    #[must_use]
    pub fn segments(&self) -> &[CharSegment] {
        &self.segments
    }

    /// How many characters the text holds.
    #[must_use]
    pub fn text_len(&self) -> usize {
        self.segments
            .iter()
            .map(|segment| segment.count as usize)
            .sum()
    }

    /// The character index a text offset names.
    ///
    /// ```
    /// # use pdfrum_text::{IndexMap, TextIndex};
    /// let map = IndexMap::default();
    /// assert_eq!(map.char_index(TextIndex::new(0)), None);
    /// ```
    #[must_use]
    pub fn char_index(&self, text_index: TextIndex) -> Option<CharIndex> {
        let mut remaining = text_index.get();
        for segment in &self.segments {
            let count = segment.count as usize;
            if remaining < count {
                return Some(CharIndex::new(segment.index as usize + remaining));
            }
            remaining -= count;
        }
        None
    }

    /// The text offset a character index names, or `None` for a character the
    /// text does not hold.
    #[must_use]
    pub fn text_index(&self, char_index: CharIndex) -> Option<TextIndex> {
        let char_index = char_index.get();
        let mut before = 0usize;
        for segment in &self.segments {
            let start = segment.index as usize;
            let count = segment.count as usize;
            if char_index < start {
                return None;
            }
            if char_index < start + count {
                return Some(TextIndex::new(before + (char_index - start)));
            }
            before += count;
        }
        None
    }

    /// The text offset a character index names, rounded **forward** to the
    /// next character the text does hold.
    ///
    /// What a start bound wants: a request that begins on a stripped
    /// character should begin at the next real one rather than failing.
    #[must_use]
    pub fn text_index_at_or_after(&self, char_index: CharIndex) -> Option<TextIndex> {
        let char_index = char_index.get();
        let mut before = 0usize;
        for segment in &self.segments {
            let start = segment.index as usize;
            let count = segment.count as usize;
            if count == 0 {
                continue;
            }
            if char_index < start {
                return Some(TextIndex::new(before));
            }
            if char_index < start + count {
                return Some(TextIndex::new(before + (char_index - start)));
            }
            before += count;
        }
        None
    }

    /// The text offset **one past** the last text character at or before
    /// `char_index`, which is what an end bound wants.
    #[must_use]
    pub fn text_index_end(&self, char_index: CharIndex) -> TextIndex {
        let char_index = char_index.get();
        let mut before = 0usize;
        let mut end = 0usize;
        for segment in &self.segments {
            let start = segment.index as usize;
            let count = segment.count as usize;
            if count == 0 {
                continue;
            }
            if char_index < start {
                break;
            }
            end = if char_index < start + count {
                before + (char_index - start) + 1
            } else {
                before + count
            };
            before += count;
        }
        TextIndex::new(end)
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
    use pdfrum_font::CharCode;

    fn info(char_type: CharType, unicode: u32, code: Option<u32>) -> CharBox {
        CharBox {
            char_type,
            unicode,
            code: code.map(CharCode),
            origin: Point::ZERO,
            char_box: Rect::ZERO,
            loose_char_box: Rect::ZERO,
            matrix: Affine::IDENTITY,
            object: None,
            font_size: 1.0,
            angle: 0.0,
        }
    }

    fn normal(ch: char) -> CharBox {
        info(CharType::Normal, u32::from(ch), Some(u32::from(ch)))
    }

    #[test]
    fn a_run_with_nothing_stripped_is_one_segment() {
        let chars: Vec<CharBox> = "hello".chars().map(normal).collect();
        let index = build(&chars);
        assert_eq!(index.segments(), [CharSegment { index: 0, count: 5 }]);
        assert_eq!(index.text_len(), 5);
        for at in 0..5 {
            assert_eq!(
                index.char_index(TextIndex::new(at)),
                Some(CharIndex::new(at))
            );
            assert_eq!(
                index.text_index(CharIndex::new(at)),
                Some(TextIndex::new(at))
            );
        }
        assert_eq!(index.char_index(TextIndex::new(5)), None);
    }

    #[test]
    fn an_empty_page_has_no_segments() {
        let index = build(&[]);
        assert!(index.segments().is_empty());
        assert_eq!(index.text_len(), 0);
        assert_eq!(index.char_index(TextIndex::new(0)), None);
        assert_eq!(index.text_index(CharIndex::new(0)), None);
    }

    #[test]
    fn a_stripped_character_splits_the_segments_and_keeps_the_char_index() {
        // The `control_characters.pdf` shape: two control characters after
        // five letters, so the *character* index of what follows is two
        // higher than its text offset.
        let mut chars: Vec<CharBox> = "Hello".chars().map(normal).collect();
        chars.push(info(CharType::Normal, 0x02, Some(2)));
        chars.push(info(CharType::Normal, 0x03, Some(3)));
        chars.extend("world".chars().map(normal));
        let index = build(&chars);
        assert_eq!(index.text_len(), 10);
        // Text offset 5 is the 'w', which is character index 7.
        assert_eq!(index.char_index(TextIndex::new(5)), Some(CharIndex::new(7)));
        assert_eq!(index.text_index(CharIndex::new(7)), Some(TextIndex::new(5)));
        // The two control characters are in no segment.
        assert_eq!(index.text_index(CharIndex::new(5)), None);
        assert_eq!(index.text_index(CharIndex::new(6)), None);
    }

    #[test]
    fn leading_stripped_characters_slide_the_first_segment_forward() {
        // Before any counting character has been seen, a stripped one moves
        // the segment's start rather than closing it.
        let mut chars = vec![info(CharType::Normal, 0x02, Some(2))];
        chars.extend("ab".chars().map(normal));
        let index = build(&chars);
        assert_eq!(index.segments(), [CharSegment { index: 1, count: 2 }]);
        assert_eq!(index.char_index(TextIndex::new(0)), Some(CharIndex::new(1)));
    }

    #[test]
    fn a_generated_character_always_counts() {
        // Even though a generated CRLF is not "normal" text, both outputs
        // hold it, so it never breaks a segment.
        let mut chars: Vec<CharBox> = "ab".chars().map(normal).collect();
        chars.push(info(CharType::Generated, u32::from('\r'), None));
        chars.push(info(CharType::Generated, u32::from('\n'), None));
        chars.extend("cd".chars().map(normal));
        let index = build(&chars);
        assert_eq!(index.segments(), [CharSegment { index: 0, count: 6 }]);
    }

    #[test]
    fn the_charcode_zero_placeholder_is_stripped() {
        // 22 NULs then "hello", which is `bug_425244539.pdf`'s shape: the
        // text is five characters and the first of them is character 22.
        let mut chars: Vec<CharBox> =
            std::iter::repeat_n(info(CharType::Normal, 0, Some(0)), 22).collect();
        chars.extend("hello".chars().map(normal));
        let index = build(&chars);
        assert_eq!(index.text_len(), 5);
        assert_eq!(
            index.char_index(TextIndex::new(0)),
            Some(CharIndex::new(22))
        );
        assert_eq!(
            index.text_index(CharIndex::new(22)),
            Some(TextIndex::new(0))
        );
    }

    #[test]
    fn the_forward_and_backward_bounds_skip_stripped_characters() {
        let mut chars: Vec<CharBox> = "ab".chars().map(normal).collect();
        chars.push(info(CharType::Normal, 0x02, Some(2)));
        chars.extend("cd".chars().map(normal));
        let index = build(&chars);
        // Character 2 is stripped: forward lands on 'c' at text offset 2.
        assert_eq!(
            index.text_index_at_or_after(CharIndex::new(2)),
            Some(TextIndex::new(2))
        );
        // And the end bound for character 2 stops after 'b'.
        assert_eq!(index.text_index_end(CharIndex::new(2)), TextIndex::new(2));
        assert_eq!(index.text_index_end(CharIndex::new(3)), TextIndex::new(3));
        assert_eq!(index.text_index_end(CharIndex::new(99)), TextIndex::new(4));
    }

    #[test]
    fn the_two_index_spaces_are_different_types() {
        // The whole point of the newtypes: a number that counts in one
        // sequence cannot be handed to a method that counts in the other.
        // That is a compile-time claim, so what is asserted here is the round
        // trip through `new`/`get` and the `From` conversions that make the
        // wrapping cheap at a boundary.
        assert_eq!(CharIndex::new(7).get(), 7);
        assert_eq!(TextIndex::new(7).get(), 7);
        assert_eq!(CharIndex::from(3usize), CharIndex::new(3));
        assert_eq!(TextIndex::from(3usize), TextIndex::new(3));
        assert_eq!(usize::from(CharIndex::new(3)), 3);
        assert_eq!(usize::from(TextIndex::new(3)), 3);
        // `Display` is the bare number, so a diagnostic reads like an index.
        assert_eq!(CharIndex::new(41).to_string(), "41");
        assert_eq!(TextIndex::new(41).to_string(), "41");
        // `Ord` orders like the number it wraps, which is what a range wants.
        assert!(CharIndex::new(1) < CharIndex::new(2));
        assert!(TextIndex::new(1) < TextIndex::new(2));
        // And `Default` is position zero, so `..` bounds have a floor.
        assert_eq!(CharIndex::default(), CharIndex::new(0));
        assert_eq!(TextIndex::default(), TextIndex::new(0));
    }

    #[test]
    fn the_conversions_round_trip_over_a_known_segment_table() {
        // `control_characters.pdf`'s shape again, walked in both directions
        // over the whole table rather than at three sample points: every text
        // offset names a character, and that character names it back.
        let mut chars: Vec<CharBox> = "Hello".chars().map(normal).collect();
        chars.push(info(CharType::Normal, 0x02, Some(2)));
        chars.push(info(CharType::Normal, 0x03, Some(3)));
        chars.extend("world".chars().map(normal));
        let map = build(&chars);

        for at in 0..map.text_len() {
            let text = TextIndex::new(at);
            let ch = map.char_index(text).expect("every text offset has a char");
            assert_eq!(map.text_index(ch), Some(text), "round trip at {text}");
            // And the forward bound agrees with the exact one on a character
            // the text does hold.
            assert_eq!(map.text_index_at_or_after(ch), Some(text));
            // The end bound is one past it.
            assert_eq!(map.text_index_end(ch), TextIndex::new(at + 1));
        }
        // Past the end in text space names no character at all.
        assert_eq!(map.char_index(TextIndex::new(map.text_len())), None);

        // The other direction over the *character* list: the two stripped
        // characters are the only ones with no text offset of their own, and
        // the forward bound rounds them up to the next real one.
        let stripped: Vec<usize> = (0..chars.len())
            .filter(|at| map.text_index(CharIndex::new(*at)).is_none())
            .collect();
        assert_eq!(stripped, [5, 6]);
        assert_eq!(
            map.text_index_at_or_after(CharIndex::new(5)),
            Some(TextIndex::new(5))
        );
        assert_eq!(
            map.text_index_at_or_after(CharIndex::new(6)),
            Some(TextIndex::new(5))
        );
        // And their end bound stops after the last real character before them.
        assert_eq!(map.text_index_end(CharIndex::new(5)), TextIndex::new(5));
        assert_eq!(map.text_index_end(CharIndex::new(6)), TextIndex::new(5));
    }
}
