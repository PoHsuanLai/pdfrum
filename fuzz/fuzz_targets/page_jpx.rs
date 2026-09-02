//! `decode_jpx` — the JPEG 2000 entry point SPEC.md §12 pins.
//!
//! The colorspace and `/SMaskInData` come from the input's control bytes,
//! because the conversion table's interesting rows are the ones where the
//! PDF's declared space and the codestream's disagree — including the two
//! that fail the whole load.
//!
//! Property: never panics, whichever row of the conversion table an input
//! lands on.

#![no_main]

use libfuzzer_sys::fuzz_target;
use pdfrum_page::ColorSpace;
use pdfrum_page::RequestedSize;
use pdfrum_page::decode_jpx;

fuzz_target!(|data: &[u8]| {
    let mut split = pdfrum_fuzz::Split::new(data);
    let which = split.byte();
    let smask_in_data = i64::from(split.byte() % 4);
    // The decode target, spread over the interesting cases: full resolution,
    // a degenerate zero request, and reductions from mild to extreme.
    let target = match split.byte() % 6 {
        0 => RequestedSize::Full,
        n => RequestedSize::Reduced {
            width: u32::from(n),
            height: u32::from(n) * 2,
        },
    };
    let body = split.rest();

    let space = match which % 4 {
        0 => None,
        1 => Some(ColorSpace::DeviceGray),
        2 => Some(ColorSpace::DeviceRgb),
        _ => Some(ColorSpace::DeviceCmyk),
    };

    let limits = pdfrum_fuzz::limits();
    let _ = decode_jpx(body, space.as_ref(), smask_in_data, target, &limits);
});
