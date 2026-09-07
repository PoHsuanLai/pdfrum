//! `decode_chain` — a whole `/Filter` pipeline over a synthesised stream.
//!
//! The stream dictionary is parsed from the input rather than built here, so
//! the fuzzer reaches the pipeline-validation logic (`/Filter` as a name, as
//! an array, as a reference, mismatched `/DecodeParms` arity, a filter name
//! that is not a filter) as well as the decoders behind it. The stream body
//! is whatever follows the dictionary.
//!
//! Property: never panics, and the returned data is bounded — by the *chain's*
//! ceiling, not by any single constant. `decode_chain` is infallible by
//! design — damage becomes a diagnostic, not an error — so any crash here is a
//! real bug.
//!
//! # Why the bound is computed rather than written down
//!
//! `max_decoded_stream_len` is **not** a whole-chain output cap, and asserting
//! that it was is what this target got wrong originally (crash artifact
//! `crash-c91cae880d64e58bba8aea9a5ac469a3fba0f26e`, now a seed). Only Flate
//! and LZW consult it. The design brief's divergence D2
//! settles this deliberately: RunLength keeps
//! PDFium's own `kMaxStreamSize` of 20 MiB as a separate filter-specific
//! constant, because that cap is a rejection the oracle really performs and
//! files depend on it. The ASCII filters have no cap at all, and do not need
//! one to be safe — but `ASCII85Decode` is not purely shrinking either, since
//! `z` spells four zero bytes in one input byte.
//!
//! So a chain amplifies. The seed reaches `/Filter [/FlateDecode /RL /RL /RL
//! /RL]`: Flate stays under `max_decoded_stream_len`, and then four RunLength
//! stages take 210 raw bytes to 7,450,260 — every stage inside its own limit,
//! the total far past the 1 MiB the target was asserting. That is the crate
//! behaving as specified, so the assertion moves rather than the crate.
//!
//! [`chain_ceiling`] therefore walks the decoder list the crate itself
//! produced and composes each stage's real ceiling. Keeping the walk here
//! rather than collapsing it to one constant is the point: it still catches a
//! stage that ignores its own cap, which a `<= 20 MiB` blanket assertion would
//! not.

#![no_main]

use libfuzzer_sys::fuzz_target;
use std::sync::Arc;

use pdfrum_filters::{Filter, RUN_LENGTH_MAX_OUTPUT};
use pdfrum_object::{ByteSpan, Dict, Name, NoResolve, Object, Stream};
use pdfrum_parser::{Strictness, parse_object};

/// The largest output the chain `filters` can produce from `raw` bytes.
///
/// Each stage's ceiling as a function of its input, composed left to right:
///
/// - Flate and LZW: `max_decoded_stream_len`, the one cap `Limits` owns.
/// - RunLength: `RUN_LENGTH_MAX_OUTPUT`, its own 20 MiB rejection.
/// - `ASCII85Decode`: four output bytes per input byte, the `z` case.
/// - `ASCIIHexDecode`: shrinking, two hex digits per byte.
/// - `Crypt`: the identity here.
/// - anything else (an image codec, an unknown name): the chain stops and
///   hands back what it has, so the running bound already covers it.
///
/// Saturating throughout: an intermediate that overflows `usize` is a bound
/// nothing can exceed, which is exactly what saturation yields.
fn chain_ceiling(filters: &[(Name, Dict)], raw: usize, max_decoded: usize) -> usize {
    let mut bound = raw;
    for (name, _) in filters {
        bound = match Filter::from_name(name) {
            Some(Filter::Flate | Filter::Lzw) => max_decoded,
            Some(Filter::RunLength) => usize::try_from(RUN_LENGTH_MAX_OUTPUT).unwrap_or(usize::MAX),
            Some(Filter::Ascii85) => bound.saturating_mul(4),
            Some(Filter::AsciiHex) => bound,
            Some(Filter::Crypt) => bound,
            // An image codec or an unrecognized name ends the chain with
            // whatever the stages so far produced.
            _ => break,
        };
    }
    // Every failure fallback returns the raw bytes instead, which can be
    // longer than a shrinking chain's output would have been.
    bound.max(raw)
}

fuzz_target!(|data: &[u8]| {
    let mut split = pdfrum_fuzz::Split::new(data);
    let estimated_size = usize::from(u16::from_le_bytes([split.byte(), split.byte()]));
    let body = split.rest();

    let limits = pdfrum_fuzz::limits();
    let mut diags = pdfrum_fuzz::diags();

    // Read a dictionary off the front; the bytes it did not consume are the
    // stream's data.
    let mut lexer = pdfrum_parser::Lexer::new(body);
    let Ok(Object::Dict(dict)) = parse_object(
        &mut lexer,
        &limits,
        &mut diags,
        Strictness::Loose,
        &NoResolve,
    ) else {
        return;
    };
    let start = lexer.pos().min(body.len());

    let file: Arc<[u8]> = Arc::from(body);
    let Ok(span) = ByteSpan::new(file, start..body.len()) else {
        return;
    };
    let stream = Stream::new(dict, span);

    // The chain the crate will actually run, so the bound below is the one
    // this input's filters permit rather than a blanket constant. `None` is an
    // unusable /Filter declaration, which falls back to the raw bytes.
    let filters = pdfrum_filters::decoder_list(&stream.dict, &NoResolve).unwrap_or_default();
    let ceiling = chain_ceiling(&filters, stream.data.len(), limits.max_decoded_stream_len);

    let decoded =
        pdfrum_filters::decode_chain(&stream, estimated_size, &NoResolve, &limits, &mut diags);
    assert!(
        decoded.data.len() <= ceiling,
        "{} bytes out of a chain capped at {ceiling}",
        decoded.data.len()
    );
});
