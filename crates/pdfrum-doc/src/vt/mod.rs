//! The variable-text layout engine: a field value in, positioned glyphs out.
//!
//! A small paragraph engine. A document is a list of sections — paragraphs,
//! split on the line breaks the text itself carries — and each section holds
//! **one entry per character**, not per word. Line breaking then groups those
//! entries into lines, and placement gives each one an x and a y.
//!
//! Two things about the coordinate space. Everything internal is y-**down**
//! from the plate's top-left corner, and [`Layout::to_pdf`] flips it back on
//! the way out. And the two quantities the formulas call `line_leading` and
//! `line_indent` are **always zero** upstream — never assigned, never
//! returned as anything else. The terms stay in the arithmetic so it reads
//! against the source, but nothing varies them.
//!
//! # Who uses it
//!
//! The free-text and pop-up annotation generators, the `/NeedAppearances`
//! form path, and the **widget** appearance builders in
//! [`crate::ap::field_body`]. There is **no second implementation**: what the
//! widget path drives is a shell over this engine whose only observable
//! addition is a vertical alignment offset, which the builders pass as
//! `edit_ap::generate`'s `offset`.

mod autosize;
mod bidi;
mod classify;
mod comb;
pub(crate) mod edit_ap;
pub mod hit;
mod place;
mod split;

use kurbo::Rect;
use std::ops::Range;

pub use bidi::Direction;

/// How a line sits within the plate.
///
/// ```
/// use pdfrum_doc::vt::Alignment;
///
/// // `/Q` names the three, and the default is flush left.
/// assert_eq!(Alignment::default(), Alignment::Left);
/// assert_eq!(Alignment::from_quadding(1), Alignment::Center);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Alignment {
    /// Flush left.
    #[default]
    Left,
    /// Centred.
    Center,
    /// Flush right.
    Right,
}

impl Alignment {
    /// Reads a `/Q` value; anything outside `0..=2` is left-aligned.
    ///
    /// ```
    /// use pdfrum_doc::vt::Alignment;
    ///
    /// assert_eq!(Alignment::from_quadding(0), Alignment::Left);
    /// assert_eq!(Alignment::from_quadding(2), Alignment::Right);
    /// // Anything outside `0..=2` is left-aligned.
    /// assert_eq!(Alignment::from_quadding(9), Alignment::Left);
    /// ```
    #[must_use]
    pub fn from_quadding(q: i64) -> Alignment {
        match q {
            1 => Alignment::Center,
            2 => Alignment::Right,
            _ => Alignment::Left,
        }
    }
}

/// The font-space scale: widths and ascents arrive per thousand units.
pub(crate) const FONT_SCALE: f32 = 0.001;

/// The sizes automatic sizing may choose from.
///
/// A **multi-line** field only ever considers the first quarter of these —
/// six entries, so it can never auto-size above 12.
pub(crate) const FONT_SIZE_STEPS: [u8; 25] = [
    4, 6, 8, 9, 10, 12, 14, 18, 20, 25, 30, 35, 40, 45, 50, 55, 60, 70, 80, 90, 100, 110, 120, 130,
    144,
];

/// What the engine needs to know about the font it is setting.
///
/// A plain record rather than a trait: there is exactly one real
/// implementation, and the test stub is a different set of numbers rather
/// than a different behavior.
///
/// ```
/// use pdfrum_doc::geom;
/// use pdfrum_doc::vt::{Config, Metrics, layout};
///
/// // Every character ten thousandths wide: the stub the layout assertions
/// // in this crate are written against.
/// let width = |_code: u32| 10;
/// let metrics = Metrics { width: &width, ascent: 1000, descent: -200 };
///
/// let config = Config {
///     plate: geom::rect(0.0, 0.0, 100.0, 40.0),
///     font_size: 10.0,
///     multi_line: true,
///     ..Config::default()
/// };
/// let laid_out = layout("Hi", &config, &metrics);
///
/// // Nothing here reads a font dictionary; the numbers are the whole
/// // contract between a caller and the engine.
/// assert_eq!(laid_out.font_size, 10.0);
/// ```
#[derive(Clone, Copy)]
pub struct Metrics<'a> {
    /// Width per character, in thousandths of an em.
    pub width: &'a dyn Fn(u32) -> i32,
    /// The font's ascent, in thousandths.
    pub ascent: i32,
    /// The font's descent, in thousandths — normally negative.
    pub descent: i32,
}

impl std::fmt::Debug for Metrics<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Metrics")
            .field("ascent", &self.ascent)
            .field("descent", &self.descent)
            .finish_non_exhaustive()
    }
}

/// How a piece of variable text is to be set.
///
/// ```
/// use pdfrum_doc::geom;
/// use pdfrum_doc::vt::{Config, Metrics, layout};
///
/// // Every character ten thousandths wide: the stub the layout assertions
/// // in this crate are written against.
/// let width = |_code: u32| 10;
/// let metrics = Metrics { width: &width, ascent: 1000, descent: -200 };
///
/// let config = Config {
///     plate: geom::rect(0.0, 0.0, 100.0, 40.0),
///     font_size: 10.0,
///     multi_line: true,
///     ..Config::default()
/// };
/// let laid_out = layout("Hi", &config, &metrics);
///
/// // A plain config struct with `Default` plus struct-update syntax.
/// assert_eq!(laid_out.sections.len(), 1);
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// The box the text is set into.
    pub plate: Rect,
    /// Where lines sit horizontally.
    pub alignment: Alignment,
    /// The size, or zero to choose one automatically.
    pub font_size: f32,
    /// Whether the text may occupy more than one line.
    pub multi_line: bool,
    /// Whether a line too long for the plate wraps.
    pub auto_return: bool,
    /// The character every character is *displayed* as, for a password field.
    /// Widths use it too.
    pub sub_word: Option<char>,
    /// A cap on how many characters are set at all.
    pub limit_char: usize,
    /// A comb field's cell count. Non-zero switches to comb layout.
    pub char_array: usize,
    /// The paragraph direction offered to the bidi resolver.
    pub direction: Direction,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            plate: Rect::ZERO,
            alignment: Alignment::Left,
            font_size: 0.0,
            multi_line: false,
            auto_return: false,
            sub_word: None,
            limit_char: 0,
            char_array: 0,
            direction: Direction::Auto,
        }
    }
}

/// One character, placed.
///
/// ```
/// use pdfrum_doc::geom;
/// use pdfrum_doc::vt::{Config, Metrics, layout};
///
/// // Every character ten thousandths wide: the stub the layout assertions
/// // in this crate are written against.
/// let width = |_code: u32| 10;
/// let metrics = Metrics { width: &width, ascent: 1000, descent: -200 };
///
/// let config = Config {
///     plate: geom::rect(0.0, 0.0, 100.0, 40.0),
///     font_size: 10.0,
///     multi_line: true,
///     ..Config::default()
/// };
/// let laid_out = layout("Hi", &config, &metrics);
///
/// // One entry per character, not per word.
/// let section = &laid_out.sections[0];
/// assert_eq!(section.words.len(), 2);
/// assert_eq!(section.words[0].ch, u32::from('H'));
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Word {
    /// The code point. A password field still records the real one; only its
    /// width and its output byte come from the substitute.
    pub ch: u32,
    /// Position, y-down from the plate's top-left.
    pub x: f32,
    /// Position, y-down from the plate's top-left.
    pub y: f32,
    /// Extra width added after this character, used by comb cells.
    pub tail: f32,
    /// Whether the character sits in a right-to-left run.
    pub is_rtl: bool,
}

/// One line of a section.
///
/// ```
/// use pdfrum_doc::geom;
/// use pdfrum_doc::vt::{Config, Metrics, layout};
///
/// // Every character ten thousandths wide: the stub the layout assertions
/// // in this crate are written against.
/// let width = |_code: u32| 10;
/// let metrics = Metrics { width: &width, ascent: 1000, descent: -200 };
///
/// let config = Config {
///     plate: geom::rect(0.0, 0.0, 100.0, 40.0),
///     font_size: 10.0,
///     multi_line: true,
///     ..Config::default()
/// };
/// let laid_out = layout("Hi", &config, &metrics);
///
/// let line = &laid_out.sections[0].lines[0];
/// // Half-open, over the section's own word indices.
/// assert_eq!(line.word_range(2), 0..2);
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    /// The words this line covers, **half-open**, or `None` for the single
    /// line of an empty section.
    pub words: Option<Range<u32>>,
    /// Position, y-down.
    pub x: f32,
    /// Position, y-down.
    pub y: f32,
    /// The line's set width.
    pub width: f32,
    /// The tallest ascent on the line, floored at zero.
    pub ascent: f32,
    /// The deepest descent on the line, capped at zero.
    pub descent: f32,
}

impl Line {
    /// The words this line covers, as a `usize` range clamped to `available`.
    ///
    /// Empty for the single line of an empty section, and for a `begin` past
    /// the section's words — a layout that shrank under an edit.
    ///
    /// ```
    /// use pdfrum_doc::geom;
    /// use pdfrum_doc::vt::{Config, Metrics, layout};
    ///
    /// // Every character ten thousandths wide: the stub the layout assertions
    /// // in this crate are written against.
    /// let width = |_code: u32| 10;
    /// let metrics = Metrics { width: &width, ascent: 1000, descent: -200 };
    ///
    /// let config = Config {
    ///     plate: geom::rect(0.0, 0.0, 100.0, 40.0),
    ///     font_size: 10.0,
    ///     multi_line: true,
    ///     ..Config::default()
    /// };
    /// let laid_out = layout("Hi", &config, &metrics);
    ///
    /// let line = &laid_out.sections[0].lines[0];
    /// assert_eq!(line.word_range(2), 0..2);
    /// // Clamped: a layout that shrank under an edit answers empty rather
    /// // than a range past the end.
    /// assert_eq!(line.word_range(0), 0..0);
    /// ```
    #[must_use]
    pub fn word_range(&self, available: usize) -> Range<usize> {
        let Some(words) = self.words.clone() else {
            return 0..0;
        };
        let begin = usize::try_from(words.start).unwrap_or(usize::MAX);
        let end = usize::try_from(words.end).unwrap_or(usize::MAX);
        if begin >= available {
            return 0..0;
        }
        begin..end.min(available)
    }

    /// The **last** word index the line covers, if it covers any.
    ///
    /// ```
    /// use pdfrum_doc::geom;
    /// use pdfrum_doc::vt::{Config, Metrics, layout};
    ///
    /// // Every character ten thousandths wide: the stub the layout assertions
    /// // in this crate are written against.
    /// let width = |_code: u32| 10;
    /// let metrics = Metrics { width: &width, ascent: 1000, descent: -200 };
    ///
    /// let config = Config {
    ///     plate: geom::rect(0.0, 0.0, 100.0, 40.0),
    ///     font_size: 10.0,
    ///     multi_line: true,
    ///     ..Config::default()
    /// };
    /// let laid_out = layout("Hi", &config, &metrics);
    ///
    /// let line = &laid_out.sections[0].lines[0];
    /// assert_eq!(line.last_word(), Some(1));
    /// ```
    #[must_use]
    pub fn last_word(&self) -> Option<u32> {
        let words = self.words.clone()?;
        words.end.checked_sub(1).filter(|last| *last >= words.start)
    }
}

/// One paragraph.
///
/// ```
/// use pdfrum_doc::geom;
/// use pdfrum_doc::vt::{Config, Metrics, layout};
///
/// // Every character ten thousandths wide: the stub the layout assertions
/// // in this crate are written against.
/// let width = |_code: u32| 10;
/// let metrics = Metrics { width: &width, ascent: 1000, descent: -200 };
///
/// let config = Config {
///     plate: geom::rect(0.0, 0.0, 100.0, 40.0),
///     font_size: 10.0,
///     multi_line: true,
///     ..Config::default()
/// };
/// let laid_out = layout("Hi", &config, &metrics);
///
/// // One paragraph, split on the line breaks the text carries.
/// let two = layout("a\nb", &config, &metrics);
/// assert_eq!(two.sections.len(), 2);
/// ```
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Section {
    /// Its characters, in logical order.
    pub words: Vec<Word>,
    /// Its lines, once broken.
    pub lines: Vec<Line>,
    /// Its extent, y-down.
    pub rect: Rect,
}

/// A laid-out piece of variable text.
///
/// ```
/// use pdfrum_doc::geom;
/// use pdfrum_doc::vt::{Config, Metrics, layout};
///
/// // Every character ten thousandths wide: the stub the layout assertions
/// // in this crate are written against.
/// let width = |_code: u32| 10;
/// let metrics = Metrics { width: &width, ascent: 1000, descent: -200 };
///
/// let config = Config {
///     plate: geom::rect(0.0, 0.0, 100.0, 40.0),
///     font_size: 10.0,
///     multi_line: true,
///     ..Config::default()
/// };
/// let laid_out = layout("Hi", &config, &metrics);
///
/// assert_eq!(laid_out.sections.len(), 1);
/// // The extent is y-down; `content_rect_pdf` flips it back.
/// let _ = laid_out.content_rect_pdf(config.plate);
/// ```
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Layout {
    /// The paragraphs.
    pub sections: Vec<Section>,
    /// The union of their extents, y-down.
    pub content_rect: Rect,
    /// The size actually used, which automatic sizing may have chosen.
    pub font_size: f32,
}

impl Layout {
    /// Converts an internal point to PDF's y-up space.
    ///
    /// ```
    /// use pdfrum_doc::geom;
    /// use pdfrum_doc::vt::{Config, Metrics, layout};
    ///
    /// // Every character ten thousandths wide: the stub the layout assertions
    /// // in this crate are written against.
    /// let width = |_code: u32| 10;
    /// let metrics = Metrics { width: &width, ascent: 1000, descent: -200 };
    ///
    /// let config = Config {
    ///     plate: geom::rect(0.0, 0.0, 100.0, 40.0),
    ///     font_size: 10.0,
    ///     multi_line: true,
    ///     ..Config::default()
    /// };
    /// let laid_out = layout("Hi", &config, &metrics);
    /// use pdfrum_doc::vt::Layout;
    ///
    /// // Internal space is y-down from the plate's top-left corner.
    /// assert_eq!(Layout::to_pdf(config.plate, 0.0, 0.0), (0.0, 40.0));
    /// ```
    #[must_use]
    pub fn to_pdf(plate: Rect, x: f32, y: f32) -> (f32, f32) {
        (crate::geom::left(plate) + x, crate::geom::top(plate) - y)
    }

    /// The content rectangle in PDF's y-up space.
    ///
    /// ```
    /// use pdfrum_doc::geom;
    /// use pdfrum_doc::vt::{Config, Metrics, layout};
    ///
    /// // Every character ten thousandths wide: the stub the layout assertions
    /// // in this crate are written against.
    /// let width = |_code: u32| 10;
    /// let metrics = Metrics { width: &width, ascent: 1000, descent: -200 };
    ///
    /// let config = Config {
    ///     plate: geom::rect(0.0, 0.0, 100.0, 40.0),
    ///     font_size: 10.0,
    ///     multi_line: true,
    ///     ..Config::default()
    /// };
    /// let laid_out = layout("Hi", &config, &metrics);
    ///
    /// let rect = laid_out.content_rect_pdf(config.plate);
    /// // Back in PDF's y-up space, so the top edge is above the bottom.
    /// assert!(geom::top(rect) >= geom::bottom(rect));
    /// ```
    #[must_use]
    pub fn content_rect_pdf(&self, plate: Rect) -> Rect {
        let (left, top) = Layout::to_pdf(
            plate,
            crate::geom::left(self.content_rect),
            crate::geom::bottom(self.content_rect),
        );
        let (right, bottom) = Layout::to_pdf(
            plate,
            crate::geom::right(self.content_rect),
            crate::geom::top(self.content_rect),
        );
        crate::geom::rect(left, bottom, right, top)
    }

    /// Every word in reading order, paired with the line it belongs to.
    ///
    /// ```
    /// use pdfrum_doc::geom;
    /// use pdfrum_doc::vt::{Config, Metrics, layout};
    ///
    /// // Every character ten thousandths wide: the stub the layout assertions
    /// // in this crate are written against.
    /// let width = |_code: u32| 10;
    /// let metrics = Metrics { width: &width, ascent: 1000, descent: -200 };
    ///
    /// let config = Config {
    ///     plate: geom::rect(0.0, 0.0, 100.0, 40.0),
    ///     font_size: 10.0,
    ///     multi_line: true,
    ///     ..Config::default()
    /// };
    /// let laid_out = layout("Hi", &config, &metrics);
    ///
    /// // Reading order, each character paired with its section and line.
    /// let chars: Vec<u32> = laid_out.words().map(|(_, _, word)| word.ch).collect();
    /// assert_eq!(chars, [u32::from('H'), u32::from('i')]);
    /// ```
    pub fn words(&self) -> impl Iterator<Item = (usize, usize, &Word)> {
        self.sections.iter().enumerate().flat_map(|(s, section)| {
            section.lines.iter().enumerate().flat_map(move |(l, line)| {
                let range = line.word_range(section.words.len());
                section
                    .words
                    .get(range)
                    .unwrap_or_default()
                    .iter()
                    .map(move |word| (s, l, word))
            })
        })
    }
}

/// Splits text into sections, one per line break, and characters into words.
///
/// Four rules the tokenizer carries:
///
/// - A carriage return followed by a line feed is **one** break, and so is a
///   line feed followed by a carriage return.
/// - In a single-line field a lone break is consumed and produces *nothing* —
///   not even a space.
/// - A tab becomes a space.
/// - The character counter advances for a **break** as well as a character,
///   so a five-character limit on `"ab\ncd"` admits `a`, `b`, the break, and
///   `c`, and stops there.
#[must_use]
pub(crate) fn split_sections(text: &str, config: &Config) -> Vec<Vec<u32>> {
    let mut sections: Vec<Vec<u32>> = vec![Vec::new()];
    let mut count = 0usize;
    let chars: Vec<char> = text.chars().collect();
    let mut index = 0;

    while index < chars.len() {
        if config.limit_char > 0 && count >= config.limit_char {
            break;
        }
        if config.char_array > 0 && count >= config.char_array {
            break;
        }
        let ch = chars.get(index).copied().unwrap_or('\0');
        match ch {
            '\r' | '\n' => {
                let partner = if ch == '\r' { '\n' } else { '\r' };
                if chars.get(index + 1) == Some(&partner) {
                    index += 1;
                }
                if config.multi_line {
                    sections.push(Vec::new());
                }
            }
            _ => {
                let ch = if ch == '\t' { ' ' } else { ch };
                if let Some(last) = sections.last_mut() {
                    last.push(ch as u32);
                }
            }
        }
        count += 1;
        index += 1;
    }
    sections
}

/// Lays out a piece of variable text.
///
/// A pure function: nothing here reads a document or a font dictionary, only
/// the numbers [`Metrics`] supplies, which is what makes the whole engine
/// testable against a stub.
///
/// ```
/// use pdfrum_doc::geom;
/// use pdfrum_doc::vt::{Config, Metrics, layout};
///
/// // Every character ten thousandths wide: the stub the layout assertions
/// // in this crate are written against.
/// let width = |_code: u32| 10;
/// let metrics = Metrics { width: &width, ascent: 1000, descent: -200 };
///
/// let config = Config {
///     plate: geom::rect(0.0, 0.0, 100.0, 40.0),
///     font_size: 10.0,
///     multi_line: true,
///     ..Config::default()
/// };
/// let laid_out = layout("Hi", &config, &metrics);
///
/// assert_eq!(laid_out.sections.len(), 1);
/// assert_eq!(laid_out.sections[0].words.len(), 2);
/// ```
#[must_use]
pub fn layout(text: &str, config: &Config, metrics: &Metrics<'_>) -> Layout {
    let mut config = config.clone();
    let mut sections: Vec<Section> = split_sections(text, &config)
        .into_iter()
        .map(|words| Section {
            words: words
                .into_iter()
                .map(|ch| Word {
                    ch,
                    x: 0.0,
                    y: 0.0,
                    tail: 0.0,
                    is_rtl: false,
                })
                .collect(),
            lines: Vec::new(),
            rect: Rect::ZERO,
        })
        .collect();

    if config.font_size == 0.0 {
        config.font_size = autosize::auto_font_size(&sections, &config, metrics);
    }

    let content_rect = rearrange(&mut sections, &config, metrics);
    Layout {
        sections,
        content_rect,
        font_size: config.font_size,
    }
}

/// Lays the sections out top to bottom, returning the union of their extents.
///
/// The union starts as the **first** section's rectangle rather than as an
/// empty one, so a document whose first paragraph is empty still contributes
/// that paragraph's zero-width box.
///
/// A section's own origin is folded into the positions it holds, on **both**
/// axes. Placement gives each word a position relative to the section box —
/// which is what makes the two alignment offsets partially cancel — and the
/// box's own left edge is where the alignment that survives lives. Reading a
/// word's position without adding it back leaves every right-aligned and
/// centred field flush left.
fn rearrange(sections: &mut [Section], config: &Config, metrics: &Metrics<'_>) -> Rect {
    let mut y = 0.0;
    let mut union: Option<Rect> = None;
    for section in sections.iter_mut() {
        let height = if config.char_array > 0 {
            comb::rearrange_char_array(section, config, metrics)
        } else {
            section.lines.clear();
            let measured = split::split_lines(section, config, metrics, true);
            place::output_lines(section, config, metrics, measured)
        };
        section.rect = crate::geom::rect(
            crate::geom::left(height),
            crate::geom::bottom(height) + y,
            crate::geom::right(height),
            crate::geom::top(height) + y,
        );
        let x = crate::geom::left(height);
        for word in &mut section.words {
            word.x += x;
            word.y += y;
        }
        for line in &mut section.lines {
            line.x += x;
            line.y += y;
        }
        y += crate::geom::height(height);
        union = Some(match union {
            None => section.rect,
            Some(previous) => crate::geom::union(previous, section.rect),
        });
    }
    union.unwrap_or(Rect::ZERO)
}

/// The width one character sets at a given size.
#[must_use]
pub(crate) fn word_width(
    word: &Word,
    config: &Config,
    metrics: &Metrics<'_>,
    font_size: f32,
) -> f32 {
    let shown = config.sub_word.map_or(word.ch, |sub| sub as u32);
    #[allow(clippy::cast_precision_loss)]
    let width = (metrics.width)(shown) as f32;
    width * font_size * FONT_SCALE + word.tail
}

/// The ascent one character contributes at a given size.
#[must_use]
pub(crate) fn font_ascent(metrics: &Metrics<'_>, font_size: f32) -> f32 {
    #[allow(clippy::cast_precision_loss)]
    let ascent = metrics.ascent as f32;
    ascent * font_size * FONT_SCALE
}

/// The descent one character contributes at a given size.
#[must_use]
pub(crate) fn font_descent(metrics: &Metrics<'_>, font_size: f32) -> f32 {
    #[allow(clippy::cast_precision_loss)]
    let descent = metrics.descent as f32;
    descent * font_size * FONT_SCALE
}

#[cfg(test)]
pub(crate) mod stub {
    use super::Metrics;

    /// Every character ten units wide, matching the upstream test provider.
    pub(crate) fn width(_: u32) -> i32 {
        10
    }

    /// The stub metrics the ported layout assertions are written against.
    pub(crate) fn metrics() -> Metrics<'static> {
        Metrics {
            width: &width,
            ascent: 10,
            descent: -2,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Alignment, Config, split_sections};

    #[test]
    fn a_quadding_outside_the_defined_range_is_left_aligned() {
        assert_eq!(Alignment::from_quadding(0), Alignment::Left);
        assert_eq!(Alignment::from_quadding(1), Alignment::Center);
        assert_eq!(Alignment::from_quadding(2), Alignment::Right);
        assert_eq!(Alignment::from_quadding(3), Alignment::Left);
        assert_eq!(Alignment::from_quadding(-1), Alignment::Left);
    }

    fn multi(text: &str) -> Vec<Vec<u32>> {
        split_sections(
            text,
            &Config {
                multi_line: true,
                ..Config::default()
            },
        )
    }

    #[test]
    fn a_paired_line_break_counts_once_in_either_order() {
        assert_eq!(multi("a\r\nb").len(), 2);
        assert_eq!(multi("a\n\rb").len(), 2);
        // Two separate breaks are two.
        assert_eq!(multi("a\n\nb").len(), 3);
    }

    #[test]
    fn a_break_in_a_single_line_field_produces_nothing_at_all() {
        let single = split_sections("ab\ncd", &Config::default());
        assert_eq!(single.len(), 1);
        // Not even a space where the break was.
        assert_eq!(single.first().map(Vec::len), Some(4));
    }

    #[test]
    fn a_tab_becomes_a_space() {
        let got = split_sections("a\tb", &Config::default());
        assert_eq!(got.first().map(Vec::as_slice), Some(&[97, 32, 98][..]));
    }

    #[test]
    fn the_character_limit_counts_line_breaks_too() {
        let config = Config {
            multi_line: true,
            limit_char: 5,
            ..Config::default()
        };
        // The counter is tested *before* each character and advanced after,
        // and the break advances it too — so `ab`, the break and `cd` are
        // five steps and all of them land. A sixth character would not.
        let got = split_sections("ab\ncd", &config);
        assert_eq!(got.len(), 2);
        assert_eq!(got.first().map(Vec::len), Some(2));
        assert_eq!(got.get(1).map(Vec::len), Some(2));

        // One more character, and the limit bites.
        let clipped = split_sections("ab\ncde", &config);
        assert_eq!(clipped.get(1).map(Vec::len), Some(2));
    }

    #[test]
    fn a_comb_cell_count_caps_the_characters_as_a_limit_does() {
        let config = Config {
            char_array: 3,
            ..Config::default()
        };
        assert_eq!(
            split_sections("abcdef", &config).first().map(Vec::len),
            Some(3)
        );
    }
}
