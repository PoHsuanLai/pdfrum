//! Blocks from typography alone, for the document that carries no tags.
//!
//! Boxes are in PDF user space, y up: the first line of a page has the
//! largest `y0`, and a wrapped line sits *below* the one before it.
//!
//! The rules, each a number a reader can argue with:
//!
//! - **Margins.** A line whose box sits in the top or bottom 10% of the
//!   page is a running header, footer or page number, and goes, when it
//!   looks like a folio — every word a number, a punctuation mark, a URL,
//!   or `page`/`of` — or when it is short (three words or fewer) and
//!   stands apart from the body: no line outside the band within 1.5 line
//!   heights of it — or when it is short and within a line height of a
//!   line that goes, so a two-line running header goes as one. A page with
//!   fewer than four lines keeps them all. What
//!   one page cannot see is repetition across pages — the same line in the
//!   same band on page after page — which needs a document-level entry
//!   point this crate does not have yet.
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
//!   a new one. Two more rules decide within the gap, the ones every layout
//!   engine applies to justified and left-aligned prose:
//!   - *Ragged right.* A line that stops short of its column's right edge
//!     is its paragraph's last. The edge is the furthest right any body
//!     line sharing its left edge (within a character width) reaches, and
//!     short is more than two character widths — 0.9 × the font size each
//!     — before it, when the next line opens with a capital or a digit. A
//!     short line followed by a lower-case word is a justified paragraph's
//!     last line met by an unlucky wrap, and joins; a short line followed
//!     by a capital is one paragraph ending and the next beginning, so a
//!     run of one-line paragraphs comes out one each.
//!   - *Lead-in.* A line that opens bold and turns plain, or whose first
//!     six words at most are a capitalised phrase — the first word and at
//!     least half of them — closed by ` - ` or ` – `, begins a paragraph
//!     whatever the gap: `XFA Form Filling - XFA (XML Form Architecture)
//!     form allows …`. It stays a paragraph, not a list item; a bold
//!     lead-in is written `**XFA Form Filling** - …`.
//! - **Ligatures.** `ﬁ`, `ﬂ`, `ﬀ`, `ﬃ`, `ﬄ` become their letters.
//! - **Dot leaders.** Five or more dots in a row, spaces between them
//!   allowed, are a table-of-contents leader and become ` ... ` — one
//!   space, three dots, one space. Three dots is an ellipsis and four an
//!   ellipsis before a full stop; both stay as written.

use kurbo::Rect;

use crate::ast::Block;
use crate::lines::Line;

/// The fraction of the page height at each edge that is margin.
const MARGIN_BAND: f64 = 0.10;
/// A margin line this many line heights or more from the body stands apart.
const MARGIN_GAP: f64 = 1.5;
/// A margin line with more words than this is body text, unless it looks
/// like a folio.
const MARGIN_MAX_WORDS: usize = 3;
/// A page with fewer lines than this has nothing to be a header to.
const MARGIN_MIN_LINES: usize = 4;
/// Heading thresholds against the body size.
const H1_RATIO: f32 = 1.6;
const H2_RATIO: f32 = 1.3;
/// A bold line with more words than this is emphasis, not a heading.
const HEADING_MAX_WORDS: usize = 12;
/// A gap between lines wider than this many line heights ends a paragraph.
const PARAGRAPH_GAP: f64 = 0.8;
/// A character's width as a fraction of the font size, the unit the
/// ragged-right rule measures in.
const CHAR_WIDTH: f64 = 0.9;
/// A line stopping more than this many character widths before its
/// column's right edge is ragged.
const RAGGED_CHARS: f64 = 2.0;
/// Lines whose left edges are within this many character widths share a
/// column.
const LEFT_EDGE_CHARS: f64 = 1.0;
/// A lead-in has at most this many words before its dash.
const LEAD_IN_MAX_WORDS: usize = 6;
/// A run of this many dots is a leader, not an ellipsis.
const LEADER_MIN_DOTS: usize = 5;

/// The blocks of a page laid out inside `page` (its crop box).
#[must_use]
pub fn blocks(lines: &[Line], page: Rect) -> Vec<Block> {
    blocks_among(lines, lines, page)
}

/// The blocks of `lines`, judged among `context` — every line the page has,
/// of which `lines` are the ones typography is to read — so a running
/// header is recognised by its distance from a body the structure tree
/// may have claimed.
#[must_use]
pub fn blocks_among(lines: &[Line], context: &[Line], page: Rect) -> Vec<Block> {
    let margin = margin_mask(context, page);
    // A line of `lines` is one of `context`, or a copy of one with the
    // same box: the tree's leftovers keep the box of the line they came
    // from.
    let in_margin = |line: &Line| {
        context
            .iter()
            .zip(&margin)
            .any(|(other, &dropped)| dropped && other.bbox == line.bbox)
    };
    let mut kept: Vec<&Line> = lines.iter().filter(|l| !in_margin(l)).collect();
    // A page with only a few lines, or with nothing outside its margins,
    // has nothing to be a header to, and keeps them all.
    let page_has_body = margin.iter().any(|dropped| !dropped);
    if context.len() < MARGIN_MIN_LINES || !page_has_body {
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

/// Whether a line's centre is in the top or bottom band of the page.
fn at_edge(line: &Line, page: Rect) -> bool {
    if page.height() <= 0.0 || line.bbox.area() <= 0.0 {
        return false;
    }
    let band = page.height() * MARGIN_BAND;
    let center = f64::midpoint(line.bbox.y0, line.bbox.y1);
    center < page.y0 + band || center > page.y1 - band
}

/// Which of the page's lines are running headers, footers or folios: in a
/// band and looking like a folio, or short and standing apart from the
/// body, or short and beside one of those — a two-line header goes as one.
fn margin_mask(context: &[Line], page: Rect) -> Vec<bool> {
    let short = |line: &Line| {
        at_edge(line, page) && line.text.split_whitespace().count() <= MARGIN_MAX_WORDS
    };
    let mut dropped: Vec<bool> = context
        .iter()
        .map(|line| {
            at_edge(line, page)
                && (folio_like(&line.text) || (short(line) && stands_apart(line, page, context)))
        })
        .collect();
    // Grow each header block line by line until nothing more is beside it;
    // every round marks at least one new line, so the loop ends.
    loop {
        let grown: Vec<bool> = context
            .iter()
            .zip(&dropped)
            .map(|(line, &gone)| {
                gone || (short(line)
                    && context
                        .iter()
                        .zip(&dropped)
                        .any(|(other, &other_gone)| other_gone && beside(line, other)))
            })
            .collect();
        if grown == dropped {
            return dropped;
        }
        dropped = grown;
    }
}

/// Whether two lines' boxes are within a line height of each other.
fn beside(a: &Line, b: &Line) -> bool {
    let reach = f64::from(a.font_size.max(b.font_size)).max(1.0);
    (a.bbox.y0 - b.bbox.y1).max(b.bbox.y0 - a.bbox.y1) < reach
}

/// A page number, a URL, or the words around them: `7`, `- 7 -`,
/// `1 / 11 www.example.com`, `Page 3 of 9`.
fn folio_like(text: &str) -> bool {
    let mut words = text.split_whitespace().peekable();
    words.peek().is_some()
        && words.all(|w| {
            w.chars()
                .all(|c| c.is_ascii_digit() || c.is_ascii_punctuation())
                || looks_like_url(w)
                || w.eq_ignore_ascii_case("page")
                || w.eq_ignore_ascii_case("of")
        })
}

fn looks_like_url(word: &str) -> bool {
    let lower = word.to_ascii_lowercase();
    lower.starts_with("www.")
        || lower.contains("://")
        || [".com", ".org", ".net", ".edu", ".gov", ".io"]
            .iter()
            .any(|tld| lower.ends_with(tld) || lower.contains(&format!("{tld}/")))
}

/// Whether no line outside the band comes within [`MARGIN_GAP`] line
/// heights of this one.
fn stands_apart(line: &Line, page: Rect, context: &[Line]) -> bool {
    let reach = f64::from(line.font_size).max(1.0) * MARGIN_GAP;
    // The line itself is in the band, so it never counts as its own body.
    !context.iter().any(|other| {
        !at_edge(other, page)
            && (line.bbox.y0 - other.bbox.y1).max(other.bbox.y0 - line.bbox.y1) < reach
    })
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

/// A character that is a bullet at the start of a line: the typographic
/// ones, the ASCII stand-ins, and any private-use code — a Symbol or
/// Wingdings glyph with no Unicode of its own, at the head of a line and
/// followed by a space, is a bullet by position.
fn is_bullet(c: char) -> bool {
    matches!(
        c,
        '•' | '·' | '‣' | '◦' | '▪' | '-' | '*' | '–' | '\u{e000}'..='\u{f8ff}'
    )
}

fn starts_with_bullet(text: &str) -> bool {
    let mut chars = text.chars();
    chars.next().is_some_and(is_bullet) && chars.next().is_some_and(char::is_whitespace)
}

/// A list item's text with the bullet the producer drew into it stripped.
pub(crate) fn strip_bullet(text: &str) -> &str {
    if starts_with_bullet(text) {
        text.trim_start_matches(is_bullet).trim_start()
    } else {
        text
    }
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
    let rest = text
        .trim_start_matches(|c: char| c.is_ascii_digit() || is_bullet(c) || matches!(c, '.' | ')'));
    rest.trim_start().to_owned()
}

/// Ligatures to letters, and a dot leader to ` ... `.
#[must_use]
pub fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while let Some(&c) = chars.get(i) {
        if c == '.' {
            // The run of dots and spaces from here; a leader is the run
            // with enough dots in it.
            let end = chars
                .iter()
                .skip(i)
                .position(|c| *c != '.' && *c != ' ')
                .map_or(chars.len(), |n| i + n);
            let run = chars.get(i..end).unwrap_or_default();
            let dots = run.iter().filter(|c| **c == '.').count();
            if dots >= LEADER_MIN_DOTS {
                let trimmed = out.trim_end().len();
                out.truncate(trimmed);
                out.push_str(" ... ");
                i = end;
                continue;
            }
            out.extend(run.iter());
            i = end;
            continue;
        }
        match c {
            '\u{fb00}' => out.push_str("ff"),
            '\u{fb01}' => out.push_str("fi"),
            '\u{fb02}' => out.push_str("fl"),
            '\u{fb03}' => out.push_str("ffi"),
            '\u{fb04}' => out.push_str("ffl"),
            '\u{fb05}' | '\u{fb06}' => out.push_str("st"),
            other => out.push(other),
        }
        i += 1;
    }
    out
}

/// Join `next` onto `text` across a line break: de-hyphenate a wrapped
/// word, else a space. Whitespace at the break is the break's, and goes.
pub(crate) fn join(text: &mut String, next: &str) {
    let trimmed = text.trim_end().len();
    text.truncate(trimmed);
    let next = next.trim_start();
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
    // Reading order goes down the page, and y grows up it: a wrapped line
    // sits below the previous one, so the gap is from its top to the
    // previous line's bottom. A line beside or above the previous one is
    // a new column or block.
    let gap = prev.bbox.y0 - next.bbox.y1;
    let size_close = (prev.font_size - next.font_size).abs() <= prev.font_size * 0.15;
    gap < height * PARAGRAPH_GAP && gap > -height * 0.5 && size_close
}

/// A character's width on `line`, the ragged-right rule's unit.
fn char_width(line: &Line) -> f64 {
    f64::from(line.font_size).max(1.0) * CHAR_WIDTH
}

/// The right edge of the column `line` sits in: the furthest right any of
/// `bodies` sharing its left edge reaches, `line` itself among them.
fn column_right_edge(line: &Line, bodies: &[&Line]) -> f64 {
    let reach = char_width(line) * LEFT_EDGE_CHARS;
    bodies
        .iter()
        .filter(|other| (other.bbox.x0 - line.bbox.x0).abs() <= reach)
        .map(|other| other.bbox.x1)
        .fold(line.bbox.x1, f64::max)
}

/// Whether `prev` is its paragraph's last line: it stops short of `edge`,
/// the column's right edge, and `next` opens with a capital or a digit. A
/// short line met by a lower-case word is a justified paragraph's last
/// line and an unlucky wrap, and joins.
fn ends_ragged(prev: &Line, next: &Line, edge: f64) -> bool {
    let short = edge - prev.bbox.x1 > char_width(prev) * RAGGED_CHARS;
    short
        && next
            .text
            .trim_start()
            .chars()
            .next()
            .is_some_and(|c| c.is_uppercase() || c.is_ascii_digit())
}

/// Whether the line's head begins a paragraph: a bold run that turns
/// plain, or a capitalised phrase closed by a dash.
fn starts_lead_in(line: &Line) -> bool {
    has_bold_lead_in(line) || has_dash_lead_in(&line.text)
}

/// A line drawn bold at its head and plain after: the lead-in is the bold.
fn has_bold_lead_in(line: &Line) -> bool {
    line.bold_prefix > 0 && line.bold_prefix < line.text.len()
}

/// A capitalised phrase of at most [`LEAD_IN_MAX_WORDS`] words — the first
/// word and at least half of them capitalised — closed by ` - ` or ` – `:
/// `XFA Form Filling - XFA …`, `Email and Phone Support - helps …`.
fn has_dash_lead_in(text: &str) -> bool {
    let Some(at) = [" - ", " \u{2013} "]
        .iter()
        .filter_map(|dash| text.find(dash))
        .min()
    else {
        return false;
    };
    let words: Vec<&str> = text
        .get(..at)
        .unwrap_or_default()
        .split_whitespace()
        .collect();
    let capitalised = |w: &str| w.chars().next().is_some_and(char::is_uppercase);
    !words.is_empty()
        && words.len() <= LEAD_IN_MAX_WORDS
        && words.first().is_some_and(|w| capitalised(w))
        && words.iter().filter(|w| capitalised(w)).count() * 2 >= words.len()
}

/// A paragraph's opening line, its bold lead-in marked when it has one.
fn paragraph_head(line: &Line) -> String {
    match (
        line.text.get(..line.bold_prefix),
        line.text.get(line.bold_prefix..),
    ) {
        (Some(lead), Some(rest)) if has_bold_lead_in(line) => {
            normalize(&format!("**{lead}**{rest}"))
        }
        _ => normalize(&line.text),
    }
}

fn group(classified: &[(Class, &Line)]) -> Vec<Block> {
    let bodies: Vec<&Line> = classified
        .iter()
        .filter(|(class, _)| *class == Class::Body)
        .map(|(_, line)| *line)
        .collect();
    let mut blocks = Vec::new();
    let mut i = 0;
    while let Some(&(class, line)) = classified.get(i) {
        match class {
            Class::Heading(level) => {
                let mut text = normalize(&line.text);
                let mut j = i + 1;
                while let Some((Class::Heading(l), next)) = classified.get(j).copied()
                    && l == level
                    && classified
                        .get(j - 1)
                        .is_some_and(|(_, prev)| continues(prev, next))
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
                        && classified
                            .get(j - 1)
                            .is_some_and(|(_, prev)| continues(prev, next))
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
                let mut text = paragraph_head(line);
                let mut j = i + 1;
                while let Some((Class::Body, next)) = classified.get(j).copied()
                    && classified.get(j - 1).is_some_and(|(_, prev)| {
                        continues(prev, next)
                            && !ends_ragged(prev, next, column_right_edge(prev, &bodies))
                    })
                    && !starts_lead_in(next)
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
    use super::{blocks, blocks_among, has_dash_lead_in, normalize};
    use crate::ast::Block;
    use crate::lines::Line;
    use kurbo::Rect;

    /// A line whose box has its *top* at `top`, y up.
    fn line(text: &str, top: f64, size: f32, bold: bool, mono: bool) -> Line {
        Line {
            text: text.to_owned(),
            bbox: Rect::new(
                72.0,
                top - f64::from(size),
                72.0 + f64::from(size)
                    * 0.5
                    * f64::from(u16::try_from(text.len()).unwrap_or(u16::MAX)),
                top,
            ),
            font_size: size,
            bold,
            bold_prefix: 0,
            mono,
            mcids: Vec::new(),
            segments: Vec::new(),
        }
    }

    #[test]
    fn headings_paragraphs_lists_and_code_come_out_in_order() {
        let page = Rect::new(0.0, 0.0, 612.0, 792.0);
        let lines = vec![
            line("Running header", 770.0, 8.0, false, false),
            line("A Title", 700.0, 20.0, true, false),
            line("Section one", 660.0, 14.0, false, false),
            line(
                "This is body text that wraps to a sec-",
                640.0,
                10.0,
                false,
                false,
            ),
            line("ond line and then stops.", 628.0, 10.0, false, false),
            line("A new paragraph after a gap.", 600.0, 10.0, false, false),
            line("• first item", 580.0, 10.0, false, false),
            line("• second item", 568.0, 10.0, false, false),
            line("let x = 1;", 540.0, 10.0, false, true),
            line("let y = 2;", 528.0, 10.0, false, true),
            line("Bold lead", 500.0, 10.0, true, false),
            line("7", 30.0, 10.0, false, false),
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

    /// Three consecutive body lines of `text_foxit_products.pdf` page 1,
    /// boxes as the text page reports them on its 841.68 pt page.
    fn body_line(text: &str, y0: f64, y1: f64) -> Line {
        Line {
            text: text.to_owned(),
            bbox: Rect::new(72.0, y0, 520.0, y1),
            font_size: 11.0,
            bold: false,
            bold_prefix: 0,
            mono: false,
            mcids: Vec::new(),
            segments: Vec::new(),
        }
    }

    #[test]
    fn wrapped_lines_at_real_coordinates_join_into_one_paragraph() {
        let page = Rect::new(0.0, 0.0, 595.0, 841.68);
        let lines = vec![
            body_line("Many businesses need more than just PDF", 709.7, 718.8),
            body_line("creation and editing. They need security", 694.1, 703.2),
            body_line("that ensures regulatory compliance.", 678.5, 687.6),
            body_line("A line thirty points lower starts anew.", 648.5, 657.6),
        ];
        assert_eq!(
            blocks(&lines, page),
            vec![
                Block::Paragraph(
                    "Many businesses need more than just PDF creation and editing. They need \
                     security that ensures regulatory compliance."
                        .into()
                ),
                Block::Paragraph("A line thirty points lower starts anew.".into()),
            ]
        );
    }

    /// A 10 pt body line in a column whose left edge is 72, from `x1`
    /// back, its top at `top`; the head of the line drawn bold for
    /// `bold_prefix` bytes.
    fn column_line(text: &str, x1: f64, top: f64, bold_prefix: usize) -> Line {
        Line {
            text: text.to_owned(),
            bbox: Rect::new(72.0, top - 10.0, x1, top),
            font_size: 10.0,
            bold: false,
            bold_prefix,
            mono: false,
            mcids: Vec::new(),
            segments: Vec::new(),
        }
    }

    #[test]
    fn a_line_short_of_the_column_ends_its_paragraph_when_a_capital_follows() {
        // Every line twelve points below the last: within the gap rule.
        let page = Rect::new(0.0, 0.0, 612.0, 792.0);
        let lines = vec![
            column_line(
                "Many businesses need more than just PDF creation. They",
                500.0,
                700.0,
                0,
            ),
            column_line("need security that ensures compliance.", 380.0, 688.0, 0),
            column_line("System Requirements", 170.0, 676.0, 0),
            column_line("Operating Systems", 160.0, 664.0, 0),
            column_line(
                "Microsoft Windows XP Home, Professional, or Tablet PC (32-bit",
                500.0,
                652.0,
                0,
            ),
            column_line("& 64-bit).", 120.0, 640.0, 0),
            column_line("Windows 7 (32-bit & 64-bit).", 210.0, 628.0, 0),
        ];
        let texts: Vec<String> = blocks(&lines, page).iter().map(Block::text).collect();
        assert_eq!(
            texts,
            [
                "Many businesses need more than just PDF creation. They need security that \
                 ensures compliance.",
                "System Requirements",
                "Operating Systems",
                "Microsoft Windows XP Home, Professional, or Tablet PC (32-bit & 64-bit).",
                "Windows 7 (32-bit & 64-bit).",
            ]
        );
    }

    #[test]
    fn a_justified_paragraphs_short_last_line_joins_a_lower_case_wrap() {
        let page = Rect::new(0.0, 0.0, 612.0, 792.0);
        let lines = vec![
            column_line(
                "This paragraph is set justified, so every line but the last",
                500.0,
                700.0,
                0,
            ),
            column_line("reaches the edge; this one is short", 320.0, 688.0, 0),
            column_line(
                "but the next begins in lower case, as a wrapped line does.",
                490.0,
                676.0,
                0,
            ),
        ];
        assert_eq!(
            blocks(&lines, page),
            vec![Block::Paragraph(
                "This paragraph is set justified, so every line but the last reaches the edge; \
                 this one is short but the next begins in lower case, as a wrapped line does."
                    .into()
            )]
        );
    }

    #[test]
    fn a_capitalised_phrase_closed_by_a_dash_begins_a_paragraph_within_the_gap() {
        let page = Rect::new(0.0, 0.0, 612.0, 792.0);
        let lines = vec![
            // Ends thirteen points short of the edge: not ragged.
            column_line(
                "XFA Form Filling - XFA (XML Form Architecture) form allows you to leverage existing XFA forms.",
                494.0,
                700.0,
                0,
            ),
            column_line(
                "High Performance - Up to 3 times faster PDF creation from over 200 of the most common office",
                507.0,
                688.0,
                0,
            ),
            column_line(
                "file types and convert multiple files to PDF in a single operation.",
                364.0,
                676.0,
                0,
            ),
            column_line(
                "Form Design - Easy to use electronic forms design tools to make your office forms work harder.",
                507.0,
                664.0,
                0,
            ),
            // A capital after a full line is a sentence, not a paragraph.
            column_line(
                "Enables you to create or convert static PDF files into professional looking forms. Form data",
                507.0,
                652.0,
                0,
            ),
            column_line(
                "import tools allow data to be automatically imported - reducing manual key entering, the",
                507.0,
                640.0,
                0,
            ),
            // Eight capitalised words before the dash is a sentence too.
            column_line(
                "Active Directory RMS Protector And Policy Manager Extends - the usage control benefits.",
                507.0,
                628.0,
                0,
            ),
        ];
        let texts: Vec<String> = blocks(&lines, page).iter().map(Block::text).collect();
        assert_eq!(
            texts,
            [
                "XFA Form Filling - XFA (XML Form Architecture) form allows you to leverage existing \
                 XFA forms.",
                "High Performance - Up to 3 times faster PDF creation from over 200 of the most common \
                 office file types and convert multiple files to PDF in a single operation.",
                "Form Design - Easy to use electronic forms design tools to make your office forms work \
                 harder. Enables you to create or convert static PDF files into professional looking \
                 forms. Form data import tools allow data to be automatically imported - reducing \
                 manual key entering, the Active Directory RMS Protector And Policy Manager Extends - \
                 the usage control benefits.",
            ]
        );
        assert!(has_dash_lead_in(
            "Email and Phone Support \u{2013} helps when you need it."
        ));
        assert!(!has_dash_lead_in("the result - as expected - was fine"));
    }

    #[test]
    fn a_bold_lead_in_begins_a_paragraph_and_is_written_in_bold() {
        const BOLD_FIRST: &str =
            "A bold line that wraps, with more than twelve words so that it is not a heading, and";
        const BOLD_WRAP: &str =
            "its wrap, bold throughout as well, and long enough to stay a paragraph line too.";
        let page = Rect::new(0.0, 0.0, 612.0, 792.0);
        let lines = vec![
            column_line(
                "Foxit PhantomPDF Business builds upon the capabilities of PhantomPDF Standard and",
                500.0,
                700.0,
                0,
            ),
            column_line(
                "Redaction Lets you permanently remove visible text and images from PDF documents.",
                500.0,
                688.0,
                "Redaction".len(),
            ),
            column_line(
                "Security Validation of digital signatures and encryption with passwords.",
                400.0,
                676.0,
                "Security".len(),
            ),
            // Bold throughout is a bold line, not a lead-in: the two wrap
            // into one paragraph.
            column_line(BOLD_FIRST, 500.0, 664.0, BOLD_FIRST.len()),
            column_line(BOLD_WRAP, 500.0, 652.0, BOLD_WRAP.len()),
        ];
        let got = blocks(&lines, page);
        assert_eq!(
            got,
            vec![
                Block::Paragraph(
                    "Foxit PhantomPDF Business builds upon the capabilities of PhantomPDF Standard and"
                        .into()
                ),
                Block::Paragraph(
                    "**Redaction** Lets you permanently remove visible text and images from PDF \
                     documents."
                        .into()
                ),
                Block::Paragraph(
                    "**Security** Validation of digital signatures and encryption with passwords."
                        .into()
                ),
                Block::Paragraph(format!("{BOLD_FIRST} {BOLD_WRAP}")),
            ]
        );
        assert!(crate::render(&got[1..2]).starts_with("**Redaction** Lets you"));
    }

    #[test]
    fn a_footer_with_a_page_number_and_a_url_is_dropped_even_with_four_words() {
        let page = Rect::new(0.0, 0.0, 595.32, 841.92);
        let lines = vec![
            line("Foxit MobilePDF", 805.0, 10.6, true, false),
            line("Quick Guide", 786.0, 10.6, true, false),
            line("Welcome to Foxit MobilePDF", 614.0, 20.0, true, false),
            line("Instructions:", 549.0, 12.0, true, false),
            line("Different Views 2", 486.0, 11.0, false, false),
            line("1 / 11 www.foxitsoftware.com", 79.7, 9.0, true, false),
        ];
        let got = blocks(&lines, page);
        let texts: Vec<String> = got.iter().map(Block::text).collect();
        assert_eq!(
            texts,
            [
                "Welcome to Foxit MobilePDF",
                "Instructions:",
                "Different Views 2"
            ]
        );
    }

    #[test]
    fn a_short_last_line_of_a_paragraph_near_the_bottom_stays() {
        let page = Rect::new(0.0, 0.0, 612.0, 792.0);
        let lines = vec![
            line("A Title", 700.0, 20.0, true, false),
            line(
                "Body text that runs to the bottom of the page",
                95.0,
                10.0,
                false,
                false,
            ),
            line("and then stops.", 83.0, 10.0, false, false),
            line("Running footer", 30.0, 8.0, false, false),
        ];
        assert_eq!(
            blocks(&lines, page),
            vec![
                Block::Heading {
                    level: 1,
                    text: "A Title".into()
                },
                Block::Paragraph(
                    "Body text that runs to the bottom of the page and then stops.".into()
                ),
            ]
        );
    }

    #[test]
    fn a_header_is_judged_against_the_body_the_tree_claimed() {
        let page = Rect::new(0.0, 0.0, 612.0, 792.0);
        let all = vec![
            line("Running header", 770.0, 8.0, true, false),
            line("Claimed body one", 600.0, 10.0, false, false),
            line("Claimed body two", 588.0, 10.0, false, false),
            line("Claimed body three", 576.0, 10.0, false, false),
        ];
        let unclaimed = vec![all[0].clone()];
        assert_eq!(blocks_among(&unclaimed, &all, page), vec![]);
        // Alone, three lines are too few to have a header.
        assert_eq!(
            blocks(&unclaimed, page),
            vec![Block::Heading {
                level: 3,
                text: "Running header".into()
            }]
        );
    }

    #[test]
    fn a_two_line_header_goes_as_one_even_close_to_a_heading() {
        // The guide's page 2: `Quick Guide` sits twelve points above the
        // `Different Views` heading, closer than it stands apart, but
        // beside `Foxit MobilePDF`, which does.
        let page = Rect::new(0.0, 0.0, 595.32, 841.92);
        let all = vec![
            line("Foxit MobilePDF", 805.1, 10.6, true, false),
            line("Quick Guide", 786.2, 10.6, true, false),
            line("Different Views", 763.7, 18.0, true, false),
            line(
                "Local documents view: list all local files",
                489.0,
                11.0,
                false,
                false,
            ),
            line(
                "Cloud view: list the files in Cloud services",
                450.0,
                11.0,
                false,
                false,
            ),
            line("2 / 11 www.foxitsoftware.com", 79.7, 9.0, true, false),
        ];
        let unclaimed = vec![all[0].clone(), all[1].clone(), all[5].clone()];
        assert_eq!(blocks_among(&unclaimed, &all, page), vec![]);
    }

    #[test]
    fn a_symbol_font_bullet_is_a_bullet() {
        let page = Rect::new(0.0, 0.0, 612.0, 792.0);
        let lines = vec![
            line(
                "\u{f0b7} You can change views by swiping",
                600.0,
                10.0,
                false,
                false,
            ),
            line("\u{f0b7} Another item", 588.0, 10.0, false, false),
        ];
        assert_eq!(
            blocks(&lines, page),
            vec![Block::List {
                ordered: false,
                items: vec![
                    "You can change views by swiping".into(),
                    "Another item".into()
                ]
            }]
        );
    }

    #[test]
    fn ligatures_become_letters() {
        assert_eq!(normalize("e\u{fb03}cient \u{fb01}nd"), "efficient find");
    }

    #[test]
    fn a_dot_leader_collapses_and_an_ellipsis_does_not() {
        assert_eq!(
            normalize("Different Views ................ ................ . 2"),
            "Different Views ... 2"
        );
        assert_eq!(normalize("Gestures......11"), "Gestures ... 11");
        assert_eq!(
            normalize("Well... maybe. Or not...."),
            "Well... maybe. Or not...."
        );
    }
}
