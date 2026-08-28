//! Getting a stream's real bytes.
//!
//! The decoding itself lives in `pdfrum-filters`, including the fallback
//! ladder that turns every failure into the undecoded bytes. This module is
//! only the parser's side of that boundary: two places inside the reader —
//! cross-reference streams and object streams — need decoded bytes before
//! anything else can happen, and consumers above need them on demand.

use pdfrum_common::{Diagnostics, Limits};
use pdfrum_object::{Resolve, Stream};

/// A stream's data with its filters applied.
///
/// Never fails: a stream whose filter chain is unusable yields its raw bytes
/// and a diagnostic, which is what makes a damaged file's content still
/// render. A chain ending in an image codec yields the codec's *input*; the
/// image path takes it from there.
///
/// ```
/// use pdfrum_common::{Diagnostics, Limits};
/// use pdfrum_object::{ByteSpan, Dict, Name, NoResolve, Object, Stream, names};
/// use pdfrum_parser::decoded_stream;
///
/// let dict = Dict::from_pairs([(
///     names::FILTER.clone(),
///     Object::Name(Name::from("ASCIIHexDecode")),
/// )]);
/// let stream = Stream::new(dict, ByteSpan::from(b"48656c6c6f>".to_vec()));
/// let mut diags = Diagnostics::default();
/// let bytes = decoded_stream(&stream, &NoResolve, &Limits::default(), &mut diags);
/// assert_eq!(bytes, b"Hello");
/// ```
#[must_use]
pub fn decoded_stream(
    stream: &Stream,
    r: &impl Resolve,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> Vec<u8> {
    decoded_bytes(stream, r, limits, diags)
}

/// The same, over a `?Sized` resolver so the store can call it behind a
/// reference.
pub(crate) fn decoded_bytes<R: Resolve + ?Sized>(
    stream: &Stream,
    r: &R,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> Vec<u8> {
    pdfrum_filters::decode_chain(stream, 0, &r, limits, diags).data
}

/// Decoded bytes for a stream the *reader itself* has to understand — a
/// cross-reference stream or an object stream.
///
/// Unlike a content stream, these are useless unless the whole chain
/// produced real bytes. A chain that stops at an image codec has handed back
/// that codec's input rather than the fields the reader is looking for, so
/// this answers `None` and the caller falls through to its next repair
/// instead of reading pixels as offsets.
pub(crate) fn structural_bytes<R: Resolve + ?Sized>(
    stream: &Stream,
    r: &R,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> Option<Vec<u8>> {
    let decoded = pdfrum_filters::decode_chain(stream, 0, &r, limits, diags);
    decoded.image.is_none().then_some(decoded.data)
}

#[cfg(test)]
mod tests {
    use super::decoded_stream;
    use pdfrum_common::{DiagKind, Diagnostics, Limits};
    use pdfrum_object::{ByteSpan, Dict, Name, NoResolve, Object, Stream, names};

    #[test]
    fn an_unfiltered_stream_is_its_own_data() {
        let stream = Stream::new(Dict::new(), ByteSpan::from(b"raw".to_vec()));
        let mut diags = Diagnostics::default();
        assert_eq!(
            decoded_stream(&stream, &NoResolve, &Limits::default(), &mut diags),
            b"raw"
        );
    }

    #[test]
    fn an_unusable_filter_declaration_yields_the_raw_bytes() {
        let dict = Dict::from_pairs([(names::FILTER.clone(), Object::Int(7))]);
        let stream = Stream::new(dict, ByteSpan::from(b"raw".to_vec()));
        let mut diags = Diagnostics::default();
        assert_eq!(
            decoded_stream(&stream, &NoResolve, &Limits::default(), &mut diags),
            b"raw"
        );
        assert!(diags.contains(&DiagKind::UndecodableStream));
    }

    #[test]
    fn a_hex_filter_decodes() {
        let dict = Dict::from_pairs([(
            names::FILTER.clone(),
            Object::Name(Name::from("ASCIIHexDecode")),
        )]);
        let stream = Stream::new(dict, ByteSpan::from(b"414243>".to_vec()));
        let mut diags = Diagnostics::default();
        assert_eq!(
            decoded_stream(&stream, &NoResolve, &Limits::default(), &mut diags),
            b"ABC"
        );
    }
}
