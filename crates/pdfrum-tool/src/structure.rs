//! `--show-structure`: the tagged-PDF logical structure tree, per page.
//!
//! The emitter itself lives in `pdfrum-doc` because its ordering rules are
//! behavior, not presentation; this module only assembles the tree and hands
//! it over.

use pdfrum_common::{Diagnostics, Limits};
use pdfrum_doc::structure::{StructTree, dump};
use pdfrum_object::{Dict, Resolve};
use pdfrum_parser::PageDict;

/// One page's structure dump, empty for an untagged document.
#[must_use]
pub fn render<R: Resolve>(catalog: &Dict, page: &PageDict, index: u32, r: &R) -> String {
    let limits = Limits::default();
    let mut diags = Diagnostics::default();
    let page_obj_num = page.reference.map_or(0, |reference| reference.num);
    let tree = StructTree::load_page(catalog, &page.dict, page_obj_num, r, &limits, &mut diags);
    dump::render(tree.as_ref(), index, r)
}
