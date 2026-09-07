//! Copying objects between documents, renumbering as they go.
//!
//! An object cloned out of a source document still holds references naming
//! *the source's* object numbers. The copier walks the clone rewriting each
//! one to the number the destination gave its copy, recursing into whatever
//! it has not seen.
//!
//! # Three rules keep the copy from swallowing the whole document
//!
//! **The mapping is recorded before the recursion.** A subtree that cycles
//! back to an object already being copied hits the map instead of recursing,
//! which is the entire cycle story — there is no separate visited set.
//!
//! **`/Parent`, `/Prev` and `/First` are skipped entirely.** Those are the
//! page tree's and the outline's back-, sibling- and child-pointers. Following
//! them would drag the source's whole page tree in behind one page, and
//! skipping them breaks the cycles they form.
//!
//! **A reference to another `/Type /Page` is refused.** Copying one page must
//! not pull in the pages it links to, so a cross-page destination is pruned
//! rather than followed.
//!
//! # Dictionaries prune, arrays abort
//!
//! When a child cannot be copied, a **dictionary** drops that one key and
//! carries on, while an **array** fails whole. That asymmetry looks like an
//! oversight and is load-bearing: an array's elements are positional, so
//! dropping one silently shifts every element after it, which for a
//! `/MediaBox` or a `/W` array is worse than having no array at all.
//!
//! # Streams travel compressed
//!
//! A stream is copied with its *encoded* bytes and its `/Filter` together —
//! it is never decoded and re-encoded. The crypto layer was already peeled
//! off at parse time, so an encrypted source yields
//! decrypted-but-still-compressed bytes, and an untouched image survives the
//! trip with its checksum intact.

use std::collections::BTreeMap;

use pdfrum_object::{Array, Dict, Name, ObjRef, Object, Resolve, names};

use crate::doc::EditDoc;

/// How deep the copy follows structure inside one object.
const MAX_DEPTH: u32 = 128;

/// The keys whose values are never followed.
///
/// Each is a pointer *back* into a tree the copy is not taking: `/Parent` up
/// the page tree, `/Prev` to an outline's previous sibling, `/First` to its
/// first child.
const SKIPPED_KEYS: [&Name; 3] = [names::PARENT, names::PREV, names::FIRST];

/// Which source object became which destination object.
///
/// **Not cleared between pages.** A font or image `XObject` shared by two
/// imported pages is copied into the destination exactly once and both pages
/// point at the one copy — that deduplication is the whole reason this lives
/// on the importer rather than inside a per-page loop.
#[derive(Debug, Default)]
pub(crate) struct ObjectMap {
    map: BTreeMap<u32, u32>,
}

impl ObjectMap {
    /// An empty map.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Record that source object `from` was copied as destination object
    /// `to`.
    ///
    /// Registering a page's own mapping *before* copying it is what lets a
    /// self-reference from inside the page — an annotation's `/P`
    /// back-pointer — resolve to the new page instead of being pruned by the
    /// cross-page rule.
    pub(crate) fn record(&mut self, from: u32, to: u32) {
        self.map.insert(from, to);
    }

    /// The destination number for a source object, if it has one.
    #[must_use]
    pub(crate) fn get(&self, from: u32) -> Option<u32> {
        self.map.get(&from).copied()
    }
}

/// Copy the object `reference` names out of `src` into `dest`, returning the
/// destination number.
///
/// `pages_node` is the destination's own `/Pages` object number: a source
/// `/Type /Pages` resolves to it rather than being copied, so an imported
/// page's tree links land in the destination's tree. (The C++ hardcodes the
/// number **4** here, which is right only because a freshly created document
/// happens to number its pages node 4.)
///
/// Returns `None` when the object cannot be copied: it dangles, it is another
/// page, or one of its array elements failed.
pub(crate) fn copy_object(
    dest: &mut EditDoc<'_>,
    src: &impl Resolve,
    reference: ObjRef,
    map: &mut ObjectMap,
    pages_node: u32,
) -> Option<u32> {
    if let Some(existing) = map.get(reference.num) {
        return Some(existing);
    }

    let resolved = src.fetch(reference).ok()?;
    if resolved.is_null() {
        return None;
    }
    let mut clone = Object::clone(&resolved);

    // `/Type` decides two special cases, both compared case-insensitively
    // because a file writing `/PAGES` means the same thing.
    if let Some(dict) = clone.as_dict()
        && let Some(kind) = dict.name(names::TYPE)
    {
        if eq_ignore_case(kind.as_bytes(), b"Pages") {
            return Some(pages_node);
        }
        // Another page: refuse, so a link out of this page is pruned rather
        // than dragging that page in behind it.
        if eq_ignore_case(kind.as_bytes(), b"Page") {
            return None;
        }
    }

    // Reserve the number and record it *before* recursing, so a subtree that
    // points back here finds the map instead of looping.
    let new = dest.add(Object::Null);
    map.record(reference.num, new.num);

    if !rewrite(dest, src, &mut clone, map, pages_node, 0) {
        return None;
    }
    dest.replace(new, clone);
    Some(new.num)
}

/// Rewrite every reference inside `value` to name the destination's copies,
/// in place.
///
/// Returns whether the value survived. A dictionary always survives — the
/// keys that failed are simply gone — while an array fails whole if any
/// element does, and a reference fails when its target cannot be copied.
pub(crate) fn rewrite_in_place(
    dest: &mut EditDoc<'_>,
    src: &impl Resolve,
    value: &mut Object,
    map: &mut ObjectMap,
    pages_node: u32,
) -> bool {
    rewrite(dest, src, value, map, pages_node, 0)
}

/// Rewrite every reference inside `obj` to name the destination's copies.
///
/// Returns whether the object survived. A dictionary always survives — the
/// keys that failed are simply gone — while an array fails whole if any
/// element does.
fn rewrite(
    dest: &mut EditDoc<'_>,
    src: &impl Resolve,
    obj: &mut Object,
    map: &mut ObjectMap,
    pages_node: u32,
    depth: u32,
) -> bool {
    if depth > MAX_DEPTH {
        return false;
    }
    match obj {
        Object::Ref(r) => match copy_object(dest, src, *r, map, pages_node) {
            Some(num) => {
                *obj = Object::Ref(ObjRef::new(num, 0));
                true
            }
            None => false,
        },
        Object::Dict(dict) => {
            *dict = rewrite_dict(dest, src, dict, map, pages_node, depth);
            true
        }
        Object::Array(array) => {
            let mut out = Array::new();
            for value in array.iter() {
                let mut value = value.clone();
                // The first failing element aborts the whole array: an
                // array's elements are positional, and dropping one shifts
                // every element after it.
                if !rewrite(
                    dest,
                    src,
                    &mut value,
                    map,
                    pages_node,
                    depth.saturating_add(1),
                ) {
                    return false;
                }
                out.push(value);
            }
            *array = out;
            true
        }
        // A stream's dictionary is rewritten; its bytes travel untouched,
        // filters and all.
        Object::Stream(stream) => {
            stream.dict = rewrite_dict(dest, src, &stream.dict, map, pages_node, depth);
            true
        }
        _ => true,
    }
}

/// Rewrite a dictionary, dropping the keys whose values could not be copied.
fn rewrite_dict(
    dest: &mut EditDoc<'_>,
    src: &impl Resolve,
    dict: &Dict,
    map: &mut ObjectMap,
    pages_node: u32,
    depth: u32,
) -> Dict {
    let mut out = Dict::new();
    for (key, value) in dict.iter() {
        // The tree's back-pointers are never followed.
        if SKIPPED_KEYS.contains(&key) {
            continue;
        }
        let mut value = value.clone();
        if rewrite(
            dest,
            src,
            &mut value,
            map,
            pages_node,
            depth.saturating_add(1),
        ) {
            out.push(key.clone(), value);
        }
    }
    out
}

/// ASCII case-insensitive comparison, which is what `/Type` matching uses:
/// a file spelling `/page` means a page.
fn eq_ignore_case(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.eq_ignore_ascii_case(y))
}

#[cfg(test)]
mod tests {
    use super::{ObjectMap, copy_object};
    use crate::doc::EditDoc;
    use pdfrum_object::{Array, ByteSpan, Dict, Name, ObjRef, Object, Resolve, Stream, names};
    use pdfrum_parser::{Document, LoadOptions, load};
    use std::collections::BTreeMap;
    use std::sync::Arc;

    /// A stand-in source document: a fixed object map.
    struct Src(BTreeMap<u32, Object>);

    impl Resolve for Src {
        fn fetch(&self, r: ObjRef) -> Result<Arc<Object>, pdfrum_object::Error> {
            Ok(Arc::new(
                self.0.get(&r.num).cloned().unwrap_or(Object::Null),
            ))
        }
    }

    /// A destination with one page, so it opens: a catalog whose page tree
    /// is empty is treated as damage and sent to the rebuild.
    fn dest_doc() -> Document {
        let file = b"%PDF-1.7\n\
1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n\
2 0 obj\n<< /Type /Pages /Count 1 /Kids [3 0 R] >>\nendobj\n\
3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>\nendobj\n\
trailer\n<< /Root 1 0 R /Size 4 >>\n";
        load(Arc::from(&file[..]), &LoadOptions::default()).expect("opens")
    }

    fn dict(pairs: impl IntoIterator<Item = (&'static str, Object)>) -> Object {
        Object::Dict(Dict::from_pairs(
            pairs.into_iter().map(|(k, v)| (Name::from(k), v)),
        ))
    }

    fn r(num: u32) -> Object {
        Object::Ref(ObjRef::new(num, 0))
    }

    #[test]
    fn a_simple_object_is_copied_and_renumbered() {
        let src = Src(BTreeMap::from([
            (7, dict([("Leaf", r(8))])),
            (8, Object::Int(42)),
        ]));
        let base = dest_doc();
        let mut dest = EditDoc::new(&base);
        let mut map = ObjectMap::new();

        let num = copy_object(&mut dest, &src, ObjRef::new(7, 0), &mut map, 2).expect("copies");
        let copied = dest.fetch(ObjRef::new(num, 0)).expect("fetch");
        let leaf = copied
            .as_dict()
            .and_then(|d| d.reference(&Name::from("Leaf")));
        // The reference names the *destination's* number, not 8.
        assert!(leaf.is_some_and(|l| l.num != 8));
        assert_eq!(
            *dest.fetch(leaf.expect("leaf")).expect("leaf"),
            Object::Int(42)
        );
    }

    // The dedup guarantee: one source object, one destination copy, however
    // many times it is reached.
    #[test]
    fn an_object_reached_twice_is_copied_once() {
        let src = Src(BTreeMap::from([
            (1, dict([("A", r(3))])),
            (2, dict([("B", r(3))])),
            (3, Object::Int(9)),
        ]));
        let base = dest_doc();
        let mut dest = EditDoc::new(&base);
        let mut map = ObjectMap::new();

        let a = copy_object(&mut dest, &src, ObjRef::new(1, 0), &mut map, 2).expect("a");
        let b = copy_object(&mut dest, &src, ObjRef::new(2, 0), &mut map, 2).expect("b");

        let leaf_of = |num: u32, key: &str| {
            dest.fetch(ObjRef::new(num, 0))
                .ok()
                .and_then(|o| o.as_dict().and_then(|d| d.reference(&Name::from(key))))
        };
        assert_eq!(leaf_of(a, "A"), leaf_of(b, "B"), "one copy, two pointers");
    }

    // The mapping is recorded before the recursion, which is the whole cycle
    // story.
    #[test]
    fn a_cycle_terminates() {
        let src = Src(BTreeMap::from([
            (1, dict([("Next", r(2))])),
            (2, dict([("Back", r(1))])),
        ]));
        let base = dest_doc();
        let mut dest = EditDoc::new(&base);
        let mut map = ObjectMap::new();
        assert!(copy_object(&mut dest, &src, ObjRef::new(1, 0), &mut map, 2).is_some());
        assert!(map.get(1).is_some());
        assert!(map.get(2).is_some());
    }

    // The back-pointer keys are skipped, so a page does not drag its tree in.
    #[test]
    fn parent_prev_and_first_are_never_followed() {
        let src = Src(BTreeMap::from([
            (
                1,
                dict([
                    ("Parent", r(50)),
                    ("Prev", r(51)),
                    ("First", r(52)),
                    ("Keep", r(53)),
                ]),
            ),
            (50, Object::Int(50)),
            (51, Object::Int(51)),
            (52, Object::Int(52)),
            (53, Object::Int(53)),
        ]));
        let base = dest_doc();
        let mut dest = EditDoc::new(&base);
        let mut map = ObjectMap::new();
        let num = copy_object(&mut dest, &src, ObjRef::new(1, 0), &mut map, 2).expect("copies");

        let copied = dest.fetch(ObjRef::new(num, 0)).expect("fetch");
        let d = copied.as_dict().expect("a dict");
        assert!(!d.contains_key(names::PARENT));
        assert!(!d.contains_key(names::PREV));
        assert!(!d.contains_key(names::FIRST));
        assert!(d.contains_key(&Name::from("Keep")));
        // Only the kept child was copied, alongside object 1 itself.
        assert!(map.get(1).is_some());
        assert!(map.get(53).is_some());
        assert!(map.get(50).is_none());
        assert!(map.get(51).is_none());
        assert!(map.get(52).is_none());
    }

    // A `/Type /Pages` resolves to the destination's real pages node,
    // not to a hardcoded object 4.
    #[test]
    fn a_pages_node_resolves_to_the_destinations_own() {
        let src = Src(BTreeMap::from([(
            9,
            dict([("Type", Object::Name(names::PAGES.clone()))]),
        )]));
        let base = dest_doc();
        let mut dest = EditDoc::new(&base);
        let mut map = ObjectMap::new();
        assert_eq!(
            copy_object(&mut dest, &src, ObjRef::new(9, 0), &mut map, 2),
            Some(2)
        );
    }

    // `/Type` matching is case-insensitive: real damage tolerance, kept.
    #[test]
    fn type_matching_ignores_case() {
        for spelling in ["PAGES", "pages", "PaGeS"] {
            let src = Src(BTreeMap::from([(
                9,
                dict([("Type", Object::Name(Name::from(spelling)))]),
            )]));
            let base = dest_doc();
            let mut dest = EditDoc::new(&base);
            let mut map = ObjectMap::new();
            assert_eq!(
                copy_object(&mut dest, &src, ObjRef::new(9, 0), &mut map, 2),
                Some(2),
                "{spelling}"
            );
        }
    }

    #[test]
    fn a_reference_to_another_page_is_refused() {
        let src = Src(BTreeMap::from([(
            9,
            dict([("Type", Object::Name(names::PAGE.clone()))]),
        )]));
        let base = dest_doc();
        let mut dest = EditDoc::new(&base);
        let mut map = ObjectMap::new();
        assert_eq!(
            copy_object(&mut dest, &src, ObjRef::new(9, 0), &mut map, 2),
            None
        );
    }

    // The asymmetry: a dict drops the bad key, an array fails whole.
    #[test]
    fn a_dangling_reference_drops_its_dict_key() {
        let src = Src(BTreeMap::from([(
            1,
            dict([("Good", Object::Int(1)), ("Bad", r(99))]),
        )]));
        let base = dest_doc();
        let mut dest = EditDoc::new(&base);
        let mut map = ObjectMap::new();
        let num = copy_object(&mut dest, &src, ObjRef::new(1, 0), &mut map, 2).expect("copies");
        let copied = dest.fetch(ObjRef::new(num, 0)).expect("fetch");
        let d = copied.as_dict().expect("a dict");
        assert!(d.contains_key(&Name::from("Good")));
        assert!(!d.contains_key(&Name::from("Bad")), "the bad key is gone");
    }

    #[test]
    fn a_dangling_reference_kills_its_whole_array() {
        let src = Src(BTreeMap::from([(
            1,
            dict([(
                "List",
                Object::Array(Array::of([Object::Int(1), r(99), Object::Int(3)])),
            )]),
        )]));
        let base = dest_doc();
        let mut dest = EditDoc::new(&base);
        let mut map = ObjectMap::new();
        let num = copy_object(&mut dest, &src, ObjRef::new(1, 0), &mut map, 2).expect("copies");
        let copied = dest.fetch(ObjRef::new(num, 0)).expect("fetch");
        // The key is gone entirely — not a two-element array.
        assert!(
            !copied
                .as_dict()
                .expect("a dict")
                .contains_key(&Name::from("List"))
        );
    }

    // Streams travel with their encoded bytes and their filter.
    #[test]
    fn a_stream_keeps_its_bytes_and_its_filter() {
        let payload: Arc<[u8]> = Arc::from(&b"\x01\x02\x03compressed"[..]);
        let stream = Object::Stream(Box::new(Stream::new(
            Dict::from_pairs([
                (
                    names::FILTER.clone(),
                    Object::Name(names::FLATE_DECODE.clone()),
                ),
                (names::LENGTH.clone(), Object::Int(14)),
            ]),
            ByteSpan::whole(payload),
        )));
        let src = Src(BTreeMap::from([(1, dict([("S", r(2))])), (2, stream)]));
        let base = dest_doc();
        let mut dest = EditDoc::new(&base);
        let mut map = ObjectMap::new();
        let num = copy_object(&mut dest, &src, ObjRef::new(1, 0), &mut map, 2).expect("copies");

        let copied = dest.fetch(ObjRef::new(num, 0)).expect("fetch");
        let s_ref = copied
            .as_dict()
            .and_then(|d| d.reference(&Name::from("S")))
            .expect("the stream reference");
        let s = dest.fetch(s_ref).expect("the stream");
        let s = s.as_stream().expect("a stream");
        assert_eq!(s.data.as_bytes(), b"\x01\x02\x03compressed");
        assert_eq!(s.dict.name(names::FILTER), Some(names::FLATE_DECODE));
    }

    // A self-reference registered first resolves to the new object rather
    // than being pruned by the cross-page rule.
    #[test]
    fn a_pre_registered_self_reference_survives() {
        let src = Src(BTreeMap::from([(
            5,
            dict([("Type", Object::Name(names::PAGE.clone())), ("Me", r(5))]),
        )]));
        let base = dest_doc();
        let mut dest = EditDoc::new(&base);
        let mut map = ObjectMap::new();
        // Register the page's own mapping before copying anything, exactly
        // as the page exporter does.
        let placeholder = dest.add(Object::Null);
        map.record(5, placeholder.num);
        assert_eq!(
            copy_object(&mut dest, &src, ObjRef::new(5, 0), &mut map, 2),
            Some(placeholder.num),
            "the map wins over the /Type /Page refusal"
        );
    }
}
