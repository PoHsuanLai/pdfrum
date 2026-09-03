//! Editing a page's object graph: what changed, and which content streams
//! have to be written again because of it (ISO 32000-1 §7.8.2).
//!
//! # A page is a value, so "dirty" is a field and not a callback
//!
//! The C++ page-object holder is a live object graph with observers: touching
//! a page object flips a flag on it and, for a removal, inserts an index into
//! a set on the holder. Our [`Page`] is a plain record, so the same two facts
//! live as two plain fields — [`Content::dirty`](crate::Content::dirty) on each object and
//! [`Page::dirty_streams`] on the page — and the functions in this module are
//! the only things that set them. Nothing observes anything; a mutation is a
//! function from a page to a page with two more bits set.
//!
//! # Why removal needs a set and modification does not
//!
//! A modified object still exists, so the regenerator finds it, sees its
//! `dirty`, and knows to rewrite the stream it names. A *removed* object is
//! gone — nothing is left to point at the stream that has to lose it — so its
//! stream index is recorded on the page before the object goes. That is the
//! whole reason `dirty_streams` exists, and it is why
//! [`Page::remove_object`] takes an index rather than being a `Vec::retain`
//! at the call site.
//!
//! An object switched to inactive is the third case and behaves like the
//! first: it stays in the list, carries `dirty`, and the regenerator skips it
//! when emitting while still counting its stream as needing a rewrite. That
//! is exactly how it disappears.
//!
//! # The streamless object
//!
//! A brand-new object has never been in a content stream, so its
//! [`Content::content_stream`](crate::Content::content_stream) is `None`. It
//! sorts before `Some(0)` in the regenerator's ordered walk, which is what
//! gives a new object the lowest free `/Contents` index rather than one past
//! the end.
//!
//! That ordering used to be bought with a public `NO_CONTENT_STREAM: i32 = -1`
//! that callers compared against. `Option`'s own `Ord` gives the same order,
//! and the compiler makes the check mandatory rather than optional.

use std::collections::BTreeSet;

use crate::page::{Page, PageObject};

/// An object index that is not on the page.
///
/// Returned by [`Page::insert_object`] when `index` is past the end — an index
/// equal to the length appends and is valid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("page object index {index} is out of range (len {len})")]
pub struct IndexOutOfRange {
    /// The index that was asked for.
    pub index: usize,
    /// How many objects the page has.
    pub len: usize,
}

impl PageObject {
    /// Whether the object has been changed since it was parsed.
    ///
    /// A dirty object's content stream is rewritten on the next save; a clean
    /// one's is left byte-for-byte alone.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.common().dirty
    }

    /// Whether the object is painted at all.
    ///
    /// An inactive object stays in the list — so its index and its stream are
    /// still known — but contributes nothing to a regenerated stream, which is
    /// how it disappears from the page without being deleted.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.common().active
    }

    /// Which `/Contents` element the object came from, or `None` for one
    /// that was created rather than parsed.
    #[must_use]
    pub fn content_stream(&self) -> Option<usize> {
        self.common().content_stream
    }

    /// Mark the object changed, so its stream is rewritten on save.
    pub fn mark_dirty(&mut self) {
        *self.common_mut().dirty = true;
    }

    /// Mark the object unchanged. The regenerator does this once a stream has
    /// been written.
    pub fn mark_clean(&mut self) {
        *self.common_mut().dirty = false;
    }

    /// Show or hide the object.
    ///
    /// Changing activity **always** dirties the object, because both
    /// directions change what the stream must contain. Setting it to the value
    /// it already has changes nothing at all — an idempotent call does not
    /// force a regeneration.
    pub fn set_active(&mut self, active: bool) {
        let common = self.common_mut();
        if *common.active == active {
            return;
        }
        *common.active = active;
        *common.dirty = true;
    }

    /// Set the `/Contents` index the object belongs to.
    ///
    /// Used by the regenerator's removal bookkeeping, which renumbers every
    /// object after elements are dropped from the array.
    pub fn set_content_stream(&mut self, stream: Option<usize>) {
        *self.common_mut().content_stream = stream;
    }
}

impl Page {
    /// The objects, for a caller who only reads them.
    #[must_use]
    pub fn objects(&self) -> &[PageObject] {
        &self.objects
    }

    /// The objects that are painted, in painting order.
    pub fn active_objects(&self) -> impl Iterator<Item = &PageObject> {
        self.objects.iter().filter(|o| o.is_active())
    }

    /// The object at `index`, marked dirty for the caller's edit.
    ///
    /// Handing out a `&mut` *is* the edit, so the flag is set on the way out
    /// rather than being the caller's to remember. A caller that only wants to
    /// read uses [`Page::objects`].
    pub fn object_mut(&mut self, index: usize) -> Option<&mut PageObject> {
        let object = self.objects.get_mut(index)?;
        object.mark_dirty();
        Some(object)
    }

    /// Append an object to the end of the page's painting order.
    ///
    /// The object arrives dirty and streamless, so the next save gives it a
    /// content stream of its own — or the lowest free index, when the page had
    /// none.
    pub fn push_object(&mut self, mut object: PageObject) {
        object.mark_dirty();
        object.set_content_stream(None);
        self.objects.push(object);
    }

    /// Insert an object at `index`, shifting the objects there and after.
    ///
    /// A streamless object **adopts its new neighbour's stream** so that the
    /// requested position survives the save: without that it would be appended
    /// to a fresh `/Contents` element, which is drawn last whatever its index
    /// in the list said. An index equal to the length appends.
    ///
    /// # Errors
    ///
    /// [`IndexOutOfRange`] when `index` is past the end.
    pub fn insert_object(
        &mut self,
        index: usize,
        mut object: PageObject,
    ) -> Result<(), IndexOutOfRange> {
        let len = self.objects.len();
        if index > len {
            return Err(IndexOutOfRange { index, len });
        }
        object.mark_dirty();
        if object.content_stream().is_none()
            && let Some(neighbour) = self.objects.get(index)
            && let Some(stream) = neighbour.content_stream()
        {
            object.set_content_stream(Some(stream));
            self.dirty_streams.insert(Some(stream));
        }
        self.objects.insert(index, object);
        Ok(())
    }

    /// Remove the object at `index` and hand it back.
    ///
    /// Its stream is recorded as dirty first — once the object is gone nothing
    /// is left to say the stream must lose it.
    pub fn remove_object(&mut self, index: usize) -> Option<PageObject> {
        if index >= self.objects.len() {
            return None;
        }
        let object = self.objects.remove(index);
        if let Some(stream) = object.content_stream() {
            self.dirty_streams.insert(Some(stream));
        }
        Some(object)
    }

    /// Whether anything on the page needs its content stream rewritten.
    ///
    /// The regenerator's early-out: a page answering `false` keeps its
    /// `/Contents` and `/Resources` bytes exactly as they were.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        !self.dirty_streams.is_empty() || self.objects.iter().any(PageObject::is_dirty)
    }

    /// Every content stream that has to be written again.
    ///
    /// The union of the streams named by dirty objects — **including inactive
    /// ones**, whose streams must be regenerated precisely so that they lose
    /// them — and the streams recorded when objects were removed.
    #[must_use]
    pub fn dirty_stream_set(&self) -> BTreeSet<Option<usize>> {
        let mut set = self.dirty_streams.clone();
        for object in &self.objects {
            if object.is_dirty() {
                set.insert(object.content_stream());
            }
        }
        set
    }

    /// Forget every pending change: the page is now as its bytes describe it.
    ///
    /// Called once a save has written the regenerated streams.
    pub fn mark_clean(&mut self) {
        self.dirty_streams.clear();
        for object in &mut self.objects {
            object.mark_clean();
        }
    }

    /// The transform in force where `stream` begins.
    ///
    /// A content stream inherits whatever transform the streams before it left
    /// behind, so a stream rewritten on its own has to undo that inheritance
    /// before it states its own state — otherwise the regenerated bytes are
    /// interpreted under a matrix the original never saw. Stream 0, and a page
    /// that recorded no transforms at all, begin at the identity; the
    /// streamless object is appended after everything and so begins wherever
    /// the last stream ended.
    #[must_use]
    pub fn ctm_at_start_of_stream(&self, stream: Option<usize>) -> kurbo::Affine {
        let Some(stream) = stream else {
            // Streamless: appended last, so it inherits the final transform.
            return self
                .stream_ctms
                .values()
                .next_back()
                .copied()
                .unwrap_or(kurbo::Affine::IDENTITY);
        };
        if stream == 0 || self.stream_ctms.is_empty() {
            return kurbo::Affine::IDENTITY;
        }
        self.ctm_at_end_of_stream(stream.saturating_sub(1))
    }

    /// The transform in force where `stream` ends.
    ///
    /// A stream that recorded nothing answers with the first later stream that
    /// did — the transform was never changed in between — and falls back to the
    /// last recorded one when there is no later stream at all.
    #[must_use]
    pub fn ctm_at_end_of_stream(&self, stream: usize) -> kurbo::Affine {
        self.stream_ctms
            .range(stream..)
            .next()
            .or_else(|| self.stream_ctms.iter().next_back())
            .map_or(kurbo::Affine::IDENTITY, |(_, m)| *m)
    }
}

#[cfg(test)]
mod tests {
    // The fixtures below build pages of a length they fix themselves, so
    // indexing one is the clearest way to name the object under test.
    #![allow(
        clippy::indexing_slicing,
        reason = "test fixtures index arrays whose length the fixture fixes"
    )]

    use super::IndexOutOfRange;
    use crate::page::{Content, Page, PageObject, PathObject};
    use crate::state::{ContentMarks, GraphicsState};
    use kurbo::{Affine, BezPath};

    fn object(stream: Option<usize>) -> PageObject {
        PageObject::Path(Box::new(Content {
            object: PathObject {
                path: BezPath::new(),
                matrix: Affine::IDENTITY,
                fill_rule: crate::ops::FillRule::Winding,
                stroke: false,
            },
            state: GraphicsState::default(),
            marks: ContentMarks::new(),
            content_stream: stream,
            dirty: false,
            active: true,
        }))
    }

    fn page(streams: &[Option<usize>]) -> Page {
        Page {
            objects: streams.iter().copied().map(object).collect(),
            ..Page::empty()
        }
    }

    /// The common case: every object came out of a real stream.
    fn page_of(streams: &[usize]) -> Page {
        page(&streams.iter().copied().map(Some).collect::<Vec<_>>())
    }

    // A freshly parsed page regenerates nothing.
    #[test]
    fn an_untouched_page_is_clean() {
        let page = page_of(&[0, 0, 1]);
        assert!(!page.is_dirty());
        assert!(page.dirty_stream_set().is_empty());
    }

    #[test]
    fn taking_a_mutable_object_dirties_it() {
        let mut page = page_of(&[0, 1]);
        assert!(page.object_mut(1).is_some());
        assert!(page.is_dirty());
        assert_eq!(page.dirty_stream_set(), [Some(1)].into_iter().collect());
        // Reading does not.
        let mut clean = page.clone();
        clean.mark_clean();
        let _ = clean.objects();
        assert!(!clean.is_dirty());
    }

    #[test]
    fn an_appended_object_is_dirty_and_streamless() {
        let mut page = page_of(&[0]);
        page.push_object(object(Some(7)));
        let added = page.objects.last().expect("pushed");
        assert!(added.is_dirty());
        assert_eq!(added.content_stream(), None);
        assert_eq!(page.dirty_stream_set(), [None].into_iter().collect());
    }

    // A streamless insert adopts the neighbour's stream so its position
    // survives the save.
    #[test]
    fn an_inserted_object_adopts_its_neighbours_stream() {
        let mut page = page_of(&[0, 2, 2]);
        let mut fresh = object(None);
        fresh.set_content_stream(None);
        assert!(page.insert_object(1, fresh).is_ok());
        assert_eq!(page.objects.len(), 4);
        assert_eq!(page.objects[1].content_stream(), Some(2));
        assert!(page.dirty_streams.contains(&Some(2)));
    }

    #[test]
    fn an_insert_past_the_end_is_refused() {
        let mut page = page_of(&[0]);
        let err = page
            .insert_object(2, object(None))
            .expect_err("past the end");
        assert_eq!(err, IndexOutOfRange { index: 2, len: 1 });
        assert_eq!(page.objects.len(), 1);
        // One past the last index appends.
        assert!(page.insert_object(1, object(None)).is_ok());
        assert_eq!(page.objects.len(), 2);
    }

    // Removal is the case that needs the page-level set: the object that knew
    // the stream is gone.
    #[test]
    fn removing_an_object_records_its_stream() {
        let mut page = page_of(&[0, 3]);
        let removed = page.remove_object(1).expect("removed");
        assert_eq!(removed.content_stream(), Some(3));
        assert_eq!(page.objects.len(), 1);
        assert!(page.is_dirty());
        assert_eq!(page.dirty_stream_set(), [Some(3)].into_iter().collect());
    }

    #[test]
    fn removing_a_streamless_object_records_nothing() {
        let mut page = page(&[None]);
        assert!(page.remove_object(0).is_some());
        assert!(page.dirty_streams.is_empty());
        assert!(!page.is_dirty());
        assert!(page.remove_object(0).is_none());
    }

    // An inactive object's stream is still regenerated — that is how the
    // object disappears from it.
    #[test]
    fn deactivating_an_object_dirties_its_stream() {
        let mut page = page_of(&[0, 1]);
        page.objects[1].set_active(false);
        assert!(!page.objects[1].is_active());
        assert!(page.objects[1].is_dirty());
        assert_eq!(page.dirty_stream_set(), [Some(1)].into_iter().collect());
        assert_eq!(page.active_objects().count(), 1);
    }

    #[test]
    fn setting_activity_to_what_it_already_is_changes_nothing() {
        let mut page = page_of(&[0]);
        page.objects[0].set_active(true);
        assert!(!page.is_dirty());
    }

    #[test]
    fn a_save_leaves_the_page_clean() {
        let mut page = page_of(&[0, 1]);
        page.object_mut(0);
        let _ = page.remove_object(1);
        assert!(page.is_dirty());
        page.mark_clean();
        assert!(!page.is_dirty());
        assert!(page.objects.iter().all(|o| !o.is_dirty()));
    }

    // The CTM lookups, against the C++'s own three cases.
    #[test]
    fn stream_zero_and_an_empty_map_begin_at_the_identity() {
        let page = page_of(&[0]);
        assert_eq!(page.ctm_at_start_of_stream(Some(0)), Affine::IDENTITY);
        assert_eq!(page.ctm_at_start_of_stream(Some(3)), Affine::IDENTITY);
        assert_eq!(page.ctm_at_end_of_stream(0), Affine::IDENTITY);
    }

    #[test]
    fn a_later_stream_begins_where_the_previous_one_ended() {
        let mut page = page_of(&[0, 1]);
        let scaled = Affine::scale(2.0);
        let moved = Affine::translate((5.0, 0.0));
        page.stream_ctms.insert(0, scaled);
        page.stream_ctms.insert(1, moved);
        assert_eq!(page.ctm_at_start_of_stream(Some(0)), Affine::IDENTITY);
        assert_eq!(page.ctm_at_start_of_stream(Some(1)), scaled);
        assert_eq!(page.ctm_at_end_of_stream(1), moved);
        // A streamless object is appended last, so it starts at the end
        // of everything.
        assert_eq!(page.ctm_at_start_of_stream(None), moved);
    }

    // A stream that changed nothing has no entry; the lookup rolls forward to
    // the first that did.
    #[test]
    fn a_stream_with_no_entry_reads_the_next_one_that_has_it() {
        let mut page = page_of(&[0]);
        let scaled = Affine::scale(3.0);
        page.stream_ctms.insert(0, Affine::IDENTITY);
        page.stream_ctms.insert(4, scaled);
        assert_eq!(page.ctm_at_end_of_stream(2), scaled);
        // Past the last entry, the last entry answers.
        assert_eq!(page.ctm_at_end_of_stream(9), scaled);
    }
}
