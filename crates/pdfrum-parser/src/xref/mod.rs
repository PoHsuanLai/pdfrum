//! Cross-reference information: where every object in the file lives
//! (ISO 32000-1 §7.5.4 to §7.5.8).
//!
//! # One map, many sources
//!
//! A file can describe its objects four ways — a classic table, a
//! cross-reference stream, both at once in a hybrid file, or a chain of
//! `/Prev`-linked sections recording successive edits — and a damaged file
//! can describe them none of those ways, in which case the whole file is
//! scanned for object headers instead. All four paths and the recovery scan
//! produce the same [`Xref`], so nothing above this module needs to know
//! which one ran.
//!
//! # Newest wins, and the merge is asymmetric
//!
//! Sections are read newest-first and merged so that an entry already present
//! is never overwritten by an older section's version of it. The trailer
//! merge inverts that for exactly two keys: `/Prev` and `/XRefStm` keep the
//! *older* section's values, because those are the pointers the walk is
//! following and taking the newer ones would make it revisit sections it has
//! already read. That asymmetry is the whole reason the walk terminates.

mod chain;
mod classic;
mod rebuild;
mod stream;

use std::collections::BTreeMap;

use pdfrum_common::{Diagnostics, Limits};
use pdfrum_object::{Dict, Name, Object, names};

pub(crate) use chain::XrefShape;
pub(crate) use rebuild::rebuild;

/// Where one object lives.
///
/// The three variants are the three things a cross-reference entry can say
/// (ISO 32000-1 tables 18 and 18): the object is at a byte offset, it is
/// packed inside an object stream, or it is not there at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entry {
    /// The object's `N G obj` header starts at this byte offset.
    Offset(u64),
    /// The object is the `index`-th member of an object stream.
    InObjStream {
        /// The object stream holding it.
        stream: pdfrum_object::ObjRef,
        /// Its position in that stream's member table.
        index: u32,
    },
    /// The slot is free: the object was deleted, or never existed.
    Free,
}

/// One row of the table: where the object is, plus the bookkeeping the
/// reader needs to merge sections correctly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct XEntry {
    /// Where the object lives.
    pub kind: Entry,
    /// The generation the table claims. Merges compare it: an entry naming a
    /// newer generation is never replaced by one naming an older.
    pub generation: u16,
    /// Whether some entry has named this object as its object stream. Only
    /// an object flagged here may be *used* as one, which is what stops a
    /// forged `/Type /ObjStm` from being decoded as a container.
    pub objstm_flag: bool,
}

impl XEntry {
    /// A free entry of the given generation.
    fn free(generation: u16) -> Self {
        Self {
            kind: Entry::Free,
            generation,
            objstm_flag: false,
        }
    }
}

impl Default for XEntry {
    fn default() -> Self {
        Self::free(0)
    }
}

/// The trailer dictionary plus which object it came out of.
///
/// The object number matters to incremental saving: a trailer written as a
/// plain `trailer` keyword has no object number, while one that is a
/// cross-reference stream's dictionary belongs to that stream's object.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Trailer {
    /// The trailer's keys.
    pub dict: Dict,
    /// The object number the trailer came from; zero when it was written as
    /// a bare `trailer` dictionary.
    pub object_number: u32,
}

/// Where every object in a document lives.
///
/// A sorted map, because reading it in object-number order is what both the
/// page walk and the writer want, and because the largest object number is a
/// value the reader consults constantly.
#[derive(Debug, Clone, Default)]
pub struct Xref {
    entries: BTreeMap<u32, XEntry>,
    /// The cross-reference sections the load followed, oldest first: each
    /// one an incremental update's table or stream, at its byte offset.
    sections: Vec<Section>,
}

/// One cross-reference section of the file, as the `/Prev` chain found it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Section {
    /// Where the section starts.
    pub offset: u64,
    /// A cross-reference stream rather than a classic table.
    pub is_stream: bool,
}

impl Xref {
    /// The sections the load followed, oldest first — one per revision of
    /// an incrementally updated file. Empty when the table was rebuilt by
    /// scanning, since no chain was followed then.
    #[must_use]
    pub fn sections(&self) -> &[Section] {
        &self.sections
    }

    /// Record the chain the load followed.
    pub(crate) fn set_sections(&mut self, sections: Vec<Section>) {
        self.sections = sections;
    }

    /// An empty table.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Where object `num` lives, if the table says anything about it.
    #[must_use]
    pub fn entry(&self, num: u32) -> Option<Entry> {
        self.entries.get(&num).map(|e| e.kind)
    }

    /// The generation the table records for `num`.
    #[must_use]
    pub fn generation(&self, num: u32) -> u16 {
        self.entries.get(&num).map_or(0, |e| e.generation)
    }

    /// Whether some entry named `num` as the object stream it lives in.
    #[must_use]
    pub fn is_object_stream(&self, num: u32) -> bool {
        self.entries.get(&num).is_some_and(|e| e.objstm_flag)
    }

    /// How many objects the table describes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the table describes nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Every object number the table describes, in ascending order.
    pub fn object_numbers(&self) -> impl Iterator<Item = u32> + '_ {
        self.entries.keys().copied()
    }

    /// Every entry, in ascending object-number order.
    pub(crate) fn iter(&self) -> impl Iterator<Item = (u32, XEntry)> + '_ {
        self.entries.iter().map(|(&num, &e)| (num, e))
    }

    /// The largest object number the table describes.
    #[must_use]
    pub fn last_object_number(&self) -> u32 {
        self.entries.keys().next_back().copied().unwrap_or(0)
    }

    /// Whether `num` is a number this table could possibly describe.
    ///
    /// Object numbers past the table's largest are unfetchable even when the
    /// bytes for them sit in the file — the table is the only index the
    /// reader has, and PDFium refuses to look past its end.
    #[must_use]
    pub fn is_valid_object_number(&self, num: u32) -> bool {
        num <= self.last_object_number()
    }

    /// Record an object at a byte offset.
    ///
    /// Ignored when an entry already present names a *newer* generation, so
    /// a later-read older section cannot undo a newer one's edit. The
    /// object-stream flag is sticky: once something has named this object as
    /// a container, overwriting the entry does not clear that.
    ///
    /// The returned flag says only whether the object *number* was usable —
    /// not whether the entry changed. Declining to overwrite a newer
    /// generation is a success: the table already knows something better
    /// about that object.
    pub(crate) fn add_normal(
        &mut self,
        num: u32,
        generation: u16,
        is_objstm: bool,
        pos: u64,
        limits: &Limits,
    ) -> bool {
        if num > limits.max_object_number {
            return false;
        }
        let flag = match self.entries.get(&num) {
            Some(existing) if existing.generation > generation => return true,
            Some(existing) => existing.objstm_flag || is_objstm,
            None => is_objstm,
        };
        self.entries.insert(
            num,
            XEntry {
                kind: Entry::Offset(pos),
                generation,
                objstm_flag: flag,
            },
        );
        true
    }

    /// Record an object as living inside an object stream.
    ///
    /// Ignored when the existing entry names a non-zero generation or is
    /// itself a known container: a compressed object always has generation
    /// zero, so an entry claiming otherwise is describing something else.
    /// The archive's own entry is created if absent and flagged either way —
    /// that flag is what later authorizes decoding it as a container.
    ///
    /// As with [`Xref::add_normal`], the returned flag reports the object
    /// numbers' usability rather than whether anything was written.
    pub(crate) fn add_compressed(
        &mut self,
        num: u32,
        archive: u32,
        index: u32,
        limits: &Limits,
    ) -> bool {
        if num > limits.max_object_number || archive > limits.max_object_number {
            return false;
        }
        if let Some(existing) = self.entries.get(&num)
            && (existing.generation > 0 || existing.objstm_flag)
        {
            return true;
        }
        self.entries.insert(
            num,
            XEntry {
                kind: Entry::InObjStream {
                    stream: pdfrum_object::ObjRef::new(archive, 0),
                    index,
                },
                generation: 0,
                objstm_flag: false,
            },
        );
        self.entries.entry(archive).or_default().objstm_flag = true;
        true
    }

    /// Mark an object free, unconditionally.
    pub(crate) fn set_free(&mut self, num: u32, generation: u16) {
        self.entries.insert(num, XEntry::free(generation));
    }

    /// Resize the table to describe exactly `size` objects.
    ///
    /// Two things happen, both observable. Entries for object numbers at or
    /// past `size` are erased — a trailer `/Size` is a claim about what the
    /// file contains and the reader honors it. And the last slot is
    /// materialized as free if nothing else claimed it, which is why a
    /// document's table often ends in an entry no section ever wrote.
    pub(crate) fn set_size(&mut self, size: u32) {
        if size == 0 {
            self.entries.clear();
            return;
        }
        self.entries.retain(|&num, _| num < size);
        self.entries.entry(size - 1).or_default();
    }

    /// Apply `top` onto `self`, with `top`'s entries winning conflicts.
    ///
    /// The one thing carried the other way: when both sides have an offset
    /// entry for the same object, `self`'s object-stream flag survives onto
    /// the winner, because knowing an object is a container is information
    /// neither section can invalidate.
    pub(crate) fn merge_up(&mut self, top: &Self) {
        for (&num, &entry) in &top.entries {
            let merged = match (self.entries.get(&num), entry.kind) {
                (Some(current), Entry::Offset(_))
                    if matches!(current.kind, Entry::Offset(_)) && current.objstm_flag =>
                {
                    XEntry {
                        objstm_flag: true,
                        ..entry
                    }
                }
                _ => entry,
            };
            self.entries.insert(num, merged);
        }
    }
}

/// Merge `winner`'s keys onto `base`, leaving the result in `base`.
///
/// Every key of `winner` overwrites `base`'s — except `/Prev` and `/XRefStm`,
/// which stay as **`base`** has them. Those two say where to look next, and
/// replacing them would send the walk through sections it has already read.
///
/// The two roles are not "older" and "newer", and getting them backwards is
/// the easiest mistake in this file. During the `/Prev` walk the trailer
/// accumulated so far is the *newer* one and wins, while the section just
/// read is the *older* one supplying the next pointer — so `base` is the
/// freshly-read older trailer and `winner` is the accumulation. During the
/// recovery scan the roles invert: what has been scanned so far is `base` and
/// each newly-found trailer wins. Callers say which they mean by argument
/// order; see [`merge_into_walk`].
pub(crate) fn merge_trailers(base: &mut Trailer, winner: &Trailer) {
    if base.dict.is_empty() && base.object_number == 0 {
        *base = winner.clone();
        return;
    }
    let kept: Vec<(Name, Option<Object>)> = [names::XREF_STM, names::PREV]
        .into_iter()
        .map(|key| (key.clone(), base.dict.raw(key).cloned()))
        .collect();

    for (key, value) in winner.dict.iter() {
        base.dict.push(key.clone(), value.clone());
    }
    for (key, value) in kept {
        match value {
            Some(v) => base.dict.push(key, v),
            // `base` had no such pointer, so the winner's must not survive.
            None => remove_key(&mut base.dict, &key),
        }
    }
}

/// Fold a section's trailer into the walk's accumulation.
///
/// The walk reads newest-first, so `section` is *older* than everything
/// `accumulated` holds: its keys lose, and its `/Prev` and `/XRefStm` — the
/// pointers that say where to go next — are the ones kept. That inversion is
/// why this wrapper exists rather than callers arranging the arguments
/// themselves.
pub(crate) fn merge_into_walk(accumulated: &mut Trailer, section: &Trailer) {
    let mut merged = section.clone();
    merge_trailers(&mut merged, accumulated);
    // The accumulated trailer keeps its own object number, since the walk's
    // identity is the newest section's.
    if !accumulated.dict.is_empty() || accumulated.object_number != 0 {
        merged.object_number = accumulated.object_number;
    }
    *accumulated = merged;
}

/// Drop every entry with this key.
fn remove_key(dict: &mut Dict, key: &Name) {
    let kept: Vec<(Name, Object)> = dict
        .iter()
        .filter(|(k, _)| k != key)
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    *dict = Dict::from_pairs(kept);
}

/// Read a document's cross-reference information, rebuilding it if needed.
///
/// `file` is the document from its `%PDF` header onwards, so every offset
/// this returns indexes straight into it. Failure means neither the
/// structured paths nor the full-file scan found anything usable.
///
/// # Errors
///
/// [`Error::XrefBroken`](crate::Error::XrefBroken) when no section could be
/// read and the rebuild found no objects or no trailer.
///
/// ```
/// use pdfrum_common::{Diagnostics, Limits};
/// use pdfrum_parser::{Entry, read_xref};
///
/// let file = b"%PDF-1.7\n\
///              1 0 obj << /Type /Catalog >> endobj\n\
///              trailer << /Root 1 0 R >>\n\
///              startxref\n0\n%%EOF\n";
/// let mut diags = Diagnostics::default();
/// let (xref, trailer) = read_xref(file, &Limits::default(), &mut diags)?;
/// // No usable startxref, so the file was scanned for object headers.
/// assert!(matches!(xref.entry(1), Some(Entry::Offset(_))));
/// assert!(trailer.raw(pdfrum_object::names::ROOT).is_some());
/// # Ok::<(), pdfrum_parser::Error>(())
/// ```
pub fn read_xref(
    file: &[u8],
    limits: &Limits,
    diags: &mut Diagnostics,
) -> Result<(Xref, Dict), crate::Error> {
    let (xref, trailer, _) = read_xref_full(file, limits, diags)?;
    Ok((xref, trailer.dict))
}

/// Read cross-reference information, reporting the shape it turned out to
/// have — see [`XrefShape`].
pub(crate) fn read_xref_full(
    file: &[u8],
    limits: &Limits,
    diags: &mut Diagnostics,
) -> Result<(Xref, Trailer, XrefShape), crate::Error> {
    chain::load(file, limits, diags)
}

#[cfg(test)]
mod tests {
    use super::{Entry, Trailer, Xref, merge_trailers};
    use pdfrum_common::Limits;
    use pdfrum_object::{Dict, Object, names};

    fn limits() -> Limits {
        Limits::default()
    }

    #[test]
    fn a_newer_generation_is_not_overwritten() {
        let mut x = Xref::new();
        assert!(x.add_normal(4, 3, false, 100, &limits()));
        // An older generation loses — and that is not a failure: the number
        // was fine, the table simply already knew something newer.
        assert!(x.add_normal(4, 1, false, 200, &limits()));
        assert_eq!(x.entry(4), Some(Entry::Offset(100)));
        // The same or a newer one wins.
        assert!(x.add_normal(4, 3, false, 300, &limits()));
        assert_eq!(x.entry(4), Some(Entry::Offset(300)));
    }

    #[test]
    fn the_object_stream_flag_is_sticky() {
        let mut x = Xref::new();
        x.add_normal(7, 0, true, 10, &limits());
        x.add_normal(7, 0, false, 20, &limits());
        assert!(x.is_object_stream(7));
    }

    #[test]
    fn compressed_entries_flag_their_archive() {
        let mut x = Xref::new();
        assert!(x.add_compressed(5, 7, 2, &limits()));
        assert!(x.is_object_stream(7));
        assert_eq!(
            x.entry(5),
            Some(Entry::InObjStream {
                stream: pdfrum_object::ObjRef::new(7, 0),
                index: 2,
            })
        );
    }

    #[test]
    fn a_compressed_entry_loses_to_a_newer_generation() {
        let mut x = Xref::new();
        x.add_normal(5, 2, false, 100, &limits());
        assert!(x.add_compressed(5, 7, 0, &limits()));
        // Accepted as a number, declined as an entry.
        assert_eq!(x.entry(5), Some(Entry::Offset(100)));
    }

    #[test]
    fn object_numbers_past_the_cap_are_refused() {
        let mut x = Xref::new();
        assert!(x.add_normal(limits().max_object_number, 0, false, 1, &limits()));
        assert!(!x.add_normal(limits().max_object_number + 1, 0, false, 1, &limits()));
    }

    #[test]
    fn resizing_truncates_and_materializes_the_last_slot() {
        let mut x = Xref::new();
        x.add_normal(1, 0, false, 10, &limits());
        x.add_normal(9, 0, false, 90, &limits());
        x.set_size(5);
        assert_eq!(x.entry(9), None);
        assert_eq!(x.entry(1), Some(Entry::Offset(10)));
        // A phantom last entry appears, free.
        assert_eq!(x.entry(4), Some(Entry::Free));
        x.set_size(0);
        assert!(x.is_empty());
    }

    #[test]
    fn merging_lets_the_top_win_but_keeps_the_container_flag() {
        let mut current = Xref::new();
        current.add_normal(1, 0, true, 10, &limits());
        current.add_normal(2, 0, false, 20, &limits());
        let mut top = Xref::new();
        top.add_normal(1, 0, false, 111, &limits());
        top.add_normal(3, 0, false, 30, &limits());

        current.merge_up(&top);
        assert_eq!(current.entry(1), Some(Entry::Offset(111)));
        assert!(current.is_object_stream(1));
        assert_eq!(current.entry(2), Some(Entry::Offset(20)));
        assert_eq!(current.entry(3), Some(Entry::Offset(30)));
    }

    #[test]
    fn trailer_merge_keeps_the_older_walk_pointers() {
        let mut older = Trailer {
            dict: Dict::from_pairs([
                (names::PREV.clone(), Object::Int(100)),
                (names::SIZE.clone(), Object::Int(5)),
            ]),
            object_number: 0,
        };
        let newer = Trailer {
            dict: Dict::from_pairs([
                (names::PREV.clone(), Object::Int(999)),
                (names::SIZE.clone(), Object::Int(9)),
                (names::ROOT.clone(), Object::Int(1)),
            ]),
            object_number: 7,
        };
        merge_trailers(&mut older, &newer);
        // The newer /Size and /Root win...
        assert_eq!(older.dict.direct_int(names::SIZE), Some(9));
        assert!(older.dict.raw(names::ROOT).is_some());
        // ...but /Prev stays the older section's, or the walk would loop.
        assert_eq!(older.dict.direct_int(names::PREV), Some(100));
        // The object number stays the accumulating trailer's.
        assert_eq!(older.object_number, 0);
    }

    #[test]
    fn a_pointer_the_older_trailer_lacked_does_not_survive() {
        let mut older = Trailer {
            dict: Dict::from_pairs([(names::SIZE.clone(), Object::Int(5))]),
            object_number: 3,
        };
        let newer = Trailer {
            dict: Dict::from_pairs([(names::PREV.clone(), Object::Int(999))]),
            object_number: 0,
        };
        merge_trailers(&mut older, &newer);
        assert_eq!(older.dict.raw(names::PREV), None);
    }

    #[test]
    fn merging_into_an_empty_trailer_just_takes_it() {
        let mut older = Trailer::default();
        let newer = Trailer {
            dict: Dict::from_pairs([(names::PREV.clone(), Object::Int(42))]),
            object_number: 7,
        };
        merge_trailers(&mut older, &newer);
        assert_eq!(older.dict.direct_int(names::PREV), Some(42));
        assert_eq!(older.object_number, 7);
    }
}
