//! `decode_chain` — a whole `/Filter` pipeline over a synthesised stream.
//!
//! The stream dictionary is parsed from the input rather than built here, so
//! the fuzzer reaches the pipeline-validation logic (`/Filter` as a name, as
//! an array, as a reference, mismatched `/DecodeParms` arity, a filter name
//! that is not a filter) as well as the decoders behind it. The stream body
//! is whatever follows the dictionary.
//!
//! Property: never panics, and the returned data is either decoder output
//! (bounded by `max_decoded_stream_len`) or the raw bytes handed back
//! untouched by one of the four fallbacks — so it never exceeds the larger of
//! the two. `decode_chain` is infallible by design — damage becomes a
//! diagnostic, not an error — so any crash here is a real bug.

#![no_main]

use libfuzzer_sys::fuzz_target;
use std::sync::Arc;

use pdfrum_object::{ByteSpan, NoResolve, Object, Stream};
use pdfrum_parser::{Strictness, parse_object};

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

    let decoded =
        pdfrum_filters::decode_chain(&stream, estimated_size, &NoResolve, &limits, &mut diags);
    assert!(decoded.data.len() <= limits.max_decoded_stream_len.max(stream.data.len()));
});
