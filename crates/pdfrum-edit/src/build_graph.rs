//! Interpreting a page's content stream into an object graph.
//!
//! Read through any `Resolve`: the base document for the page as it was
//! opened, or an editing session's overlay for the page as that session's
//! edits leave it, so a stream an earlier edit appended is a clean stream of
//! the graph and the names it uses are kept.

use pdfrum_common::{Diagnostics, Limits};
use pdfrum_object::{Name, Resolve};
use pdfrum_page::BuildContext;
use pdfrum_parser::{PageDict, content_segments};

/// The object graph of `dict`'s page for editing, read through `r`: the
/// base document for a page as it was opened, or an editing session's
/// overlay for the page as that session's edits leave it — so a stream an
/// earlier edit appended is a clean stream of the graph, and the names it
/// uses are kept.
pub fn build_graph(
    dict: &PageDict,
    r: &impl Resolve,
    limits: &Limits,
    ctx: &mut BuildContext,
    diags: &mut Diagnostics,
) -> pdfrum_page::Page {
    let (bytes, ends) = content_segments(dict, r, limits, diags);
    let ops = pdfrum_page::parse_content(&bytes, limits, diags);
    let bounds = pdfrum_page::StreamBounds::from_joined(&bytes, ops.len(), &ends, limits);
    let resources = pdfrum_page::Resources::for_page(
        dict.inherited(&Name::from("Resources"), r)
            .and_then(|object| object.resolve(r).ok()?.as_dict().cloned()),
    );
    pdfrum_page::build_page_streams(
        &ops,
        &bounds,
        &dict.dict,
        |key| dict.inherited(key, r),
        &resources,
        r,
        ctx,
        limits,
        diags,
    )
}
