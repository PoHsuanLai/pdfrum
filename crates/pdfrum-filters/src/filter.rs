//! A stream filter, as named by `/Filter` (ISO 32000-1 table 6), and applying
//! one filter to bytes.

use pdfrum_common::{Diagnostics, Limits};
use pdfrum_object::{Dict, Name, Resolve, names};

use crate::error::Error;
use crate::{
    PredictorParams, decode_ascii_hex, decode_ascii85, decode_flate, decode_lzw, decode_run_length,
    predictor,
};

/// A stream filter, as named by `/Filter` (ISO 32000-1 table 6).
///
/// The variants this crate decodes and the ones it hands on to the image path
/// are all here; [`Filter::is_image_codec`] separates them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Filter {
    /// `/FlateDecode` — deflate, optionally followed by a predictor.
    Flate,
    /// `/LZWDecode` — LZW, optionally followed by a predictor.
    Lzw,
    /// `/ASCIIHexDecode` — hexadecimal text.
    AsciiHex,
    /// `/ASCII85Decode` — base-85 text.
    Ascii85,
    /// `/RunLengthDecode` — byte-oriented run-length compression.
    RunLength,
    /// `/CCITTFaxDecode` — Group 3/4 fax; decoded from the image path, which
    /// knows the image's width and height.
    CcittFax,
    /// `/JBIG2Decode` — bi-level image compression.
    Jbig2,
    /// `/DCTDecode` — baseline JPEG.
    Dct,
    /// `/JPXDecode` — JPEG 2000.
    Jpx,
    /// `/Crypt` — decryption already performed by the security handler; the
    /// filter itself is the identity here.
    Crypt,
}

impl Filter {
    /// Classify a `/Filter` name, accepting the inline-image abbreviations
    /// (`/Fl`, `/AHx`, `/A85`, `/RL`, `/LZW`, `/CCF`, `/DCT`).
    ///
    /// `None` is **not** an error: PDFium's filter dispatch is a chain of name
    /// comparisons whose final `else` treats every unrecognized name as an
    /// image codec to be resolved later, so a garbage `/FooBar` and a real
    /// `/JPXDecode` take the same path out of the chain executor. See
    /// [`decode_chain`](crate::decode_chain).
    ///
    /// ```
    /// use pdfrum_filters::Filter;
    /// use pdfrum_object::Name;
    ///
    /// assert_eq!(Filter::from_name(&Name::from("Fl")), Some(Filter::Flate));
    /// assert_eq!(Filter::from_name(&Name::from("FlateDecode")), Some(Filter::Flate));
    /// assert_eq!(Filter::from_name(&Name::from("FooBar")), None);
    /// ```
    #[must_use]
    pub fn from_name(n: &Name) -> Option<Filter> {
        // Both spellings of each filter, in ISO 32000-1 table 6's order.
        let table = [
            (names::FLATE_DECODE, Filter::Flate),
            (names::FL, Filter::Flate),
            (names::LZW_DECODE, Filter::Lzw),
            (names::LZW, Filter::Lzw),
            (names::ASCII85_DECODE, Filter::Ascii85),
            (names::A85, Filter::Ascii85),
            (names::ASCII_HEX_DECODE, Filter::AsciiHex),
            (names::AHX, Filter::AsciiHex),
            (names::RUN_LENGTH_DECODE, Filter::RunLength),
            (names::RL, Filter::RunLength),
            (names::CCITT_FAX_DECODE, Filter::CcittFax),
            (names::CCF, Filter::CcittFax),
            (names::DCT_DECODE, Filter::Dct),
            (names::DCT, Filter::Dct),
            (names::JPX_DECODE, Filter::Jpx),
            (names::JBIG2_DECODE, Filter::Jbig2),
            (names::CRYPT, Filter::Crypt),
        ];
        table
            .iter()
            .find_map(|(name, filter)| (*name == n).then_some(*filter))
    }

    /// The name a consumer dispatches on, with the abbreviations expanded.
    ///
    /// PDFium expands `/DCT` to `/DCTDecode` and `/CCF` to `/CCITTFaxDecode`
    /// before recording the codec name, so the image path never sees the short
    /// spelling; every other filter is spelled out here for the same reason.
    #[must_use]
    pub fn canonical_name(self) -> &'static Name {
        match self {
            Filter::Flate => names::FLATE_DECODE,
            Filter::Lzw => names::LZW_DECODE,
            Filter::AsciiHex => names::ASCII_HEX_DECODE,
            Filter::Ascii85 => names::ASCII85_DECODE,
            Filter::RunLength => names::RUN_LENGTH_DECODE,
            Filter::CcittFax => names::CCITT_FAX_DECODE,
            Filter::Jbig2 => names::JBIG2_DECODE,
            Filter::Dct => names::DCT_DECODE,
            Filter::Jpx => names::JPX_DECODE,
            Filter::Crypt => names::CRYPT,
        }
    }

    /// Whether the filter must be handed to the image path rather than decoded
    /// here: `/CCITTFaxDecode`, `/JBIG2Decode`, `/DCTDecode`, `/JPXDecode`.
    ///
    /// These four produce pixels, not bytes, and need the image dictionary's
    /// width, height and colour space to be interpreted at all.
    #[must_use]
    pub fn is_image_codec(self) -> bool {
        matches!(
            self,
            Filter::CcittFax | Filter::Jbig2 | Filter::Dct | Filter::Jpx
        )
    }

    /// Whether the filter may appear anywhere but last in a `/Filter` array.
    ///
    /// PDFium validates the shape of a multi-filter chain against exactly this
    /// set, so `/Filter [/Crypt /FlateDecode]` is rejected outright even
    /// though the chain executor would happily skip the `/Crypt`.
    #[must_use]
    pub fn is_chainable(self) -> bool {
        matches!(
            self,
            Filter::Flate | Filter::Lzw | Filter::Ascii85 | Filter::AsciiHex | Filter::RunLength
        )
    }
}

/// What one filter produced.
#[derive(Debug, Clone, PartialEq)]
pub enum DecodeOutput {
    /// Decoded bytes.
    Bytes(Vec<u8>),
    /// The chain reached a filter this crate does not decode; the image path
    /// takes over.
    Image(NeedsImageCodec),
}

/// A filter chain that ended at an image codec this crate does not decode.
///
/// The codec's *input* is not here: it is [`DecodedStream::data`](crate::DecodedStream::data), which is
/// already the right bytes whether earlier filters produced them or the codec
/// was the whole chain and the raw stream is what it should read.
// `PartialEq` only: `params` holds `Object`s, and `Object::Real(f32)` has no
// total equality.
#[derive(Debug, Clone, PartialEq)]
pub struct NeedsImageCodec {
    /// The filter, when the name is one we know. `None` for a name we do not
    /// know at all, which is how a stream declaring `/Filter /FooBar` reaches
    /// the image path — and where it finally fails, having no codec to match.
    pub filter: Option<Filter>,
    /// The name as written, with `/DCT` and `/CCF` expanded.
    pub name: Name,
    /// That filter's `/DecodeParms`, already resolved to a dictionary.
    pub params: Dict,
}

/// Apply one filter to `input`.
///
/// `params` is that filter's `/DecodeParms` entry; pass an empty [`Dict`] when
/// the stream has none. Filter *chains* are [`decode_chain`](crate::decode_chain)'s business. Flate
/// and LZW apply their `/Predictor` inside this call. The four image codecs
/// return [`DecodeOutput::Image`] rather than decoding, since they need an
/// image dictionary this function does not have; `/Crypt` is the identity,
/// because decryption happened in the security handler long before any filter.
///
/// # Errors
///
/// [`Error::OutputTooLarge`] past `limits.max_decoded_stream_len`,
/// [`Error::RunLengthTooLarge`] past run-length's own 20 MiB cap,
/// [`Error::BadPredictorParams`] / [`Error::SizeOverflow`] for a
/// `/DecodeParms` that cannot describe a row, and [`Error::LzwMalformed`] for
/// the two LZW malformations PDFium rejects. Everything else is recovered and
/// recorded in `diags`.
pub fn decode(
    filter: Filter,
    input: &[u8],
    params: &Dict,
    r: &impl Resolve,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> Result<DecodeOutput, Error> {
    let bytes = match filter {
        Filter::Crypt => input.to_vec(),
        Filter::AsciiHex => decode_ascii_hex(input).0,
        Filter::Ascii85 => decode_ascii85(input)?.0,
        Filter::RunLength => decode_run_length(input, diags)?.0,
        Filter::Flate => {
            let params = PredictorParams::from_dict(params, r)?;
            let raw = decode_flate(input, 0, limits, diags)?;
            predictor(raw, params)?
        }
        Filter::Lzw => {
            let predictor_params = PredictorParams::from_dict(params, r)?;
            let raw = decode_lzw(input, params_early_change(params, r), limits, diags)?;
            predictor(raw, predictor_params)?
        }
        Filter::CcittFax | Filter::Jbig2 | Filter::Dct | Filter::Jpx => {
            return Ok(DecodeOutput::Image(NeedsImageCodec {
                filter: Some(filter),
                name: filter.canonical_name().clone(),
                params: params.clone(),
            }));
        }
    };
    Ok(DecodeOutput::Bytes(bytes))
}

/// Read `/EarlyChange`, which defaults to 1 — both when the key is absent and
/// when there is no `/DecodeParms` at all.
fn params_early_change(params: &Dict, r: &impl Resolve) -> bool {
    params.int(names::EARLY_CHANGE, r).unwrap_or(1) != 0
}

/// A `/DecodeParms` entry, resolved to a dictionary.
///
/// Anything that is not a dictionary — a missing key, an array, a number —
/// reads as an empty one, so every filter sees its defaults. PDFium reaches
/// the same place by asking a possibly-null object for its dictionary.
pub(crate) fn params_dict(obj: Option<&pdfrum_object::Object>, r: &impl Resolve) -> Dict {
    let Some(obj) = obj else {
        return Dict::new();
    };
    obj.resolve(r)
        .ok()
        .and_then(|resolved| resolved.as_direct().and_then(|o| o.as_dict().cloned()))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{DecodeOutput, Filter, decode};
    use pdfrum_common::{Diagnostics, Limits};
    use pdfrum_object::{Dict, Name, NoResolve, names};

    fn bytes(out: DecodeOutput) -> Vec<u8> {
        match out {
            DecodeOutput::Bytes(b) => b,
            DecodeOutput::Image(need) => panic!("expected bytes, got {need:?}"),
        }
    }

    fn run(filter: Filter, input: &[u8]) -> Vec<u8> {
        let mut diags = Diagnostics::default();
        bytes(
            decode(
                filter,
                input,
                &Dict::new(),
                &NoResolve,
                &Limits::default(),
                &mut diags,
            )
            .expect("decodes"),
        )
    }

    #[test]
    fn every_spelling_of_every_filter_name() {
        let cases: [(&str, Option<Filter>); 20] = [
            ("FlateDecode", Some(Filter::Flate)),
            ("Fl", Some(Filter::Flate)),
            ("LZWDecode", Some(Filter::Lzw)),
            ("LZW", Some(Filter::Lzw)),
            ("ASCII85Decode", Some(Filter::Ascii85)),
            ("A85", Some(Filter::Ascii85)),
            ("ASCIIHexDecode", Some(Filter::AsciiHex)),
            ("AHx", Some(Filter::AsciiHex)),
            ("RunLengthDecode", Some(Filter::RunLength)),
            ("RL", Some(Filter::RunLength)),
            ("CCITTFaxDecode", Some(Filter::CcittFax)),
            ("CCF", Some(Filter::CcittFax)),
            ("DCTDecode", Some(Filter::Dct)),
            ("DCT", Some(Filter::Dct)),
            ("JPXDecode", Some(Filter::Jpx)),
            ("JBIG2Decode", Some(Filter::Jbig2)),
            ("Crypt", Some(Filter::Crypt)),
            // Unknown names classify as None, which the chain executor turns
            // into an image punt rather than an error.
            ("FooBar", None),
            ("FlateEncode", None),
            ("", None),
        ];
        for (spelling, expected) in cases {
            assert_eq!(
                Filter::from_name(&Name::from(spelling)),
                expected,
                "/{spelling}"
            );
        }
    }

    #[test]
    fn abbreviations_canonicalize_for_the_image_path() {
        assert_eq!(Filter::Dct.canonical_name(), names::DCT_DECODE);
        assert_eq!(Filter::CcittFax.canonical_name(), names::CCITT_FAX_DECODE);
        assert_eq!(Filter::Flate.canonical_name(), names::FLATE_DECODE);
    }

    #[test]
    fn image_codecs_and_chainable_filters_partition_the_enum() {
        let all = [
            Filter::Flate,
            Filter::Lzw,
            Filter::AsciiHex,
            Filter::Ascii85,
            Filter::RunLength,
            Filter::CcittFax,
            Filter::Jbig2,
            Filter::Dct,
            Filter::Jpx,
            Filter::Crypt,
        ];
        for f in all {
            // Crypt is neither: it decodes as the identity but may not sit in
            // the middle of a chain.
            assert!(!(f.is_image_codec() && f.is_chainable()), "{f:?}");
        }
        assert!(!Filter::Crypt.is_image_codec());
        assert!(!Filter::Crypt.is_chainable());
    }

    #[test]
    fn crypt_decodes_as_the_identity() {
        assert_eq!(
            run(Filter::Crypt, b"\x00\x01\xffplain"),
            b"\x00\x01\xffplain"
        );
    }

    #[test]
    fn image_codecs_punt_with_no_data_of_their_own() {
        let mut diags = Diagnostics::default();
        for f in [Filter::Dct, Filter::Jpx, Filter::Jbig2, Filter::CcittFax] {
            let out = decode(
                f,
                b"whatever",
                &Dict::new(),
                &NoResolve,
                &Limits::default(),
                &mut diags,
            )
            .expect("punting never fails");
            match out {
                DecodeOutput::Image(need) => {
                    assert_eq!(need.filter, Some(f));
                    assert_eq!(&need.name, f.canonical_name());
                }
                DecodeOutput::Bytes(b) => panic!("expected a punt, got {b:?}"),
            }
        }
    }

    #[test]
    fn the_text_filters_route_to_their_decoders() {
        assert_eq!(run(Filter::AsciiHex, b"48656C6C6F>"), b"Hello");
        assert_eq!(run(Filter::Ascii85, b"FCfN8~>"), b"test");
        assert_eq!(run(Filter::RunLength, &[0, b'x', 128]), b"x");
    }
}
