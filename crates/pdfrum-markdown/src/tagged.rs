//! Blocks from the structure tree, for the document that carries tags.
//!
//! ISO 32000-1 §14.8 names the structure types; after the role map they
//! are what [`pdfrum_doc::StructElement::kind`] holds. Grouping elements
//! (`Document`, `Part`, `Sect`, `Div`, `Art`) are walked through; block
//! elements become blocks; inline ones (`Span`, `Link`, `Lbl`, `Quote`,
//! `Code` inside a paragraph) contribute their text. An element's text is
//! its `/ActualText` when it has one, else its kids' text in order, each
//! marked-content kid read through the id the page's lines were stamped
//! with.

use std::collections::HashSet;

use pdfrum_doc::structure::{Kid, StructElement, StructTree};
use pdfrum_object::Resolve;

use crate::ast::Block;
use crate::heuristics::normalize;
use crate::lines::McidText;

/// The blocks the tree describes, and the marked-content ids it claimed on
/// the way — what it did not claim is the caller's to keep.
pub fn blocks<R: Resolve>(
    tree: &StructTree,
    text_by_mcid: &McidText,
    r: &R,
) -> (Vec<Block>, HashSet<i64>) {
    let mut out = Vec::new();
    // The roots of what the page reached: the upward walk links kids to
    // parents, and an element with no parent is a root whether or not the
    // root's `/K` slot for it was filled.
    for (index, element) in tree.elements.iter().enumerate() {
        if element.parent.is_none() {
            visit(tree, index, 1, text_by_mcid, r, &mut out);
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
    text_by_mcid: &McidText,
    r: &R,
    out: &mut Vec<Block>,
) {
    let Some(element) = tree.elements.get(index) else {
        return;
    };
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
            figures_within(tree, element, r, out);
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
                    if !body.trim().is_empty() {
                        items.push(body);
                    }
                }
            }
            if !items.is_empty() {
                out.push(Block::List { ordered, items });
            }
        }
        "TABLE" => {
            let mut rows = Vec::new();
            collect_rows(tree, element, text_by_mcid, r, &mut rows);
            if !rows.is_empty() {
                out.push(Block::Table(rows));
            }
        }
        "FIGURE" | "FORMULA" => {
            let alt = element.alt_text(r);
            let alt = if alt.is_empty() {
                element.actual_text(r)
            } else {
                alt
            };
            out.push(Block::Image {
                alt: normalize(&alt),
            });
        }
        _ => {
            // A grouping element, or something unknown: the kids decide. Text
            // sitting directly on it becomes a paragraph of its own.
            let mut direct = String::new();
            for kid in &element.kids {
                match kid {
                    Kid::Element {
                        linked: Some(i), ..
                    } => {
                        if !direct.trim().is_empty() {
                            out.push(Block::Paragraph(std::mem::take(&mut direct)));
                        }
                        visit(tree, *i, depth.saturating_add(1), text_by_mcid, r, out);
                    }
                    Kid::PageContent { content_id } | Kid::StreamContent { content_id, .. } => {
                        if let Some(t) = text_by_mcid.get(*content_id) {
                            append(&mut direct, t);
                        }
                    }
                    _ => {}
                }
            }
            if !direct.trim().is_empty() {
                out.push(Block::Paragraph(direct));
            }
        }
    }
}

/// Image blocks for every figure nested anywhere under `element`.
fn figures_within<R: Resolve>(
    tree: &StructTree,
    element: &StructElement,
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
            let alt = child.alt_text(r);
            let alt = if alt.is_empty() {
                child.actual_text(r)
            } else {
                alt
            };
            out.push(Block::Image {
                alt: normalize(&alt),
            });
        } else {
            figures_within(tree, child, r, out);
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

/// An element's text: `/ActualText` if it has one, else its kids' in order.
fn text_of<R: Resolve>(
    tree: &StructTree,
    element: &StructElement,
    text_by_mcid: &McidText,
    r: &R,
) -> String {
    let actual = element.actual_text(r);
    if !actual.is_empty() {
        return normalize(&actual);
    }
    let mut text = String::new();
    for kid in &element.kids {
        match kid {
            Kid::Element {
                linked: Some(i), ..
            } => {
                if let Some(child) = tree.elements.get(*i) {
                    append(&mut text, &text_of(tree, child, text_by_mcid, r));
                }
            }
            Kid::PageContent { content_id } | Kid::StreamContent { content_id, .. } => {
                if let Some(t) = text_by_mcid.get(*content_id) {
                    append(&mut text, t);
                }
            }
            _ => {}
        }
    }
    normalize(&text)
}

fn append(text: &mut String, piece: &str) {
    if piece.is_empty() {
        return;
    }
    if !text.is_empty() && !text.ends_with(' ') && !piece.starts_with(' ') {
        text.push(' ');
    }
    text.push_str(piece);
}
