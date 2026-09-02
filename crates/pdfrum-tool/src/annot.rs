//! `--annot`: one text file per page describing its annotations.
//!
//! The format lives in `pdfrum-doc` — field order, the two colour lines'
//! failure wording and the `%.3f` rounding are all behavior. This module
//! supplies the two things that crate cannot: where the file goes, and what
//! each annotation's appearance stream actually draws, which needs a content
//! parse.

use std::path::{Path, PathBuf};

use crate::annot_dump::{self, ObjectKind};
use pdfrum_common::{Diagnostics, Limits};
use pdfrum_doc::annot::appearance::{ApMode, annot_ap};
use pdfrum_doc::ap;
use pdfrum_object::{Dict, Resolve};
use pdfrum_page::{BuildContext, PageObject, Resources};
use pdfrum_parser::PageDict;

/// Where a page's dump is written: `<input>.<page>.annot.txt`.
#[must_use]
pub fn output_path(input: &Path, page: u32) -> Option<PathBuf> {
    let name = format!("{}.{page}.annot.txt", input.to_string_lossy());
    // The oracle builds this name into a fixed buffer and gives up when it
    // does not fit.
    (name.len() < 256).then(|| PathBuf::from(name))
}

/// One page's annotation dump.
///
/// The overlay is built first and threaded through, because the format
/// describes the document *after* appearance generation has run — a sticky
/// note's rectangle is the 20×20 box the generator produced, not the one the
/// file declared.
#[must_use]
pub fn render<R: Resolve>(
    page: &PageDict,
    catalog: &Dict,
    r: &R,
    ctx: &mut BuildContext,
) -> String {
    let mut diags = Diagnostics::default();
    let limits = Limits::default();
    // The oracle runs `FORM_DoDocumentOpenAction` before it reads any page
    // (`pdfium_test.cc:1779`), so a `/Hide` in the catalog's open action has
    // already rewritten the flag words this dump reports.
    let hidden = pdfrum_doc::nav::hidden_by_open_action(catalog, r, &limits, &mut diags);
    // A free-text annotation and a form field both need a font to set their
    // text with, and each needs the one its own `/DA` names — see
    // `ap::FormFonts` for what taking a stock Helvetica for all of them costs.
    let fonts = ap::FormFonts::load(catalog, r, ctx);
    let overlay =
        ap::generate_appearances_with_text(&page.dict, catalog, Some(&fonts), r, &mut diags);

    let resources = page
        .inherited(pdfrum_object::names::RESOURCES, r)
        .and_then(|object| object.as_dict().cloned());
    let objects = |index: usize, dict: &Dict| {
        appearance_objects(index, dict, &overlay, resources.clone(), r, ctx)
    };
    annot_dump::render(&page.dict, Some(&overlay), &hidden, objects, r, &mut diags)
}

/// What one annotation's normal appearance stream draws.
///
/// The stream is parsed as a form `XObject` against the page's resources, and
/// its top-level objects are reported in painting order. An annotation with
/// no resolvable stream draws nothing — and one whose appearance *we*
/// generated draws whatever we just wrote.
fn appearance_objects<R: Resolve>(
    index: usize,
    dict: &Dict,
    overlay: &ap::AnnotOverlay,
    page_resources: Option<Dict>,
    r: &R,
    ctx: &mut BuildContext,
) -> Vec<ObjectKind> {
    let limits = Limits::default();
    let mut diags = Diagnostics::default();

    let (bytes, own_resources) = if let Some(generated) = overlay.get(index) {
        (generated.stream.clone(), Some(generated.resources.clone()))
    } else {
        let Some(stream) = annot_ap(dict, ApMode::Normal, true, r) else {
            return Vec::new();
        };
        let resources = stream.dict.dict(pdfrum_object::names::RESOURCES, r);
        (
            pdfrum_filters::decode_chain(&stream, 0, r, &limits, &mut diags).data,
            resources,
        )
    };

    let ops = pdfrum_page::parse_content(&bytes, &limits, &mut diags);
    let resources = Resources::choose(own_resources, None, page_resources);
    let form = pdfrum_page::build_page(&ops, &resources, r, ctx, &limits, &mut diags);
    form.objects.iter().map(kind_of).collect()
}

/// The dump's name for one page object.
fn kind_of(object: &PageObject) -> ObjectKind {
    match object {
        PageObject::Text(_) => ObjectKind::Text,
        PageObject::Path(_) => ObjectKind::Path,
        PageObject::Image(_) => ObjectKind::Image,
        PageObject::Shading(_) => ObjectKind::Shading,
        PageObject::Form(_) => ObjectKind::Form,
    }
}

#[cfg(test)]
mod tests {
    use super::output_path;
    use std::path::Path;

    #[test]
    fn the_output_name_follows_the_oracles_convention() {
        assert_eq!(
            output_path(Path::new("input.pdf"), 3),
            Some(Path::new("input.pdf.3.annot.txt").to_path_buf())
        );
    }

    #[test]
    fn a_name_too_long_for_the_oracles_buffer_writes_nothing() {
        let long = "x".repeat(300);
        assert_eq!(output_path(Path::new(&long), 0), None);
    }
}
