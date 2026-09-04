//! Copying `/ViewerPreferences` between documents (ISO 32000-1 §12.2).
//!
//! # The filter is the whole defence
//!
//! Viewer preferences are simple values — booleans, numbers, strings, names,
//! and flat arrays of those. The copy **rejects references outright**, at
//! every level, and that single rule is what stops a maliciously or
//! accidentally circular preference graph from being walked at all. There is
//! no cycle detection here because there is nothing that can cycle: a value
//! that could point somewhere is not copied.
//!
//! An array is checked one level deep and rejected if **any** element is
//! itself an array, a dictionary, a reference or a stream. No nesting, no
//! indirection.
//!
//! The destination's existing entry is **replaced unconditionally**, and the
//! copy reports success even when the filter dropped every entry — an empty
//! `/ViewerPreferences` is a legitimate outcome, distinct from the source
//! having none at all.

#[cfg(test)]
use pdfrum_object::Array;
use pdfrum_object::{Dict, Object, Resolve, names};

/// Whether a value may be copied into a viewer-preferences dictionary.
///
/// Dictionaries, nulls, references and streams are refused outright; an array
/// is refused when any element is one of those or is itself an array.
#[must_use]
pub(crate) fn is_copyable(value: &Object) -> bool {
    match value {
        Object::Bool(_) | Object::Int(_) | Object::Real(_) | Object::Str(_) | Object::Name(_) => {
            true
        }
        Object::Array(a) => a.iter().all(is_flat_element),
        // A reference is what a cycle would be made of, so refusing it here
        // is the entire circular-graph defence.
        Object::Null | Object::Dict(_) | Object::Stream(_) | Object::Ref(_) => false,
    }
}

/// Whether an array element is one of the flat kinds.
fn is_flat_element(value: &Object) -> bool {
    matches!(
        value,
        Object::Bool(_) | Object::Int(_) | Object::Real(_) | Object::Str(_) | Object::Name(_)
    )
}

/// The filtered `/ViewerPreferences` to write onto a destination catalog, or
/// `None` when the source has none to copy.
///
/// A source whose `/ViewerPreferences` is not a dictionary — dangling, or a
/// value of some other kind — reads as absent.
#[must_use]
pub(crate) fn filtered(src_catalog: &Dict, r: &impl Resolve) -> Option<Dict> {
    let source = src_catalog.dict(names::VIEWER_PREFERENCES, r)?;
    let mut out = Dict::new();
    for (key, value) in source.iter() {
        if is_copyable(value) {
            out.push(key.clone(), value.clone());
        }
    }
    Some(out)
}

/// Whether an array would survive the filter, for a caller inspecting one
/// value rather than a whole dictionary.
#[must_use]
#[cfg(test)]
pub(crate) fn array_is_flat(a: &Array) -> bool {
    a.iter().all(is_flat_element)
}

#[cfg(test)]
mod tests {
    use super::{array_is_flat, filtered, is_copyable};
    use pdfrum_object::{Array, Dict, Name, ObjRef, Object, PdfString, Resolve, names};
    use std::collections::BTreeMap;
    use std::sync::Arc;

    struct Store(BTreeMap<u32, Object>);

    impl Resolve for Store {
        fn fetch(&self, r: ObjRef) -> Result<Arc<Object>, pdfrum_object::Error> {
            Ok(Arc::new(
                self.0.get(&r.num).cloned().unwrap_or(Object::Null),
            ))
        }
    }

    // CopyViewerPrefTypes (:387): the exact accept set — six entries survive.
    #[test]
    fn the_six_accepted_kinds_survive() {
        let prefs = Dict::from_pairs([
            (Name::from("Bool"), Object::Bool(true)),
            (Name::from("Num"), Object::Int(1)),
            (Name::from("Str"), Object::Str(PdfString::literal(b"str"))),
            (Name::from("Name"), Object::Name(Name::from("name"))),
            (Name::from("EmptyArray"), Object::Array(Array::new())),
            (
                Name::from("GoodArray"),
                Object::Array(Array::of([
                    Object::Int(1),
                    Object::Int(2),
                    Object::Int(3),
                    Object::Int(4),
                ])),
            ),
        ]);
        let catalog = Dict::from_pairs([(names::VIEWER_PREFERENCES.clone(), Object::Dict(prefs))]);
        let out = filtered(&catalog, &Store(BTreeMap::new())).expect("a dict");
        assert_eq!(out.len(), 6);
        assert_eq!(out.bool(&Name::from("Bool")), Some(true));
        assert_eq!(out.direct_int(&Name::from("Num")), Some(1));
        assert_eq!(
            out.string(&Name::from("Str")).map(|s| s.bytes.to_vec()),
            Some(b"str".to_vec())
        );
        assert_eq!(out.name(&Name::from("Name")), Some(&Name::from("name")));
        assert_eq!(
            out.array(&Name::from("GoodArray"), &Store(BTreeMap::new()))
                .map(|a| a.len()),
            Some(4)
        );
    }

    // A reference is refused wherever it appears — the whole defence.
    #[test]
    fn references_are_refused_at_every_level() {
        let reference = Object::Ref(ObjRef::new(9, 0));
        assert!(!is_copyable(&reference));
        assert!(!is_copyable(&Object::Array(Array::of([
            Object::Int(1),
            reference
        ]))));
    }

    #[test]
    fn dictionaries_nulls_and_streams_are_refused() {
        assert!(!is_copyable(&Object::Dict(Dict::new())));
        assert!(!is_copyable(&Object::Null));
        assert!(!is_copyable(&Object::Stream(Box::new(
            pdfrum_object::Stream::new(Dict::new(), pdfrum_object::ByteSpan::empty())
        ))));
    }

    // One level only: an array of arrays is refused whole.
    #[test]
    fn a_nested_array_is_refused() {
        let nested = Object::Array(Array::of([
            Object::Int(1),
            Object::Array(Array::of([Object::Int(2)])),
        ]));
        assert!(!is_copyable(&nested));
    }

    #[test]
    fn an_empty_array_is_flat() {
        assert!(array_is_flat(&Array::new()));
    }

    // BadRepeatViewerPref / BadCircularViewerPref: a graph that would cycle
    // is simply not copied, so nothing hangs.
    #[test]
    fn a_circular_graph_is_filtered_out_rather_than_walked() {
        let prefs = Dict::from_pairs([
            (Name::from("Loop"), Object::Ref(ObjRef::new(1, 0))),
            (Name::from("Fine"), Object::Bool(true)),
        ]);
        let catalog = Dict::from_pairs([(
            names::VIEWER_PREFERENCES.clone(),
            Object::Dict(prefs.clone()),
        )]);
        // Object 1 points back at the preferences dictionary.
        let store = Store(BTreeMap::from([(1, Object::Dict(prefs))]));
        let out = filtered(&catalog, &store).expect("a dict");
        assert_eq!(out.len(), 1);
        assert!(out.contains_key(&Name::from("Fine")));
    }

    // The filter dropping everything is success, not failure — an empty
    // dictionary and no dictionary are different outcomes.
    #[test]
    fn dropping_every_entry_still_reports_success() {
        let prefs = Dict::from_pairs([(Name::from("Loop"), Object::Ref(ObjRef::new(1, 0)))]);
        let catalog = Dict::from_pairs([(names::VIEWER_PREFERENCES.clone(), Object::Dict(prefs))]);
        let out = filtered(&catalog, &Store(BTreeMap::new())).expect("still a dict");
        assert!(out.is_empty());
    }

    // NoViewerPreferences (:63): a catalog with none reports none.
    #[test]
    fn a_catalog_without_preferences_copies_nothing() {
        let catalog =
            Dict::from_pairs([(names::TYPE.clone(), Object::Name(Name::from("Catalog")))]);
        assert!(filtered(&catalog, &Store(BTreeMap::new())).is_none());
    }

    #[test]
    fn a_non_dictionary_value_reads_as_absent() {
        let catalog = Dict::from_pairs([(names::VIEWER_PREFERENCES.clone(), Object::Int(5))]);
        assert!(filtered(&catalog, &Store(BTreeMap::new())).is_none());
    }
}
