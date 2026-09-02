//! The curated page mutations `--mutate=` applies before a save.
//!
//! # Why the tool mutates at all
//!
//! The oracle cannot save a document, so the milestone's exit check is a
//! sequence rather than a flag diff: pdfrum mutates and saves, then the
//! *oracle* reopens the result and renders it, and its render is compared
//! against ours of the same file. That answers the question no test inside
//! this workspace can — whether a page we rewrote is a page another
//! implementation draws the same way — and it needs a mutation the harness
//! can ask for by name.
//!
//! # Three mutations, chosen to break in different places
//!
//! - **`add-rect`** exercises a brand-new streamless object: a fresh
//!   `/Contents` element, the shape transition that creates it, and the
//!   `FXE`-named graphics state it paints under.
//! - **`remove-first`** exercises removal bookkeeping: the dirty-stream set a
//!   vanished object leaves behind, and — when it was the only object in its
//!   element — the element deletion and index collapse that follow.
//! - **`touch-all`** exercises regeneration itself, changing nothing: every
//!   object is marked dirty, so every stream is rewritten from the graph. Its
//!   render is the one that measures the emitter's fidelity, because the page
//!   it produces is meant to be the page it started from, minus the documented
//!   losses.
//!
//! Every one is applied to **page 0 only**, so a multi-page document's other
//! pages are the control: they must come through byte-untouched in the same
//! save that rewrote the first.

use std::collections::BTreeMap;

use pdfrum_common::{Diagnostics, Limits};
use pdfrum_edit::EditDoc;
use pdfrum_page::GraphicsState;
use pdfrum_page::{BuildContext, ColorSpace, Content, FillRule, PageObject, PathObject};
use pdfrum_parser::Document;

/// Which mutation `--mutate=` names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mutation {
    /// Add a filled rectangle over the middle of page 0.
    AddRect,
    /// Remove page 0's first page object.
    RemoveFirst,
    /// Mark every object on page 0 changed, without changing any of them.
    TouchAll,
}

impl Mutation {
    /// The mutation a `--mutate=` value names, or `None`.
    ///
    /// Matched against [`ALL`] rather than a second spelling of the same
    /// list, so a mutation cannot be added and left unreachable.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        ALL.into_iter().find(|m| m.name() == value)
    }

    /// The name this mutation is asked for by.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::AddRect => "add-rect",
            Self::RemoveFirst => "remove-first",
            Self::TouchAll => "touch-all",
        }
    }
}

/// Every mutation the tool offers, for a harness that sweeps all of them.
pub const ALL: [Mutation; 3] = [Mutation::AddRect, Mutation::RemoveFirst, Mutation::TouchAll];

/// What applying a mutation did, for the line the tool prints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Applied {
    /// The page was rewritten, and this many `/Contents` elements were
    /// written.
    Rewritten { streams: usize },
    /// Nothing was rewritten, and why.
    Skipped(String),
}

/// Apply `mutation` to page 0 of `doc`, staging the result in `edit`.
///
/// The page's object graph is built, mutated and regenerated; the resulting
/// streams and swept resources go into the overlay, and the save that follows
/// writes them. Everything else in the document is untouched.
pub fn apply(
    doc: &Document,
    edit: &mut EditDoc<'_>,
    mutation: Mutation,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> Applied {
    let Ok(page_dict) = doc.page(0) else {
        return Applied::Skipped("the document has no page 0".to_owned());
    };
    let Some(page_ref) = page_dict.reference else {
        // A page written inline in its parent's `/Kids` has no object to
        // replace, so its content cannot be rewritten.
        return Applied::Skipped("page 0 is not an indirect object".to_owned());
    };

    let resources = page_dict
        .inherited(pdfrum_object::names::RESOURCES, doc)
        .and_then(|object| object.resolve(doc).ok()?.as_dict().cloned())
        .unwrap_or_default();

    let mut ctx = BuildContext::new();
    let mut page =
        crate::content::build_page_for_edit(doc, &page_dict, &resources, &mut ctx, limits, diags);

    match mutation {
        Mutation::AddRect => page.push_object(rectangle(&page)),
        Mutation::RemoveFirst => {
            if page.remove_object(0).is_none() {
                return Applied::Skipped("page 0 draws nothing to remove".to_owned());
            }
        }
        Mutation::TouchAll => {
            if page.objects().is_empty() {
                return Applied::Skipped("page 0 draws nothing to touch".to_owned());
            }
            for index in 0..page.objects().len() {
                page.object_mut(index);
            }
        }
    }

    let Some(rewrite) = pdfrum_edit::regenerate(&page, &resources, doc) else {
        return Applied::Skipped("the mutation dirtied nothing".to_owned());
    };
    let streams = rewrite.streams.len();
    let shared = pdfrum_edit::shared_objects(edit);
    let _: BTreeMap<usize, usize> =
        pdfrum_edit::apply_rewrite(edit, page_ref, &page_dict.dict, &rewrite, &shared);
    Applied::Rewritten { streams }
}

/// A mid-grey rectangle over the middle half of the page.
///
/// Sized from the page's own crop box so it lands on the page whatever the
/// page's size, and grey so that it is visible against both a white ground and
/// black text — which is what makes a pixel comparison of the result mean
/// something.
fn rectangle(page: &pdfrum_page::Page) -> PageObject {
    let box_rect = page.crop_box;
    let (w, h) = (box_rect.width(), box_rect.height());
    let rect = kurbo::Rect::new(
        box_rect.x0 + w * 0.25,
        box_rect.y0 + h * 0.25,
        box_rect.x0 + w * 0.75,
        box_rect.y0 + h * 0.75,
    );
    let mut path = kurbo::BezPath::new();
    path.move_to((rect.x0, rect.y0));
    path.line_to((rect.x1, rect.y0));
    path.line_to((rect.x1, rect.y1));
    path.line_to((rect.x0, rect.y1));
    path.close_path();

    let mut state = GraphicsState::default();
    state
        .fill
        .set_stock(ColorSpace::DeviceRgb, &[0.5, 0.5, 0.5]);

    PageObject::Path(Box::new(Content::new(
        PathObject {
            path,
            matrix: kurbo::Affine::IDENTITY,
            fill_rule: FillRule::Winding,
            stroke: false,
        },
        state,
    )))
}

#[cfg(test)]
mod tests {
    use super::{ALL, Mutation};

    #[test]
    fn every_mutation_round_trips_through_its_name() {
        for mutation in ALL {
            assert_eq!(Mutation::parse(mutation.name()), Some(mutation));
        }
        assert_eq!(Mutation::parse("nonsense"), None);
    }

    // The three exist to break in three different places; a duplicate name
    // would silently drop one from the harness's sweep.
    #[test]
    fn the_names_are_distinct() {
        let mut names: Vec<&str> = ALL.iter().map(|m| m.name()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), ALL.len());
    }
}
