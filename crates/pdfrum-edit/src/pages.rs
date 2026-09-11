//! Page-tree edits that are not imports: a blank page, deleting pages, and
//! the per-page attributes a save carries — rotation and the boxes.
//!
//! Every function works on the page tree as the base document has it, so
//! page indices are the base document's numbering throughout; a caller who
//! deletes and then adds does both against that numbering, and the writer
//! resolves the edits together.

use pdfrum_common::PageIndex;
use pdfrum_object::{Array, Dict, Name, ObjRef, Object, Resolve, names};

use crate::doc::EditDoc;
use crate::error::Error;
use crate::import::{PageRange, init_dest, insert_into_tree};

/// One of the five page boxes (ISO 32000-1 §14.11.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageBox {
    /// `/MediaBox`.
    Media,
    /// `/CropBox`.
    Crop,
    /// `/BleedBox`.
    Bleed,
    /// `/TrimBox`.
    Trim,
    /// `/ArtBox`.
    Art,
}

impl PageBox {
    fn key(self) -> &'static Name {
        match self {
            Self::Media => names::MEDIA_BOX,
            Self::Crop => names::CROP_BOX,
            Self::Bleed => names::BLEED_BOX,
            Self::Trim => names::TRIM_BOX,
            Self::Art => names::ART_BOX,
        }
    }
}

/// Add an empty page of `width` by `height` points at index `at` (past the
/// end appends), and return its reference.
///
/// The page carries only `/Type`, `/Parent` and `/MediaBox`; it has no
/// contents and no resources until a caller draws on it.
///
/// # Errors
///
/// [`Error::NoDestinationCatalog`] when the document has no catalog to hang
/// a page tree on.
pub fn add_blank_page(
    dest: &mut EditDoc<'_>,
    width: f32,
    height: f32,
    at: impl Into<PageIndex>,
) -> Result<ObjRef, Error> {
    let at = u32::from(at.into());
    let pages_node = init_dest(dest)?;
    let page = Dict::from_pairs([
        (names::TYPE.clone(), Object::Name(names::PAGE.clone())),
        (
            names::PARENT.clone(),
            Object::Ref(ObjRef::new(pages_node, 0)),
        ),
        (
            names::MEDIA_BOX.clone(),
            rect_object([0.0, 0.0, width, height]),
        ),
    ]);
    let created = dest.add(Object::Dict(page));
    insert_into_tree(dest, pages_node, &[created], at);
    Ok(created)
}

/// Delete the pages `range` names, by the base document's numbering.
///
/// Each page is unlinked from its parent's `/Kids` and every `/Count` up the
/// chain is decremented; the page object itself is left for the writer's
/// garbage collection. A duplicate index deletes once.
///
/// # Errors
///
/// [`Error::PageIndexOutOfRange`] when the range names a page the document
/// does not have; the document is untouched on that.
pub fn delete_pages(dest: &mut EditDoc<'_>, range: &PageRange) -> Result<(), Error> {
    let mut refs = Vec::with_capacity(range.indices().len());
    for &index in range.indices() {
        let page = dest
            .base()
            .page(index)
            .map_err(|_| Error::PageIndexOutOfRange(index))?;
        let Some(reference) = page.reference else {
            // A page written inline in its parent's `/Kids` has no reference
            // to unlink by; the oracle cannot delete one either.
            return Err(Error::PageIndexOutOfRange(index));
        };
        if !refs.contains(&reference) {
            refs.push(reference);
        }
    }
    for reference in refs {
        unlink(dest, reference);
    }
    Ok(())
}

/// Unlink one page from its parent and decrement the counts above it.
fn unlink(dest: &mut EditDoc<'_>, page: ObjRef) {
    let Some(page_dict) = dict_of(dest, page) else {
        return;
    };
    let Some(Object::Ref(parent_ref)) = page_dict.raw(names::PARENT).cloned() else {
        return;
    };
    let Some(mut parent) = dict_of(dest, parent_ref) else {
        return;
    };
    let kids: Array = parent
        .raw(names::KIDS)
        .and_then(Object::as_array)
        .map(|kids| {
            kids.iter()
                .filter(|kid| !matches!(kid, Object::Ref(r) if *r == page))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    parent.insert(names::KIDS.clone(), Object::Array(kids));
    dest.replace(parent_ref, Object::Dict(parent));
    decrement_counts(dest, parent_ref);
}

/// Take one off `/Count` on `node` and every ancestor.
fn decrement_counts(dest: &mut EditDoc<'_>, mut node_ref: ObjRef) {
    // A cycle in `/Parent` would loop forever; the tree's depth bounds it.
    for _ in 0..64 {
        let Some(mut node) = dict_of(dest, node_ref) else {
            return;
        };
        let count = node
            .raw(names::COUNT)
            .and_then(Object::as_int)
            .unwrap_or(0)
            .saturating_sub(1)
            .max(0);
        node.insert(names::COUNT.clone(), Object::Int(count));
        let parent = node.raw(names::PARENT).cloned();
        dest.replace(node_ref, Object::Dict(node));
        match parent {
            Some(Object::Ref(parent_ref)) => node_ref = parent_ref,
            _ => return,
        }
    }
}

/// Set a page's `/Rotate`. `degrees` is normalized to a multiple of 90; a
/// value that is not one is rounded to the nearest.
///
/// # Errors
///
/// [`Error::PageIndexOutOfRange`] when there is no such page or the page is
/// written inline.
pub fn set_page_rotation(
    dest: &mut EditDoc<'_>,
    index: impl Into<PageIndex>,
    degrees: i32,
) -> Result<(), Error> {
    let index = index.into();
    // Nearest multiple of 90, then modulo a full turn: 45 rounds up, -90
    // is 270.
    let quarter_turns =
        (degrees.div_euclid(90) + i32::from(degrees.rem_euclid(90) >= 45)).rem_euclid(4);
    let quarter_turns = i64::from(quarter_turns);
    edit_page(dest, index, |page| {
        page.insert(names::ROTATE.clone(), Object::Int(quarter_turns * 90));
    })
}

/// Set one of a page's boxes.
///
/// # Errors
///
/// As [`set_page_rotation`].
pub fn set_page_box(
    dest: &mut EditDoc<'_>,
    index: impl Into<PageIndex>,
    which: PageBox,
    rect: [f32; 4],
) -> Result<(), Error> {
    let index = index.into();
    edit_page(dest, index, |page| {
        page.insert(which.key().clone(), rect_object(rect));
    })
}

/// Fetch a page's dictionary as the edits leave it, change it, write it back.
fn edit_page(
    dest: &mut EditDoc<'_>,
    index: PageIndex,
    change: impl FnOnce(&mut Dict),
) -> Result<(), Error> {
    let page = dest
        .base()
        .page(index)
        .map_err(|_| Error::PageIndexOutOfRange(index))?;
    let Some(reference) = page.reference else {
        return Err(Error::PageIndexOutOfRange(index));
    };
    let mut dict = dict_of(dest, reference).unwrap_or(page.dict);
    change(&mut dict);
    dest.replace(reference, Object::Dict(dict));
    Ok(())
}

/// A dictionary as the edits currently leave it.
fn dict_of(dest: &EditDoc<'_>, reference: ObjRef) -> Option<Dict> {
    dest.fetch(reference)
        .ok()
        .and_then(|object| object.as_dict().cloned())
}

fn rect_object(rect: [f32; 4]) -> Object {
    Object::Array(rect.into_iter().map(Object::Real).collect())
}
