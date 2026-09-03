//! Which objects anything still points at.
//!
//! A full save garbage-collects: only objects reachable from the trailer
//! survive, which is what makes deleting every object on a page actually
//! shrink the file rather than leave orphaned images behind it.
//!
//! # Counting, not just reaching
//!
//! The walk records how many *distinct* references point at each object,
//! because two questions need different answers. "Is it reachable?" decides
//! what the writer emits. "Is it referenced more than once?" decides whether
//! the content generator may edit a resource in place or must copy it first —
//! a shared `/Resources` dictionary edited in place would change a page
//! nobody asked to change.
//!
//! Two references are deliberately not counted:
//!
//! - a **self-reference** — an object naming itself does not make itself
//!   shared;
//! - a reference whose **source has already been seen referencing something**
//!   *and* whose target has too. That is the circular-reference filter, and it
//!   is why `circular_viewer_ref.pdf` reaches exactly `{1}` instead of
//!   spiralling.
//!
//! # Object numbers, not pointers
//!
//! The visited set is keyed on object number — `ObjRef` for indirect objects,
//! plus a depth cap for inline structure — because our objects are values
//! with no identity of their own. The one case that can double-count is a
//! *shared inline sub-object* reached through two parents; the cycle filter
//! above already suppresses the second count whenever both endpoints have
//! been seen. `hello_world_2_pages.pdf` reaching `{5,6,7}` is the case that
//! pins it.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use pdfrum_object::{Dict, Object, Resolve, names};

/// How deep the walk follows structure inside one indirect object.
///
/// Parsing already bounded nesting, so this is a belt-and-braces cap on a
/// tree the overlay may have built rather than parsed.
const MAX_INLINE_DEPTH: u32 = 128;

/// How many objects were pointed at, and by how many distinct sources.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ReachCounts {
    counts: BTreeMap<u32, u32>,
}

impl ReachCounts {
    /// Every object number something points at.
    #[must_use]
    #[cfg(test)]
    pub(crate) fn reachable(&self) -> BTreeSet<u32> {
        self.counts.keys().copied().collect()
    }

    /// Whether anything points at `num`.
    #[must_use]
    pub(crate) fn is_reachable(&self, num: u32) -> bool {
        self.counts.contains_key(&num)
    }

    /// The object numbers more than one distinct source points at.
    #[must_use]
    #[cfg(test)]
    pub(crate) fn multiply_referenced(&self) -> BTreeSet<u32> {
        self.counts
            .iter()
            .filter(|(_, count)| **count > 1)
            .map(|(num, _)| *num)
            .collect()
    }
}

/// Walk the document from its trailer, counting references.
///
/// `trailer_number` is the object number the trailer itself has — non-zero
/// for an xref-stream dictionary, zero for a bare `trailer` keyword. A
/// non-zero one is **seeded into the result** with a count of one, which is
/// why an xref-stream document's trailer object appears in the reachable set
/// even though nothing in the document body points at it.
pub(crate) fn walk(trailer: &Dict, trailer_number: u32, r: &impl Resolve) -> ReachCounts {
    let mut counts: BTreeMap<u32, u32> = BTreeMap::new();
    if trailer_number != 0 {
        counts.insert(trailer_number, 1);
    }

    // Sources already known to reference something. Both endpoints being in
    // here is what identifies a reference as part of a cycle already walked.
    // The two exclusions are `object_tree_traversal_util.cpp:118-137`.
    let mut seen_sources: BTreeSet<u32> = BTreeSet::new();
    // Indirect objects already queued, so a cycle terminates. The C++ keys
    // this set on `CPDF_Object*`, so pointer identity distinguishes two
    // structurally equal inline dictionaries; our objects are values, so the
    // key is the object number. The two walks differ only on a shared inline
    // sub-object reached through two parents — the pointer set walks it once,
    // this walks it twice — and `count_reference`'s cycle filter suppresses
    // the second count.
    let mut visited: BTreeSet<u32> = BTreeSet::new();

    // Each item is (the object to walk, the number of the indirect object it
    // came from). That second half is the C++'s `object_number_map_`: an
    // inline sub-object's references are attributed to their top-level
    // container, so a dictionary nested three deep inside object 7 still
    // counts as object 7 referencing what it names.
    let mut queue: VecDeque<(Object, u32, u32)> = VecDeque::new();
    for (_, value) in trailer.iter() {
        queue.push_back((value.clone(), trailer_number, 0));
    }

    while let Some((obj, owner, depth)) = queue.pop_front() {
        if depth > MAX_INLINE_DEPTH {
            continue;
        }
        match obj {
            Object::Ref(target) => {
                let Ok(resolved) = r.fetch(target) else {
                    continue;
                };
                // A reference resolving to nothing points at nothing.
                if resolved.is_null() {
                    continue;
                }
                count_reference(&mut counts, &mut seen_sources, owner, target.num);
                if visited.insert(target.num) {
                    queue.push_back((
                        Object::clone(&resolved),
                        target.num,
                        depth.saturating_add(1),
                    ));
                }
            }
            Object::Array(a) => {
                for value in a.iter() {
                    queue.push_back((value.clone(), owner, depth.saturating_add(1)));
                }
            }
            Object::Dict(d) => {
                for (_, value) in d.iter() {
                    queue.push_back((value.clone(), owner, depth.saturating_add(1)));
                }
            }
            // A stream's dictionary is walked; its bytes are not a graph.
            Object::Stream(s) => {
                for (_, value) in s.dict.iter() {
                    queue.push_back((value.clone(), owner, depth.saturating_add(1)));
                }
            }
            _ => {}
        }
    }

    ReachCounts { counts }
}

/// Record one reference, applying the two filters.
fn count_reference(
    counts: &mut BTreeMap<u32, u32>,
    seen_sources: &mut BTreeSet<u32>,
    source: u32,
    target: u32,
) {
    // An object naming itself does not make itself shared.
    if source == target {
        return;
    }
    // Both endpoints already seen referencing: this closes a cycle the walk
    // has been round once, so counting it again would inflate the count.
    if seen_sources.contains(&source) && seen_sources.contains(&target) {
        return;
    }
    *counts.entry(target).or_insert(0) += 1;
    seen_sources.insert(source);
}

/// The trailer keys the writer never copies forward, because it recomputes
/// each of them (§1.12).
///
/// Eleven keys, and the list is exact rather than a category: `/Type` is here
/// because an xref-stream document's trailer *is* the stream dictionary and
/// says `/Type /XRef`, which would be a lie on a classic trailer. Everything
/// else — including keys no specification defines — is copied through
/// verbatim, which is how a malformed `/Foo` survives a save.
pub(crate) const SUPPRESSED_TRAILER_KEYS: [&pdfrum_object::Name; 11] = [
    names::ENCRYPT,
    names::SIZE,
    names::FILTER,
    names::INDEX,
    names::LENGTH,
    names::PREV,
    names::W,
    names::XREF_STM,
    names::ID,
    names::DECODE_PARMS,
    names::TYPE,
];

#[cfg(test)]
mod tests {
    use super::{SUPPRESSED_TRAILER_KEYS, walk};
    use pdfrum_object::{Array, Dict, Name, ObjRef, Object, Resolve, names};
    use std::collections::BTreeMap;
    use std::sync::Arc;

    /// A store over a fixed object map.
    struct Store(BTreeMap<u32, Object>);

    impl Resolve for Store {
        fn fetch(&self, r: ObjRef) -> Result<Arc<Object>, pdfrum_object::Error> {
            Ok(Arc::new(
                self.0.get(&r.num).cloned().unwrap_or(Object::Null),
            ))
        }
    }

    fn r(num: u32) -> Object {
        Object::Ref(ObjRef::new(num, 0))
    }

    fn dict(pairs: impl IntoIterator<Item = (&'static str, Object)>) -> Object {
        Object::Dict(Dict::from_pairs(
            pairs.into_iter().map(|(k, v)| (Name::from(k), v)),
        ))
    }

    /// `hello_world.pdf`'s shape: catalog, pages, page, content, font,
    /// font-descriptor — six objects, each reached once.
    fn hello_world() -> (Dict, Store) {
        let store = Store(BTreeMap::from([
            (
                1,
                dict([
                    ("Type", Object::Name(Name::from("Catalog"))),
                    ("Pages", r(2)),
                ]),
            ),
            (
                2,
                dict([
                    ("Type", Object::Name(Name::from("Pages"))),
                    ("Kids", Object::Array(Array::of([r(3)]))),
                ]),
            ),
            (
                3,
                dict([
                    ("Type", Object::Name(Name::from("Page"))),
                    ("Contents", r(4)),
                    ("Resources", dict([("Font", dict([("F1", r(5))]))])),
                ]),
            ),
            (4, Object::Int(0)),
            (
                5,
                dict([
                    ("Type", Object::Name(Name::from("Font"))),
                    ("FontDescriptor", r(6)),
                ]),
            ),
            (
                6,
                dict([("Type", Object::Name(Name::from("FontDescriptor")))]),
            ),
        ]));
        let trailer = Dict::from_pairs([(names::ROOT.clone(), r(1))]);
        (trailer, store)
    }

    // object_tree_traversal_util_embeddertest.cpp:30-112, restated over
    // object numbers.
    #[test]
    fn hello_world_reaches_all_six() {
        let (trailer, store) = hello_world();
        let counts = walk(&trailer, 0, &store);
        assert_eq!(counts.reachable(), [1, 2, 3, 4, 5, 6].into());
        assert!(counts.multiply_referenced().is_empty());
    }

    #[test]
    fn a_new_empty_document_reaches_its_catalog_and_pages() {
        let store = Store(BTreeMap::from([
            (
                1,
                dict([
                    ("Type", Object::Name(Name::from("Catalog"))),
                    ("Pages", r(2)),
                ]),
            ),
            (
                2,
                dict([
                    ("Type", Object::Name(Name::from("Pages"))),
                    ("Kids", Object::Array(Array::new())),
                ]),
            ),
        ]));
        let trailer = Dict::from_pairs([(names::ROOT.clone(), r(1))]);
        assert_eq!(walk(&trailer, 0, &store).reachable(), [1, 2].into());
    }

    // circular_viewer_ref.pdf: the catalog's viewer preferences point back at
    // the catalog. The self-reference filter keeps the set at {1}.
    #[test]
    fn a_circular_viewer_reference_reaches_only_the_catalog() {
        let store = Store(BTreeMap::from([(
            1,
            dict([
                ("Type", Object::Name(Name::from("Catalog"))),
                ("ViewerPreferences", r(1)),
            ]),
        )]));
        let trailer = Dict::from_pairs([(names::ROOT.clone(), r(1))]);
        let counts = walk(&trailer, 0, &store);
        assert_eq!(counts.reachable(), [1].into());
        assert!(counts.multiply_referenced().is_empty());
    }

    // An unreferenced object is simply absent, which is what makes a full
    // save drop it.
    #[test]
    fn an_unreferenced_object_is_not_reached() {
        let (trailer, mut store) = hello_world();
        store.0.insert(99, Object::Int(1));
        assert!(!walk(&trailer, 0, &store).is_reachable(99));
    }

    // bug_1399.pdf: the trailer *is* an object, so its number is seeded.
    #[test]
    fn an_xref_stream_trailer_seeds_its_own_number() {
        let (trailer, store) = hello_world();
        let counts = walk(&trailer, 16, &store);
        assert!(counts.is_reachable(16), "the trailer object survives");
        assert_eq!(counts.reachable(), [1, 2, 3, 4, 5, 6, 16].into());
    }

    // Two pages naming the same content stream: the second reference counts,
    // because page 4 has not yet been recorded as a referencing source when
    // it names object 5.
    #[test]
    fn two_pages_naming_one_stream_report_it_as_shared() {
        let page = |contents: u32| {
            dict([
                ("Type", Object::Name(Name::from("Page"))),
                ("Contents", r(contents)),
            ])
        };
        let store = Store(BTreeMap::from([
            (1, dict([("Pages", r(2))])),
            (2, dict([("Kids", Object::Array(Array::of([r(3), r(4)])))])),
            (3, page(5)),
            (4, page(5)),
            (5, Object::Int(0)),
        ]));
        let trailer = Dict::from_pairs([(names::ROOT.clone(), r(1))]);
        let counts = walk(&trailer, 0, &store);
        let shared = counts.multiply_referenced();
        assert!(shared.contains(&5), "both pages name the same content");
        assert!(!shared.contains(&3));
        assert!(!shared.contains(&4));
    }

    // hello_world_2_pages.pdf → {5,6,7}: two pages sharing a font that in
    // turn owns a descriptor. The whole chain reads as shared, which is D8's
    // pin — the objnum-keyed walk agrees with the C++'s pointer-keyed one on
    // exactly the sets that matter.
    //
    // Breadth-first order is load-bearing here. Both pages are dequeued
    // before either page's font is descended into, so page 4's reference to
    // the font is counted *before* the font has itself referenced anything —
    // and the circular filter, which needs both endpoints already recorded
    // as sources, does not fire. A depth-first walk would see the same graph
    // and report the font as unshared.
    #[test]
    fn two_pages_sharing_a_font_report_the_whole_chain_as_shared() {
        let page = |contents: u32| {
            dict([
                ("Type", Object::Name(Name::from("Page"))),
                ("Contents", r(contents)),
                ("Resources", dict([("Font", dict([("F1", r(6))]))])),
            ])
        };
        let store = Store(BTreeMap::from([
            (1, dict([("Pages", r(2))])),
            (2, dict([("Kids", Object::Array(Array::of([r(3), r(4)])))])),
            (3, page(5)),
            (4, page(5)),
            (5, Object::Int(0)),
            (6, dict([("FontDescriptor", r(7))])),
            (
                7,
                dict([("Type", Object::Name(Name::from("FontDescriptor")))]),
            ),
        ]));
        let trailer = Dict::from_pairs([
            (names::ROOT.clone(), r(1)),
            // A second path to the descriptor, as the fixture has.
            (Name::from("Extra"), r(7)),
        ]);
        let counts = walk(&trailer, 0, &store);
        assert_eq!(counts.reachable(), [1, 2, 3, 4, 5, 6, 7].into());
        assert_eq!(counts.multiply_referenced(), [5, 6, 7].into());
    }

    // A dangling reference points at nothing, so it neither counts nor
    // resolves — the same reading a damaged file already gets.
    #[test]
    fn a_dangling_reference_reaches_nothing() {
        let store = Store(BTreeMap::from([(1, dict([("Missing", r(42))]))]));
        let trailer = Dict::from_pairs([(names::ROOT.clone(), r(1))]);
        let counts = walk(&trailer, 0, &store);
        assert_eq!(counts.reachable(), [1].into());
    }

    // A stream's dictionary is part of the graph; its bytes are not.
    #[test]
    fn a_stream_dictionary_is_walked() {
        let stream = Object::Stream(pdfrum_object::Stream::new(
            Dict::from_pairs([(Name::from("Ref"), r(9))]),
            pdfrum_object::ByteSpan::empty(),
        ));
        let store = Store(BTreeMap::from([
            (1, dict([("S", r(2))])),
            (2, stream),
            (9, Object::Int(1)),
        ]));
        let trailer = Dict::from_pairs([(names::ROOT.clone(), r(1))]);
        assert_eq!(walk(&trailer, 0, &store).reachable(), [1, 2, 9].into());
    }

    // A cycle terminates: the visited set stops the second descent.
    #[test]
    fn a_cycle_terminates() {
        let store = Store(BTreeMap::from([
            (1, dict([("Next", r(2))])),
            (2, dict([("Next", r(3))])),
            (3, dict([("Back", r(1))])),
        ]));
        let trailer = Dict::from_pairs([(names::ROOT.clone(), r(1))]);
        assert_eq!(walk(&trailer, 0, &store).reachable(), [1, 2, 3].into());
    }

    #[test]
    fn the_suppression_list_is_exactly_eleven_keys() {
        let spelled: Vec<&str> = SUPPRESSED_TRAILER_KEYS
            .iter()
            .filter_map(|n| n.as_str())
            .collect();
        assert_eq!(
            spelled,
            vec![
                "Encrypt",
                "Size",
                "Filter",
                "Index",
                "Length",
                "Prev",
                "W",
                "XRefStm",
                "ID",
                "DecodeParms",
                "Type",
            ]
        );
    }
}
