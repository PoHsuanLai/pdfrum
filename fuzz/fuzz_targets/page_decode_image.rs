//! `decode_image` — dictionary plus remaining bytes as stream data.
//!
//! Property: never panics. An error is fine; an allocation from an unchecked
//! product is not.

#![no_main]

use libfuzzer_sys::fuzz_target;
use pdfrum_object::{ByteSpan, NoResolve, Object, Stream};
use pdfrum_page::FunctionCache;
use pdfrum_page::RequestedSize;
use pdfrum_page::decode_image;
use pdfrum_parser::{Lexer, Strictness, parse_object};

fuzz_target!(|data: &[u8]| {
    let mut split = pdfrum_fuzz::Split::new(data);
    let reduce = split.byte();
    let dict_bytes = split.take();
    let body = split.rest();

    let limits = pdfrum_fuzz::limits();
    let mut diags = pdfrum_fuzz::diags();
    let mut lexer = Lexer::new(dict_bytes);
    let Ok(Object::Dict(dict)) =
        parse_object(&mut lexer, &limits, &mut diags, Strictness::Loose, &NoResolve)
    else {
        return;
    };

    let stream = Stream::new(dict, ByteSpan::from(body.to_vec()));
    let size = if reduce & 1 == 0 {
        RequestedSize::Full
    } else {
        RequestedSize::Reduced {
            width: u32::from(reduce),
            height: u32::from(reduce),
        }
    };

    let mut functions = FunctionCache::new();
    let _ = decode_image(
        &stream,
        None,
        None,
        size,
        &NoResolve,
        &mut functions,
        &limits,
        &mut diags,
    );
});
