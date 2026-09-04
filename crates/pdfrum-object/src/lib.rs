//! The PDF object model as plain values (ISO 32000 §7.3): the [`Object`] enum
//! (Null/Bool/Int/Real/String/Name/Array/Dict/Stream/Ref), dictionary-key
//! name constants, and the [`Resolve`] trait for indirect-reference lookup.
//! Construction and typed access only — no parsing lives here. Everything
//! above this crate reads PDF through the typed accessors on [`Dict`] and
//! [`Array`], threading a `&impl Resolve` for the store of indirect objects:
//!
//! ```
//! use pdfrum_object::{Array, Dict, NoResolve, Object, names};
//!
//! let page = Dict::from_pairs([
//!     (names::TYPE.clone(), Object::Name(names::PAGE.clone())),
//!     (
//!         names::RECT.clone(),
//!         Object::Array(Array::of([0, 0, 612, 792].map(Object::from))),
//!     ),
//! ]);
//!
//! assert_eq!(page.name(names::TYPE), Some(names::PAGE));
//! assert_eq!(page.rect(names::RECT, &NoResolve).width(), 612.0);
//! ```
//!
//! **Resolution is one hop**: a reference to a reference is absent, and each
//! accessor comes in a resolving and a non-resolving flavour ([`Resolve`],
//! [`Dict`]). **Integers have two readings**: a permissions word written
//! `4294967295` means `-1` through [`narrow_to_signed32`] and
//! `4294967296.0` through [`widen_to_f32`].

#![forbid(unsafe_code)]
// Every byte in this crate came from an untrusted file: index with `get()`.
#![warn(clippy::indexing_slicing)]

mod array;
mod dict;
mod error;
mod name;
pub mod names;
mod number;
mod object;
mod resolve;
mod stream;
mod string;

#[cfg(test)]
mod test_resolve;

pub use array::Array;
pub use dict::Dict;
pub use error::Error;
pub use name::{Name, name_decode, name_encode};
pub use number::{
    INT_RANGE, fmt_int, fmt_number, narrow_to_signed32, truncate_to_signed32, widen_to_f32,
};
pub use object::{ObjRef, Object};
pub use resolve::{NoResolve, Resolve, Resolved};
pub use stream::{ByteSpan, Stream};
pub use string::{
    PDF_DOC_ENCODING, PdfString, StringSyntax, decode_text, encode_string_hex,
    encode_string_literal, encode_text,
};

#[cfg(test)]
mod tests {
    use crate::test_resolve::TestStore;
    use crate::{Array, ByteSpan, Dict, Name, NoResolve, ObjRef, Object, PdfString, Stream, names};

    fn every_variant() -> Dict {
        Dict::from_pairs([
            (Name::from("null"), Object::Null),
            (Name::from("bool"), Object::Bool(true)),
            (Name::from("int"), Object::Int(1245)),
            (Name::from("real"), Object::Real(9.003_45)),
            (
                Name::from("str"),
                Object::Str(PdfString::literal(b"A simple test")),
            ),
            (
                Name::from("hexstr"),
                Object::Str(PdfString::hex(b"\x12\xAC")),
            ),
            (Name::from("name"), Object::Name(Name::from("space"))),
            (
                Name::from("array"),
                Object::Array(Array::of([
                    Object::Int(8902),
                    Object::Name(Name::from("address")),
                ])),
            ),
            (
                Name::from("dict"),
                Object::Dict(Dict::from_pairs([(Name::from("k"), Object::Int(1))])),
            ),
            (Name::from("ref"), Object::Ref(ObjRef::new(7, 0))),
        ])
    }

    // From cpdf_object_unittest.cpp:210-236: the GetString table, restated.
    #[test]
    fn byte_string_spelling_per_type() {
        let dict = every_variant();
        let spell = |k: &str| {
            dict.byte_string(&Name::from(k), &NoResolve)
                .unwrap_or_default()
        };
        assert_eq!(spell("bool"), b"true");
        assert_eq!(spell("int"), b"1245");
        assert_eq!(spell("real"), b"9.00345");
        assert_eq!(spell("str"), b"A simple test");
        assert_eq!(spell("name"), b"space");
        // Composites and null have no spelling.
        assert_eq!(spell("null"), b"");
        assert_eq!(spell("array"), b"");
        assert_eq!(spell("dict"), b"");
    }

    // From cpdf_object_unittest.cpp:227-240: the GetUnicodeText table.
    #[test]
    fn text_reading_per_type() {
        let dict = every_variant();
        let text = |k: &str| dict.text(&Name::from(k), &NoResolve).unwrap_or_default();
        assert_eq!(text("str"), "A simple test");
        assert_eq!(text("name"), "space");
        assert_eq!(text("bool"), "");
        assert_eq!(text("int"), "");
        assert_eq!(text("real"), "");
        assert_eq!(text("array"), "");
        assert_eq!(text("dict"), "");
        assert_eq!(text("null"), "");
    }

    // From cpdf_object_unittest.cpp:242-272: GetNumber and GetInteger.
    #[test]
    fn numeric_readings_per_type() {
        let dict = every_variant();
        let num = |k: &str| dict.number(&Name::from(k), &NoResolve);
        let int = |k: &str| dict.int(&Name::from(k), &NoResolve);

        assert_eq!(num("int"), Some(1245.0));
        assert_eq!(num("real"), Some(9.003_45));
        // Booleans are not numbers...
        assert_eq!(num("bool"), None);
        assert_eq!(num("str"), None);
        assert_eq!(num("name"), None);
        assert_eq!(num("null"), None);

        // ...but do have an integer reading.
        assert_eq!(int("bool"), Some(1));
        assert_eq!(int("int"), Some(1245));
        assert_eq!(int("real"), Some(9));
        assert_eq!(int("str"), None);
        assert_eq!(int("name"), None);
        assert_eq!(int("null"), None);
    }

    #[test]
    fn stream_reads_as_its_own_dictionary_and_keeps_its_bytes() {
        let dict = Dict::from_pairs([(names::LENGTH.clone(), Object::Int(3))]);
        let stream = Object::Stream(Box::new(Stream::new(
            dict.clone(),
            ByteSpan::from(b"abc".to_vec()),
        )));
        assert_eq!(stream.as_dict(), Some(&dict));
        assert_eq!(stream.as_stream().map(|s| &*s.data), Some(&b"abc"[..]));
        assert_eq!(stream.to_byte_string(), b"");
        assert_eq!(stream.number(), None);
    }

    // From cpdf_object_unittest.cpp:845-862 and :947-1013 — clone_direct
    // flattens references and drops the edges that would close a cycle.
    #[test]
    fn clone_direct_flattens_references() {
        let store = TestStore::from_pairs([(7, Object::Int(42))]);
        let obj = Object::Array(Array::of([Object::Ref(ObjRef::new(7, 0)), Object::Int(1)]));
        assert_eq!(
            obj.clone_direct(&store),
            Object::Array(Array::of([Object::Int(42), Object::Int(1)]))
        );
    }

    #[test]
    fn clone_direct_cuts_self_referential_edges() {
        // Object 1 is an array whose only element points back at object 1.
        let store = TestStore::from_pairs([(
            1,
            Object::Array(Array::of([Object::Ref(ObjRef::new(1, 0))])),
        )]);
        let root = Object::Ref(ObjRef::new(1, 0));
        // The cycle edge disappears; the array survives, empty.
        assert_eq!(root.clone_direct(&store), Object::Array(Array::new()));
    }

    #[test]
    fn clone_direct_cuts_a_dictionary_stream_loop() {
        // A stream whose dictionary refers back to the stream's own object.
        let inner = Dict::from_pairs([(Name::from("Self"), Object::Ref(ObjRef::new(2, 0)))]);
        let store = TestStore::from_pairs([(
            2,
            Object::Stream(Box::new(Stream::new(
                inner,
                ByteSpan::from(b"data".to_vec()),
            ))),
        )]);
        let cloned = Object::Ref(ObjRef::new(2, 0)).clone_direct(&store);
        let stream = cloned.as_stream().expect("still a stream");
        assert!(stream.dict.is_empty(), "the looping key was dropped");
        assert_eq!(&*stream.data, b"data");
    }

    #[test]
    fn clone_direct_keeps_shared_substructure_for_siblings() {
        // Two siblings pointing at the same object is not a cycle.
        let store = TestStore::from_pairs([(3, Object::Int(5))]);
        let obj = Object::Array(Array::of([
            Object::Ref(ObjRef::new(3, 0)),
            Object::Ref(ObjRef::new(3, 0)),
        ]));
        assert_eq!(
            obj.clone_direct(&store),
            Object::Array(Array::of([Object::Int(5), Object::Int(5)]))
        );
    }

    #[test]
    fn clone_direct_drops_dangling_references() {
        let store = TestStore::default();
        let obj = Object::Dict(Dict::from_pairs([
            (Name::from("gone"), Object::Ref(ObjRef::new(9, 0))),
            (Name::from("here"), Object::Int(1)),
        ]));
        assert_eq!(
            obj.clone_direct(&store),
            Object::Dict(Dict::from_pairs([(Name::from("here"), Object::Int(1))]))
        );
    }

    // The shape that made `clone_direct` panic on ordinary corpus files: a
    // `/Resources` whose `/XObject` entries are indirect streams. Flattening
    // stores each stream as a *direct* dictionary value, which is exactly
    // what `CPDF_Dictionary::CloneNonCyclic` does — its loop inserts into
    // `map_` directly and so never reaches the `CHECK(!IsStream())` that
    // guards the ordinary setters. §7.3.8.1 binds the writer, not this type.
    #[test]
    fn clone_direct_stores_a_resolved_stream_as_a_direct_dict_value() {
        let image = Stream::new(
            Dict::from_pairs([
                (names::TYPE.clone(), Object::Name(Name::from("XObject"))),
                (names::SUBTYPE.clone(), Object::Name(Name::from("Image"))),
                (names::LENGTH.clone(), Object::Int(4)),
            ]),
            ByteSpan::from(b"\xDE\xAD\xBE\xEF".to_vec()),
        );
        let store = TestStore::from_pairs([(9, Object::Stream(Box::new(image.clone())))]);

        let resources = Object::Dict(Dict::from_pairs([(
            Name::from("XObject"),
            Object::Dict(Dict::from_pairs([(
                Name::from("Image9"),
                Object::Ref(ObjRef::new(9, 0)),
            )])),
        )]));

        let cloned = resources.clone_direct(&store);
        let xobject = cloned
            .as_dict()
            .and_then(|d| d.raw(&Name::from("XObject")))
            .and_then(Object::as_dict)
            .expect("the /XObject sub-dictionary survives");
        // Stored directly, not as a reference and not dropped.
        assert_eq!(
            xobject.raw(&Name::from("Image9")),
            Some(&Object::Stream(Box::new(image.clone())))
        );
        // And the resolving accessor reads it back, as `GetStreamFor` does.
        assert_eq!(
            xobject.stream(&Name::from("Image9"), &NoResolve),
            Some(image)
        );
    }

    #[test]
    fn clone_direct_stores_a_resolved_stream_as_a_direct_array_element() {
        let stream = Stream::new(
            Dict::from_pairs([(names::LENGTH.clone(), Object::Int(2))]),
            ByteSpan::from(b"hi".to_vec()),
        );
        let store = TestStore::from_pairs([(4, Object::Stream(Box::new(stream.clone())))]);
        let array = Object::Array(Array::of([Object::Ref(ObjRef::new(4, 0))]));

        let cloned = array.clone_direct(&store);
        let cloned = cloned.as_array().expect("still an array");
        assert_eq!(
            cloned.raw_at(0),
            Some(&Object::Stream(Box::new(stream.clone())))
        );
        assert_eq!(cloned.stream_at(0, &NoResolve), Some(stream));
    }

    // The cloned stream carries its **raw**, still-encoded bytes, matching
    // `CPDF_Stream::CloneNonCyclic`'s `LoadAllDataRaw()` — no filter is run
    // and `/Filter` stays in the cloned dictionary.
    #[test]
    fn clone_direct_keeps_a_streams_raw_bytes_and_filter() {
        let stream = Stream::new(
            Dict::from_pairs([
                (
                    names::FILTER.clone(),
                    Object::Name(Name::from("FlateDecode")),
                ),
                (names::LENGTH.clone(), Object::Int(3)),
            ]),
            ByteSpan::from(b"\x78\x9C\x03".to_vec()),
        );
        let store = TestStore::from_pairs([(1, Object::Stream(Box::new(stream)))]);

        let cloned = Object::Ref(ObjRef::new(1, 0)).clone_direct(&store);
        let cloned = cloned.as_stream().expect("a stream");
        assert_eq!(&*cloned.data, b"\x78\x9C\x03");
        assert_eq!(
            cloned.dict.name(names::FILTER).map(Name::as_str),
            Some(Some("FlateDecode"))
        );
    }

    // A stream reachable by two sibling paths is not a cycle: both clone.
    #[test]
    fn clone_direct_clones_a_shared_stream_down_both_sibling_paths() {
        let stream = Stream::new(Dict::new(), ByteSpan::from(b"xy".to_vec()));
        let store = TestStore::from_pairs([(6, Object::Stream(Box::new(stream.clone())))]);
        let dict = Object::Dict(Dict::from_pairs([
            (Name::from("a"), Object::Ref(ObjRef::new(6, 0))),
            (Name::from("b"), Object::Ref(ObjRef::new(6, 0))),
        ]));

        assert_eq!(
            dict.clone_direct(&store),
            Object::Dict(Dict::from_pairs([
                (Name::from("a"), Object::Stream(Box::new(stream.clone()))),
                (Name::from("b"), Object::Stream(Box::new(stream))),
            ]))
        );
    }

    // A stream whose own dictionary points back at the dictionary holding it
    // still cuts only the cycle edge — the stream itself survives inline.
    #[test]
    fn clone_direct_cuts_only_the_cycle_edge_of_an_inlined_stream() {
        let inner = Stream::new(
            Dict::from_pairs([(Name::from("Up"), Object::Ref(ObjRef::new(1, 0)))]),
            ByteSpan::from(b"body".to_vec()),
        );
        let store = TestStore::from_pairs([
            (
                1,
                Object::Dict(Dict::from_pairs([(
                    Name::from("Down"),
                    Object::Ref(ObjRef::new(2, 0)),
                )])),
            ),
            (2, Object::Stream(Box::new(inner))),
        ]);

        let cloned = Object::Ref(ObjRef::new(1, 0)).clone_direct(&store);
        let down = cloned
            .as_dict()
            .and_then(|d| d.raw(&Name::from("Down")))
            .and_then(Object::as_stream)
            .expect("the stream is stored directly under /Down");
        assert_eq!(&*down.data, b"body");
        assert!(down.dict.is_empty(), "the /Up cycle edge was cut");
    }

    // Containers accept a stream value in memory: the file-format rule of
    // §7.3.8.1 is the writer's to enforce, not this type's.
    #[test]
    fn containers_accept_a_stream_value_in_memory() {
        let stream = Object::Stream(Box::new(Stream::new(
            Dict::new(),
            ByteSpan::from(b"z".to_vec()),
        )));
        let mut dict = Dict::new();
        dict.push(Name::from("S"), stream.clone());
        assert!(dict.stream(&Name::from("S"), &NoResolve).is_some());

        let mut array = Array::new();
        array.push(stream);
        assert!(array.stream_at(0, &NoResolve).is_some());
    }

    #[test]
    fn plain_clone_keeps_references_as_references() {
        let obj = every_variant();
        let copy = obj.clone();
        assert_eq!(obj, copy);
        assert_eq!(copy.reference(&Name::from("ref")), Some(ObjRef::new(7, 0)));
    }

    #[test]
    fn objects_are_send_and_sync() {
        const fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Object>();
        assert_send_sync::<Dict>();
        assert_send_sync::<Array>();
        assert_send_sync::<Stream>();
        assert_send_sync::<ByteSpan>();
        assert_send_sync::<Name>();
        assert_send_sync::<PdfString>();
    }

    #[test]
    fn reference_identity_is_by_number_and_generation() {
        assert_eq!(ObjRef::new(1, 0), ObjRef::new(1, 0));
        assert_ne!(ObjRef::new(1, 0), ObjRef::new(1, 1));
        // The sentinel is private, so the test names the value the *file*
        // would contain rather than importing a constant to compare against —
        // which is the whole point of `docs/design/idiomatic-api.md` §C.
        assert!(ObjRef::new(0xFFFF_FFFF, 0).is_invalid());
        assert!(!ObjRef::new(1, 0).is_invalid());
    }

    #[test]
    fn debug_dump_of_a_composite_tree_is_readable() {
        // Guards against an accidental representation change: every variant
        // shows its payload, and a stream shows its length rather than its
        // bytes.
        let dump = format!("{:?}", every_variant());
        for expected in [
            "/null",
            "Null",
            "Bool(true)",
            "Int(1245)",
            "Real(9.00345)",
            "(A simple test)",
            "<12AC>",
            "Name(/space)",
            "Ref(ObjRef { num: 7, generation: 0 })",
        ] {
            assert!(dump.contains(expected), "missing {expected} in {dump}");
        }
        let stream = Stream::new(Dict::new(), ByteSpan::from(vec![0u8; 4096]));
        let stream_dump = format!("{stream:?}");
        assert!(stream_dump.contains("len: 4096"), "{stream_dump}");
        assert!(stream_dump.len() < 200, "a stream dump stays short");
    }
}
