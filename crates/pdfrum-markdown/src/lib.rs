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

pub use ast::Block;
pub use lines::{DrawnImage, Line};
pub use render::{render, render_with_images};

/// What the page's text needs to be read.
#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    /// Whether the document reads right to left (`/ViewerPreferences
    /// /Direction /R2L`); the extractor orders bidirectional text by it.
    pub rtl: bool,
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
    Content {
        lines: lines::lines(&text, &facts),
        images: facts.images().to_vec(),
        crop_box: page.crop_box,
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
    contents
        .iter()
        .zip(pages)
        .map(|(content, input)| blocks_of(content, input.tree, resolver))
        .collect()
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
