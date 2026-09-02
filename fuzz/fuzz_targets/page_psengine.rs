//! The type 4 PostScript calculator: parsing a program and running it.
//!
//! Every error path in this engine is silent by design — push overflow drops
//! the value, pop underflow yields zero, a malformed `if` aborts its own
//! procedure and no more — so there is no error channel to assert on. The
//! property is that a program the parser accepted always evaluates, however
//! hostile its stack discipline.
//!
//! Property: never panics, always terminates. Nesting is capped at 128
//! comparing with `>`, so a deeply nested program must fail to *parse*
//! rather than blowing the stack, and execution recursion is bounded by that
//! same parse depth.

#![no_main]

use libfuzzer_sys::fuzz_target;
use pdfrum_page::parse_program;

fuzz_target!(|data: &[u8]| {
    let mut split = pdfrum_fuzz::Split::new(data);
    let outputs = usize::from(split.byte() % 8);
    let source = split.rest();

    let domain = [0.0f32, 1.0];
    let range: Vec<f32> = (0..outputs * 2)
        .map(|i| if i % 2 == 0 { 0.0 } else { 1.0 })
        .collect();

    // The bare input, and the input forced into a procedure — a program must
    // begin with `{`, so most raw inputs would otherwise fail at byte one.
    for program in [source.to_vec(), {
        let mut braced = Vec::with_capacity(source.len() + 2);
        braced.push(b'{');
        braced.extend_from_slice(source);
        braced.push(b'}');
        braced
    }] {
        let Some(function) = parse_program(&program, &domain, &range) else {
            continue;
        };
        let mut out = vec![0.0f32; outputs.max(1)];
        let mut diags = pdfrum_fuzz::diags();
        for input in [0.0f32, 0.5, 1.0, f32::NAN, f32::INFINITY, -1e30] {
            let _ = function.eval_with_diagnostics(&[input], &mut out, &mut diags);
            let _ = function.stack_depth_after(&[input]);
        }
    }
});
