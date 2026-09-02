//! Inherited page attributes (ISO 32000-1 §7.7.3.4).
//!
//! Four keys — `/Resources`, `/MediaBox`, `/CropBox`, `/Rotate` — may be
//! stated on a page or on any ancestor in its `/Parent` chain. Importing a
//! page detaches it from that chain, so whatever it was inheriting has to be
//! written onto the copy or it is lost.
//!
//! # The walk is stricter than you would guess
//!
//! Four conditions must all hold before the chain is even entered, and each
//! failure reports "not found" rather than falling back:
//!
//! - the dictionary must have **both** a `/Parent` and a `/Type`;
//! - that `/Type` must be `Page`, compared **case-sensitively** — unlike the
//!   copier's own `/Type` test, which ignores case;
//! - `/Parent` must resolve to a dictionary.
//!
//! Then the page's *own* value wins if it has one, and only otherwise does
//! the walk climb. Revisiting a node — a `/Parent` cycle — returns "not
//! found" **outright**, discarding any value found on the way up rather than
//! keeping it.
//!
//! Ancestors are not `/Type`-checked at all, which is why a page whose parent
//! chain runs through a dictionary that is not a `/Pages` node still inherits
//! from it.

#![allow(
    dead_code,
    reason = "`copy_inheritable` is exercised by this module's own tests only — \
              `import_pages` reaches for `inheritable` directly. It became visible \
              to the lint when `pub mod import` went private (§A.11 step 12)"
)]

use pdfrum_object::{Dict, Name, Object, Resolve, names};

/// How far up a `/Parent` chain to walk before giving up.
const MAX_ANCESTORS: usize = 64;

/// The value of `key` for `page`, looked up through its `/Parent` chain.
///
/// Returns `None` when the chain cannot be entered, the key is nowhere in it,
/// or the chain cycles.
#[must_use]
pub(crate) fn inheritable(page: &Dict, key: &Name, r: &impl Resolve) -> Option<Object> {
    // Both keys must be present for the chain to be entered at all.
    if !page.contains_key(names::PARENT) || !page.contains_key(names::TYPE) {
        return None;
    }
    // Case-sensitive here, deliberately: a `/page` is not a page for the
    // purpose of inheritance even though the copier's own test accepts it.
    if page.name(names::TYPE)?.as_bytes() != b"Page" {
        return None;
    }
    // The parent must be a dictionary before anything is looked up, even the
    // page's own value.
    page.dict(names::PARENT, r)?;

    // The page's own value wins.
    if let Some(own) = page.get(key, r) {
        return Some(own.get().clone());
    }

    let mut seen: Vec<Dict> = vec![page.clone()];
    let mut node = page.dict(names::PARENT, r)?;
    for _ in 0..MAX_ANCESTORS {
        // A revisit discards everything, rather than returning what was
        // found on the way up.
        if seen.contains(&node) {
            return None;
        }
        if let Some(value) = node.get(key, r) {
            return Some(value.get().clone());
        }
        seen.push(node.clone());
        node = node.dict(names::PARENT, r)?;
    }
    None
}

/// Copy `key` onto `out` from the source page's chain, unless `out` already
/// has it.
///
/// Returns whether the key is present afterwards, which the `/MediaBox`
/// fallback ladder reads.
pub(crate) fn copy_inheritable(out: &mut Dict, page: &Dict, key: &Name, r: &impl Resolve) -> bool {
    // Already copied by the shallow key sweep: the page's own value takes
    // precedence over any ancestor's, and this is where that precedence is
    // enforced.
    if out.contains_key(key) {
        return true;
    }
    match inheritable(page, key, r) {
        Some(value) => {
            out.push(key.clone(), value);
            true
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{copy_inheritable, inheritable};
    use pdfrum_object::{Array, Dict, Name, ObjRef, Object, Resolve, names};
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

    fn page(pairs: impl IntoIterator<Item = (&'static str, Object)>) -> Dict {
        let mut d = Dict::from_pairs([(names::TYPE.clone(), Object::Name(names::PAGE.clone()))]);
        for (k, v) in pairs {
            d.push(Name::from(k), v);
        }
        d
    }

    fn media_box(width: f32) -> Object {
        Object::Array(Array::of([
            Object::Int(0),
            Object::Int(0),
            Object::Real(width),
            Object::Int(792),
        ]))
    }

    fn r(num: u32) -> Object {
        Object::Ref(ObjRef::new(num, 0))
    }

    #[test]
    fn a_page_inherits_from_its_grandparent() {
        let store = Store(BTreeMap::from([
            (
                2,
                Object::Dict(Dict::from_pairs([(names::PARENT.clone(), r(1))])),
            ),
            (
                1,
                Object::Dict(Dict::from_pairs([(
                    names::MEDIA_BOX.clone(),
                    media_box(612.0),
                )])),
            ),
        ]));
        let p = page([("Parent", r(2))]);
        let found = inheritable(&p, names::MEDIA_BOX, &store).expect("inherits");
        assert_eq!(found.as_array().map(Array::len), Some(4));
    }

    // The page's own value shadows every ancestor's.
    #[test]
    fn a_pages_own_value_wins() {
        let store = Store(BTreeMap::from([(
            1,
            Object::Dict(Dict::from_pairs([(
                names::MEDIA_BOX.clone(),
                media_box(612.0),
            )])),
        )]));
        let p = page([("Parent", r(1)), ("MediaBox", media_box(200.0))]);
        let found = inheritable(&p, names::MEDIA_BOX, &store).expect("its own");
        assert_eq!(found.as_array().and_then(|a| a.number_at(2)), Some(200.0));
    }

    // No `/Parent` at all means nothing is inheritable — not even a value
    // the page itself holds.
    #[test]
    fn no_parent_means_nothing_is_inheritable() {
        let store = Store(BTreeMap::new());
        let p = page([("MediaBox", media_box(200.0))]);
        assert!(inheritable(&p, names::MEDIA_BOX, &store).is_none());
    }

    // The `/Type` test is case-sensitive here, unlike the copier's.
    #[test]
    fn the_type_test_is_case_sensitive() {
        let store = Store(BTreeMap::from([(
            1,
            Object::Dict(Dict::from_pairs([(
                names::MEDIA_BOX.clone(),
                media_box(612.0),
            )])),
        )]));
        let mut p = Dict::from_pairs([
            (names::TYPE.clone(), Object::Name(Name::from("page"))),
            (names::PARENT.clone(), r(1)),
        ]);
        assert!(inheritable(&p, names::MEDIA_BOX, &store).is_none());
        // Spelled correctly, it works.
        p = Dict::from_pairs([
            (names::TYPE.clone(), Object::Name(names::PAGE.clone())),
            (names::PARENT.clone(), r(1)),
        ]);
        assert!(inheritable(&p, names::MEDIA_BOX, &store).is_some());
    }

    #[test]
    fn a_missing_type_stops_the_walk() {
        let store = Store(BTreeMap::from([(
            1,
            Object::Dict(Dict::from_pairs([(
                names::MEDIA_BOX.clone(),
                media_box(612.0),
            )])),
        )]));
        let p = Dict::from_pairs([(names::PARENT.clone(), r(1))]);
        assert!(inheritable(&p, names::MEDIA_BOX, &store).is_none());
    }

    // A `/Parent` that is not a dictionary ends the chain before it starts.
    #[test]
    fn a_non_dictionary_parent_stops_the_walk() {
        let store = Store(BTreeMap::from([(1, Object::Int(5))]));
        let p = page([("Parent", r(1)), ("MediaBox", media_box(200.0))]);
        assert!(inheritable(&p, names::MEDIA_BOX, &store).is_none());
    }

    // A cycle returns nothing at all, discarding what was found on the way.
    #[test]
    fn a_parent_cycle_discards_everything() {
        let store = Store(BTreeMap::from([
            (
                1,
                Object::Dict(Dict::from_pairs([(names::PARENT.clone(), r(2))])),
            ),
            (
                2,
                Object::Dict(Dict::from_pairs([(names::PARENT.clone(), r(1))])),
            ),
        ]));
        let p = page([("Parent", r(1))]);
        assert!(inheritable(&p, names::ROTATE, &store).is_none());
    }

    // An ancestor's `/Type` is never checked, so a chain through something
    // that is not a `/Pages` node still inherits.
    #[test]
    fn an_ancestors_type_is_not_checked() {
        let store = Store(BTreeMap::from([(
            1,
            Object::Dict(Dict::from_pairs([
                (names::TYPE.clone(), Object::Name(Name::from("Whatever"))),
                (names::ROTATE.clone(), Object::Int(90)),
            ])),
        )]));
        let p = page([("Parent", r(1))]);
        assert_eq!(
            inheritable(&p, names::ROTATE, &store).and_then(|o| o.as_int()),
            Some(90)
        );
    }

    // The copy is a no-op when the destination already has the key, which is
    // what gives page-over-ancestor precedence.
    #[test]
    fn copying_never_overwrites_what_is_already_there() {
        let store = Store(BTreeMap::from([(
            1,
            Object::Dict(Dict::from_pairs([(
                names::MEDIA_BOX.clone(),
                media_box(612.0),
            )])),
        )]));
        let p = page([("Parent", r(1))]);
        let mut out = Dict::from_pairs([(names::MEDIA_BOX.clone(), media_box(100.0))]);
        assert!(copy_inheritable(&mut out, &p, names::MEDIA_BOX, &store));
        assert_eq!(
            out.array(names::MEDIA_BOX, &store)
                .and_then(|a| a.number_at(2)),
            Some(100.0),
            "the existing value is untouched"
        );
    }

    #[test]
    fn copying_reports_whether_the_key_ended_up_present() {
        let store = Store(BTreeMap::new());
        let p = page([]);
        let mut out = Dict::new();
        assert!(!copy_inheritable(&mut out, &p, names::MEDIA_BOX, &store));
        assert!(!out.contains_key(names::MEDIA_BOX));
    }
}
