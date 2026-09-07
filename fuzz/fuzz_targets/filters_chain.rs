//! `decode_chain` — a `/Filter` pipeline over a synthesised stream.
//!
//! Property: never panics; output stays under [`chain_ceiling`]. That bound
//! is per stage: Flate/LZW use `max_decoded_stream_len`, RunLength uses
//! 20 MiB, ASCII85 can expand 4×. A 1 MiB whole-chain cap is wrong —
//! `/Filter [/FlateDecode /RL /RL /RL /RL]` is in-spec at 7.4 MB.

#![no_main]

use libfuzzer_sys::fuzz_target;
use std::sync::Arc;

use pdfrum_filters::{Filter, RUN_LENGTH_MAX_OUTPUT};
use pdfrum_object::{ByteSpan, Dict, Name, NoResolve, Object, Stream};
use pdfrum_parser::{Strictness, parse_object};

/// Largest output `filters` can produce from `raw` bytes. Flate/LZW:
/// `max_decoded`; RunLength: 20 MiB; ASCII85: 4×; ASCIIHex/Crypt: identity.
/// Anything else ends the chain. Saturating; failure fallback is `raw`.
fn chain_ceiling(filters: &[(Name, Dict)], raw: usize, max_decoded: usize) -> usize {
    let mut bound = raw;
    for (name, _) in filters {
        bound = match Filter::from_name(name) {
            Some(Filter::Flate | Filter::Lzw) => max_decoded,
            Some(Filter::RunLength) => usize::try_from(RUN_LENGTH_MAX_OUTPUT).unwrap_or(usize::MAX),
            Some(Filter::Ascii85) => bound.saturating_mul(4),
            Some(Filter::AsciiHex) => bound,
            Some(Filter::Crypt) => bound,
            _ => break,
        };
    }
    bound.max(raw)
}

fuzz_target!(|data: &[u8]| {
    let mut split = pdfrum_fuzz::Split::new(data);
    let estimated_size = usize::from(u16::from_le_bytes([split.byte(), split.byte()]));
    let body = split.rest();

    let limits = pdfrum_fuzz::limits();
    let mut diags = pdfrum_fuzz::diags();

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
