//! Per-type object serialization (ISO 32000 §7.3), and the indirect-object
//! frame that wraps it.
//!
//! # Tokens carry their own separators
//!
//! Arrays and dictionaries write **no separators of their own**. That reads
//! like a bug and is not: numbers, booleans, nulls and references all carry a
//! *leading* space, references additionally a trailing one, and names and
//! strings are self-delimiting. So `[1 2 3]` comes out `[ 1 2 3]` and
//! `<</A 1/B/C>>` comes out unchanged. Keeping the convention matters because
//! it is what makes a trailer read `/Info 9 0 R /Root 11 0 R /Size 36/ID[…` —
//! the space before `/Root` is the *reference's* trailing space, and there is
//! none before `/ID` because `36` has no trailing space.
//!
//! # What the encryptor is allowed to touch
//!
//! Two exemptions are mandatory rather than stylistic. A signature
//! dictionary's `/Contents` covers a byte range of the finished file, so
//! re-enciphering it would invalidate the signature; and an XMP metadata
//! stream must be readable without the file key (ISO 32000-1 §14.3.2). Both
//! are threaded as an [`Exempt`] flag rather than as a special case inside the
//! cipher, so the rule is visible at the site that knows the context.

use crate::encrypt::Encryptor;
use pdfrum_crypt::CryptClass;
use pdfrum_object::{
    Array, Dict, Name, Object, PdfString, Stream, encode_string_hex, encode_string_literal,
    fmt_int, fmt_number, name_encode, names,
};

/// Whether the value being written is one the security handler must not
/// touch (a signature's `/Contents`, a metadata stream's payload).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Exempt {
    /// Encrypt normally.
    No,
    /// Write the bytes as they stand.
    Yes,
}

/// Serialize one object's bytes, appending to `out`.
///
/// `enc` is the encryptor for the *enclosing indirect object*; direct
/// sub-objects share it, which is why it is threaded down rather than looked
/// up per value.
pub(crate) fn write_object(out: &mut Vec<u8>, obj: &Object, enc: Option<&Encryptor<'_>>) {
    match obj {
        Object::Null => out.extend_from_slice(b" null"),
        Object::Bool(b) => {
            out.push(b' ');
            out.extend_from_slice(if *b { b"true" } else { b"false" });
        }
        Object::Int(i) => {
            out.push(b' ');
            out.extend_from_slice(fmt_int(*i).as_bytes());
        }
        Object::Real(r) => {
            out.push(b' ');
            out.extend_from_slice(fmt_number(*r).as_bytes());
        }
        Object::Str(s) => write_string(out, s, enc, Exempt::No),
        Object::Name(n) => write_name(out, n),
        Object::Array(a) => write_array(out, a, enc),
        Object::Dict(d) => write_dict(out, d, enc),
        Object::Stream(s) => write_stream(out, s, enc),
        // Generation is always 0 on output: every object is renumbered to
        // generation 0 by the writer, so every reference must name it.
        Object::Ref(r) => {
            out.push(b' ');
            out.extend_from_slice(r.num.to_string().as_bytes());
            out.extend_from_slice(b" 0 R ");
        }
    }
}

/// `/Name`, with `#xx` escapes. An encoding that comes out empty writes a
/// bare `/`, which is a legal (if useless) name.
pub(crate) fn write_name(out: &mut Vec<u8>, name: &Name) {
    out.push(b'/');
    out.extend_from_slice(&name_encode(name.as_bytes()));
}

fn write_string(out: &mut Vec<u8>, s: &PdfString, enc: Option<&Encryptor<'_>>, exempt: Exempt) {
    let bytes = match (enc, exempt) {
        (Some(e), Exempt::No) => e.encrypt(CryptClass::String, s.as_bytes()),
        _ => s.as_bytes().to_vec(),
    };
    // The spelling round-trips: a file that wrote `<48656C6C6F>` gets it back.
    if s.is_hex() {
        out.extend_from_slice(&encode_string_hex(&bytes));
    } else {
        out.extend_from_slice(&encode_string_literal(&bytes));
    }
}

fn write_array(out: &mut Vec<u8>, a: &Array, enc: Option<&Encryptor<'_>>) {
    out.push(b'[');
    for value in a.iter() {
        write_object(out, value, enc);
    }
    out.push(b']');
}

/// A dictionary, keys in **insertion order** — not sorted.
pub(crate) fn write_dict(out: &mut Vec<u8>, d: &Dict, enc: Option<&Encryptor<'_>>) {
    let signature = is_signature_dict(d);
    out.extend_from_slice(b"<<");
    for (key, value) in d.iter() {
        write_name(out, key);
        // A signature's /Contents is written in the clear even though its
        // siblings are enciphered.
        if signature
            && key == names::CONTENTS
            && let Object::Str(s) = value
        {
            write_string(out, s, enc, Exempt::Yes);
            continue;
        }
        write_object(out, value, enc);
    }
    out.extend_from_slice(b">>");
}

/// A dictionary is a signature dictionary when its `/Type` — or, absent that,
/// its `/FT` — spells `Sig`.
fn is_signature_dict(d: &Dict) -> bool {
    let key = if d.contains_key(names::TYPE) {
        names::TYPE
    } else {
        names::FT
    };
    d.name(key).is_some_and(|n| n.as_bytes() == b"Sig")
}

/// A stream: its dictionary, then `stream\r\n`, the payload, `\r\nendstream`.
///
/// The payload has already been through [`crate::write::stream::encode`],
/// whose decision table owns the flate/metadata rules; this function only
/// lays out the bytes.
fn write_stream(out: &mut Vec<u8>, s: &Stream, enc: Option<&Encryptor<'_>>) {
    let encoded = crate::write::stream::encode(s, enc);
    write_dict(out, &encoded.dict, enc);
    out.extend_from_slice(b"stream\r\n");
    out.extend_from_slice(&encoded.data);
    out.extend_from_slice(b"\r\nendstream");
}

/// The `N 0 obj … endobj` frame.
///
/// The generation is the literal `0` for every object the writer emits: the
/// xref entries say `00000` to match, and every reference written says
/// `N 0 R`. Generations are read from a file and never written back — the
/// single most important round-trip simplification.
pub(crate) fn write_indirect(
    out: &mut Vec<u8>,
    num: u32,
    obj: &Object,
    enc: Option<&Encryptor<'_>>,
) {
    out.extend_from_slice(num.to_string().as_bytes());
    out.extend_from_slice(b" 0 obj\r\n");
    write_object(out, obj, enc);
    out.extend_from_slice(b"\r\nendobj\r\n");
}

#[cfg(test)]
mod tests {
    use super::{write_indirect, write_object};
    use pdfrum_object::{Array, Dict, Name, ObjRef, Object, PdfString, names};

    fn bytes(obj: &Object) -> Vec<u8> {
        let mut out = Vec::new();
        write_object(&mut out, obj, None);
        out
    }

    fn text(obj: &Object) -> String {
        String::from_utf8_lossy(&bytes(obj)).into_owned()
    }

    #[test]
    fn scalars_carry_a_leading_space() {
        assert_eq!(text(&Object::Null), " null");
        assert_eq!(text(&Object::Bool(true)), " true");
        assert_eq!(text(&Object::Bool(false)), " false");
        assert_eq!(text(&Object::Int(1245)), " 1245");
        assert_eq!(text(&Object::Real(9.003_45)), " 9.00345");
    }

    // An integer beyond i32::MAX spells through the C-int view, the same one
    // every reader uses (pdfrum-object's `fmt_int`).
    #[test]
    fn integers_spell_through_the_c_int_view() {
        assert_eq!(text(&Object::Int(4_294_967_295)), " -1");
    }

    #[test]
    fn a_reference_is_bracketed_by_spaces_and_always_generation_zero() {
        assert_eq!(text(&Object::Ref(ObjRef::new(9, 0))), " 9 0 R ");
        // The generation the file recorded is dropped on output.
        assert_eq!(text(&Object::Ref(ObjRef::new(9, 42))), " 9 0 R ");
    }

    #[test]
    fn names_escape_and_may_be_empty() {
        assert_eq!(text(&Object::Name(Name::from("Simple"))), "/Simple");
        assert_eq!(
            text(&Object::Name(Name::from("Property Name With Space"))),
            "/Property#20Name#20With#20Space"
        );
        assert_eq!(text(&Object::Name(Name::from(""))), "/");
        assert_eq!(text(&Object::Name(Name::from("A#B"))), "/A#23B");
    }

    #[test]
    fn strings_keep_the_spelling_they_were_parsed_with() {
        assert_eq!(text(&Object::Str(PdfString::literal(b"hi"))), "(hi)");
        assert_eq!(text(&Object::Str(PdfString::hex(b"\x12\xAC"))), "<12AC>");
    }

    // The no-separator rule: the elements' own leading spaces do the work.
    #[test]
    fn arrays_and_dicts_write_no_separators() {
        let arr = Object::Array(Array::of([Object::Int(1), Object::Int(2), Object::Int(3)]));
        assert_eq!(text(&arr), "[ 1 2 3]");

        let dict = Object::Dict(Dict::from_pairs([
            (Name::from("A"), Object::Int(1)),
            (Name::from("B"), Object::Name(Name::from("C"))),
        ]));
        assert_eq!(text(&dict), "<</A 1/B/C>>");
    }

    // Bug873's trailer shape in miniature: a reference's trailing space is
    // what separates it from the next key, and an integer has none.
    #[test]
    fn reference_trailing_space_separates_the_next_key() {
        let dict = Object::Dict(Dict::from_pairs([
            (names::INFO.clone(), Object::Ref(ObjRef::new(9, 0))),
            (names::ROOT.clone(), Object::Ref(ObjRef::new(11, 0))),
            (names::SIZE.clone(), Object::Int(36)),
            (names::ID.clone(), Object::Array(Array::new())),
        ]));
        assert_eq!(text(&dict), "<</Info 9 0 R /Root 11 0 R /Size 36/ID[]>>");
    }

    #[test]
    fn dict_keys_keep_insertion_order() {
        let dict = Object::Dict(Dict::from_pairs([
            (Name::from("Z"), Object::Int(1)),
            (Name::from("A"), Object::Int(2)),
            (Name::from("M"), Object::Int(3)),
        ]));
        assert_eq!(text(&dict), "<</Z 1/A 2/M 3>>");
    }

    #[test]
    fn indirect_frame_is_always_generation_zero() {
        let mut out = Vec::new();
        write_indirect(&mut out, 7, &Object::Int(1), None);
        assert_eq!(String::from_utf8_lossy(&out), "7 0 obj\r\n 1\r\nendobj\r\n");
    }
}
