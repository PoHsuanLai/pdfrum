//! Blocks from typography alone, for the document that carries no tags.
//!
//! The rules, each a number a reader can argue with:
//!
//! - **Margins.** A short line whose box sits in the top or bottom 8% of
//!   the page is a running header, footer or page number, and goes.
//! - **Body size.** The font size most characters use, rounded to a half
//!   point.
//! - **Headings.** A line at 1.6× the body size or more is `#`; at 1.3× or
//!   more, `##`; a bold line at body size, short enough to be a title,
//!   `###`.
//! - **Code.** Lines in a monospaced font, kept line by line.
//! - **Lists.** A line starting with a bullet, or a number followed by a
//!   point or a bracket.
//! - **Paragraphs.** Consecutive body lines join with a space; a line ending
//!   in a hyphen followed by a lower-case letter is de-hyphenated; a
//!   vertical gap of more than 0.8 line heights, or a size change, starts
//!   a new one.
//! - **Ligatures.** `ﬁ`, `ﬂ`, `ﬀ`, `ﬃ`, `ﬄ` become their letters.

use kurbo::Rect;

use crate::ast::Block;
use crate::lines::Line;

/// The fraction of the page height at each edge that is margin.
const MARGIN_BAND: f64 = 0.08;
/// Heading thresholds against the body size.
const H1_RATIO: f32 = 1.6;
const H2_RATIO: f32 = 1.3;
/// A bold line with more words than this is emphasis, not a heading.
const HEADING_MAX_WORDS: usize = 12;
/// A gap between lines wider than this many line heights ends a paragraph.
const PARAGRAPH_GAP: f64 = 0.8;

/// The blocks of a page laid out inside `page` (its crop box).
#[must_use]
pub fn blocks(lines: &[Line], page: Rect) -> Vec<Block> {
    // Headers and footers are a pattern across a page's lines; a page with
    // only a few has nothing to be a header to, and keeps them all.
    let mut kept: Vec<&Line> = lines.iter().filter(|l| !in_margin(l, page)).collect();
    if lines.len() < 4 || kept.is_empty() {
        kept = lines.iter().collect();
    }
    let body = body_size(&kept);
    let classified: Vec<(Class, &Line)> = kept.iter().map(|l| (classify(l, body), *l)).collect();
    group(&classified)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    Heading(u8),
    Code,
    Bullet,
    Numbered,
    Body,
}

fn in_margin(line: &Line, page: Rect) -> bool {
    if page.height() <= 0.0 {
        return false;
    }
    let band = page.height() * MARGIN_BAND;
    let center = f64::midpoint(line.bbox.y0, line.bbox.y1);
    let at_edge = center < page.y0 + band || center > page.y1 - band;
    let short = line.text.split_whitespace().count() <= 3;
    at_edge && short
}

/// The most common size, weighted by characters, rounded to a half point.
fn body_size(lines: &[&Line]) -> f32 {
    let mut votes: Vec<(f32, usize)> = Vec::new();
    for line in lines {
        let size = (line.font_size * 2.0).round() / 2.0;
        if size <= 0.0 {
            continue;
        }
        match votes.iter_mut().find(|(s, _)| (*s - size).abs() < 0.01) {
            Some((_, n)) => *n += line.text.len(),
            None => votes.push((size, line.text.len())),
        }
    }
    votes
        .into_iter()
        .max_by_key(|(_, n)| *n)
        .map_or(10.0, |(size, _)| size)
}

fn classify(line: &Line, body: f32) -> Class {
    let words = line.text.split_whitespace().count();
    if line.mono {
        return Class::Code;
    }
    if body > 0.0 {
        let ratio = line.font_size / body;
        if ratio >= H1_RATIO && words <= HEADING_MAX_WORDS * 2 {
            return Class::Heading(1);
        }
        if ratio >= H2_RATIO && words <= HEADING_MAX_WORDS * 2 {
            return Class::Heading(2);
        }
        if line.bold && ratio >= 0.95 && words <= HEADING_MAX_WORDS && !ends_like_prose(&line.text)
        {
            return Class::Heading(3);
        }
    }
    if starts_with_bullet(&line.text) {
        return Class::Bullet;
    }
    if starts_with_number(&line.text) {
        return Class::Numbered;
    }
    Class::Body
}

fn ends_like_prose(text: &str) -> bool {
    text.ends_with(['.', ',', ';', ':'])
}

fn starts_with_bullet(text: &str) -> bool {
    let mut chars = text.chars();
    matches!(
        chars.next(),
        Some('•' | '·' | '‣' | '◦' | '▪' | '-' | '*' | '–')
    ) && chars.next().is_some_and(char::is_whitespace)
}

fn starts_with_number(text: &str) -> bool {
    let digits: String = text.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() || digits.len() > 3 {
        return false;
    }
    let rest = &text[digits.len()..];
    let mut chars = rest.chars();
    matches!(chars.next(), Some('.' | ')')) && chars.next().is_some_and(char::is_whitespace)
}

/// Bullet or number stripped.
fn item_text(text: &str) -> String {
    let rest = text.trim_start_matches(|c: char| {
        c.is_ascii_digit() || matches!(c, '•' | '·' | '‣' | '◦' | '▪' | '-' | '*' | '–' | '.' | ')')
    });
    rest.trim_start().to_owned()
}

/// Ligatures to letters.
pub fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\u{fb00}' => out.push_str("ff"),
            '\u{fb01}' => out.push_str("fi"),
            '\u{fb02}' => out.push_str("fl"),
            '\u{fb03}' => out.push_str("ffi"),
            '\u{fb04}' => out.push_str("ffl"),
            '\u{fb05}' | '\u{fb06}' => out.push_str("st"),
            other => out.push(other),
        }
    }
    out
}

/// Join `next` onto `text`: de-hyphenate a wrapped word, else a space.
fn join(text: &mut String, next: &str) {
    let wrapped = text.ends_with('-')
        && text.chars().rev().nth(1).is_some_and(char::is_alphabetic)
        && next.chars().next().is_some_and(char::is_lowercase);
    if wrapped {
        text.pop();
    } else if !text.is_empty() {
        text.push(' ');
    }
    text.push_str(next);
}

/// Whether two consecutive lines belong to one paragraph.
fn continues(prev: &Line, next: &Line) -> bool {
    let height = f64::from(prev.font_size.max(next.font_size)).max(1.0);
    // Boxes are in the text page's space, y down the page.
    let gap = next.bbox.y0 - prev.bbox.y1;
    let size_close = (prev.font_size - next.font_size).abs() <= prev.font_size * 0.15;
    // Reading order goes down the page: a line beside or above the previous
    // one is a new column or block, and a wrapped line sits below it.
    gap < height * PARAGRAPH_GAP && gap > -height * 0.5 && size_close
}

fn group(classified: &[(Class, &Line)]) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut i = 0;
    while i < classified.len() {
        let (class, line) = classified[i];
        match class {
            Class::Heading(level) => {
                let mut text = normalize(&line.text);
                let mut j = i + 1;
                while let Some((Class::Heading(l), next)) = classified.get(j).copied()
                    && l == level
                    && continues(classified[j - 1].1, next)
                {
                    join(&mut text, &normalize(&next.text));
                    j += 1;
                }
                blocks.push(Block::Heading { level, text });
                i = j;
            }
            Class::Code => {
                let mut text = line.text.clone();
                let mut j = i + 1;
                while let Some((Class::Code, next)) = classified.get(j).copied() {
                    text.push('\n');
                    text.push_str(&next.text);
                    j += 1;
                }
                blocks.push(Block::Code(text));
                i = j;
            }
            Class::Bullet | Class::Numbered => {
                let ordered = class == Class::Numbered;
                let mut items = vec![normalize(&item_text(&line.text))];
                let mut j = i + 1;
                while let Some((c, next)) = classified.get(j).copied() {
                    if c == class {
                        items.push(normalize(&item_text(&next.text)));
                    } else if c == Class::Body
                        && continues(classified[j - 1].1, next)
                        && next.bbox.x0 > line.bbox.x0 + 2.0
                    {
                        // A wrapped item continues indented past the bullet.
                        if let Some(last) = items.last_mut() {
                            join(last, &normalize(&next.text));
                        }
                    } else {
                        break;
                    }
                    j += 1;
                }
                blocks.push(Block::List { ordered, items });
                i = j;
            }
            Class::Body => {
                let mut text = normalize(&line.text);
                let mut j = i + 1;
                while let Some((Class::Body, next)) = classified.get(j).copied()
                    && continues(classified[j - 1].1, next)
                {
                    join(&mut text, &normalize(&next.text));
                    j += 1;
                }
                blocks.push(Block::Paragraph(text));
                i = j;
            }
        }
    }
    blocks
}

#[cfg(test)]
mod tests {
    use super::{blocks, normalize};
    use crate::ast::Block;
    use crate::lines::Line;
    use kurbo::Rect;

    fn line(text: &str, y: f64, size: f32, bold: bool, mono: bool) -> Line {
        Line {
            text: text.to_owned(),
            bbox: Rect::new(
                72.0,
                y,
                72.0 + f64::from(size)
                    * 0.5
                    * f64::from(u16::try_from(text.len()).unwrap_or(u16::MAX)),
                y + f64::from(size),
            ),
            font_size: size,
            bold,
            mono,
            mcids: Vec::new(),
            segments: Vec::new(),
        }
    }

    #[test]
    fn headings_paragraphs_lists_and_code_come_out_in_order() {
        let page = Rect::new(0.0, 0.0, 612.0, 792.0);
        let lines = vec![
            line("Running header", 22.0, 8.0, false, false),
            line("A Title", 92.0, 20.0, true, false),
            line("Section one", 132.0, 14.0, false, false),
            line(
                "This is body text that wraps to a sec-",
                152.0,
                10.0,
                false,
                false,
            ),
            line("ond line and then stops.", 164.0, 10.0, false, false),
            line("A new paragraph after a gap.", 192.0, 10.0, false, false),
            line("• first item", 212.0, 10.0, false, false),
            line("• second item", 224.0, 10.0, false, false),
            line("let x = 1;", 252.0, 10.0, false, true),
            line("let y = 2;", 264.0, 10.0, false, true),
            line("Bold lead", 292.0, 10.0, true, false),
            line("7", 772.0, 10.0, false, false),
        ];
        let got = blocks(&lines, page);
        assert_eq!(
            got,
            vec![
                Block::Heading {
                    level: 1,
                    text: "A Title".into()
                },
                Block::Heading {
                    level: 2,
                    text: "Section one".into()
                },
                Block::Paragraph(
                    "This is body text that wraps to a second line and then stops.".into()
                ),
                Block::Paragraph("A new paragraph after a gap.".into()),
                Block::List {
                    ordered: false,
                    items: vec!["first item".into(), "second item".into()]
                },
                Block::Code("let x = 1;\nlet y = 2;".into()),
                Block::Heading {
                    level: 3,
                    text: "Bold lead".into()
                },
            ]
        );
    }

    #[test]
    fn ligatures_become_letters() {
        assert_eq!(normalize("e\u{fb03}cient \u{fb01}nd"), "efficient find");
    }
}
