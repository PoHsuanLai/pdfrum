//! Turning a regenerated page into objects on the way out.
//!
//! [`super::regen::regenerate`] produces bytes and a resource dictionary;
//! this puts them into an [`EditDoc`] — replacing the streams that survive,
//! adding the ones that are new, reshaping `/Contents`, and repointing
//! `/Resources`.
//!
//! # An element is replaced in place unless something else points at it
//!
//! A content stream shared between two pages cannot be edited in place:
//! rewriting it for one page would silently rewrite the other. So a shared
//! element is copied into a fresh object and the *reference* is moved,
//! leaving the original where the other page still finds it. Same for a
//! shared `/Contents` array and a shared `/Resources` dictionary.
//!
//! # A regenerated stream carries no filter
//!
//! The bytes written are the operators, uncompressed, and any `/Filter` the
//! original element had is dropped along with the `/DecodeParms` that went
//! with it. Keeping the key while replacing the data with plaintext would
//! describe the stream as compressed when it is not, which is a file nothing
//! can read.

use std::collections::{BTreeMap, BTreeSet};

use pdfrum_object::{ByteSpan, Dict, Name, ObjRef, Object, Resolve, Stream};

use crate::content::regen::{ContentsShape, PageRewrite};
use crate::doc::EditDoc;

/// How many objects still reference each object number.
///
/// Two page dictionaries naming one content stream is what makes that stream
/// shared, and a shared stream is copied rather than edited.
pub type ShareCounts = BTreeSet<u32>;

/// Apply a page's regenerated content to `edit`.
///
/// `page_ref` names the page dictionary; `page_dict` is its current contents;
/// `shared` names the object numbers more than one object points at.
///
/// Returns the map from each object's old content-stream index to its new one,
/// so the caller can renumber the page-object graph it still holds. An index
/// missing from the map belongs to an element that was removed, and collapses
/// to zero.
pub fn apply_rewrite(
    edit: &mut EditDoc<'_>,
    page_ref: ObjRef,
    page_dict: &Dict,
    rewrite: &PageRewrite,
    shared: &ShareCounts,
) -> BTreeMap<usize, usize> {
    let mut shape = ContentsShape::read(page_dict, edit);
    let mut removed: BTreeSet<usize> = BTreeSet::new();
    let mut contents_changed = false;

    for regenerated in &rewrite.streams {
        let index = regenerated.stream;
        if regenerated.bytes.is_empty() {
            // An empty buffer is the deletion signal, not an empty stream.
            // A streamless element has nothing to delete.
            if let Some(index) = index {
                removed.insert(index);
                contents_changed = true;
            }
            continue;
        }
        let stream = content_stream(regenerated.bytes.as_bytes());
        let existing = index.and_then(|i| shape.elements().get(i).copied());
        match existing {
            // The element exists: rewrite it, or copy it if it is shared.
            Some(reference) if !shared.contains(&reference.num) => {
                edit.replace(reference, Object::Stream(stream));
            }
            Some(_) => {
                let fresh = edit.add(Object::Stream(stream));
                if let Some(index) = index {
                    shape = replace_element(&shape, index, fresh);
                    contents_changed = true;
                }
            }
            // A brand-new element, or one past the end of the array.
            None => {
                let fresh = edit.add(Object::Stream(stream));
                let (_, next) = shape.with_added(fresh);
                shape = next;
                contents_changed = true;
            }
        }
    }

    let (shape, mapping) = if removed.is_empty() {
        (shape, BTreeMap::new())
    } else {
        shape.with_removed(&removed)
    };

    let mut dict = page_dict.clone();
    if contents_changed {
        dict = set_contents(edit, &dict, &shape, page_dict, shared);
    }
    dict = set_resources(edit, &dict, &rewrite.resources, shared);
    edit.replace(page_ref, Object::Dict(dict));
    mapping
}

/// A content stream holding `bytes`, with no filter.
fn content_stream(bytes: &[u8]) -> Stream {
    let dict = Dict::from_pairs([(
        pdfrum_object::names::LENGTH.clone(),
        Object::Int(i64::try_from(bytes.len()).unwrap_or(0)),
    )]);
    Stream::new(dict, ByteSpan::from(bytes.to_vec()))
}

/// The shape with element `index` repointed at `fresh`.
fn replace_element(shape: &ContentsShape, index: usize, fresh: ObjRef) -> ContentsShape {
    match shape {
        ContentsShape::Single(_) if index == 0 => ContentsShape::Single(fresh),
        ContentsShape::Array(elements) => {
            let mut next = elements.clone();
            if let Some(slot) = next.get_mut(index) {
                *slot = fresh;
            }
            ContentsShape::Array(next)
        }
        other => other.clone(),
    }
}

/// Write the new `/Contents` into a copy of the page dictionary.
///
/// An array reached through a reference is written back through that same
/// reference — unless it is shared, in which case a fresh array is added and
/// the page points at that instead.
fn set_contents(
    edit: &mut EditDoc<'_>,
    dict: &Dict,
    shape: &ContentsShape,
    original: &Dict,
    shared: &ShareCounts,
) -> Dict {
    let key = pdfrum_object::names::CONTENTS;
    let value = match shape {
        ContentsShape::Absent => None,
        ContentsShape::Single(reference) => Some(Object::Ref(*reference)),
        ContentsShape::Array(elements) => {
            let array = shape
                .to_object(None)
                .unwrap_or(Object::Array(pdfrum_object::Array::new()));
            // The array object the page already points at is reused, when the
            // page had one to itself — and when it is not one of the elements.
            // A page whose `/Contents` was a lone *stream* points at that
            // stream, and writing the new array over it would destroy the very
            // element the array's first entry names.
            let reusable = match original.raw(key) {
                Some(Object::Ref(reference)) => {
                    !shared.contains(&reference.num) && !elements.contains(reference)
                }
                _ => false,
            };
            match original.raw(key) {
                Some(Object::Ref(reference)) if reusable => {
                    edit.replace(*reference, array);
                    Some(Object::Ref(*reference))
                }
                _ => Some(Object::Ref(edit.add(array))),
            }
        }
    };
    with_key(dict, key, value)
}

/// Write the swept `/Resources` into a copy of the page dictionary.
fn set_resources(
    edit: &mut EditDoc<'_>,
    dict: &Dict,
    resources: &Dict,
    shared: &ShareCounts,
) -> Dict {
    let key = pdfrum_object::names::RESOURCES;
    let value = match dict.raw(key) {
        // The page reaches its resources through a reference it does not
        // share: rewrite that object and leave the page's key alone.
        Some(Object::Ref(reference)) if !shared.contains(&reference.num) => {
            edit.replace(*reference, Object::Dict(resources.clone()));
            return dict.clone();
        }
        // Shared, or written inline, or absent: the page gets its own copy.
        _ => Some(Object::Dict(resources.clone())),
    };
    with_key(dict, key, value)
}

/// A copy of `dict` with `key` set to `value`, or removed when `value` is
/// `None`, keeping every other entry in its place.
fn with_key(dict: &Dict, key: &Name, value: Option<Object>) -> Dict {
    let mut out = Dict::new();
    let mut written = false;
    for (existing, held) in dict.iter() {
        if existing == key {
            if let Some(value) = value.clone()
                && !written
            {
                out.push(existing.clone(), value);
                written = true;
            }
        } else {
            out.push(existing.clone(), held.clone());
        }
    }
    if !written && let Some(value) = value {
        out.push(key.clone(), value);
    }
    out
}

/// The object numbers more than one object in `doc` points at.
///
/// A shared object cannot be edited in place, so this is what decides between
/// rewriting an element and copying it. The sweep walks the reachable graph
/// once, counting references; anything reached twice is shared.
#[must_use]
pub fn shared_objects(doc: &EditDoc<'_>) -> ShareCounts {
    let mut seen: BTreeMap<u32, u32> = BTreeMap::new();
    let mut queue: Vec<Object> = Vec::new();
    let mut visited: BTreeSet<u32> = BTreeSet::new();

    if let Ok(root) = doc.base().catalog() {
        queue.push(Object::Dict(root));
    }
    for (_, object) in doc.edited() {
        queue.push((**object).clone());
    }

    while let Some(object) = queue.pop() {
        match object {
            Object::Ref(reference) => {
                *seen.entry(reference.num).or_default() += 1;
                if visited.insert(reference.num)
                    && let Ok(target) = doc.fetch(reference)
                {
                    queue.push((*target).clone());
                }
            }
            Object::Dict(dict) => {
                for (_, value) in dict.iter() {
                    queue.push(value.clone());
                }
            }
            Object::Array(array) => {
                for value in array.iter() {
                    queue.push(value.clone());
                }
            }
            Object::Stream(stream) => {
                for (_, value) in stream.dict.iter() {
                    queue.push(value.clone());
                }
            }
            _ => {}
        }
    }

    seen.into_iter()
        .filter(|(_, count)| *count > 1)
        .map(|(num, _)| num)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{apply_rewrite, shared_objects};
    use crate::content::regen::{PageRewrite, Regenerated};
    use crate::doc::EditDoc;
    use pdfrum_object::{Dict, Name, ObjRef, Object, Resolve};
    use pdfrum_parser::{Document, LoadOptions, load};
    use std::collections::BTreeSet;
    use std::sync::Arc;

    /// A one-page document whose `/Contents` is a lone stream (object 4).
    fn single_stream() -> Document {
        let file = b"%PDF-1.7\n\
1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n\
2 0 obj\n<< /Type /Pages /Count 1 /Kids [3 0 R] >>\nendobj\n\
3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R >>\nendobj\n\
4 0 obj\n<< /Length 8 >>\nstream\n0 0 1 1 re f\nendstream\nendobj\n\
trailer\n<< /Root 1 0 R /Size 5 >>\n";
        load(Arc::from(&file[..]), &LoadOptions::default()).expect("opens")
    }

    /// A one-page document whose `/Contents` is an array of two streams.
    fn two_streams() -> Document {
        let file = b"%PDF-1.7\n\
1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n\
2 0 obj\n<< /Type /Pages /Count 1 /Kids [3 0 R] >>\nendobj\n\
3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents [4 0 R 5 0 R] >>\nendobj\n\
4 0 obj\n<< /Length 4 >>\nstream\n1 w\nendstream\nendobj\n\
5 0 obj\n<< /Length 4 >>\nstream\n2 w\nendstream\nendobj\n\
trailer\n<< /Root 1 0 R /Size 6 >>\n";
        load(Arc::from(&file[..]), &LoadOptions::default()).expect("opens")
    }

    fn rewrite(streams: &[(Option<usize>, &str)]) -> PageRewrite {
        PageRewrite {
            streams: streams
                .iter()
                .map(|(stream, bytes)| Regenerated {
                    stream: *stream,
                    bytes: (*bytes).to_owned(),
                })
                .collect(),
            resources: Dict::new(),
        }
    }

    fn contents_of(edit: &EditDoc<'_>, page: ObjRef) -> Object {
        let dict = edit.fetch(page).expect("page");
        dict.as_dict()
            .and_then(|d| d.raw(pdfrum_object::names::CONTENTS).cloned())
            .unwrap_or(Object::Null)
    }

    // A rewritten element that nothing else points at is replaced in place, so
    // `/Contents` still names the same object.
    #[test]
    fn a_private_stream_is_rewritten_in_place() {
        let base = single_stream();
        let mut edit = EditDoc::new(&base);
        let page = ObjRef::new(3, 0);
        let dict = base.page(0).expect("page").dict;
        apply_rewrite(
            &mut edit,
            page,
            &dict,
            &rewrite(&[(Some(0), "q\nQ\n")]),
            &BTreeSet::new(),
        );
        assert_eq!(contents_of(&edit, page), Object::Ref(ObjRef::new(4, 0)));
        let stream = edit.fetch(ObjRef::new(4, 0)).expect("stream");
        assert_eq!(
            stream.as_stream().map(|s| s.data.as_ref().to_vec()),
            Some(b"q\nQ\n".to_vec())
        );
    }

    // A regenerated stream carries no filter: keeping one would describe
    // plaintext as compressed.
    #[test]
    fn a_regenerated_stream_has_no_filter() {
        let base = single_stream();
        let mut edit = EditDoc::new(&base);
        let dict = base.page(0).expect("page").dict;
        apply_rewrite(
            &mut edit,
            ObjRef::new(3, 0),
            &dict,
            &rewrite(&[(Some(0), "q\nQ\n")]),
            &BTreeSet::new(),
        );
        let stream = edit.fetch(ObjRef::new(4, 0)).expect("stream");
        let stream = stream.as_stream().expect("a stream");
        assert!(!stream.dict.contains_key(pdfrum_object::names::FILTER));
        assert_eq!(
            stream.dict.direct_int(pdfrum_object::names::LENGTH),
            Some(4)
        );
    }

    // A shared stream is copied rather than edited, so the page that shares it
    // still sees the original bytes.
    #[test]
    fn a_shared_stream_is_copied_and_the_reference_moved() {
        let base = single_stream();
        let mut edit = EditDoc::new(&base);
        let page = ObjRef::new(3, 0);
        let dict = base.page(0).expect("page").dict;
        let shared: BTreeSet<u32> = [4].into_iter().collect();
        apply_rewrite(
            &mut edit,
            page,
            &dict,
            &rewrite(&[(Some(0), "q\nQ\n")]),
            &shared,
        );

        // The page now names a different object.
        let Object::Ref(now) = contents_of(&edit, page) else {
            panic!("not a reference");
        };
        assert_ne!(now.num, 4);
        // And the original still holds what it always did.
        let original = edit.fetch(ObjRef::new(4, 0)).expect("original");
        assert_eq!(
            original.as_stream().map(|s| s.data.as_ref().to_vec()),
            Some(b"0 0 1 1 re f".to_vec())
        );
    }

    // `AddStream` (cpdf_pagecontentmanager.cpp:104-117): a lone stream gaining
    // a second becomes an array of two.
    #[test]
    fn a_lone_stream_gaining_one_becomes_an_array_of_two() {
        let base = single_stream();
        let mut edit = EditDoc::new(&base);
        let page = ObjRef::new(3, 0);
        let dict = base.page(0).expect("page").dict;
        apply_rewrite(
            &mut edit,
            page,
            &dict,
            &rewrite(&[(None, "q\nQ\n")]),
            &BTreeSet::new(),
        );
        let Object::Ref(array_ref) = contents_of(&edit, page) else {
            panic!("expected a reference to an array");
        };
        let array = edit.fetch(array_ref).expect("array");
        let array = array.as_array().expect("an array");
        assert_eq!(array.len(), 2);
        assert_eq!(array.reference_at(0), Some(ObjRef::new(4, 0)));
    }

    // `ExecuteScheduledRemovals` (:194-202): a lone stream emptied loses the
    // `/Contents` key entirely.
    #[test]
    fn emptying_a_lone_stream_removes_the_contents_key() {
        let base = single_stream();
        let mut edit = EditDoc::new(&base);
        let page = ObjRef::new(3, 0);
        let dict = base.page(0).expect("page").dict;
        apply_rewrite(
            &mut edit,
            page,
            &dict,
            &rewrite(&[(Some(0), "")]),
            &BTreeSet::new(),
        );
        assert_eq!(contents_of(&edit, page), Object::Null);
    }

    // `ExecuteScheduledRemovals` (:204-238): an emptied array element is
    // dropped, the survivors renumber, and it stays an array.
    #[test]
    fn emptying_one_array_element_renumbers_the_survivors() {
        let base = two_streams();
        let mut edit = EditDoc::new(&base);
        let page = ObjRef::new(3, 0);
        let dict = base.page(0).expect("page").dict;
        let mapping = apply_rewrite(
            &mut edit,
            page,
            &dict,
            &rewrite(&[(Some(0), "")]),
            &BTreeSet::new(),
        );
        assert_eq!(mapping, [(1, 0)].into_iter().collect());
        let Object::Ref(array_ref) = contents_of(&edit, page) else {
            panic!("expected a reference to an array");
        };
        let array = edit.fetch(array_ref).expect("array");
        let array = array.as_array().expect("still an array");
        assert_eq!(array.len(), 1);
        assert_eq!(array.reference_at(0), Some(ObjRef::new(5, 0)));
    }

    // The resource dictionary lands where the page can see it.
    #[test]
    fn the_swept_resources_are_written_onto_the_page() {
        let base = single_stream();
        let mut edit = EditDoc::new(&base);
        let page = ObjRef::new(3, 0);
        let dict = base.page(0).expect("page").dict;
        let mut rewrite = rewrite(&[(Some(0), "q\nQ\n")]);
        rewrite.resources = Dict::from_pairs([(Name::from("ExtGState"), Object::Int(1))]);
        apply_rewrite(&mut edit, page, &dict, &rewrite, &BTreeSet::new());
        let page_dict = edit.fetch(page).expect("page");
        let resources = page_dict
            .as_dict()
            .and_then(|d| d.dict(pdfrum_object::names::RESOURCES, &edit))
            .expect("resources");
        assert_eq!(
            resources.raw(&Name::from("ExtGState")),
            Some(&Object::Int(1))
        );
    }

    // The sharing sweep: an object two dictionaries point at is shared, one
    // that only one points at is not.
    #[test]
    fn the_sweep_finds_the_objects_two_things_point_at() {
        let file = b"%PDF-1.7\n\
1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n\
2 0 obj\n<< /Type /Pages /Count 2 /Kids [3 0 R 4 0 R] >>\nendobj\n\
3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 5 0 R >>\nendobj\n\
4 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 5 0 R >>\nendobj\n\
5 0 obj\n<< /Length 0 >>\nstream\n\nendstream\nendobj\n\
trailer\n<< /Root 1 0 R /Size 6 >>\n";
        let base = load(Arc::from(&file[..]), &LoadOptions::default()).expect("opens");
        let edit = EditDoc::new(&base);
        let shared = shared_objects(&edit);
        // Both pages point at stream 5, and both point at the page tree.
        assert!(shared.contains(&5), "the shared stream");
        assert!(shared.contains(&2), "both pages name their parent");
        // Nothing points at page 3 twice.
        assert!(!shared.contains(&3));
    }
}
