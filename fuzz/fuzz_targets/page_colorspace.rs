//! Colorspace loading and conversion over an arbitrary object graph.
//!
//! The object is parsed from the input with the file grammar, then handed to
//! the colorspace loader — so the fuzzer explores the family dispatch, the
//! cycle guards, the depth cap, and every family's own validation.
//!
//! Property: loading never panics, and neither does converting whatever
//! loaded — including on component vectors of the wrong length and on NaN
//! and infinity, which several families' unguarded arithmetic produces.

#![no_main]

use libfuzzer_sys::fuzz_target;
use pdfrum_object::NoResolve;
use pdfrum_page::color::load_colorspace;
use pdfrum_page::function::FunctionCache;
use pdfrum_parser::{Lexer, Strictness, parse_object};

fuzz_target!(|data: &[u8]| {
    let limits = pdfrum_fuzz::limits();
    let mut diags = pdfrum_fuzz::diags();
    let mut lexer = Lexer::new(data);
    let Ok(object) = parse_object(&mut lexer, &limits, &mut diags, Strictness::Loose, &NoResolve)
    else {
        return;
    };

    let mut functions = FunctionCache::new();
    let Some(space) = load_colorspace(&object, None, &NoResolve, &mut functions, &limits, &mut diags)
    else {
        return;
    };

    // Whatever loaded must convert without panicking, at any arity.
    for comps in [
        &[][..],
        &[0.0],
        &[0.5, 0.5],
        &[1.0, 1.0, 1.0],
        &[0.0, 1.0, 0.5, 0.25],
        &[f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1e30],
    ] {
        let _ = space.to_rgb(comps);
        let _ = space.try_to_rgb(comps, true);
    }

    // …and so must the bulk path, including into a destination too short to
    // hold the pixels it is asked for.
    let samples = [0x5Au8; 64];
    let mut dest = [0u8; 3 * 8];
    space.translate_image_line(&mut dest, &samples, 8, false, false);
    space.translate_image_line(&mut dest, &samples, 8, true, true);
    space.translate_image_line(&mut [], &samples, 8, false, false);
});
