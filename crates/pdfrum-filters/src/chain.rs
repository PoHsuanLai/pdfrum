//! Walking a stream's `/Filter` chain, and what to do when it goes wrong.
//!
//! # The four fallbacks
//!
//! A PDF consumer never learns that a stream failed to decode. PDFium's stream
//! accessor has no error channel at all: it hands back bytes and the name of
//! an image codec, and four separate failures all resolve the same way, by
//! quietly substituting the **raw, undecoded** stream bytes.
//!
//! 1. `/Filter` is neither a name nor an array, or the chain's shape is
//!    invalid — raw bytes.
//! 2. There are no filters at all — raw bytes, which is simply correct.
//! 3. Some filter could not produce anything — raw bytes, discarding whatever
//!    earlier filters in the chain had produced.
//! 4. The chain succeeded but produced **nothing** — raw bytes.
//!
//! The fourth does double duty, and both of its jobs matter. It is what
//! delivers the compressed bytes of a Flate stream that inflates to nothing
//! (the "not a zlib header" case), and it is the *normal* path for an image:
//! a stream filtered only by `/DCTDecode` produces no bytes here at all, so
//! the raw JPEG reaches the image decoder while the codec's name travels
//! alongside it.
//!
//! Because the substitution is silent in the C++ and load-bearing in ours,
//! each of the three real failures records a diagnostic on the way past.
//!
//! # Why the pipeline validator is stricter than the executor
//!
//! `ValidateDecoderPipeline` gates the *shape* of a multi-filter chain: every
//! entry but the last must be one of the five byte-oriented filters. The
//! executor, meanwhile, would happily skip a `/Crypt` anywhere. The two
//! disagree, and the validator wins: `/Filter [/Crypt /FlateDecode]` is
//! rejected outright and its stream is used raw, even though nothing about
//! decoding it is hard. Files depend on that rejection, so it is reproduced
//! rather than reasoned away.

use pdfrum_common::{DiagKind, Diagnostics, Limits, Severity};
use pdfrum_object::{Array, Dict, Name, Object, Resolve, Stream, names};

use crate::{DecodeOutput, Filter, NeedsImageCodec, decode, params_dict};

/// What a stream's bytes turned out to be.
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedStream {
    /// The bytes a consumer should read. For an image punt these are the
    /// codec's *input*, which is either what earlier filters produced or —
    /// when the codec was the first filter — the raw stream bytes.
    pub data: Vec<u8>,
    /// Set when the chain ended at a filter this crate does not decode; the
    /// image path takes over from here.
    pub image: Option<NeedsImageCodec>,
}

/// Check the shape of a `/Filter` array (PDFium's `ValidateDecoderPipeline`).
///
/// An empty array is valid, and so is any single element that resolves to a
/// name — including a name nobody recognizes. Past one element, every entry
/// but the last must be a filter that turns bytes into bytes, so an image
/// codec may appear only in final position and `/Crypt` may not appear in a
/// chain at all.
///
/// ```
/// use pdfrum_filters::validate_pipeline;
/// use pdfrum_object::{Array, Name, NoResolve, Object};
///
/// let name = |s: &str| Object::Name(Name::from(s));
/// // An image codec is fine as long as it comes last.
/// let good = Array::of([name("RL"), name("A85"), name("DCTDecode")]);
/// assert!(validate_pipeline(&good, &NoResolve));
///
/// let bad = Array::of([name("DCTDecode"), name("FlateDecode")]);
/// assert!(!validate_pipeline(&bad, &NoResolve));
/// ```
#[must_use]
pub fn validate_pipeline(filters: &Array, r: &impl Resolve) -> bool {
    if filters.is_empty() {
        return true;
    }
    let names: Option<Vec<Name>> = (0..filters.len())
        .map(|i| filter_name_at(filters, i, r))
        .collect();
    let Some(names) = names else {
        return false;
    };
    if names.len() == 1 {
        return true;
    }
    // Every entry but the last must be byte-to-byte.
    names
        .get(..names.len() - 1)
        .unwrap_or_default()
        .iter()
        .all(|n| Filter::from_name(n).is_some_and(Filter::is_chainable))
}

/// One `/Filter` array element, resolved one level, if it is a name.
fn filter_name_at(filters: &Array, index: usize, r: &impl Resolve) -> Option<Name> {
    filters
        .get(index, r)?
        .as_direct()
        .and_then(Object::as_name)
        .cloned()
}

/// Read a stream dictionary's `/Filter` and `/DecodeParms` into the chain to
/// run (PDFium's `GetDecoderArray`).
///
/// `None` means the filter declaration is unusable and the caller should read
/// the raw bytes; an empty list means "no filters", which is ordinary.
///
/// `/DecodeParms` is only read in the two shapes that match `/Filter`: a
/// dictionary beside a name, and an array beside an array, positionally. Every
/// other combination — a lone dictionary beside a two-filter array, say —
/// yields an empty parameter dictionary rather than a guess at which filter it
/// meant.
///
/// ```
/// use pdfrum_filters::decoder_list;
/// use pdfrum_object::{Dict, Name, NoResolve, Object, names};
///
/// // No /Filter is an empty chain, not a failure.
/// assert_eq!(decoder_list(&Dict::new(), &NoResolve).map(|v| v.len()), Some(0));
///
/// // A string is not a filter declaration.
/// let bad = Dict::from_pairs([(
///     names::FILTER.clone(),
///     Object::Str(pdfrum_object::PdfString::literal(b"RL")),
/// )]);
/// assert!(decoder_list(&bad, &NoResolve).is_none());
/// ```
#[must_use]
pub fn decoder_list(dict: &Dict, r: &impl Resolve) -> Option<Vec<(Name, Dict)>> {
    let filter = dict.get(names::FILTER, r);
    let Some(filter) = filter.as_ref().and_then(pdfrum_object::Resolved::as_direct) else {
        // No /Filter, or one that resolves to nothing: an empty chain.
        return Some(Vec::new());
    };
    let declared = dict.get(names::DECODE_PARMS, r);
    let declared = declared
        .as_ref()
        .and_then(pdfrum_object::Resolved::as_direct);

    match filter {
        Object::Name(name) => {
            // A lone /DecodeParms array here yields no parameters, because
            // asking an array for its dictionary produces nothing.
            Some(vec![(name.clone(), params_dict(declared, r))])
        }
        Object::Array(filters) => {
            if !validate_pipeline(filters, r) {
                return None;
            }
            // Positional parameters only when /DecodeParms is itself an array.
            let positional = declared.and_then(Object::as_array);
            Some(
                (0..filters.len())
                    .map(|i| {
                        let name = filter_name_at(filters, i, r).unwrap_or_else(|| Name::from(""));
                        let params = positional
                            .and_then(|a| a.raw_at(i))
                            .map_or_else(Dict::new, |o| params_dict(Some(o), r));
                        (name, params)
                    })
                    .collect(),
            )
        }
        // A string, a number, a dictionary: not a filter declaration at all.
        _ => None,
    }
}

/// Decode a stream's data through its whole `/Filter` chain, falling back to
/// the raw bytes whenever the chain cannot deliver.
///
/// This is the function `pdfrum-parser`'s stream accessor calls, and it is
/// where PDFium's silent damage tolerance lives. It never fails: every failure
/// resolves to the undecoded bytes plus a diagnostic.
///
/// `estimated_size` is a buffer-sizing hint for the *last* filter only, and
/// only Flate reads it. Pass `/Length1 + /Length2 + /Length3` for an embedded
/// font, `pitch * height` for an image, and 0 for everything else.
///
/// ```
/// use pdfrum_common::{Diagnostics, Limits};
/// use pdfrum_filters::decode_chain;
/// use pdfrum_object::{ByteSpan, Dict, Name, NoResolve, Object, Stream, names};
///
/// let dict = Dict::from_pairs([(
///     names::FILTER.clone(),
///     Object::Name(Name::from("ASCIIHexDecode")),
/// )]);
/// let stream = Stream::new(dict, ByteSpan::from(b"48690a>".to_vec()));
///
/// let mut diags = Diagnostics::default();
/// let decoded = decode_chain(&stream, 0, &NoResolve, &Limits::default(), &mut diags);
/// assert_eq!(decoded.data, b"Hi\n");
/// assert!(decoded.image.is_none());
/// ```
pub fn decode_chain(
    stream: &Stream,
    estimated_size: usize,
    r: &impl Resolve,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> DecodedStream {
    let raw = &*stream.data;
    if raw.is_empty() {
        return DecodedStream {
            data: Vec::new(),
            image: None,
        };
    }
    let undecoded = || DecodedStream {
        data: raw.to_vec(),
        image: None,
    };

    // Fallback 1: an unusable /Filter declaration.
    let Some(filters) = decoder_list(&stream.dict, r) else {
        diags.record(Severity::Suspicious, DiagKind::UndecodableStream, None);
        return undecoded();
    };
    // Fallback 2: nothing to do.
    if filters.is_empty() {
        return undecoded();
    }

    let last = filters.len() - 1;
    let mut current = raw.to_vec();
    for (i, (name, params)) in filters.iter().enumerate() {
        let filter = Filter::from_name(name);
        match filter {
            // An unrecognized name is the image path's problem, not an error:
            // the chain stops here and hands over what it has.
            None => {
                return DecodedStream {
                    data: current,
                    image: Some(NeedsImageCodec {
                        filter: None,
                        name: name.clone(),
                        params: params.clone(),
                    }),
                };
            }
            Some(f) if f.is_image_codec() => {
                return DecodedStream {
                    data: current,
                    image: Some(NeedsImageCodec {
                        filter: Some(f),
                        // Whatever the file spelled it, the consumer sees the
                        // canonical name.
                        name: f.canonical_name().clone(),
                        params: params.clone(),
                    }),
                };
            }
            Some(f) => {
                let hint = if i == last { estimated_size } else { 0 };
                match decode_one(f, &current, params, hint, r, limits, diags) {
                    // Fallback 3: this filter could produce nothing, so the
                    // whole chain's work is discarded.
                    Err(_) => {
                        diags.record(Severity::Suspicious, DiagKind::UndecodableStream, None);
                        return undecoded();
                    }
                    Ok(DecodeOutput::Bytes(bytes)) => current = bytes,
                    Ok(DecodeOutput::Image(need)) => {
                        return DecodedStream {
                            data: current,
                            image: Some(need),
                        };
                    }
                }
            }
        }
    }

    // Fallback 4: the chain ran and produced nothing.
    if current.is_empty() {
        diags.record(Severity::Recovered, DiagKind::UndecodableStream, None);
        return undecoded();
    }
    DecodedStream {
        data: current,
        image: None,
    }
}

/// [`decode`] with the Flate size hint threaded through, which the public
/// entry point has no parameter for.
fn decode_one(
    filter: Filter,
    input: &[u8],
    params: &Dict,
    estimated_size: usize,
    r: &impl Resolve,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> Result<DecodeOutput, crate::Error> {
    if filter == Filter::Flate {
        let predictor_params = crate::PredictorParams::from_dict(params, r)?;
        let raw = crate::decode_flate(input, estimated_size, limits, diags)?;
        return Ok(DecodeOutput::Bytes(crate::predictor(
            raw,
            predictor_params,
        )?));
    }
    decode(filter, input, params, r, limits, diags)
}

#[cfg(test)]
mod tests {
    use super::{DecodedStream, decode_chain, decoder_list, validate_pipeline};
    use crate::Filter;
    use pdfrum_common::{Diagnostics, Limits};
    use pdfrum_object::{
        Array, ByteSpan, Dict, Name, NoResolve, ObjRef, Object, PdfString, Resolve, Stream, names,
    };
    use std::sync::Arc;

    /// A store holding a handful of objects, for the indirect-reference cases.
    #[derive(Debug, Default)]
    struct Store(Vec<(u32, Object)>);

    impl Resolve for Store {
        fn fetch(&self, r: ObjRef) -> Result<Arc<Object>, pdfrum_object::Error> {
            self.0
                .iter()
                .find(|(num, _)| *num == r.num)
                .map(|(_, obj)| Arc::new(obj.clone()))
                .ok_or(pdfrum_object::Error::UnresolvedRef(r))
        }
    }

    fn name(s: &str) -> Object {
        Object::Name(Name::from(s))
    }

    fn text(s: &str) -> Object {
        Object::Str(PdfString::literal(s.as_bytes()))
    }

    // From fpdf_parser_decode_unittest.cpp:46-145 (ValidateDecoderPipeline).
    #[test]
    fn pipeline_shapes_accepted_and_rejected() {
        let cases: [(Vec<Object>, bool); 12] = [
            (vec![], true),
            // A single element is valid whatever it names.
            (vec![name("FlateEncode")], true),
            (vec![name("FooBar")], true),
            (vec![name("AHx"), name("LZWDecode")], true),
            (vec![name("ASCII85Decode"), name("ASCII85Decode")], true),
            (
                vec![name("A85"), name("A85"), name("RL"), name("Fl"), name("RL")],
                true,
            ),
            // An image codec is allowed, but only last.
            (
                vec![
                    name("RL"),
                    name("A85"),
                    name("Fl"),
                    name("LZW"),
                    name("DCTDecode"),
                ],
                true,
            ),
            // A wrong type is invalid even at length one.
            (vec![text("FlateEncode")], false),
            (vec![name("DCTDecode"), name("CCITTFaxDecode")], false),
            (vec![name("DCTDecode"), name("FlateDecode")], false),
            (vec![text("AHx"), name("LZWDecode")], false),
            (
                vec![
                    name("Fl"),
                    name("Fl"),
                    name("DCTDecode"),
                    name("Fl"),
                    name("Fl"),
                ],
                false,
            ),
        ];
        for (elements, expected) in cases {
            let array = Array::of(elements.clone());
            assert_eq!(
                validate_pipeline(&array, &NoResolve),
                expected,
                "{elements:?}"
            );
        }
    }

    #[test]
    fn a_string_in_the_last_position_is_still_the_wrong_type() {
        let array = Array::of([name("A85"), name("A85"), name("RL"), name("Fl"), text("RL")]);
        assert!(!validate_pipeline(&array, &NoResolve));
    }

    // /Crypt is not in the chainable set, though the executor would skip it.
    #[test]
    fn crypt_is_tolerated_only_alone_or_last() {
        let cases: [(Vec<Object>, bool); 3] = [
            (vec![name("Crypt")], true),
            (vec![name("FlateDecode"), name("Crypt")], true),
            (vec![name("Crypt"), name("FlateDecode")], false),
        ];
        for (elements, expected) in cases {
            assert_eq!(
                validate_pipeline(&Array::of(elements.clone()), &NoResolve),
                expected,
                "{elements:?}"
            );
        }
    }

    // From fpdf_parser_decode_unittest.cpp:147-200.
    #[test]
    fn indirect_filter_names_behave_as_the_names_they_point_at() {
        let store = Store(vec![
            (1, name("FlateDecode")),
            (2, name("LZW")),
            (3, text("FlateDecode")),
            (4, name("DCTDecode")),
        ]);
        let reference = |num| Object::Ref(ObjRef::new(num, 0));

        assert!(validate_pipeline(
            &Array::of([reference(1), name("LZW")]),
            &store
        ));
        assert!(validate_pipeline(
            &Array::of([
                name("RunLengthDecode"),
                name("ASCII85Decode"),
                name("FlateDecode"),
                reference(2),
                name("DCTDecode"),
            ]),
            &store
        ));
        // A reference to a string is the wrong type.
        assert!(!validate_pipeline(
            &Array::of([reference(3), name("LZW")]),
            &store
        ));
        // A reference to an image codec is still an image codec, and still
        // may not come first in a chain.
        assert!(!validate_pipeline(
            &Array::of([reference(4), name("LZW")]),
            &store
        ));
    }

    // From fpdf_parser_decode_unittest.cpp:203-266 (GetDecoderArray).
    #[test]
    fn decoder_lists_from_stream_dictionaries() {
        let filter = |value: Object| Dict::from_pairs([(names::FILTER.clone(), value)]);

        // No /Filter at all: an empty chain, not a failure.
        assert_eq!(decoder_list(&Dict::new(), &NoResolve), Some(Vec::new()));
        // The wrong type: a failure.
        assert_eq!(decoder_list(&filter(text("RL")), &NoResolve), None);
        // A name: one entry.
        let one = decoder_list(&filter(name("RL")), &NoResolve).expect("valid");
        assert_eq!(one.len(), 1);
        assert_eq!(one.first().map(|(n, _)| n), Some(&Name::from("RL")));
        // An empty array: an empty chain.
        assert_eq!(
            decoder_list(&filter(Object::Array(Array::new())), &NoResolve),
            Some(Vec::new())
        );
        // A name nobody knows is preserved verbatim for the image path.
        let unknown = decoder_list(
            &filter(Object::Array(Array::of([name("FooBar")]))),
            &NoResolve,
        )
        .expect("valid");
        assert_eq!(unknown.first().map(|(n, _)| n), Some(&Name::from("FooBar")));
        // Two entries keep their order.
        let two = decoder_list(
            &filter(Object::Array(Array::of([name("AHx"), name("LZWDecode")]))),
            &NoResolve,
        )
        .expect("valid");
        assert_eq!(
            two.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>(),
            vec![Name::from("AHx"), Name::from("LZWDecode")]
        );
        // An invalid pipeline is a failure.
        assert_eq!(
            decoder_list(
                &filter(Object::Array(Array::of([
                    name("DCTDecode"),
                    name("CCITTFaxDecode")
                ]))),
                &NoResolve
            ),
            None
        );
    }

    #[test]
    fn a_lone_parameter_dictionary_beside_a_filter_array_reaches_nobody() {
        let dict = Dict::from_pairs([
            (
                names::FILTER.clone(),
                Object::Array(Array::of([name("Fl"), name("Fl")])),
            ),
            (
                names::DECODE_PARMS.clone(),
                Object::Dict(Dict::from_pairs([(
                    names::PREDICTOR.clone(),
                    Object::Int(12),
                )])),
            ),
        ]);
        let list = decoder_list(&dict, &NoResolve).expect("valid");
        assert_eq!(list.len(), 2);
        assert!(
            list.iter().all(|(_, params)| params.is_empty()),
            "positional parameters need an array"
        );
    }

    #[test]
    fn a_parameter_array_is_matched_up_positionally() {
        let parms = |predictor: i64| {
            Object::Dict(Dict::from_pairs([(
                names::PREDICTOR.clone(),
                Object::Int(predictor),
            )]))
        };
        let dict = Dict::from_pairs([
            (
                names::FILTER.clone(),
                Object::Array(Array::of([name("Fl"), name("Fl")])),
            ),
            (
                names::DECODE_PARMS.clone(),
                Object::Array(Array::of([Object::Null, parms(12)])),
            ),
        ]);
        let list = decoder_list(&dict, &NoResolve).expect("valid");
        assert!(list.first().is_some_and(|(_, p)| p.is_empty()));
        assert_eq!(
            list.get(1)
                .and_then(|(_, p)| p.int(names::PREDICTOR, &NoResolve)),
            Some(12)
        );
    }

    #[test]
    fn a_parameter_array_beside_a_name_filter_reaches_nobody() {
        let dict = Dict::from_pairs([
            (names::FILTER.clone(), name("Fl")),
            (
                names::DECODE_PARMS.clone(),
                Object::Array(Array::of([Object::Int(1)])),
            ),
        ]);
        let list = decoder_list(&dict, &NoResolve).expect("valid");
        assert!(list.first().is_some_and(|(_, p)| p.is_empty()));
    }

    /// A stream with the given `/Filter` value and payload.
    fn stream(filter: Option<Object>, data: &[u8]) -> Stream {
        let mut dict = Dict::new();
        if let Some(filter) = filter {
            dict.push(names::FILTER.clone(), filter);
        }
        Stream::new(dict, ByteSpan::from(data.to_vec()))
    }

    fn run(stream: &Stream) -> (DecodedStream, usize) {
        let mut diags = Diagnostics::default();
        let decoded = decode_chain(stream, 0, &NoResolve, &Limits::default(), &mut diags);
        (decoded, diags.len())
    }

    fn zlib(payload: &[u8]) -> Vec<u8> {
        miniz_oxide::deflate::compress_to_vec_zlib(payload, 6)
    }

    #[test]
    fn an_empty_stream_stays_empty() {
        let (decoded, diags) = run(&stream(Some(name("FlateDecode")), b""));
        assert!(decoded.data.is_empty());
        assert!(decoded.image.is_none());
        assert_eq!(diags, 0);
    }

    #[test]
    fn no_filter_yields_the_bytes_as_they_are() {
        let (decoded, diags) = run(&stream(None, b"plain bytes"));
        assert_eq!(decoded.data, b"plain bytes");
        assert_eq!(diags, 0, "no filters is not damage");
    }

    // Fallback 1.
    #[test]
    fn an_invalid_pipeline_falls_back_to_the_raw_bytes() {
        let filter = Object::Array(Array::of([name("DCTDecode"), name("FlateDecode")]));
        let (decoded, diags) = run(&stream(Some(filter), b"raw payload"));
        assert_eq!(decoded.data, b"raw payload");
        assert!(decoded.image.is_none());
        assert_eq!(diags, 1);
    }

    #[test]
    fn a_filter_of_the_wrong_type_falls_back_to_the_raw_bytes() {
        let (decoded, diags) = run(&stream(Some(text("RL")), b"raw payload"));
        assert_eq!(decoded.data, b"raw payload");
        assert_eq!(diags, 1);
    }

    // Fallback 3: the partial result is thrown away, not returned.
    #[test]
    fn a_failing_filter_discards_the_whole_chains_work() {
        // A run-length stream over the cap, behind a working A85 stage.
        let mut runs = Vec::new();
        for _ in 0..200_000 {
            runs.extend_from_slice(&[129, 0]);
        }
        let filter = Object::Array(Array::of([name("RunLengthDecode")]));
        let (decoded, diags) = run(&stream(Some(filter), &runs));
        assert_eq!(decoded.data, runs, "the raw bytes, not a partial decode");
        assert_eq!(diags, 1);
    }

    // Fallback 4, in its damage-tolerance role: a corrupt flate stream
    // delivers its own compressed bytes.
    #[test]
    fn a_flate_stream_that_inflates_to_nothing_delivers_its_compressed_bytes() {
        let (decoded, diags) = run(&stream(Some(name("FlateDecode")), b"preposterous nonsense"));
        assert_eq!(decoded.data, b"preposterous nonsense");
        assert!(decoded.image.is_none());
        assert_eq!(diags, 2, "the truncation and the fallback both recorded");
    }

    // Fallback 4, in its ordinary role: an image's only filter produces
    // nothing here, so the codec receives the raw stream.
    #[test]
    fn a_lone_image_codec_hands_the_raw_bytes_to_the_image_path() {
        let (decoded, _) = run(&stream(Some(name("DCTDecode")), b"\xff\xd8\xff\xe0 jpeg"));
        let image = decoded.image.expect("an image punt");
        assert_eq!(image.filter, Some(Filter::Dct));
        assert_eq!(image.name, Name::from("DCTDecode"));
        assert_eq!(
            decoded.data, b"\xff\xd8\xff\xe0 jpeg",
            "the codec reads the raw stream"
        );
    }

    #[test]
    fn an_abbreviated_image_codec_reports_its_full_name() {
        let (decoded, _) = run(&stream(Some(name("DCT")), b"jpeg"));
        let image = decoded.image.expect("an image punt");
        assert_eq!(image.name, Name::from("DCTDecode"));
        assert_eq!(image.filter, Some(Filter::Dct));

        let (decoded, _) = run(&stream(Some(name("CCF")), b"fax"));
        let image = decoded.image.expect("an image punt");
        assert_eq!(image.name, Name::from("CCITTFaxDecode"));
    }

    #[test]
    fn an_unknown_filter_name_punts_with_no_filter_at_all() {
        let (decoded, _) = run(&stream(Some(name("FooBar")), b"mystery"));
        let image = decoded.image.expect("an image punt");
        assert_eq!(image.filter, None);
        assert_eq!(image.name, Name::from("FooBar"));
        assert_eq!(decoded.data, b"mystery");
    }

    #[test]
    fn filters_before_an_image_codec_run_first() {
        // The JPEG's bytes, ASCII85-encoded, then punted to the image path.
        let filter = Object::Array(Array::of([name("A85"), name("DCTDecode")]));
        let (decoded, _) = run(&stream(Some(filter), b"FCfN8~>"));
        let image = decoded.image.expect("an image punt");
        assert_eq!(image.filter, Some(Filter::Dct));
        assert_eq!(decoded.data, b"test", "the A85 stage already ran");
    }

    #[test]
    fn a_chain_of_two_flate_stages_runs_both() {
        let inner = zlib(b"the innermost payload");
        let outer = zlib(&inner);
        let filter = Object::Array(Array::of([name("Fl"), name("Fl")]));
        let (decoded, diags) = run(&stream(Some(filter), &outer));
        assert_eq!(decoded.data, b"the innermost payload");
        assert_eq!(diags, 0);
    }

    #[test]
    fn a_crypt_entry_decodes_as_the_identity() {
        let filter = Object::Array(Array::of([name("Crypt")]));
        let (decoded, diags) = run(&stream(Some(filter), b"already decrypted"));
        assert_eq!(decoded.data, b"already decrypted");
        assert_eq!(diags, 0);
    }

    #[test]
    fn a_flate_stream_with_a_predictor_is_predicted_inside_the_chain() {
        // Two rows of four bytes, PNG Sub-filtered.
        let predicted = [1u8, 1, 1, 1, 1, 1, 1, 1, 1, 1];
        let dict = Dict::from_pairs([
            (names::FILTER.clone(), name("FlateDecode")),
            (
                names::DECODE_PARMS.clone(),
                Object::Dict(Dict::from_pairs([
                    (names::PREDICTOR.clone(), Object::Int(12)),
                    (names::COLUMNS.clone(), Object::Int(4)),
                ])),
            ),
        ]);
        let stream = Stream::new(dict, ByteSpan::from(zlib(&predicted)));
        let (decoded, _) = run(&stream);
        assert_eq!(decoded.data, vec![1, 2, 3, 4, 1, 2, 3, 4]);
    }

    #[test]
    fn a_bad_predictor_parameter_discards_a_perfectly_good_stream() {
        let dict = Dict::from_pairs([
            (names::FILTER.clone(), name("FlateDecode")),
            (
                names::DECODE_PARMS.clone(),
                Object::Dict(Dict::from_pairs([
                    (names::PREDICTOR.clone(), Object::Int(12)),
                    (names::COLORS.clone(), Object::Int(-1)),
                ])),
            ),
        ]);
        let compressed = zlib(b"a payload that would have decoded fine");
        let stream = Stream::new(dict, ByteSpan::from(compressed.clone()));
        let (decoded, diags) = run(&stream);
        assert_eq!(decoded.data, compressed, "back to the raw bytes");
        assert_eq!(diags, 1);
    }
}
