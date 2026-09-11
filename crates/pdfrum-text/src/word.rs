//! The words of a page, as a view over the character list.
//!
//! A word here is what an extraction or citation pipeline means by one: a
//! run of the reading-order text between two whitespace characters, with the
//! box its glyphs cover and the font the first glyph was set in. Nothing is
//! laid out again — every field is read off the [`TextPage`] the extractor
//! already built, so the split costs one pass over `chars`.
//!
//! This is **not** the scripting API's word list ([`crate::content_words`]), which
//! walks the content stream as written and splits on a different rule.

use crate::TextPage;
use crate::charinfo::CharBox;
use crate::index::CharIndex;
use kurbo::Rect;
use std::ops::Range;

/// One word of a page's text, from [`TextPage::words`].
#[derive(Debug, Clone, PartialEq)]
pub struct Word {
    /// The word, without surrounding whitespace.
    pub text: String,
    /// The union of its characters' boxes, in page space — the same
    /// coordinates [`TextPage::rects`] reports. A word none of whose glyphs
    /// has a visible box reports [`Rect::ZERO`].
    pub rect: Rect,
    /// The characters this word was cut from, so [`TextPage::slice`] on it
    /// gives back [`text`](Self::text) and the characters around it give
    /// context.
    pub range: Range<CharIndex>,
    /// The base font name of the first character's font, when the font has
    /// one — see [`TextPage::font_name`].
    pub font: Option<String>,
    /// The first character's font size in page points: the `Tf` operand
    /// scaled by the text matrix, so `1 Tf` under a `12 0 0 12` matrix reads
    /// as 12.
    pub size: f64,
}

/// Whether a character ends a word: the extractor invented it (a space or a
/// line break the geometry implied), or it is Unicode whitespace.
fn is_separator(info: &CharBox) -> bool {
    info.is_generated() || char::from_u32(info.unicode).is_some_and(char::is_whitespace)
}

/// The visible box of one character, normalized, or `None` where
/// [`TextPage::rects`] would skip it.
fn visible_box(info: &CharBox) -> Option<Rect> {
    if info.is_generated() {
        return None;
    }
    let rect = info.char_box;
    if rect.width().abs() < 0.01 || rect.height().abs() < 0.01 {
        return None;
    }
    Some(Rect::new(
        rect.x0.min(rect.x1),
        rect.y0.min(rect.y1),
        rect.x0.max(rect.x1),
        rect.y0.max(rect.y1),
    ))
}

/// The font size a character was drawn at, in page points.
fn point_size(info: &CharBox) -> f64 {
    let [_, _, c, d, ..] = info.matrix.as_coeffs();
    f64::from(info.font_size).abs() * c.hypot(d)
}

impl TextPage {
    /// The page's words in reading order: the runs of
    /// [`chars`](Self::chars) between whitespace, each with the box its
    /// glyphs cover and the font of its first character.
    ///
    /// A word's [`text`](Word::text) is what [`slice`](Self::slice) gives for
    /// its [`range`](Word::range), so a hyphenated line break shows up as
    /// two words, the first ending in `U+00AD`. A run whose characters all
    /// vanish from the search text — the charcode-0 passthrough, the control
    /// code points — is not a word.
    ///
    /// # Examples
    ///
    /// ```
    /// # use pdfrum_text::TextPage;
    /// let page = TextPage::default();
    /// assert!(page.words().is_empty());
    /// ```
    #[must_use]
    pub fn words(&self) -> Vec<Word> {
        let mut out = Vec::new();
        let mut start: Option<usize> = None;
        for (position, info) in self.chars.iter().enumerate() {
            match (start, is_separator(info)) {
                (None, false) => start = Some(position),
                (Some(from), true) => {
                    out.extend(self.word(from..position));
                    start = None;
                }
                (None, true) | (Some(_), false) => {}
            }
        }
        if let Some(from) = start {
            out.extend(self.word(from..self.chars.len()));
        }
        out
    }

    /// One word over a run of consecutive non-separator characters, or
    /// `None` when the run has no text.
    fn word(&self, positions: Range<usize>) -> Option<Word> {
        let range = CharIndex::new(positions.start)..CharIndex::new(positions.end);
        let text = self.slice(range.clone());
        let text = text.trim();
        if text.is_empty() {
            return None;
        }
        let chars = self.chars.get(positions).unwrap_or_default();
        let first = chars.first()?;
        let rect = chars
            .iter()
            .filter_map(visible_box)
            .reduce(|a, b| a.union(b))
            .unwrap_or(Rect::ZERO);
        let font = self.font_name(range.start).map(str::to_owned);
        Some(Word {
            text: text.to_owned(),
            rect,
            range,
            font,
            size: point_size(first),
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::float_cmp,
        clippy::indexing_slicing,
        reason = "test fixtures pin exact values"
    )]

    use super::*;
    use crate::charinfo::{CharType, ObjectIndex};
    use crate::index;
    use kurbo::{Affine, Point};
    use pdfrum_font::CharCode;

    /// A page whose characters are `text`, each one glyph wide at `x = 10 *
    /// position`, drawn by object `0` unless the character is a space — a
    /// space is a generated one, as the extractor would emit for an inter-word
    /// gap.
    fn page(text: &str, matrix: Affine) -> TextPage {
        let chars: Vec<CharBox> = text
            .chars()
            .enumerate()
            .map(|(position, ch)| {
                let generated = ch == ' ';
                #[expect(clippy::cast_precision_loss, reason = "a test index is tiny")]
                let x = position as f64 * 10.0;
                let char_box = if generated {
                    Rect::ZERO
                } else {
                    Rect::new(x, 0.0, x + 8.0, 10.0)
                };
                CharBox {
                    char_type: if generated {
                        CharType::Generated
                    } else {
                        CharType::Normal
                    },
                    unicode: u32::from(ch),
                    code: (!generated).then_some(CharCode(u32::from(ch))),
                    origin: Point::new(x, 0.0),
                    char_box,
                    loose_char_box: char_box,
                    matrix,
                    object: (!generated).then_some(ObjectIndex(0)),
                    font_size: if generated { 1.0 } else { 12.0 },
                    angle: 0.0,
                }
            })
            .collect();
        let runs = index::build(&chars);
        let mut page = TextPage {
            search_text: text.chars().collect(),
            chars,
            runs,
            ..TextPage::default()
        };
        page.fonts.insert(ObjectIndex(0), "Helvetica".to_owned());
        page
    }

    #[test]
    fn words_split_on_whitespace_and_slice_back_to_themselves() {
        let page = page("Hello, world!", Affine::IDENTITY);
        let words = page.words();
        let texts: Vec<&str> = words.iter().map(|w| w.text.as_str()).collect();
        assert_eq!(texts, ["Hello,", "world!"]);
        for word in &words {
            assert_eq!(page.slice(word.range.clone()), word.text);
        }
        assert_eq!(words[0].range, CharIndex::new(0)..CharIndex::new(6));
        assert_eq!(words[1].range, CharIndex::new(7)..CharIndex::new(13));
    }

    #[test]
    fn a_word_covers_the_union_of_its_glyph_boxes() {
        let page = page("ab cd", Affine::IDENTITY);
        let words = page.words();
        assert_eq!(words[0].rect, Rect::new(0.0, 0.0, 18.0, 10.0));
        assert_eq!(words[1].rect, Rect::new(30.0, 0.0, 48.0, 10.0));
    }

    #[test]
    fn the_font_and_size_come_from_the_first_character() {
        let page = page("ab", Affine::scale(2.0));
        let words = page.words();
        assert_eq!(words[0].font.as_deref(), Some("Helvetica"));
        // `12 Tf` under a doubling matrix is 24 points on the page.
        assert_eq!(words[0].size, 24.0);
    }

    #[test]
    fn leading_and_trailing_whitespace_makes_no_word() {
        let page = page("  one  ", Affine::IDENTITY);
        let words = page.words();
        assert_eq!(words.len(), 1);
        assert_eq!(words[0].text, "one");
        assert_eq!(words[0].range, CharIndex::new(2)..CharIndex::new(5));
    }

    #[test]
    fn a_run_the_search_text_drops_is_not_a_word() {
        // `U+0003` is a control code point `search_text` drops.
        let mut page = page("a \u{3} b", Affine::IDENTITY);
        page.search_text = "a  b".chars().collect();
        page.runs = index::build(&page.chars);
        let texts: Vec<String> = page.words().into_iter().map(|w| w.text).collect();
        assert_eq!(texts, ["a", "b"]);
    }

    #[test]
    fn font_name_reads_through_the_character_to_its_object() {
        let page = page("a b", Affine::IDENTITY);
        assert_eq!(page.font_name(CharIndex::new(0)), Some("Helvetica"));
        // The generated space has no object.
        assert_eq!(page.font_name(CharIndex::new(1)), None);
        assert_eq!(page.font_name(CharIndex::new(9)), None);
    }
}
