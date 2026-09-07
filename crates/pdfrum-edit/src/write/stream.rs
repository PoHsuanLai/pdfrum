//! What happens to a stream's payload on the way out (ISO 32000-1 §7.3.8).
//!
//! # The four-row decision table
//!
//! Two questions decide everything: does the stream already declare a
//! `/Filter`, and does the writer want to flate-encode it?
//!
//! | has filter | want flate | payload | dictionary |
//! |---|---|---|---|
//! | yes | yes | raw, verbatim | untouched — an already-compressed stream is copied whole, filters and all |
//! | yes | no | raw, verbatim | `/Filter` **removed** |
//! | no | yes | deflated | `/Filter /FlateDecode`, `/Length` updated, `/DecodeParms` removed |
//! | no | no | raw, verbatim | untouched |
//!
//! Row 1 is why an untouched image survives a save bit-for-bit: the
//! compressed bytes never go through a decode/encode cycle, so no codec's
//! rounding can change them.
//!
//! Row 2 is the odd one — it strips `/Filter` while writing bytes that are
//! still filtered, so the dictionary lies about the payload. It is reachable
//! only for a metadata stream, whose caller has already decided not to touch
//! the bytes; we keep it because that is what an XMP packet copied out of a
//! compressed source looks like in the oracle's output too.
//!
//! # Metadata is exempt from compression, and from encryption on request
//!
//! A `/Type /Metadata /Subtype /XML` stream is never compressed: ISO 32000-1
//! §14.3.2 requires an XMP packet be readable by a consumer that has no flate
//! decoder.
//!
//! Encryption is the document's own call, through `/EncryptMetadata`. A
//! document declaring it **false** gets a plaintext packet — which is the
//! whole point of the flag, and matches what the parser then expects to find.
//! A document declaring it **true** gets an enciphered one.
//!
//! This is a deliberate divergence. The oracle skips the cipher for a
//! metadata stream *unconditionally*, without consulting the flag, so it
//! writes a file whose `/Encrypt` says the metadata is enciphered and whose
//! metadata is not — which its own reader then deciphers into rubbish on the
//! way back in. Reproducing that would mean knowingly destroying a document's
//! metadata on every save; we follow the flag instead, which the oracle's
//! *reader* honours, so the file it writes and the file we write are both
//! openable by both — ours simply still has its metadata.
//!
//! # `/Length` is never allowed to be wrong
//!
//! Whatever row runs, the emitted `/Length` equals the byte count between
//! `stream\r\n` and `\r\nendstream`. The one subtlety is that
//! an unchanged `/Length` is left alone rather than rewritten, which is what
//! keeps `bug_905142.pdf`'s empty `/Filter /FlateDecode /Length 0` stream
//! declaring `/Length 0` on the way out.

use crate::encrypt::Encryptor;
use pdfrum_crypt::CryptClass;
use pdfrum_filters::encode_flate;
use pdfrum_object::{Dict, Name, Object, Stream, names};

/// A stream's payload and the dictionary that describes it, after the table
/// above has run.
pub(crate) struct Encoded {
    /// The dictionary to write, `/Length` already correct.
    pub(crate) dict: Dict,
    /// The bytes between `stream\r\n` and `\r\nendstream`.
    pub(crate) data: Vec<u8>,
}

/// Run the decision table over one stream.
pub(crate) fn encode(s: &Stream, enc: Option<&Encryptor<'_>>) -> Encoded {
    let metadata = is_metadata(&s.dict);
    let has_filter = s.dict.contains_key(names::FILTER);
    let want_flate = !metadata;
    // The document decides whether its own metadata is enciphered; see the
    // module docs for why. `CPDF_Stream::WriteTo` never consults
    // `IsMetadataEncrypted()` — it has callers only in the parser — so the
    // C++ skips the cipher unconditionally here.
    let want_cipher = !metadata || enc.is_some_and(Encryptor::encrypts_metadata);

    // Three of the four rows copy the payload verbatim and differ only in
    // what they say about it; the fourth is the only compression this crate
    // performs.
    let (mut dict, mut data) = match (has_filter, want_flate) {
        (false, true) => {
            let encoded = encode_flate(s.data.as_bytes());
            let mut d = without(&s.dict, names::DECODE_PARMS);
            set(
                &mut d,
                names::FILTER,
                Object::Name(names::FLATE_DECODE.clone()),
            );
            (d, encoded)
        }
        // The lying row: the bytes stay filtered while `/Filter` goes, so
        // the dictionary describes a payload that is not there. Reachable
        // only for a metadata stream, whose caller has already decided not
        // to touch the bytes.
        (true, false) => (without(&s.dict, names::FILTER), s.data.as_bytes().to_vec()),
        // Verbatim, dictionary and all: an already-compressed stream that
        // stays compressed, and an uncompressed one that stays uncompressed.
        (true, true) | (false, false) => (s.dict.clone(), s.data.as_bytes().to_vec()),
    };

    // Encryption happens after encoding and before /Length is settled, so the
    // declared length describes the ciphertext the file actually holds.
    if let Some(e) = enc
        && want_cipher
    {
        data = e.encrypt(CryptClass::Stream, &data);
    }

    update_length(&mut dict, data.len());
    Encoded { dict, data }
}

/// `/Type /Metadata` with `/Subtype /XML`.
fn is_metadata(dict: &Dict) -> bool {
    dict.name(names::TYPE).is_some_and(|n| n == names::METADATA)
        && dict.name(names::SUBTYPE).is_some_and(|n| n == names::XML)
}

fn without(dict: &Dict, key: &Name) -> Dict {
    Dict::from_pairs(
        dict.iter()
            .filter(|(k, _)| k != key)
            .map(|(k, v)| (k.clone(), v.clone())),
    )
}

/// Set a key, replacing in place so the insertion order — and therefore the
/// emitted key order — does not shuffle on a rewrite.
fn set(dict: &mut Dict, key: &Name, value: Object) {
    let existing: Vec<(Name, Object)> = dict.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    if existing.iter().any(|(k, _)| k == key) {
        *dict = Dict::from_pairs(existing.into_iter().map(|(k, v)| {
            if &k == key {
                (k, value.clone())
            } else {
                (k, v)
            }
        }));
    } else {
        dict.push(key.clone(), value);
    }
}

/// Make `/Length` say `len` — but leave it alone when it already does, so a
/// dictionary that needed no change keeps the spelling it arrived with.
fn update_length(dict: &mut Dict, len: usize) {
    let want = i64::try_from(len).unwrap_or(i64::MAX);
    if dict.direct_int(names::LENGTH) == Some(want) {
        return;
    }
    set(dict, names::LENGTH, Object::Int(want));
}

#[cfg(test)]
mod tests {
    use super::encode;
    use pdfrum_common::{Diagnostics, Limits};
    use pdfrum_filters::decode_flate;
    use pdfrum_object::{ByteSpan, Dict, Name, Object, Stream, names};
    use std::sync::Arc;

    fn stream(pairs: impl IntoIterator<Item = (Name, Object)>, data: &[u8]) -> Stream {
        let file: Arc<[u8]> = Arc::from(data);
        Stream::new(Dict::from_pairs(pairs), ByteSpan::whole(file))
    }

    #[test]
    fn row_filter_present_copies_the_stream_verbatim() {
        let s = stream(
            [
                (
                    names::FILTER.clone(),
                    Object::Name(names::FLATE_DECODE.clone()),
                ),
                (names::LENGTH.clone(), Object::Int(5)),
            ],
            b"\x01\x02\x03\x04\x05",
        );
        let out = encode(&s, None);
        assert_eq!(out.data, b"\x01\x02\x03\x04\x05");
        // Dictionary untouched: the filter travels with the bytes.
        assert_eq!(
            out.dict.name(names::FILTER),
            Some(names::FLATE_DECODE),
            "an already-compressed stream keeps its filter"
        );
        assert_eq!(out.dict.direct_int(names::LENGTH), Some(5));
    }

    // Bug905142: an empty stream declaring /Filter /FlateDecode /Length 0
    // takes the verbatim row, and /Length 0 survives untouched.
    #[test]
    fn an_empty_filtered_stream_keeps_length_zero() {
        let s = stream(
            [
                (
                    names::FILTER.clone(),
                    Object::Name(names::FLATE_DECODE.clone()),
                ),
                (names::LENGTH.clone(), Object::Int(0)),
            ],
            b"",
        );
        let out = encode(&s, None);
        assert!(out.data.is_empty());
        assert_eq!(out.dict.direct_int(names::LENGTH), Some(0));
    }

    #[test]
    fn row_no_filter_compresses_and_rewrites_the_dictionary() {
        let payload = vec![b'x'; 4000];
        let s = stream(
            [
                (names::LENGTH.clone(), Object::Int(4000)),
                (
                    names::DECODE_PARMS.clone(),
                    Object::Dict(Dict::from_pairs([(names::COLUMNS.clone(), Object::Int(4))])),
                ),
            ],
            &payload,
        );
        let out = encode(&s, None);

        assert_eq!(out.dict.name(names::FILTER), Some(names::FLATE_DECODE));
        assert!(
            !out.dict.contains_key(names::DECODE_PARMS),
            "/DecodeParms described the old payload and must go"
        );
        assert_eq!(
            out.dict.direct_int(names::LENGTH),
            Some(i64::try_from(out.data.len()).expect("fits")),
        );
        assert!(out.data.len() < payload.len());

        let round = decode_flate(
            &out.data,
            0,
            &Limits::default(),
            &mut Diagnostics::default(),
        )
        .expect("our own output decodes");
        assert_eq!(round, payload);
    }

    // Metadata takes the "want_flate = false" branch, so a metadata stream
    // that already had a filter comes out with /Filter stripped (the lying
    // row) and one that did not comes out uncompressed.
    #[test]
    fn a_metadata_stream_is_never_compressed() {
        let meta = [
            (names::TYPE.clone(), Object::Name(names::METADATA.clone())),
            (names::SUBTYPE.clone(), Object::Name(names::XML.clone())),
            (names::LENGTH.clone(), Object::Int(9)),
        ];
        let out = encode(&stream(meta.clone(), b"<x:xmpmeta/>"), None);
        assert!(!out.dict.contains_key(names::FILTER));
        assert_eq!(out.data, b"<x:xmpmeta/>");
        assert_eq!(out.dict.direct_int(names::LENGTH), Some(12));

        let mut filtered: Vec<_> = meta.to_vec();
        filtered.push((
            names::FILTER.clone(),
            Object::Name(names::FLATE_DECODE.clone()),
        ));
        let out = encode(&stream(filtered, b"compressed"), None);
        assert!(
            !out.dict.contains_key(names::FILTER),
            "row 2 strips the filter it will not honour"
        );
    }

    #[test]
    fn length_always_describes_the_emitted_bytes() {
        for (pairs, data) in [
            (vec![], &b"abc"[..]),
            (vec![(names::LENGTH.clone(), Object::Int(999))], &b"abc"[..]),
            (
                vec![(
                    names::FILTER.clone(),
                    Object::Name(names::FLATE_DECODE.clone()),
                )],
                &b"\x01\x02"[..],
            ),
        ] {
            let out = encode(&stream(pairs, data), None);
            assert_eq!(
                out.dict.direct_int(names::LENGTH),
                Some(i64::try_from(out.data.len()).expect("fits")),
            );
        }
    }

    #[test]
    fn rewriting_length_does_not_shuffle_the_key_order() {
        let s = stream(
            [
                (names::TYPE.clone(), Object::Name(Name::from("Thing"))),
                (names::LENGTH.clone(), Object::Int(999)),
                (names::N.clone(), Object::Int(4)),
            ],
            b"abc",
        );
        let out = encode(&s, None);
        let keys: Vec<&Name> = out.dict.keys().collect();
        // The three original keys keep their positions; `/Filter` is new and
        // so appends. What must not happen is `/Length` moving to the end
        // because it was rewritten.
        assert_eq!(
            keys,
            vec![names::TYPE, names::LENGTH, names::N, names::FILTER]
        );
    }
}
