//! The trailer (ISO 32000-1 §7.5.5) and the xref-stream object that replaces
//! it (§7.5.8).
//!
//! # Copy everything, recompute eleven things
//!
//! The trailer is built by copying the input's, key for key, **except** the
//! eleven keys the writer recomputes ([`super::reach::SUPPRESSED_TRAILER_KEYS`]).
//! Everything else survives — including keys no specification defines, which
//! is why a file whose trailer holds a stray `/Foo` still holds it afterwards.
//! That is not tolerance for its own sake: a trailer is where producers put
//! private data, and dropping what we do not recognise would lose it.
//!
//! # The xref-stream shape
//!
//! When the cross-reference goes into a stream, the trailer becomes an
//! ordinary indirect object carrying the same keys plus `/W`, `/Index`,
//! `/Length` and a payload of five-byte records. `/W [0 4 1]` means: no type
//! field (defaulting to in-use), a four-byte offset, a one-byte generation.
//! `/Size` counts two higher than a classic trailer's because the trailer
//! object is itself an object the table must describe.

#![allow(
    dead_code,
    reason = "`from_scratch` and `is_suppressed` are read by this module's own \
              tests only — `build` applies the suppression list inline, and no \
              save path yet writes a trailer with no source. They became visible \
              to the lint when `pub mod write` went private (§A.11 step 12)"
)]

use pdfrum_object::{Array, Dict, Name, Object, names};

use crate::write::object::{write_dict, write_name, write_object};
use crate::write::reach::SUPPRESSED_TRAILER_KEYS;
use crate::write::xref::{ObjectOffsets, stream_record};

/// Everything the trailer needs to know.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TrailerParts<'a> {
    /// The input's trailer, whose keys are copied except the suppressed ones.
    pub(crate) source: &'a Dict,
    /// The `/ID` array to declare.
    pub(crate) id: &'a Array,
    /// The highest object number written.
    pub(crate) last_object_number: u32,
    /// The `/Prev` to declare, when this is an incremental save over a
    /// document that had a previous section.
    pub(crate) prev: Option<u64>,
    /// The object number of the `/Encrypt` dictionary, when one is written.
    pub(crate) encrypt: Option<u32>,
}

/// Assemble the trailer dictionary a classic save writes after `trailer`.
///
/// Returned as a `Dict` rather than bytes so the xref-stream path can add its
/// own keys to the same value — the two shapes differ in what they carry, not
/// in how the shared keys are chosen.
pub(crate) fn build(parts: TrailerParts<'_>) -> Dict {
    let mut out = Dict::new();
    for (key, value) in parts.source.iter() {
        if SUPPRESSED_TRAILER_KEYS.contains(&key) {
            continue;
        }
        out.push(key.clone(), value.clone());
    }

    if let Some(num) = parts.encrypt {
        out.push(
            names::ENCRYPT.clone(),
            Object::Ref(pdfrum_object::ObjRef::new(num, 0)),
        );
    }
    out.push(
        names::SIZE.clone(),
        Object::Int(i64::from(parts.last_object_number).saturating_add(1)),
    );
    if let Some(prev) = parts.prev {
        out.push(
            names::PREV.clone(),
            Object::Int(i64::try_from(prev).unwrap_or(i64::MAX)),
        );
    }
    out.push(names::ID.clone(), Object::Array(parts.id.clone()));
    out
}

/// Write a classic `trailer << … >>` section.
pub(crate) fn write_classic(out: &mut Vec<u8>, trailer: &Dict) {
    out.extend_from_slice(b"trailer\r\n");
    // The trailer is not an indirect object, so its strings are never
    // encrypted — a reader must be able to read `/ID` without the key.
    write_dict(out, trailer, None);
}

/// Write the cross-reference as a stream object numbered `num`, covering
/// exactly `written` in order.
///
/// `/Size` counts one higher than the classic form because this object is
/// itself in the table.
pub(crate) fn write_stream(
    out: &mut Vec<u8>,
    num: u32,
    trailer: &Dict,
    offsets: &ObjectOffsets,
    written: &[u32],
) {
    let mut payload = Vec::with_capacity(written.len().saturating_mul(5));
    for n in written {
        stream_record(&mut payload, offsets.get(*n).unwrap_or(0));
    }

    out.extend_from_slice(format!("{num} 0 obj <<").as_bytes());
    for (key, value) in trailer.iter() {
        // `/Size` is written below, one higher than the classic form.
        if key == names::SIZE {
            continue;
        }
        write_name(out, key);
        write_object(out, value, None);
    }
    write_name(out, names::TYPE);
    write_name(out, names::XREF);
    write_name(out, names::SIZE);
    let size = trailer
        .direct_int(names::SIZE)
        .unwrap_or(0)
        .saturating_add(1);
    write_object(out, &Object::Int(size), None);

    out.extend_from_slice(b"/W[0 4 1]/Index[");
    for n in written {
        out.extend_from_slice(format!("{n} 1 ").as_bytes());
    }
    out.extend_from_slice(b"]/Length ");
    // The true byte count. The C++ writes `last_obj_num * 5` on one path
    // even when fewer records follow, which a strict reader would reject —
    // it survives only because PDFium's own reader re-scans. We do not
    // reproduce that (divergence D11).
    out.extend_from_slice(payload.len().to_string().as_bytes());
    out.extend_from_slice(b">>stream\r\n");
    out.extend_from_slice(&payload);
    out.extend_from_slice(b"\r\nendstream\r\nendobj\r\n");
}

/// The `startxref` tail every file ends with.
pub(crate) fn write_tail(out: &mut Vec<u8>, xref_start: u64) {
    out.extend_from_slice(b"\r\nstartxref\r\n");
    out.extend_from_slice(xref_start.to_string().as_bytes());
    out.extend_from_slice(b"\r\n%%EOF\r\n");
}

/// A trailer for a document that never had one — a catalog reference, and an
/// `/Info` when there is one to name.
pub(crate) fn from_scratch(root: u32, info: Option<u32>) -> Dict {
    let mut out = Dict::new();
    out.push(
        names::ROOT.clone(),
        Object::Ref(pdfrum_object::ObjRef::new(root, 0)),
    );
    if let Some(info) = info {
        out.push(
            names::INFO.clone(),
            Object::Ref(pdfrum_object::ObjRef::new(info, 0)),
        );
    }
    out
}

/// Whether a key survives the trailer copy.
#[must_use]
pub(crate) fn is_suppressed(key: &Name) -> bool {
    SUPPRESSED_TRAILER_KEYS.contains(&key)
}

#[cfg(test)]
mod tests {
    use super::{TrailerParts, build, is_suppressed, write_classic, write_stream, write_tail};
    use crate::write::xref::ObjectOffsets;
    use pdfrum_object::{Array, Dict, Name, ObjRef, Object, PdfString, names};

    fn source() -> Dict {
        Dict::from_pairs([
            (names::ROOT.clone(), Object::Ref(ObjRef::new(11, 0))),
            (names::INFO.clone(), Object::Ref(ObjRef::new(9, 0))),
            (names::SIZE.clone(), Object::Int(999)),
            (names::PREV.clone(), Object::Int(4242)),
            (names::ENCRYPT.clone(), Object::Ref(ObjRef::new(5, 0))),
            (names::TYPE.clone(), Object::Name(names::XREF.clone())),
            (names::W.clone(), Object::Int(1)),
        ])
    }

    fn id() -> Array {
        Array::of([
            Object::Str(PdfString::hex(b"0123456789ABCDEF")),
            Object::Str(PdfString::hex(b"FEDCBA9876543210")),
        ])
    }

    fn text(f: impl FnOnce(&mut Vec<u8>)) -> String {
        let mut out = Vec::new();
        f(&mut out);
        String::from_utf8_lossy(&out).into_owned()
    }

    #[test]
    fn the_suppressed_keys_are_dropped_and_recomputed() {
        let src = source();
        let ids = id();
        let out = build(TrailerParts {
            source: &src,
            id: &ids,
            last_object_number: 35,
            prev: None,
            encrypt: None,
        });
        // Copied through.
        assert_eq!(out.reference(names::ROOT), Some(ObjRef::new(11, 0)));
        assert_eq!(out.reference(names::INFO), Some(ObjRef::new(9, 0)));
        // Recomputed, not copied: /Size says 36, not 999.
        assert_eq!(out.direct_int(names::SIZE), Some(36));
        // Dropped entirely.
        assert!(!out.contains_key(names::PREV));
        assert!(!out.contains_key(names::ENCRYPT));
        assert!(!out.contains_key(names::TYPE));
        assert!(!out.contains_key(names::W));
    }

    // Bug1328389: a trailer key no specification defines survives.
    #[test]
    fn an_unrecognised_key_survives() {
        let src = Dict::from_pairs([
            (names::ROOT.clone(), Object::Ref(ObjRef::new(1, 0))),
            (Name::from("Foo"), Object::Name(Name::from(""))),
        ]);
        let ids = id();
        let out = build(TrailerParts {
            source: &src,
            id: &ids,
            last_object_number: 1,
            prev: None,
            encrypt: None,
        });
        let bytes = text(|o| write_classic(o, &out));
        assert!(bytes.contains("/Foo/"), "got {bytes}");
    }

    #[test]
    fn prev_is_written_only_when_asked_for() {
        let src = source();
        let ids = id();
        let with = build(TrailerParts {
            source: &src,
            id: &ids,
            last_object_number: 1,
            prev: Some(1234),
            encrypt: None,
        });
        assert_eq!(with.direct_int(names::PREV), Some(1234));
    }

    #[test]
    fn encrypt_names_the_object_it_was_written_as() {
        let src = source();
        let ids = id();
        let out = build(TrailerParts {
            source: &src,
            id: &ids,
            last_object_number: 40,
            prev: None,
            encrypt: Some(41),
        });
        assert_eq!(out.reference(names::ENCRYPT), Some(ObjRef::new(41, 0)));
    }

    // Bug873's shape, modulo dict order (SPEC §2): the reference's trailing
    // space separates the next key, and /ID comes last.
    #[test]
    fn a_classic_trailer_reads_the_way_the_golden_does() {
        let src = Dict::from_pairs([
            (names::INFO.clone(), Object::Ref(ObjRef::new(9, 0))),
            (names::ROOT.clone(), Object::Ref(ObjRef::new(11, 0))),
        ]);
        let ids = Array::of([
            Object::Str(PdfString::hex(
                b"\xD8\x89\xEB\x6B\x9A\xDF\x88\xE5\xED\xA7\xDC\x08\xFE\x85\x97\x8B",
            )),
            Object::Str(PdfString::hex([0u8; 16])),
        ]);
        let out = build(TrailerParts {
            source: &src,
            id: &ids,
            last_object_number: 35,
            prev: None,
            encrypt: None,
        });
        let bytes = text(|o| write_classic(o, &out));
        assert!(bytes.starts_with("trailer\r\n<<"));
        assert!(bytes.contains("/Info 9 0 R "));
        assert!(bytes.contains("/Root 11 0 R "));
        assert!(bytes.contains("/Size 36"));
        assert!(bytes.contains("/ID[<D889EB6B9ADF88E5EDA7DC08FE85978B><"));
        assert!(bytes.ends_with(">]>>"), "got {bytes}");
    }

    #[test]
    fn the_tail_names_the_offset_and_ends_the_file() {
        assert_eq!(
            text(|o| write_tail(o, 1234)),
            "\r\nstartxref\r\n1234\r\n%%EOF\r\n"
        );
    }

    // The stream form declares its own /Length truthfully, unlike the C++'s
    // rebuilt-table path (D11).
    #[test]
    fn an_xref_stream_declares_the_true_record_length() {
        let src = Dict::from_pairs([(names::ROOT.clone(), Object::Ref(ObjRef::new(1, 0)))]);
        let ids = id();
        let trailer = build(TrailerParts {
            source: &src,
            id: &ids,
            last_object_number: 4,
            prev: None,
            encrypt: None,
        });
        let mut offsets = ObjectOffsets::new();
        offsets.set(2, 0x10);
        offsets.set(3, 0x20);

        let mut out = Vec::new();
        write_stream(&mut out, 6, &trailer, &offsets, &[2, 3]);
        let bytes = String::from_utf8_lossy(&out).into_owned();

        assert!(bytes.starts_with("6 0 obj <<"));
        assert!(bytes.contains("/Type/XRef"));
        assert!(bytes.contains("/W[0 4 1]"));
        assert!(bytes.contains("/Index[2 1 3 1 ]"));
        // Two records of five bytes each.
        assert!(bytes.contains("/Length 10>>stream\r\n"));
        // /Size counts one higher than the classic form: the trailer object
        // is itself in the table.
        assert!(bytes.contains("/Size 6"));
        assert!(out.ends_with(b"\r\nendstream\r\nendobj\r\n"));
        // The records themselves: big-endian offset then a zero generation.
        assert!(out.windows(5).any(|w| w == [0, 0, 0, 0x10, 0]));
        assert!(out.windows(5).any(|w| w == [0, 0, 0, 0x20, 0]));
    }

    #[test]
    fn a_from_scratch_trailer_names_the_catalog() {
        let out = super::from_scratch(1, Some(2));
        assert_eq!(out.reference(names::ROOT), Some(ObjRef::new(1, 0)));
        assert_eq!(out.reference(names::INFO), Some(ObjRef::new(2, 0)));
        assert!(!super::from_scratch(1, None).contains_key(names::INFO));
    }

    #[test]
    fn suppression_answers_for_each_of_the_eleven() {
        for key in [
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
        ] {
            assert!(is_suppressed(key), "{key:?}");
        }
        assert!(!is_suppressed(names::ROOT));
        assert!(!is_suppressed(names::INFO));
    }
}
