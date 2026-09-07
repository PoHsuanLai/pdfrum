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
//!   fewer than four lines keeps them all. What one page cannot see is
//!   repetition across pages — the same line in the same band on page
//!   after page — which is [`crate::running`]'s to drop before this runs.
//! - **Images.** An image drawn at least 4 pt on each side is a picture and
//!   becomes a [`Block::Image`], placed before the first line that lies
//!   below its centre and overlaps it across — after every line when none
//!   does — and ending whatever paragraph or list was open. An untagged
//!   document says nothing about what a picture shows, so its alternative
//!   text is the only thing the page does say — which image it is, and how
//!   big it was drawn — which is enough to tell two apart and to know a
//!   full-page scan from an icon. A tagged document's `/Alt` is better and
//!   [`crate::tagged`] uses it. Smaller is a
//!   rule, a spacer or a tracking dot, and is not.
//! - **Body size.** The font size most characters use, rounded to a half
//!   point.
//! - **Headings.** A line at 1.6× the body size or more is `#`; at 1.3× or
//!   more, `##`; a bold line at body size, short enough to be a title,
//!   `###`.
//! - **Code.** Lines in a monospaced font, kept line by line.
//! - **Lists.** A line starting with a bullet, or a number followed by a
//!   point or a bracket — `1.`, `2)` — or a section number of several
//!   parts, `2.1` or `5.4.2`, closed by the space before its text. When
//!   every item of a run carries a number, the document's own numbers are
//!   kept: they are what a cross-reference points at, and a renderer that
//!   numbered from one would write the wrong section. Items with no
//!   numbers of their own are numbered by the Markdown renderer.
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
//!   - *Alignment.* A wrapped line keeps some edge of the one above it:
//!     its left in a left-aligned or justified column, its right in a
//!     right-aligned one, or — a hanging indent — it starts inside a
//!     predecessor that reached the body's right edge and had to wrap.
//!     When both edges move and the line above stopped short, each line
//!     was placed on its own — centred, or tabbed — and neither wraps
//!     into the other.
//!   - *Dot leaders.* A line whose text ends in a leader and a target is
//!     a table-of-contents entry, complete in itself, and never wraps
//!     into the next however its edges line up.
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

use crate::ast::{Block, ListMarker};
use crate::lines::{DrawnImage, Line};

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
/// Lines whose left *and* right edges both move by more than this many
/// character widths were each placed on their own, not wrapped.
const ALIGNED_EDGE_CHARS: f64 = 1.0;
/// A lead-in has at most this many words before its dash.
const LEAD_IN_MAX_WORDS: usize = 6;
/// A run of this many dots is a leader, not an ellipsis.
const LEADER_MIN_DOTS: usize = 5;
/// An image drawn narrower or shorter than this many points is a rule or a
/// spacer, not a picture.
const IMAGE_MIN_SIDE: f64 = 4.0;

/// The blocks of a page laid out inside `page` (its crop box), `images`
/// being what it draws besides text.
#[must_use]
pub fn blocks(lines: &[Line], images: &[DrawnImage], page: Rect) -> Vec<Block> {
    blocks_among(lines, lines, images, page)
}

/// The blocks of `lines`, judged among `context` — every line the page has,
/// of which `lines` are the ones typography is to read — so a running
/// header is recognised by its distance from a body the structure tree
/// may have claimed; `images` are placed among them by height.
#[must_use]
pub fn blocks_among(
    lines: &[Line],
    context: &[Line],
    images: &[DrawnImage],
    page: Rect,
) -> Vec<Block> {
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
    let bodies: Vec<&Line> = classified
        .iter()
        .filter(|(class, _)| *class == Class::Body)
        .map(|(_, line)| *line)
        .collect();
    // An image ends whatever was open: the lines on either side of it are
    // grouped apart, against the same column edges.
    let mut blocks = Vec::new();
    let mut run: Vec<(Class, &Line)> = Vec::new();
    for item in place_images(&classified, images) {
        match item {
            Item::Line(class, line) => run.push((class, line)),
            Item::Image(image) => {
                blocks.extend(group(&run, &bodies));
                run.clear();
                blocks.push(Block::Image {
                    alt: image_alt(&image),
                    index: Some(image.index),
                });
            }
        }
    }
    blocks.extend(group(&run, &bodies));
    blocks
}

/// A line or an image, in reading order.
#[derive(Debug, Clone, Copy)]
enum Item<'a> {
    Line(Class, &'a Line),
    Image(DrawnImage),
}

/// An untagged image's alternative text. The document said nothing about
/// what it shows — that is what tagging is for — so the only honest
/// description is the one thing the page does say: which image it is and
/// how big it was drawn, rounded to the point. That is enough for a reader
/// to tell two pictures apart and to know a full-page scan from an icon.
fn image_alt(image: &DrawnImage) -> String {
    format!(
        "Image {} ({}x{})",
        image.index + 1,
        image.bbox.width().round(),
        image.bbox.height().round(),
    )
}

/// The lines with the pictures among them — the images at least
/// [`IMAGE_MIN_SIDE`] on each side — each before the first line that lies
/// below its centre and overlaps it across, else before the first line
/// below it at all, else after every line.
fn place_images<'a>(classified: &[(Class, &'a Line)], images: &[DrawnImage]) -> Vec<Item<'a>> {
    let mut placed: Vec<(usize, DrawnImage)> = images
        .iter()
        .filter(|image| image.bbox.width().min(image.bbox.height()) >= IMAGE_MIN_SIDE)
        .map(|image| {
            let center = f64::midpoint(image.bbox.y0, image.bbox.y1);
            let below = |line: &Line| line.bbox.y1 < center;
            let across = |line: &Line| line.bbox.x0 < image.bbox.x1 && image.bbox.x0 < line.bbox.x1;
            let at = classified
                .iter()
                .position(|(_, line)| below(line) && across(line))
                .or_else(|| classified.iter().position(|(_, line)| below(line)))
                .unwrap_or(classified.len());
            (at, *image)
        })
        .collect();
    placed.sort_by_key(|(at, image)| (*at, image.index));
    let mut out = Vec::with_capacity(classified.len() + placed.len());
    let mut pictures = placed.into_iter().peekable();
    for (at, &(class, line)) in classified.iter().enumerate() {
        while let Some((_, image)) = pictures.next_if(|(slot, _)| *slot == at) {
            out.push(Item::Image(image));
        }
        out.push(Item::Line(class, line));
    }
    out.extend(pictures.map(|(_, image)| Item::Image(image)));
    out
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

/// A script's numerals and the punctuation that closes a label written in
/// them. A marker style is one row of this table, so supporting a new
/// script is one entry rather than an edit in three `matches!` arms.
///
/// `open` is the bracket a *bracketing* style opens with — `（1）`, whose
/// number is fenced on both sides rather than merely closed. It is a
/// generalisation of the plain style, not a separate one: a style with no
/// `open` simply has nothing to consume before the numerals.
///
/// `joins_groups` says whether a label may be multi-part (`2.1`, `5.4.2`).
/// Only the plain style is: a bracketed number is a single ordinal.
///
/// Every field is filled from a shape a corpus document actually carries.
/// Closers and brackets with no document behind them — `(1)` in ASCII
/// brackets, `1。`, a bare `1）` — are deliberately absent: widening the
/// table on a guess would make lines into lists the document never marked,
/// which is the inference this crate declines to make.
struct MarkerStyle {
    /// The bracket the label opens with, for a bracketing style.
    open: Option<char>,
    /// Whether a character is a numeral of this style's script.
    numeral: fn(char) -> bool,
    /// The punctuation that may close the label.
    closers: &'static [char],
    /// Whether `n.n` may join two groups into one label.
    joins_groups: bool,
}

/// Every marker style read here, most specific first. Each is justified by
/// a shape the corpus actually carries; a style with no evidence in a
/// document is not guessed at, per the crate's scope rule.
const MARKER_STYLES: &[MarkerStyle] = &[
    // `（1）` — a full-width bracketed ordinal, the sub-item marker of
    // `text_cjk_functions`.
    MarkerStyle {
        open: Some('（'),
        numeral: |c| c.is_ascii_digit() || is_fullwidth_digit(c),
        closers: &['）'],
        joins_groups: false,
    },
    // `1.`, `2)`, `2.1`, and the CJK-closed `1.1、` of
    // `text_cjk_functions`.
    MarkerStyle {
        open: None,
        numeral: |c| c.is_ascii_digit() || is_fullwidth_digit(c),
        closers: &['.', ')', '、'],
        joins_groups: true,
    },
];

/// U+FF10-U+FF19, the full-width forms of `0`-`9`.
fn is_fullwidth_digit(c: char) -> bool {
    ('\u{ff10}'..='\u{ff19}').contains(&c)
}

/// The number a line opens with, when it opens with one: `1.`, `2)`, the
/// section numbers `2.1` and `5.4.2` that a table of contents and a
/// numbered heading carry, the CJK-closed `1.1、`, and the full-width
/// bracketed `（1）`. Each group is at most three numerals, and the label
/// is closed by one of its style's closers or — for a multi-part number —
/// by the space before the text.
fn leading_number(text: &str) -> Option<&str> {
    MARKER_STYLES
        .iter()
        .find_map(|style| leading_number_in(text, style))
}

/// The label at the head of `text` read as one style, or `None` if the
/// text does not open in that style.
fn leading_number_in<'a>(text: &'a str, style: &MarkerStyle) -> Option<&'a str> {
    let mut end = 0;
    if let Some(open) = style.open {
        if !text.starts_with(open) {
            return None;
        }
        end += open.len_utf8();
    }
    let mut groups = 0;
    loop {
        let numerals: usize = text
            .get(end..)?
            .chars()
            .take_while(|c| (style.numeral)(*c))
            .map(char::len_utf8)
            .sum();
        let count = text.get(end..end + numerals)?.chars().count();
        if count == 0 || count > 3 {
            return None;
        }
        end += numerals;
        groups += 1;
        match text.get(end..)?.chars().next() {
            // A point may close the label or join the next group; a
            // numeral after it means another group.
            Some('.')
                if style.joins_groups
                    && text
                        .get(end + 1..)?
                        .starts_with(|c: char| (style.numeral)(c)) =>
            {
                end += 1;
            }
            Some(c) if style.closers.contains(&c) => {
                let closed = text.get(end + c.len_utf8()..)?;
                // A bracketed label needs no space after it: `（1）显示`
                // is closed by the bracket itself.
                let separated = style.open.is_some() || closed.starts_with(char::is_whitespace);
                return (separated && !closed.trim().is_empty())
                    .then(|| text.get(..end + c.len_utf8()))?;
            }
            // `2.1 Background`: the space closes a multi-part number.
            Some(c) if c.is_whitespace() && groups > 1 => {
                return (!text.get(end..)?.trim().is_empty()).then(|| text.get(..end))?;
            }
            _ => return None,
        }
    }
}

fn starts_with_number(text: &str) -> bool {
    leading_number(text).is_some()
}

/// Bullet or number stripped.
fn item_text(text: &str) -> String {
    let rest = match leading_number(text) {
        Some(label) => text.get(label.len()..).unwrap_or_default(),
        None => text.trim_start_matches(is_bullet),
    };
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

/// The furthest right any body line on the page reaches: the right edge a
/// wrapped line must have run into.
fn page_right_edge(bodies: &[&Line]) -> f64 {
    bodies
        .iter()
        .map(|line| line.bbox.x1)
        .fold(f64::NEG_INFINITY, f64::max)
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

/// Whether `prev` and `next` were laid out as one wrapped column: a
/// wrapped line keeps *some* edge — a left-aligned or justified paragraph
/// keeps its left, a right-aligned one its right, and a centred pair of
/// lines the same length keeps both. When both edges move, each line was
/// placed on its own — centred, right-aligned or tabbed — and neither
/// wraps into the other.
///
/// This is what a page of `text_tcpdf_063.pdf` needs: nine left-aligned
/// lines break on the ragged-right rule, and the centred, right-aligned
/// and justified blocks below them, whose every line starts and ends
/// somewhere else, would run together into one paragraph if no
/// column edge could be measured for them.
fn shares_an_edge(prev: &Line, next: &Line, edge: f64) -> bool {
    let reach = char_width(prev).max(char_width(next)) * ALIGNED_EDGE_CHARS;
    let same_left = (prev.bbox.x0 - next.bbox.x0).abs() <= reach;
    let same_right = (prev.bbox.x1 - next.bbox.x1).abs() <= reach;
    // A hanging indent — a wrapped cell in a two-column definition list —
    // shares neither edge, and is still a continuation. What marks it is
    // that the line it continues *ran out of room*: it reached the body's
    // right edge and had to wrap. A centred line stops well short of that
    // edge, so a shorter centred line that happens to sit inside its
    // predecessor's span is not mistaken for one.
    let wrapped = prev.bbox.x1 >= edge - char_width(prev) * RAGGED_CHARS;
    let indented = wrapped && next.bbox.x0 > prev.bbox.x0 + reach && next.bbox.x1 <= prev.bbox.x1;
    same_left || same_right || indented
}

/// Whether a line is a table-of-contents entry: text, a dot leader, and a
/// page number at the end. [`normalize`] has not run yet, so the leader is
/// still the producer's own run of dots.
///
/// Such a line is complete in itself and never wraps into the next, which
/// is what `vector_en_tem.pdf`'s `2.1 Background ... 4` needs: those
/// entries share a left edge *and*, the leaders reaching the same page
/// number column, a right edge, so no other rule tells them apart.
fn is_leader_entry(text: &str) -> bool {
    // The entry's target: the page number, or a producer's `错误!未定义书签。`
    // where the reference broke. Whatever it is, it does not contain a dot,
    // so the leader is the run of dots and spaces just before it.
    let head = text
        .trim_end()
        .trim_end_matches(|c: char| c != '.' && c != ' ');
    head.chars()
        .rev()
        .take_while(|c| *c == '.' || *c == ' ')
        .filter(|c| *c == '.')
        .count()
        >= LEADER_MIN_DOTS
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

/// The blocks of a run of classified lines, `bodies` being every body line
/// of the page, which is what the ragged-right rule measures columns on.
fn group(classified: &[(Class, &Line)], bodies: &[&Line]) -> Vec<Block> {
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
                let mut labels: Vec<String> = leading_number(&line.text)
                    .map(|l| vec![l.trim_end_matches(')').to_owned()])
                    .unwrap_or_default();
                let mut items = vec![normalize(&item_text(&line.text))];
                let mut j = i + 1;
                while let Some((c, next)) = classified.get(j).copied() {
                    if c == class {
                        if let Some(label) = leading_number(&next.text) {
                            labels.push(label.trim_end_matches(')').to_owned());
                        }
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
                // The document's own numbers are kept when every item
                // carries one, so `2.1` stays `2.1` and a list starting at
                // 3 starts at 3; anything else the renderer numbers.
                let marker = if class == Class::Bullet {
                    ListMarker::Bullet
                } else if labels.len() == items.len() {
                    ListMarker::Labelled(labels)
                } else {
                    ListMarker::Ordered
                };
                blocks.push(Block::List { marker, items });
                i = j;
            }
            Class::Body => {
                let mut text = paragraph_head(line);
                let mut j = i + 1;
                while let Some((Class::Body, next)) = classified.get(j).copied()
                    && classified.get(j - 1).is_some_and(|(_, prev)| {
                        continues(prev, next)
                            && shares_an_edge(prev, next, page_right_edge(bodies))
                            && !is_leader_entry(&prev.text)
                            && !ends_ragged(prev, next, column_right_edge(prev, bodies))
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
    use super::{
        blocks, blocks_among, has_dash_lead_in, is_leader_entry, item_text, leading_number,
        normalize,
    };
    use crate::ast::{Block, ListMarker};
    use crate::lines::{DrawnImage, Line};
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
    fn a_picture_is_placed_by_height_and_a_spacer_is_not_a_picture() {
        let page = Rect::new(0.0, 0.0, 612.0, 792.0);
        let lines = vec![
            line("First paragraph line one", 700.0, 10.0, false, false),
            line("and line two.", 688.0, 10.0, false, false),
            line(
                "Second paragraph, below the picture.",
                500.0,
                10.0,
                false,
                false,
            ),
        ];
        let image = |index: usize, bbox: Rect| DrawnImage {
            index,
            mcid: None,
            bbox,
        };
        let images = [
            // A rule the width of the column, half a point tall.
            image(0, Rect::new(72.0, 680.0, 400.0, 680.5)),
            // The picture, between the paragraphs.
            image(1, Rect::new(72.0, 540.0, 300.0, 660.0)),
            // A picture below every line, in another column.
            image(2, Rect::new(400.0, 100.0, 500.0, 200.0)),
        ];
        assert_eq!(
            blocks(&lines, &images, page),
            vec![
                Block::Paragraph("First paragraph line one and line two.".into()),
                Block::Image {
                    alt: "Image 2 (228x120)".into(),
                    index: Some(1),
                },
                Block::Paragraph("Second paragraph, below the picture.".into()),
                Block::Image {
                    alt: "Image 3 (100x100)".into(),
                    index: Some(2),
                },
            ]
        );
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
        let got = blocks(&lines, &[], page);
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
                    marker: ListMarker::Bullet,
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
            blocks(&lines, &[], page),
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
        let texts: Vec<String> = blocks(&lines, &[], page).iter().map(Block::text).collect();
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
            blocks(&lines, &[], page),
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
        let texts: Vec<String> = blocks(&lines, &[], page).iter().map(Block::text).collect();
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
        let got = blocks(&lines, &[], page);
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
        let got = blocks(&lines, &[], page);
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
            blocks(&lines, &[], page),
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
        assert_eq!(blocks_among(&unclaimed, &all, &[], page), vec![]);
        // Alone, three lines are too few to have a header.
        assert_eq!(
            blocks(&unclaimed, &[], page),
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
        assert_eq!(blocks_among(&unclaimed, &all, &[], page), vec![]);
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
            blocks(&lines, &[], page),
            vec![Block::List {
                marker: ListMarker::Bullet,
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

    /// Defect 1, `text_tcpdf_063.pdf`: nine centred lines at normal
    /// leading, each starting and ending somewhere else. Every one is its
    /// own paragraph; they would run together if no rule could
    /// measure a column edge for text that is not left-aligned.
    #[test]
    fn centred_lines_that_share_no_edge_are_each_their_own_paragraph() {
        let page = Rect::new(0.0, 0.0, 612.0, 792.0);
        let centred = |text: &str, half_width: f64, top: f64| Line {
            text: text.to_owned(),
            bbox: Rect::new(306.0 - half_width, top - 12.0, 306.0 + half_width, top),
            font_size: 12.0,
            bold: false,
            bold_prefix: 0,
            mono: false,
            mcids: Vec::new(),
            segments: Vec::new(),
        };
        let lines = vec![
            centred("CENTER | Stretching = 90%", 120.0, 700.0),
            centred("CENTER | Stretching = 100%", 132.0, 682.0),
            centred("CENTER | Stretching = 110%", 145.0, 664.0),
        ];
        let texts: Vec<String> = blocks(&lines, &[], page).iter().map(Block::text).collect();
        assert_eq!(
            texts,
            [
                "CENTER | Stretching = 90%",
                "CENTER | Stretching = 100%",
                "CENTER | Stretching = 110%",
            ]
        );
    }

    /// A wrapped line indented inside its predecessor still joins it, so
    /// the rule above does not break `text_cjk_page.pdf`'s two-column
    /// definition rows, whose first line reaches the body's right edge.
    #[test]
    fn a_hanging_indent_after_a_full_line_still_joins_its_paragraph() {
        let page = Rect::new(0.0, 0.0, 612.0, 792.0);
        let lines = vec![
            column_line(
                "MediaBox rectangle a rectangle in user space",
                505.0,
                700.0,
                0,
            ),
            column_line("that bounds the page", 324.0, 688.0, 0),
        ];
        // The continuation starts indented, at x0 230 rather than 72.
        let mut lines = lines;
        if let Some(second) = lines.get_mut(1) {
            second.bbox = Rect::new(230.0, 678.0, 324.0, 688.0);
        }
        assert_eq!(
            blocks(&lines, &[], page),
            vec![Block::Paragraph(
                "MediaBox rectangle a rectangle in user space that bounds the page".into()
            )]
        );
    }

    /// Defect 1, `vector_en_tem.pdf`: table-of-contents entries share a
    /// left edge and, their leaders all reaching the page-number column, a
    /// right edge too. The leader itself says each is complete.
    #[test]
    fn a_dot_leader_entry_never_wraps_into_the_next() {
        assert!(is_leader_entry("2.1 Background ...................4"));
        assert!(is_leader_entry(
            "5.4 Virus Scan ......... \u{9519}\u{8bef}!"
        ));
        // Three dots is an ellipsis, not a leader.
        assert!(!is_leader_entry("and so on ... more words"));
        assert!(!is_leader_entry("A plain sentence that wraps"));

        let page = Rect::new(0.0, 0.0, 612.0, 792.0);
        // Entries with no number of their own: body lines sharing both a
        // left edge and, the leaders reaching the same column, a right one.
        let lines = vec![
            column_line("Revision History ...................... 2", 500.0, 700.0, 0),
            column_line("Appendix ............................. 11", 500.0, 688.0, 0),
        ];
        let texts: Vec<String> = blocks(&lines, &[], page).iter().map(Block::text).collect();
        assert_eq!(texts, ["Revision History ... 2", "Appendix ... 11"]);
    }

    /// Defect 2: the document's own numbers are what a cross-reference
    /// points at, so `2.1` stays `2.1` and a run that goes 1, 2, 2.1, 3
    /// stays one list instead of splitting and restarting at 1.
    #[test]
    fn a_documents_own_numbers_are_kept_and_do_not_split_the_list() {
        assert_eq!(leading_number("1. Definition"), Some("1."));
        assert_eq!(leading_number("2) Introduction"), Some("2)"));
        assert_eq!(leading_number("2.1 Background"), Some("2.1"));
        assert_eq!(leading_number("5.4.2 Virus Scan"), Some("5.4.2"));
        assert_eq!(leading_number("2.1"), None);
        assert_eq!(leading_number("2019 was a year"), None);
        assert_eq!(leading_number("Plain text"), None);

        // Widening to the CJK closers and the bracketing marker must not
        // widen what counts as a marker in the first place: a decimal that
        // opens a sentence is still prose, and so is a bare bracketed
        // number with nothing after it.
        assert_eq!(leading_number("3,14 is pi"), None);
        assert_eq!(leading_number("1,024 bytes"), None);
        assert_eq!(leading_number("（1）"), None);
    }

    /// `1.1、 全部文档` and `（1）显示` from `text_cjk_functions`: a label
    /// closed by the CJK enumeration comma, and one fenced in full-width
    /// brackets. Both were read as prose before, so each line swallowed
    /// the sub-items under it into one paragraph.
    #[test]
    fn a_cjk_closer_and_a_full_width_bracket_are_list_markers() {
        assert_eq!(leading_number("1.1、 全部文档"), Some("1.1、"));
        assert_eq!(leading_number("1、 文件管理"), Some("1、"));
        assert_eq!(leading_number("（1）显示: 大小图标切换"), Some("（1）"));
        assert_eq!(leading_number("（12） 十二"), Some("（12）"));
        // The same shapes in the full-width numerals U+FF10-U+FF19.
        assert_eq!(leading_number("１. 全部文档"), Some("１."));
        assert_eq!(leading_number("（１）显示"), Some("（１）"));

        // A bracket with no number, an unclosed bracket, and a marker with
        // nothing after it are all not markers.
        assert_eq!(leading_number("（一）显示"), None);
        assert_eq!(leading_number("（1 显示"), None);
        assert_eq!(leading_number("1、"), None);
    }

    /// The label a marker carries is stripped from the item's text, so a
    /// `Labelled` list does not print its number twice.
    #[test]
    fn a_cjk_marker_is_stripped_from_the_item_text() {
        assert_eq!(item_text("1.1、 全部文档"), "全部文档");
        assert_eq!(item_text("（1）显示: 大小图标切换"), "显示: 大小图标切换");
        assert_eq!(item_text("1. Definition"), "Definition");

        let page = Rect::new(0.0, 0.0, 612.0, 792.0);
        let lines = vec![
            column_line("1. Definition", 200.0, 700.0, 0),
            column_line("2. Introduction", 200.0, 688.0, 0),
            column_line("2.1 Background", 200.0, 676.0, 0),
            column_line("3. Purpose", 200.0, 664.0, 0),
        ];
        assert_eq!(
            blocks(&lines, &[], page),
            vec![Block::List {
                marker: ListMarker::Labelled(vec![
                    "1.".into(),
                    "2.".into(),
                    "2.1".into(),
                    "3.".into(),
                ]),
                items: vec![
                    "Definition".into(),
                    "Introduction".into(),
                    "Background".into(),
                    "Purpose".into(),
                ],
            }]
        );
    }

    /// Defect 4: an untagged image says nothing about itself, so its alt
    /// text is the one thing the page does say — which image, and how big.
    #[test]
    fn an_untagged_image_is_described_by_its_index_and_size() {
        let page = Rect::new(0.0, 0.0, 612.0, 792.0);
        let lines = vec![column_line("Some text.", 200.0, 700.0, 0)];
        let images = [
            DrawnImage {
                index: 0,
                mcid: None,
                bbox: Rect::new(72.0, 400.0, 300.0, 600.0),
            },
            DrawnImage {
                index: 1,
                mcid: None,
                bbox: Rect::new(72.0, 100.0, 122.0, 140.0),
            },
        ];
        assert_eq!(
            blocks(&lines, &images, page),
            vec![
                Block::Paragraph("Some text.".into()),
                Block::Image {
                    alt: "Image 1 (228x200)".into(),
                    index: Some(0),
                },
                Block::Image {
                    alt: "Image 2 (50x40)".into(),
                    index: Some(1),
                },
            ]
        );
    }
}
