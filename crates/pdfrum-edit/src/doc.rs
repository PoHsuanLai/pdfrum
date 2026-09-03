//! The editable view of a document: a read-only base plus an overlay of
//! changes.
//!
//! # Why the base stays immutable
//!
//! [`Document`] is a `Sync`, lazily-caching reader over `Arc<[u8]>`, and every
//! crate above it borrows from that shape. Making it mutable so the writer
//! could edit in place would cost the `OnceLock` store, `Sync`, and every
//! downstream borrow — to serve exactly one caller.
//!
//! So edits live here instead. [`EditDoc`] holds `&Document` plus a map of
//! added and replaced objects and a set of removed ones, and implements
//! [`Resolve`] by asking the overlay first and the base second, so an editor
//! reads a flattened view of the document without adding a new seam.
//!
//! The C++'s writer has a memory dance this shape removes entirely: it fetches
//! an old object, writes it, and then *deletes it from the document again* so
//! that saving does not permanently grow the in-memory object map. Our overlay
//! never materializes an object it did not need, so there is nothing to undo —
//! and "save twice, get the same bytes" falls out rather than being arranged.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use pdfrum_object::{ObjRef, Object, Resolve};
use pdfrum_parser::Document;

/// A document plus the edits made to it.
///
/// Cheap to create and to drop: it borrows the base and owns only what
/// changed.
#[derive(Debug)]
pub struct EditDoc<'a> {
    base: &'a Document,
    /// Objects added or replaced, by number. Sorted, because the writer walks
    /// new objects in ascending order and the subsetter binary-searches them.
    overlay: BTreeMap<u32, Arc<Object>>,
    /// Objects removed. A removed object resolves as null and is not written.
    removed: BTreeSet<u32>,
    /// The next number [`EditDoc::add`] will hand out.
    next_num: u32,
}

impl<'a> EditDoc<'a> {
    /// An unedited view of `base`.
    #[must_use]
    pub fn new(base: &'a Document) -> Self {
        Self {
            base,
            overlay: BTreeMap::new(),
            removed: BTreeSet::new(),
            // One past the highest number the file used, so a fresh object
            // can never collide with one the xref already names.
            next_num: base.xref().last_object_number().saturating_add(1),
        }
    }

    /// The document underneath the edits.
    #[must_use]
    pub fn base(&self) -> &'a Document {
        self.base
    }

    /// Add `obj` as a new indirect object, returning the reference that names
    /// it. Generation is always 0: the writer emits nothing else.
    pub fn add(&mut self, obj: Object) -> ObjRef {
        let num = self.next_num;
        self.next_num = self.next_num.saturating_add(1);
        self.overlay.insert(num, Arc::new(obj));
        self.removed.remove(&num);
        ObjRef::new(num, 0)
    }

    /// Replace what `r` names.
    ///
    /// The base is untouched: the overlay simply answers first from now on.
    pub fn replace(&mut self, r: ObjRef, obj: Object) {
        self.overlay.insert(r.num, Arc::new(obj));
        self.removed.remove(&r.num);
        self.next_num = self.next_num.max(r.num.saturating_add(1));
    }

    /// Remove what `r` names. It then resolves as null and is not written.
    pub fn remove(&mut self, r: ObjRef) {
        self.overlay.remove(&r.num);
        self.removed.insert(r.num);
    }

    /// Whether `num` was removed.
    #[must_use]
    pub fn is_removed(&self, num: u32) -> bool {
        self.removed.contains(&num)
    }

    /// The overlay's objects in ascending number order — everything this
    /// editing session added or replaced.
    pub fn edited(&self) -> impl Iterator<Item = (u32, &Arc<Object>)> {
        self.overlay.iter().map(|(n, o)| (*n, o))
    }

    /// Whether `num` has an overlay entry.
    #[must_use]
    pub fn is_edited(&self, num: u32) -> bool {
        self.overlay.contains_key(&num)
    }

    /// The number one past the highest this view can name.
    #[must_use]
    pub fn next_object_number(&self) -> u32 {
        self.next_num
    }

    /// The highest object number in play, across the base and the overlay.
    #[must_use]
    pub fn last_object_number(&self) -> u32 {
        let base = self.base.xref().last_object_number();
        self.overlay
            .keys()
            .next_back()
            .copied()
            .unwrap_or(0)
            .max(base)
    }
}

impl Resolve for EditDoc<'_> {
    fn fetch(&self, r: ObjRef) -> Result<Arc<Object>, pdfrum_object::Error> {
        if self.removed.contains(&r.num) {
            return Ok(Arc::new(Object::Null));
        }
        if let Some(obj) = self.overlay.get(&r.num) {
            return Ok(Arc::clone(obj));
        }
        self.base.fetch(r)
    }
}

#[cfg(test)]
mod tests {
    use super::EditDoc;
    use pdfrum_object::{ObjRef, Object, Resolve};
    use pdfrum_parser::{Document, LoadOptions, load};
    use std::sync::Arc;

    fn doc() -> Document {
        let file = b"%PDF-1.7\n\
1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n\
2 0 obj\n<< /Type /Pages /Count 1 /Kids [3 0 R] >>\nendobj\n\
3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>\nendobj\n\
trailer\n<< /Root 1 0 R /Size 4 >>\n";
        load(Arc::from(&file[..]), &LoadOptions::default()).expect("opens")
    }

    #[test]
    fn an_unedited_view_reads_straight_through() {
        let base = doc();
        let edit = EditDoc::new(&base);
        let catalog = edit.fetch(ObjRef::new(1, 0)).expect("catalog");
        assert!(catalog.as_dict().is_some());
        assert!(edit.edited().next().is_none());
    }

    // New numbers start past the file's highest, so nothing collides.
    #[test]
    fn added_objects_take_fresh_numbers() {
        let base = doc();
        let mut edit = EditDoc::new(&base);
        let first = edit.add(Object::Int(1));
        let second = edit.add(Object::Int(2));
        assert!(first.num > base.xref().last_object_number());
        assert_eq!(second.num, first.num + 1);
        assert_eq!(first.generation, 0);
        assert_eq!(*edit.fetch(first).expect("added"), Object::Int(1));
    }

    #[test]
    fn the_overlay_answers_before_the_base() {
        let base = doc();
        let mut edit = EditDoc::new(&base);
        let page = ObjRef::new(3, 0);
        assert!(edit.fetch(page).expect("page").as_dict().is_some());
        edit.replace(page, Object::Int(99));
        assert_eq!(*edit.fetch(page).expect("replaced"), Object::Int(99));
        // The base itself never changed.
        assert!(base.fetch(page).expect("page").as_dict().is_some());
    }

    // A removed object resolves as null rather than as an error, which is how
    // a dangling reference already reads to everything downstream.
    #[test]
    fn a_removed_object_reads_as_null() {
        let base = doc();
        let mut edit = EditDoc::new(&base);
        let page = ObjRef::new(3, 0);
        edit.remove(page);
        assert!(edit.fetch(page).expect("null").is_null());
        assert!(edit.is_removed(3));
        assert!(!edit.is_edited(3));
    }

    #[test]
    fn replacing_a_removed_object_brings_it_back() {
        let base = doc();
        let mut edit = EditDoc::new(&base);
        let page = ObjRef::new(3, 0);
        edit.remove(page);
        edit.replace(page, Object::Int(7));
        assert!(!edit.is_removed(3));
        assert_eq!(*edit.fetch(page).expect("back"), Object::Int(7));
    }

    #[test]
    fn edits_come_back_in_ascending_number_order() {
        let base = doc();
        let mut edit = EditDoc::new(&base);
        edit.replace(ObjRef::new(9, 0), Object::Int(9));
        edit.replace(ObjRef::new(2, 0), Object::Int(2));
        edit.replace(ObjRef::new(5, 0), Object::Int(5));
        let nums: Vec<u32> = edit.edited().map(|(n, _)| n).collect();
        assert_eq!(nums, vec![2, 5, 9]);
    }

    // Replacing past the end moves the allocator, so a later add cannot
    // land on a number an edit already claimed.
    #[test]
    fn replacing_past_the_end_moves_the_allocator() {
        let base = doc();
        let mut edit = EditDoc::new(&base);
        edit.replace(ObjRef::new(100, 0), Object::Int(1));
        assert_eq!(edit.add(Object::Int(2)).num, 101);
        assert_eq!(edit.last_object_number(), 101);
    }
}
