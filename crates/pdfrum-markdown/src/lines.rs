//! From characters to lines: the model both tiers and the layout printer
//! read.
//!
//! The extractor already decided where lines break (it inserts a generated
//! `\r\n` between them, the way the oracle does); this module walks its
//! characters, joins each run into a [`Line`] with a box, a font size and
//! the typographic facts the page graph knows about the objects that drew
//! it — whether the font's name says bold or monospaced, whether the text
//! was filled and stroked for a faux bold, and which marked-content id it
//! sits under.
//!
//! Boxes are the text page's, in PDF user space: y grows **up** the page,
//! so the first line of a column has the largest `y0` and a wrapped line
//! sits below its predecessor at a smaller one.
//!
//! A line's text is the text page's text for that line, spacing included:
//! the extractor's spaces — drawn or generated from a gap — are the
//! contract, and nothing here invents or removes one. The one thing removed
//! is a character drawn again where one already is: same character, boxes
//! overlapping by half or origins within a point. A producer's faux bold or
//! shadow draws a whole run twice, and the extractor drops the second text
//! object only when it matches the first exactly; this catches the rest.

use std::collections::HashMap;

use kurbo::Rect;
use pdfrum_page::TextRenderMode;
use pdfrum_page::{Page, PageObject};
use pdfrum_text::{CharBox, CharIndex, CharType, TextPage};

/// One line of text as laid out on the page.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    /// The characters, generated line breaks excluded, trailing space
    /// trimmed.
    pub text: String,
    /// The union of the characters' boxes, in page space, y up.
    pub bbox: Rect,
    /// The median size the characters are drawn at, in points: the font
    /// size in force scaled by the text matrix.
    pub font_size: f32,
    /// Whether most of the characters came from a font whose name says
    /// bold, or were filled and stroked for a faux bold.
    pub bold: bool,
    /// Whether most came from a font whose name says monospaced.
    pub mono: bool,
    /// The marked-content ids the characters sit under, in order, deduplicated.
    pub mcids: Vec<i64>,
    /// The line cut at every change of marked-content id, in order: what
    /// the tagged tier reads.
    pub segments: Vec<Segment>,
}

/// One stretch of a line under one marked-content id.
#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    /// The id, or `None` for text outside any mark.
    pub mcid: Option<i64>,
    /// The characters, spacing exactly as the text page has it — a space
    /// the extractor generated at the boundary belongs to the stretch
    /// before it.
    pub text: String,
    /// The union of the characters' boxes; [`Rect::ZERO`] when none had
    /// area, which is a stretch of nothing but spaces.
    pub bbox: Rect,
}

/// What the page graph knows per text object, keyed by the extractor's own
/// object numbering.
#[derive(Debug, Clone, Default)]
pub struct ObjectFacts {
    by_index: HashMap<u32, Facts>,
}

#[derive(Debug, Clone, Copy, Default)]
struct Facts {
    bold: bool,
    mono: bool,
    mcid: Option<i64>,
}

impl ObjectFacts {
    /// Walk the graph the way the extractor numbers it: every object counts,
    /// a form's contents after the form itself.
    #[must_use]
    pub fn from_page(page: &Page) -> Self {
        let mut facts = Self::default();
        let mut next = 0u32;
        collect(&page.objects, &mut next, &mut facts);
        facts
    }

    fn get(&self, index: u32) -> Facts {
        self.by_index.get(&index).copied().unwrap_or_default()
    }
}

fn collect(objects: &[PageObject], next: &mut u32, out: &mut ObjectFacts) {
    for object in objects {
        match object {
            PageObject::Text(content) => {
                let name = content
                    .object
                    .font
                    .as_ref()
                    .map(|(font, _)| {
                        String::from_utf8_lossy(font.base_font_name()).to_ascii_lowercase()
                    })
                    .unwrap_or_default();
                // Filling and stroking the same glyph (`2 Tr`) is how a
                // producer with one font weight fakes a bold.
                let faux_bold = matches!(
                    content.object.render_mode,
                    TextRenderMode::FillStroke | TextRenderMode::FillStrokeClip
                );
                out.by_index.insert(
                    *next,
                    Facts {
                        bold: faux_bold || name_says_bold(&name),
                        mono: name_says_mono(&name),
                        mcid: content.marks.content_id(),
                    },
                );
                *next += 1;
            }
            PageObject::Form(content) => {
                *next += 1;
                collect(&content.object.objects, next, out);
            }
            PageObject::Path(_) | PageObject::Image(_) | PageObject::Shading(_) => {
                *next += 1;
            }
        }
    }
}

/// A `/BaseFont` that says bold: the style suffix or a weight word.
fn name_says_bold(lower: &str) -> bool {
    [
        "bold",
        "black",
        "heavy",
        "semibold",
        "demibold",
        "extrabold",
        "ultrabold",
    ]
    .iter()
    .any(|w| lower.contains(w))
}

/// A `/BaseFont` that says monospaced: the fonts everyone has, plus the word.
fn name_says_mono(lower: &str) -> bool {
    [
        "courier",
        "mono",
        "consolas",
        "menlo",
        "monaco",
        "inconsolata",
        "sourcecodepro",
        "firacode",
    ]
    .iter()
    .any(|w| lower.contains(w))
}

/// The page's lines, in the extractor's reading order.
///
/// A character drawn again where the same one already is on the line —
/// faux bold, a shadow, a run the producer painted twice — is kept once;
/// the extractor keeps what it keeps because the oracle does, and a reader
/// wants one.
#[must_use]
pub fn lines(text: &TextPage, facts: &ObjectFacts) -> Vec<Line> {
    let mut lines = Vec::new();
    let mut current = LineBuilder::default();
    for i in 0..text.char_count() {
        let Ok(c) = text.char(CharIndex::from(i)) else {
            continue;
        };
        let unicode = char::from_u32(c.unicode).unwrap_or('\u{fffd}');
        if c.char_type == CharType::Generated {
            if unicode == '\n' || unicode == '\r' {
                lines.extend(current.finish());
                current = LineBuilder::default();
            } else if unicode == ' ' {
                current.push_space();
            }
            continue;
        }
        if current.already_drawn(unicode, c.char_box) {
            continue;
        }
        let object = c.object.map(|o| o.0);
        current.push(
            unicode,
            c.char_box,
            drawn_size(c),
            object.map(|o| facts.get(o)),
        );
    }
    lines.extend(current.finish());
    lines
}

/// The size a character is drawn at: the font size in force, scaled by
/// its matrix. A producer that sets `Tf 1` and sizes the text through
/// `Tm` still draws 10.56 pt text, and it is that size the typography
/// reads.
fn drawn_size(c: &CharBox) -> f32 {
    let [_, _, skew, scale_y, _, _] = c.matrix.as_coeffs();
    let scale = skew.hypot(scale_y);
    if scale > 0.0 {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "a point size; f32 is what the text page carries"
        )]
        let size = (f64::from(c.font_size) * scale) as f32;
        size
    } else {
        c.font_size
    }
}

/// Whether two boxes are the same place: they overlap by half the smaller
/// one on each axis, or their origins are within a point. A degenerate
/// axis — a space's box has no height — counts by origin alone.
fn same_place(a: Rect, b: Rect) -> bool {
    let within_a_point = (a.x0 - b.x0).abs() < 1.0 && (a.y0 - b.y0).abs() < 1.0;
    let overlap_x = a.x1.min(b.x1) - a.x0.max(b.x0);
    let overlap_y = a.y1.min(b.y1) - a.y0.max(b.y0);
    let width = a.width().min(b.width());
    let height = a.height().min(b.height());
    let x_ok = if width > 0.0 {
        overlap_x >= width * 0.5
    } else {
        (a.x0 - b.x0).abs() < 1.0
    };
    let y_ok = if height > 0.0 {
        overlap_y >= height * 0.5
    } else {
        (a.y0 - b.y0).abs() < 1.0
    };
    within_a_point || (x_ok && y_ok)
}

#[derive(Default)]
struct LineBuilder {
    text: String,
    bbox: Option<Rect>,
    sizes: Vec<f32>,
    bold_votes: usize,
    mono_votes: usize,
    voters: usize,
    mcids: Vec<i64>,
    segments: Vec<Segment>,
    /// Every character kept so far with its box, for the repeat check.
    drawn: Vec<(char, Rect)>,
    pending_space: bool,
}

impl LineBuilder {
    fn push_space(&mut self) {
        if !self.text.is_empty() {
            self.pending_space = true;
        }
    }

    /// Whether `ch` is already on this line at `char_box`.
    fn already_drawn(&self, ch: char, char_box: Rect) -> bool {
        !ch.is_whitespace()
            && self
                .drawn
                .iter()
                .any(|(other, b)| *other == ch && same_place(*b, char_box))
    }

    fn push(&mut self, ch: char, char_box: Rect, font_size: f32, facts: Option<Facts>) {
        if self.pending_space {
            self.text.push(' ');
            if let Some(segment) = self.segments.last_mut() {
                segment.text.push(' ');
            }
            self.pending_space = false;
        }
        self.text.push(ch);
        self.drawn.push((ch, char_box));
        let has_area = char_box.width() > 0.0 && char_box.height() > 0.0;
        let mcid = facts.and_then(|f| f.mcid);
        match self.segments.last_mut() {
            Some(segment) if segment.mcid == mcid => {
                segment.text.push(ch);
                if has_area {
                    segment.bbox = if segment.bbox.area() > 0.0 {
                        segment.bbox.union(char_box)
                    } else {
                        char_box
                    };
                }
            }
            _ => self.segments.push(Segment {
                mcid,
                text: ch.to_string(),
                bbox: if has_area { char_box } else { Rect::ZERO },
            }),
        }
        if char_box.width() > 0.0 || char_box.height() > 0.0 {
            self.bbox = Some(self.bbox.map_or(char_box, |b| b.union(char_box)));
        }
        if font_size > 0.0 {
            self.sizes.push(font_size);
        }
        if let Some(f) = facts {
            self.voters += 1;
            self.bold_votes += usize::from(f.bold);
            self.mono_votes += usize::from(f.mono);
            if let Some(mcid) = f.mcid
                && self.mcids.last() != Some(&mcid)
            {
                self.mcids.push(mcid);
            }
        }
    }

    fn finish(mut self) -> Option<Line> {
        let text = std::mem::take(&mut self.text);
        let text = text.trim_end().to_owned();
        if text.trim().is_empty() {
            return None;
        }
        self.sizes.sort_by(f32::total_cmp);
        let font_size = self.sizes.get(self.sizes.len() / 2).copied().unwrap_or(0.0);
        Some(Line {
            text,
            bbox: self.bbox.unwrap_or(Rect::ZERO),
            font_size,
            bold: self.voters > 0 && self.bold_votes * 2 > self.voters,
            mono: self.voters > 0 && self.mono_votes * 2 > self.voters,
            mcids: self.mcids,
            segments: self.segments,
        })
    }
}

/// One line's stretch of text under a marked-content id.
#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    /// Which of the page's lines drew it, by index in reading order.
    pub line: usize,
    /// The characters, spacing as the text page has it.
    pub text: String,
    /// The union of the characters' boxes; [`Rect::ZERO`] for spaces alone.
    pub bbox: Rect,
}

/// The text under each marked-content id, for the tagged tier to look up.
#[derive(Debug, Clone, Default)]
pub struct McidText(HashMap<i64, Vec<Run>>);

impl McidText {
    /// The stretches drawn under `id`, in reading order; empty for an id
    /// nothing on the page carries.
    #[must_use]
    pub fn runs(&self, id: i64) -> &[Run] {
        self.0.get(&id).map_or(&[], Vec::as_slice)
    }
}

/// The text under each marked-content id, in reading order, one [`Run`]
/// per line it touches.
#[must_use]
pub fn text_by_mcid(lines: &[Line]) -> McidText {
    let mut out: HashMap<i64, Vec<Run>> = HashMap::new();
    for (index, line) in lines.iter().enumerate() {
        for segment in &line.segments {
            let Some(mcid) = segment.mcid else {
                continue;
            };
            if segment.text.is_empty() {
                continue;
            }
            out.entry(mcid).or_default().push(Run {
                line: index,
                text: segment.text.clone(),
                bbox: segment.bbox,
            });
        }
    }
    McidText(out)
}

#[cfg(test)]
mod tests {
    use super::{ObjectFacts, lines};
    use kurbo::{Affine, Point, Rect};
    use pdfrum_text::{CharBox, CharType, TextPage};

    fn drawn(ch: char, x: f64, y: f64) -> CharBox {
        let char_box = Rect::new(x, y, x + 5.0, y + 7.0);
        CharBox {
            char_type: CharType::Normal,
            unicode: u32::from(ch),
            code: None,
            origin: Point::new(x, y),
            char_box,
            loose_char_box: char_box,
            matrix: Affine::IDENTITY,
            object: None,
            font_size: 10.0,
            angle: 0.0,
        }
    }

    fn generated(ch: char) -> CharBox {
        CharBox {
            char_type: CharType::Generated,
            ..drawn(ch, 0.0, 0.0)
        }
    }

    /// `text` set down from `x`, six points per character, at `y`.
    fn word(text: &str, x: f64, y: f64) -> Vec<CharBox> {
        text.chars()
            .enumerate()
            .map(|(i, ch)| drawn(ch, x + 6.0 * f64::from(u8::try_from(i).unwrap()), y))
            .collect()
    }

    #[test]
    fn a_run_drawn_again_a_fraction_of_a_point_away_is_kept_once() {
        let mut chars = word("Welcome", 100.0, 700.0);
        chars.extend(word("Welcome", 100.4, 700.3));
        chars.push(generated('\r'));
        chars.push(generated('\n'));
        chars.extend(word("Welcome", 100.0, 680.0));
        let page = TextPage {
            chars,
            ..TextPage::default()
        };
        let got = lines(&page, &ObjectFacts::default());
        let texts: Vec<&str> = got.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(texts, ["Welcome", "Welcome"]);
    }

    #[test]
    fn a_leader_of_dots_is_not_a_repeat() {
        let page = TextPage {
            chars: word(".....", 100.0, 700.0),
            ..TextPage::default()
        };
        let got = lines(&page, &ObjectFacts::default());
        assert_eq!(got[0].text, ".....");
    }

    #[test]
    fn a_generated_space_is_kept_where_the_text_page_put_it() {
        let mut chars = word("Wi-Fi", 100.0, 700.0);
        chars.push(generated(' '));
        chars.extend(word("file", 140.0, 700.0));
        let page = TextPage {
            chars,
            ..TextPage::default()
        };
        let got = lines(&page, &ObjectFacts::default());
        assert_eq!(got[0].text, "Wi-Fi file");
    }
}
