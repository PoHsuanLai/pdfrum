//! From characters to lines: the model both tiers and the layout printer
//! read.
//!
//! The extractor already decided where lines break (it inserts a generated
//! `\r\n` between them, the way the oracle does); this module walks its
//! characters, joins each run into a [`Line`] with a box, a font size and
//! the typographic facts the page graph knows about the objects that drew
//! it — whether the font's name says bold or monospaced, and which
//! marked-content id it sits under.

use std::collections::HashMap;

use kurbo::Rect;
use pdfrum_page::{Page, PageObject};
use pdfrum_text::{CharIndex, CharType, TextPage};

/// One line of text as laid out on the page.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    /// The characters, generated ones excluded.
    pub text: String,
    /// The union of the characters' boxes, in page space.
    pub bbox: Rect,
    /// The median font size of the characters, in points.
    pub font_size: f32,
    /// Whether most of the characters came from a font whose name says bold.
    pub bold: bool,
    /// Whether most came from a font whose name says monospaced.
    pub mono: bool,
    /// The marked-content ids the characters sit under, in order, deduplicated.
    pub mcids: Vec<i64>,
    /// The text under each id, in order: what the tagged tier reads.
    pub segments: Vec<(Option<i64>, String)>,
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
                out.by_index.insert(
                    *next,
                    Facts {
                        bold: name_says_bold(&name),
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
/// A character drawn twice in the same place — faux bold, a shadow — is
/// kept once; the extractor keeps both because the oracle does, and a
/// reader wants one.
#[must_use]
pub fn lines(text: &TextPage, facts: &ObjectFacts) -> Vec<Line> {
    let mut lines = Vec::new();
    let mut current = LineBuilder::default();
    let mut previous: Option<(u32, Rect)> = None;
    for i in 0..text.char_count() {
        let Ok(c) = text.char(CharIndex::from(i)) else {
            continue;
        };
        let unicode = char::from_u32(c.unicode).unwrap_or('\u{fffd}');
        if c.char_type == CharType::Generated {
            if unicode == '\n' || unicode == '\r' {
                if let Some(line) = current.finish() {
                    lines.push(line);
                }
                current = LineBuilder::default();
                previous = None;
            } else if unicode == ' ' {
                current.push_space();
            }
            continue;
        }
        if let Some((prev_unicode, prev_box)) = previous
            && prev_unicode == c.unicode
            && (prev_box.x0 - c.char_box.x0).abs() < 1.0
            && (prev_box.y0 - c.char_box.y0).abs() < 1.0
        {
            continue;
        }
        previous = Some((c.unicode, c.char_box));
        let object = c.object.map(|o| o.0);
        current.push(
            unicode,
            c.char_box,
            c.font_size,
            object.map(|o| facts.get(o)),
        );
    }
    if let Some(line) = current.finish() {
        lines.push(line);
    }
    lines
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
    segments: Vec<(Option<i64>, String)>,
    pending_space: bool,
}

impl LineBuilder {
    fn push_space(&mut self) {
        if !self.text.is_empty() {
            self.pending_space = true;
        }
    }

    fn push(&mut self, ch: char, char_box: Rect, font_size: f32, facts: Option<Facts>) {
        if self.pending_space {
            self.text.push(' ');
            if let Some((_, run)) = self.segments.last_mut() {
                run.push(' ');
            }
            self.pending_space = false;
        }
        self.text.push(ch);
        let mcid = facts.and_then(|f| f.mcid);
        match self.segments.last_mut() {
            Some((id, run)) if *id == mcid => run.push(ch),
            _ => self.segments.push((mcid, ch.to_string())),
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

/// The text under each marked-content id, for the tagged tier to look up.
#[derive(Debug, Clone, Default)]
pub struct McidText(HashMap<i64, String>);

impl McidText {
    /// The text under `id`, if any.
    #[must_use]
    pub fn get(&self, id: i64) -> Option<&str> {
        self.0.get(&id).map(String::as_str)
    }
}

/// The text under each marked-content id, in reading order.
#[must_use]
pub fn text_by_mcid(lines: &[Line]) -> McidText {
    let mut out: HashMap<i64, String> = HashMap::new();
    for line in lines {
        for (mcid, run) in &line.segments {
            let Some(mcid) = mcid else {
                continue;
            };
            let run = run.trim();
            if run.is_empty() {
                continue;
            }
            let entry = out.entry(*mcid).or_default();
            if !entry.is_empty() {
                entry.push(' ');
            }
            entry.push_str(run);
        }
    }
    McidText(out)
}
