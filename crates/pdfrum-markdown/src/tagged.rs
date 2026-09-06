//! Blocks from the structure tree, for the document that carries tags.
//!
//! ISO 32000-1 §14.8 names the structure types; after the role map they
//! are what [`pdfrum_doc::StructElement::kind`] holds. Grouping elements
//! (`Document`, `Part`, `Sect`, `Div`, `Art`) are walked through; block
//! elements become blocks; inline ones (`Span`, `Link`, `Lbl`, `Quote`,
//! `Code` inside a paragraph) contribute their text.
//!
//! An element's text is its kids' text in order, each marked-content kid
//! read through the id the page's lines were stamped with. The pieces are
//! joined as the page drew them: the text page's spacing is the contract,
//! so two pieces on one line are put end to end — a producer that cuts
//! `documents` into `d` and `ocuments` gets its word back — and two pieces
//! on different lines meet across a line break, with a space, or with a
//! wrapped word de-hyphenated. An `/ActualText` replaces what its element
//! drew, keeping the whitespace at the drawn text's edges so the join
//! still knows where the words end.
//!
//! A piece that says what the piece before it said, drawn on top of it or
//! not drawn at all, is the producer's second copy — faux bold, a shadow,
//! an `/ActualText` span whose own characters the extractor already dropped
//! as a repeat — and is kept once.
//!
//! A `Figure` is the image drawn under one of its marked-content ids — the
//! first of the page's, in drawing order, when it names several — with the
//! element's alternative text; a figure that drew no image is its
//! alternative text alone. An image the tree puts in no figure is not
//! read: the tree said what the page's pictures are.

use std::collections::HashSet;

use kurbo::Rect;
use pdfrum_doc::structure::{Kid, StructElement, StructTree};
use pdfrum_object::Resolve;

use crate::ast::{Block, ListMarker};
use crate::heuristics::{join, normalize, strip_bullet};
use crate::lines::{DrawnImage, McidText, Run};

/// What the page drew, for the tree to read: its text by marked-content
/// id, and its images with the ids they were drawn under.
#[derive(Debug, Clone, Copy)]
pub struct Drawn<'a> {
    /// The text under each marked-content id.
    pub text: &'a McidText,
    /// The page's images in drawing order.
    pub images: &'a [DrawnImage],
}

/// The blocks the tree describes, and the marked-content ids it claimed on
/// the way — what it did not claim is the caller's to keep.
pub fn blocks<R: Resolve>(
    tree: &StructTree,
    drawn: Drawn<'_>,
    r: &R,
) -> (Vec<Block>, HashSet<i64>) {
    let mut out = Vec::new();
    // The roots of what the page reached: the upward walk links kids to
    // parents, and an element with no parent is a root whether or not the
    // root's `/K` slot for it was filled.
    for (index, element) in tree.elements.iter().enumerate() {
        if element.parent.is_none() {
            visit(tree, index, 1, drawn, r, &mut out);
        }
    }
    out.retain(|b| !b.text().trim().is_empty() || matches!(b, Block::Image { .. }));
    (out, claimed(tree))
}

/// Every content id any element of the tree names.
fn claimed(tree: &StructTree) -> HashSet<i64> {
    tree.elements
        .iter()
        .flat_map(|e| e.kids.iter())
        .filter_map(|kid| match kid {
            Kid::PageContent { content_id } | Kid::StreamContent { content_id, .. } => {
                Some(*content_id)
            }
            _ => None,
        })
        .collect()
}

fn visit<R: Resolve>(
    tree: &StructTree,
    index: usize,
    depth: u8,
    drawn: Drawn<'_>,
    r: &R,
    out: &mut Vec<Block>,
) {
    let Some(element) = tree.elements.get(index) else {
        return;
    };
    let text_by_mcid = drawn.text;
    let kind = String::from_utf8_lossy(&element.kind).to_ascii_uppercase();
    match kind.as_str() {
        "H1" | "H2" | "H3" | "H4" | "H5" | "H6" => {
            let level = kind.as_bytes().get(1).map_or(1, |d| d - b'0');
            out.push(Block::Heading {
                level: level.clamp(1, 6),
                text: text_of(tree, element, text_by_mcid, r),
            });
        }
        "H" | "TITLE" => out.push(Block::Heading {
            level: depth.clamp(1, 6),
            text: text_of(tree, element, text_by_mcid, r),
        }),
        "P" | "PARA" | "BLOCKQUOTE" | "CAPTION" | "NOTE" | "INDEX" | "TOCI" => {
            out.push(Block::Paragraph(text_of(tree, element, text_by_mcid, r)));
            // A figure inside a paragraph is still a figure.
            figures_within(tree, element, drawn.images, r, out);
        }
        "CODE" => out.push(Block::Code(text_of(tree, element, text_by_mcid, r))),
        "L" => {
            let mut items = Vec::new();
            let mut ordered = false;
            for kid in &element.kids {
                if let Kid::Element {
                    linked: Some(li), ..
                } = kid
                    && let Some(item) = tree.elements.get(*li)
                {
                    let label = child_of_kind(tree, item, b"LBL")
                        .map(|l| text_of(tree, l, text_by_mcid, r))
                        .unwrap_or_default();
                    ordered |= label.chars().next().is_some_and(|c| c.is_ascii_digit());
                    let body = child_of_kind(tree, item, b"LBODY").map_or_else(
                        || text_of(tree, item, text_by_mcid, r),
                        |b| text_of(tree, b, text_by_mcid, r),
                    );
                    // A producer that draws the bullet into the body
                    // instead of a `Lbl` still gets one bullet, not two.
                    let body = strip_bullet(&body).to_owned();
                    if !body.trim().is_empty() {
                        items.push(body);
                    }
                }
            }
            if !items.is_empty() {
                let marker = if ordered {
                    ListMarker::Ordered
                } else {
                    ListMarker::Bullet
                };
                out.push(Block::List { marker, items });
            }
        }
        "TABLE" => {
            let mut rows = Vec::new();
            collect_rows(tree, element, text_by_mcid, r, &mut rows);
            if !rows.is_empty() {
                out.push(Block::Table(rows));
            }
        }
        "FIGURE" | "FORMULA" => out.push(figure(tree, element, drawn.images, r)),
        _ => {
            // A grouping element, or something unknown: the kids decide. Text
            // sitting directly on it becomes a paragraph of its own.
            let mut direct: Vec<Piece> = Vec::new();
            for kid in &element.kids {
                match kid {
                    Kid::Element {
                        linked: Some(i), ..
                    } => {
                        flush_direct(&mut direct, out);
                        visit(tree, *i, depth.saturating_add(1), drawn, r, out);
                    }
                    Kid::PageContent { content_id } | Kid::StreamContent { content_id, .. } => {
                        direct.extend(text_by_mcid.runs(*content_id).iter().map(Piece::drawn));
                    }
                    _ => {}
                }
            }
            flush_direct(&mut direct, out);
        }
    }
}

/// The paragraph a grouping element's own text makes, if it says anything.
fn flush_direct(direct: &mut Vec<Piece>, out: &mut Vec<Block>) {
    let text = normalize(concat(direct).text.trim());
    direct.clear();
    if !text.is_empty() {
        out.push(Block::Paragraph(text));
    }
}

/// A figure's block: its alternative text, else its actual text, and the
/// first of the page's images drawn under a marked-content id the figure
/// names, when one was.
fn figure<R: Resolve>(
    tree: &StructTree,
    element: &StructElement,
    images: &[DrawnImage],
    r: &R,
) -> Block {
    let alt = element.alt_text(r);
    let alt = if alt.is_empty() {
        element.actual_text(r)
    } else {
        alt
    };
    let mut ids = HashSet::new();
    content_ids(tree, element, &mut ids);
    let index = images
        .iter()
        .find(|image| image.mcid.is_some_and(|id| ids.contains(&id)))
        .map(|image| image.index);
    Block::Image {
        alt: normalize(alt.trim()),
        index,
    }
}

/// Every content id `element` or anything under it names.
fn content_ids(tree: &StructTree, element: &StructElement, out: &mut HashSet<i64>) {
    for kid in &element.kids {
        match kid {
            Kid::PageContent { content_id } | Kid::StreamContent { content_id, .. } => {
                out.insert(*content_id);
            }
            Kid::Element {
                linked: Some(i), ..
            } => {
                if let Some(child) = tree.elements.get(*i) {
                    content_ids(tree, child, out);
                }
            }
            _ => {}
        }
    }
}

/// Image blocks for every figure nested anywhere under `element`.
fn figures_within<R: Resolve>(
    tree: &StructTree,
    element: &StructElement,
    images: &[DrawnImage],
    r: &R,
    out: &mut Vec<Block>,
) {
    for kid in &element.kids {
        let Kid::Element {
            linked: Some(i), ..
        } = kid
        else {
            continue;
        };
        let Some(child) = tree.elements.get(*i) else {
            continue;
        };
        if child.kind.eq_ignore_ascii_case(b"FIGURE") || child.kind.eq_ignore_ascii_case(b"FORMULA")
        {
            out.push(figure(tree, child, images, r));
        } else {
            figures_within(tree, child, images, r, out);
        }
    }
}

fn child_of_kind<'t>(
    tree: &'t StructTree,
    parent: &StructElement,
    kind: &[u8],
) -> Option<&'t StructElement> {
    parent.kids.iter().find_map(|kid| match kid {
        Kid::Element {
            linked: Some(i), ..
        } => tree
            .elements
            .get(*i)
            .filter(|e| e.kind.eq_ignore_ascii_case(kind)),
        _ => None,
    })
}

fn collect_rows<R: Resolve>(
    tree: &StructTree,
    element: &StructElement,
    text_by_mcid: &McidText,
    r: &R,
    rows: &mut Vec<Vec<String>>,
) {
    for kid in &element.kids {
        let Kid::Element {
            linked: Some(i), ..
        } = kid
        else {
            continue;
        };
        let Some(child) = tree.elements.get(*i) else {
            continue;
        };
        if child.kind.eq_ignore_ascii_case(b"TR") {
            let cells: Vec<String> = child
                .kids
                .iter()
                .filter_map(|k| match k {
                    Kid::Element {
                        linked: Some(c), ..
                    } => tree.elements.get(*c),
                    _ => None,
                })
                .filter(|c| {
                    c.kind.eq_ignore_ascii_case(b"TD") || c.kind.eq_ignore_ascii_case(b"TH")
                })
                .map(|c| text_of(tree, c, text_by_mcid, r))
                .collect();
            if !cells.is_empty() {
                rows.push(cells);
            }
        } else {
            // `THead`, `TBody`, `TFoot` wrap rows.
            collect_rows(tree, child, text_by_mcid, r, rows);
        }
    }
}

/// A stretch of an element's text: what it says, which lines drew it and
/// where.
#[derive(Debug, Clone, Default)]
struct Piece {
    /// The text, spacing as drawn.
    text: String,
    /// The first and last of the page's lines that drew it, when any did.
    lines: Option<(usize, usize)>,
    /// The union of its characters' boxes, when any had area.
    bbox: Option<Rect>,
}

impl Piece {
    fn drawn(run: &Run) -> Self {
        Self {
            text: run.text.clone(),
            lines: Some((run.line, run.line)),
            bbox: (run.bbox.area() > 0.0).then_some(run.bbox),
        }
    }

    /// Whether this piece says what `previous` said, in the same place —
    /// or in no place at all, when one of the two was never drawn.
    fn repeats(&self, previous: &Self) -> bool {
        let said = self.text.trim();
        !said.is_empty()
            && said == previous.text.trim()
            && match (self.bbox, previous.bbox) {
                (Some(a), Some(b)) => overlaps_by_half(a, b),
                _ => true,
            }
    }
}

/// Whether two boxes overlap by half the smaller one on each axis.
fn overlaps_by_half(a: Rect, b: Rect) -> bool {
    let overlap_x = a.x1.min(b.x1) - a.x0.max(b.x0);
    let overlap_y = a.y1.min(b.y1) - a.y0.max(b.y0);
    overlap_x >= a.width().min(b.width()) * 0.5 && overlap_y >= a.height().min(b.height()) * 0.5
}

/// The pieces end to end: on one line as drawn, across lines with a space
/// or a de-hyphenation, a repeat kept once.
fn concat(pieces: &[Piece]) -> Piece {
    let mut out = Piece::default();
    let mut last_said: Option<&Piece> = None;
    for piece in pieces {
        if piece.text.trim().is_empty() {
            // Spaces keep their place; they are the text page's.
            out.text.push_str(&piece.text);
            continue;
        }
        if last_said.is_some_and(|previous| piece.repeats(previous)) {
            continue;
        }
        let line_break = match (out.lines, piece.lines) {
            (Some((_, last)), Some((first, _))) => last != first,
            _ => false,
        };
        if line_break {
            join(&mut out.text, &piece.text);
        } else {
            out.text.push_str(&piece.text);
        }
        out.lines = match (out.lines, piece.lines) {
            (Some((first, _)), Some((_, last))) => Some((first, last)),
            (None, lines) | (lines, None) => lines,
        };
        out.bbox = match (out.bbox, piece.bbox) {
            (Some(a), Some(b)) => Some(a.union(b)),
            (None, b) | (b, None) => b,
        };
        last_said = Some(piece);
    }
    out
}

/// An element's text and where it was drawn: `/ActualText` if it has one,
/// over what its kids drew, else the kids' pieces end to end.
fn piece_of<R: Resolve>(
    tree: &StructTree,
    element: &StructElement,
    text_by_mcid: &McidText,
    r: &R,
) -> Piece {
    let mut pieces: Vec<Piece> = Vec::new();
    for kid in &element.kids {
        match kid {
            Kid::Element {
                linked: Some(i), ..
            } => {
                if let Some(child) = tree.elements.get(*i) {
                    pieces.push(piece_of(tree, child, text_by_mcid, r));
                }
            }
            Kid::PageContent { content_id } | Kid::StreamContent { content_id, .. } => {
                pieces.extend(text_by_mcid.runs(*content_id).iter().map(Piece::drawn));
            }
            _ => {}
        }
    }
    let drawn = concat(&pieces);
    let actual = element.actual_text(r);
    if actual.trim().is_empty() {
        return drawn;
    }
    // The actual text stands for the drawn characters; the whitespace at
    // the drawn text's edges is where the words end, and stays.
    let leading = drawn.text.len() - drawn.text.trim_start().len();
    let trailing = drawn.text.len() - drawn.text.trim_end().len();
    let mut text = String::with_capacity(actual.len() + leading + trailing);
    text.push_str(drawn.text.get(..leading).unwrap_or_default());
    text.push_str(actual.trim());
    text.push_str(
        drawn
            .text
            .get(drawn.text.len() - trailing..)
            .unwrap_or_default(),
    );
    Piece { text, ..drawn }
}

/// An element's text, normalized and trimmed.
fn text_of<R: Resolve>(
    tree: &StructTree,
    element: &StructElement,
    text_by_mcid: &McidText,
    r: &R,
) -> String {
    normalize(piece_of(tree, element, text_by_mcid, r).text.trim())
}

#[cfg(test)]
mod tests {
    use super::{Piece, concat};
    use kurbo::Rect;

    fn on_line(text: &str, line: usize, x: f64) -> Piece {
        Piece {
            text: text.to_owned(),
            lines: Some((line, line)),
            bbox: Some(Rect::new(
                x,
                700.0,
                x + 6.0 * f64::from(u16::try_from(text.len()).unwrap_or(u16::MAX)),
                710.0,
            )),
        }
    }

    fn said(text: &str) -> Piece {
        Piece {
            text: text.to_owned(),
            lines: None,
            bbox: None,
        }
    }

    #[test]
    fn pieces_on_one_line_keep_the_text_page_spacing() {
        let got = concat(&[
            on_line("Local d", 0, 73.0),
            on_line("ocuments view: list ", 0, 104.0),
            on_line("all", 0, 226.0),
        ]);
        assert_eq!(got.text, "Local documents view: list all");
        assert_eq!(got.lines, Some((0, 0)));
    }

    #[test]
    fn pieces_on_different_lines_meet_across_the_break() {
        let got = concat(&[
            on_line("a wrapped para-", 0, 73.0),
            on_line("graph of two ", 1, 73.0),
            on_line("lines", 1, 160.0),
        ]);
        assert_eq!(got.text, "a wrapped paragraph of two lines");
        assert_eq!(got.lines, Some((0, 1)));
    }

    #[test]
    fn a_span_that_repeats_its_neighbour_is_kept_once() {
        // An `/ActualText` span whose own characters the extractor dropped,
        // followed by the drawn copy: the guide's `Welcome to Foxit
        // MobilePDF Welcome to Foxit MobilePDF`.
        let got = concat(&[
            said("Welcome to Foxit MobilePDF"),
            on_line("Welcome to Foxit MobilePDF ", 0, 168.0),
        ]);
        assert_eq!(got.text, "Welcome to Foxit MobilePDF");
        // Drawn twice, a fraction of a point apart.
        let got = concat(&[
            on_line("Instructions", 0, 55.0),
            on_line("Instructions", 0, 55.4),
            on_line("：", 0, 130.0),
        ]);
        assert_eq!(got.text, "Instructions：");
        // The same words somewhere else are just the same words.
        let got = concat(&[on_line("no ", 0, 55.0), on_line("no", 0, 80.0)]);
        assert_eq!(got.text, "no no");
    }
}
