//! The PDF object model as plain values (ISO 32000 §7.3): the [`Object`] enum
//! (Null/Bool/Int/Real/String/Name/Array/Dict/Stream/Ref), dictionary-key
//! name constants, and the [`Resolve`] trait for indirect-reference lookup.
//! Construction and typed access only — no parsing lives here (SPEC.md §2).
//!
//! # Reading a document's structure
//!
//! Everything above this crate reads PDF through the typed accessors on
//! [`Dict`] and [`Array`], threading a `&impl Resolve` for the store that
//! holds indirect objects:
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
//! # Two things to know before writing an accessor call
//!
//! **Resolution is one level, and which accessors do it is deliberate.** An
//! indirect `/Prev` is ignored by the cross-reference reader while an
//! indirect `/Length` is chased; both behaviors keep real files opening. Each
//! accessor comes in a resolving and a non-resolving flavour, documented on
//! [`Dict`], and a caller picks the one whose C++ counterpart it is matching.
//!
//! **Integers have two readings.** [`Object::Int`] stores the mathematical
//! value, but a file that writes `4294967295` for a permissions word means
//! `-1` when read as an integer and `4294967296.0` when read as a number. The
//! accessors reproduce both; see the [`number`] module.

#![forbid(unsafe_code)]
// Every byte in this crate came from an untrusted file: index with `get()`.
#![warn(clippy::indexing_slicing)]

mod array;
mod dict;
mod error;
mod name;
pub mod names;
pub mod number;
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
pub use number::{INT_RANGE, as_c_float, as_c_int, fmt_int, fmt_number, real_as_c_int};
pub use object::{ObjRef, Object, SharedObject};
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
        let stream = Object::Stream(Stream::new(dict.clone(), ByteSpan::from(b"abc".to_vec())));
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
            Object::Stream(Stream::new(inner, ByteSpan::from(b"data".to_vec()))),
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
        assert!(ObjRef::new(ObjRef::INVALID_NUM, 0).is_invalid());
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
