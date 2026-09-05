//! Markdown and layout-preserving text from a PDF page.
//!
//! Two tiers, one output. A tagged document (ISO 32000-1 §14.8) says what
//! its content is — headings, paragraphs, lists, tables, figures — and
//! [`page_blocks`] reads that structure tree and joins each element to the
//! text under its marked-content ids. An untagged document says nothing,
//! and the same function falls back to typography: font sizes, weights,
//! bullets, gaps and margins, the rules in [`heuristics`]. Either way the
//! result is a list of [`Block`]s, and [`render()`] turns it into
//! GitHub-flavoured Markdown.
//!
//! [`page_layout`] is the other reading of the same lines: text kept where
//! the page put it, for a `--layout` flag.
//!
//! Inputs are the page graph and the resolver, so this crate sits beside
//! `pdfrum-text` and `pdfrum-doc` rather than on top of the facade; the
//! facade's `Page::markdown` is one call into here.

pub mod ast;
pub mod heuristics;
pub mod layout;
pub mod lines;
pub mod render;
pub mod tagged;

use pdfrum_common::{Diagnostics, Limits};
use pdfrum_doc::structure::StructTree;
use pdfrum_object::Resolve;
use pdfrum_page::Page;

pub use ast::Block;
pub use lines::Line;
pub use render::render;

/// What the page's text needs to be read.
#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    /// Whether the document reads right to left (`/ViewerPreferences
    /// /Direction /R2L`); the extractor orders bidirectional text by it.
    pub rtl: bool,
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
    let mut diags = Diagnostics::default();
    let text = pdfrum_text::extract(
        page,
        resolver,
        &pdfrum_text::ExtractOptions { rtl: options.rtl },
        limits,
        &mut diags,
    );
    let facts = lines::ObjectFacts::from_page(page);
    lines::lines(&text, &facts)
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
    let lines = page_lines(page, resolver, options, limits);
    if let Some(tree) = tree {
        let by_mcid = lines::text_by_mcid(&lines);
        let (mut blocks, claimed) = tagged::blocks(tree, &by_mcid, resolver);
        if blocks.iter().any(|b| !b.text().trim().is_empty()) {
            // Text the tree did not claim — an id it names for another page,
            // a run outside any mark, a running header the producer left
            // untagged — is still text; it follows, read by typography among
            // the page's lines, so nothing on the page is lost and a header
            // is still a header.
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
            blocks.extend(heuristics::blocks_among(&unclaimed, &lines, page.crop_box));
            return blocks;
        }
    }
    heuristics::blocks(&lines, page.crop_box)
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
