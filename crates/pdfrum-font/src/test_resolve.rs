//! A map-backed [`Resolve`] for the tests.
//!
//! Needed because a stream may never be a *direct* dictionary value
//! (ISO 32000-1 §7.3.8.1) — `Dict::push` enforces that — so any test involving
//! an embedded font program, a `/ToUnicode` CMap or a `/CIDToGIDMap` table has
//! to reference it indirectly, exactly as a real file does.

use pdfrum_object::{ByteSpan, Dict, Error, ObjRef, Object, Resolve, Stream};
use std::collections::HashMap;
use std::sync::Arc;

/// Objects keyed by number, as a cross-reference table keys them.
#[derive(Debug, Default)]
pub(crate) struct TestStore {
    objects: HashMap<u32, Arc<Object>>,
    next: u32,
}

impl TestStore {
    /// An empty store.
    pub(crate) fn new() -> Self {
        Self {
            objects: HashMap::new(),
            next: 1,
        }
    }

    /// Store an object and return a reference to it.
    pub(crate) fn add(&mut self, obj: Object) -> Object {
        let num = self.next;
        self.next += 1;
        self.objects.insert(num, Arc::new(obj));
        Object::Ref(ObjRef::new(num, 0))
    }

    /// Store a stream over `bytes` with an empty dictionary, and return a
    /// reference to it.
    pub(crate) fn add_stream(&mut self, bytes: Vec<u8>) -> Object {
        self.add(Object::Stream(Box::new(Stream::new(
            Dict::new(),
            ByteSpan::from(bytes),
        ))))
    }
}

impl Resolve for TestStore {
    fn fetch(&self, r: ObjRef) -> Result<Arc<Object>, Error> {
        self.objects
            .get(&r.num)
            .map(Arc::clone)
            .ok_or(Error::UnresolvedRef(r))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stored_stream_resolves_back() {
        let mut store = TestStore::new();
        let reference = store.add_stream(b"hello".to_vec());
        let Object::Ref(r) = reference else {
            panic!("add_stream returns a reference")
        };
        let fetched = store.fetch(r).expect("resolves");
        assert_eq!(
            fetched.as_stream().map(|s| s.data.as_bytes().to_vec()),
            Some(b"hello".to_vec())
        );
    }

    #[test]
    fn an_unknown_reference_dangles() {
        let store = TestStore::new();
        assert!(store.fetch(ObjRef::new(99, 0)).is_err());
    }
}
