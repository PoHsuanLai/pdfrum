#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_cfg))]
pub mod ast;
pub mod heuristics;
pub mod layout;
pub mod lines;
pub mod render;
pub mod running;
pub mod tagged;

use kurbo::Rect;
use pdfrum_common::{Diagnostics, Limits};
use pdfrum_doc::structure::StructTree;
use pdfrum_object::Resolve;
use pdfrum_page::Page;

pub use ast::{Block, ListItem};
pub use lines::{DrawnImage, Line};
pub use render::{render, render_with_images};

/// What the page's text needs to be read.
#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    /// Whether the document reads right to left (`/ViewerPreferences
    /// /Direction /R2L`); the extractor orders bidirectional text by it.
    pub rtl: bool,
    /// Whether a hyphen that ends a line is the author's, never the
    /// typesetter's. True of a producer known to draw its own hyphens as a
    /// different character — Chrome (`Skia/PDF`) draws `U+2010` where it
    /// hyphenates — so a `-` it wraps after is the word's and stays:
    /// `cross-reference`, not `crossreference`. False, the default, leaves
    /// the break to the guess [`heuristics`] makes.
    pub authors_hyphens: bool,
}

/// One page of a document, for [`document_blocks`].
#[derive(Debug, Clone, Copy)]
pub struct PageInput<'a> {
    /// The page graph, images included.
    pub page: &'a Page,
    /// The page's view of the structure tree, when the document is tagged.
    pub tree: Option<&'a StructTree>,
}

/// What a page drew, read once: its lines, its images and its crop box.
#[derive(Debug)]
struct Content {
    lines: Vec<Line>,
    images: Vec<DrawnImage>,
    crop_box: Rect,
}

fn content<R: Resolve>(page: &Page, resolver: &R, options: Options, limits: &Limits) -> Content {
    let mut diags = Diagnostics::default();
    let text = pdfrum_text::extract(
        page,
        resolver,
        &pdfrum_text::ExtractOptions { rtl: options.rtl },
        limits,
        &mut diags,
    );
    let facts = lines::ObjectFacts::from_page(page);
    let mut lines = lines::lines(&text, &facts);
    if options.authors_hyphens {
        for line in &mut lines {
            authors_hyphen(&mut line.text);
            for segment in &mut line.segments {
                authors_hyphen(&mut segment.text);
            }
        }
    }
    Content {
        lines,
        images: facts.images().to_vec(),
        crop_box: page.crop_box,
    }
}

/// The text layer's mark for a line that ended in a hyphen and was joined
/// to the next — `U+0002` in the records, `U+00AD` in the text — put back
/// as the hyphen it was.
fn authors_hyphen(text: &mut String) {
    if text.contains(['\u{2}', '\u{ad}']) {
        *text = text.replace(['\u{2}', '\u{ad}'], "-");
    }
}

/// The page's lines, in the extractor's reading order, with the typographic
/// facts the graph carries.
#[must_use]
pub fn page_lines<R: Resolve>(
    page: &Page,
    resolver: &R,
    options: Options,
    limits: &Limits,
) -> Vec<Line> {
    content(page, resolver, options, limits).lines
}

/// The blocks of what a page drew: from `tree` when it is `Some` and yields
/// anything with text in it, else from typography.
fn blocks_of<R: Resolve>(content: &Content, tree: Option<&StructTree>, resolver: &R) -> Vec<Block> {
    let Content {
        lines,
        images,
        crop_box,
    } = content;
    if let Some(tree) = tree {
        let by_mcid = lines::text_by_mcid(lines);
        let drawn = tagged::Drawn {
            text: &by_mcid,
            images,
        };
        let (mut blocks, claimed) = tagged::blocks(tree, drawn, resolver);
        if blocks.iter().any(|b| !b.text().trim().is_empty()) {
            // Text the tree did not claim — an id it names for another page,
            // a run outside any mark, a running header the producer left
            // untagged — is still text; it follows, read by typography among
            // the page's lines, so nothing on the page is lost and a header
            // is still a header. An image the tree put in no figure is what
            // the tree says it is, a bullet or a banner, and is not read.
            let unclaimed: Vec<Line> = lines
                .iter()
                .filter_map(|line| {
                    let text: String = line
                        .segments
                        .iter()
                        .filter(|s| s.mcid.is_none_or(|id| !claimed.contains(&id)))
                        .map(|s| s.text.as_str())
                        .collect();
                    (!text.trim().is_empty()).then(|| Line {
                        text: text.trim().to_owned(),
                        ..line.clone()
                    })
                })
                .collect();
            blocks.extend(heuristics::blocks_among(&unclaimed, lines, &[], *crop_box));
            return blocks;
        }
    }
    heuristics::blocks(lines, images, *crop_box)
}

/// The page's blocks: from `tree` when it is `Some` and yields anything with
/// text in it, else from typography.
#[must_use]
pub fn page_blocks<R: Resolve>(
    page: &Page,
    tree: Option<&StructTree>,
    resolver: &R,
    options: Options,
    limits: &Limits,
) -> Vec<Block> {
    blocks_of(&content(page, resolver, options, limits), tree, resolver)
}

/// Every page's blocks, read as one document: a line repeated in the top
/// or bottom band on a majority of the pages, three at least, is a running
/// header or footer and goes before the page rules run — see [`running`].
/// Page by page the result is otherwise [`page_blocks`].
#[must_use]
pub fn document_blocks<R: Resolve>(
    pages: &[PageInput<'_>],
    resolver: &R,
    options: Options,
    limits: &Limits,
) -> Vec<Vec<Block>> {
    let mut contents: Vec<Content> = pages
        .iter()
        .map(|input| content(input.page, resolver, options, limits))
        .collect();
    let per_page: Vec<(&[Line], Rect)> = contents
        .iter()
        .map(|c| (c.lines.as_slice(), c.crop_box))
        .collect();
    let masks = running::mask(&per_page);
    for (content, mask) in contents.iter_mut().zip(&masks) {
        let mut dropped = mask.iter().copied();
        content.lines.retain(|_| !dropped.next().unwrap_or(false));
    }
    let mut blocks: Vec<Vec<Block>> = contents
        .iter()
        .zip(pages)
        .map(|(content, input)| blocks_of(content, input.tree, resolver))
        .collect();
    rejoin_across_pages(&mut blocks, pages);
    blocks
}

/// A paragraph the page break cut in two, put back together on the page it
/// starts on. The structure tree says so exactly: one paragraph element
/// with content on both pages. Typography alone could only guess, and
/// guesses wrong at every heading-less section break, so an untagged
/// document keeps its pages as they are.
fn rejoin_across_pages(blocks: &mut [Vec<Block>], pages: &[PageInput<'_>]) {
    for i in 1..blocks.len().min(pages.len()) {
        let (Some(before), Some(after)) = (pages.get(i - 1), pages.get(i)) else {
            continue;
        };
        let (Some(a), Some(b)) = (before.tree, after.tree) else {
            continue;
        };
        if !tagged::paragraph_spans(a, b) {
            continue;
        }
        let (head, tail) = blocks.split_at_mut(i);
        // The page it started on, past any page it wholly filled.
        let Some(Block::Paragraph(open)) = head
            .iter_mut()
            .rev()
            .find(|page| !page.is_empty())
            .and_then(|page| page.last_mut())
        else {
            continue;
        };
        let Some(page) = tail.first_mut() else {
            continue;
        };
        if let Some(Block::Paragraph(_)) = page.first()
            && let Block::Paragraph(rest) = page.remove(0)
        {
            heuristics::join(open, &rest);
        }
    }
}

/// The page as Markdown: [`page_blocks`] then [`render()`].
#[must_use]
pub fn page_markdown<R: Resolve>(
    page: &Page,
    tree: Option<&StructTree>,
    resolver: &R,
    options: Options,
    limits: &Limits,
) -> String {
    render(&page_blocks(page, tree, resolver, options, limits))
}

/// The page's text with its layout kept: [`layout::layout`] over
/// [`page_lines`].
#[must_use]
pub fn page_layout<R: Resolve>(
    page: &Page,
    resolver: &R,
    options: Options,
    limits: &Limits,
) -> String {
    layout::layout(&page_lines(page, resolver, options, limits), page.crop_box)
}

#[cfg(test)]
mod tests {
    use super::authors_hyphen;

    #[test]
    fn the_text_layers_hyphen_mark_is_put_back_as_a_hyphen() {
        let mut text = "a cross\u{2}reference and a cross\u{ad}check".to_owned();
        authors_hyphen(&mut text);
        assert_eq!(text, "a cross-reference and a cross-check");
    }
}
