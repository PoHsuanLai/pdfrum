//! `decode_ccitt` — `/CCITTFaxDecode`, Group 3 and Group 4.
//!
//! Property: never panics; the decoded buffer never exceeds
//! `max_decoded_stream_len`. `/Columns` and `/Rows` are the stream's to
//! declare and reach the size arithmetic unmediated, so they are driven to
//! their full 16-bit range here rather than left at their defaults.

#![no_main]

use libfuzzer_sys::fuzz_target;

use pdfrum_filters::CcittParams;

fuzz_target!(|data: &[u8]| {
    let mut split = pdfrum_fuzz::Split::new(data);
    let flags = split.byte();
    // A signed `/K`: Group 4 below zero, Group 3 1-D at zero, mixed above it.
    let k = i64::from(split.byte() as i8);
    let columns = i64::from(u16::from_le_bytes([split.byte(), split.byte()]));
    let rows = i64::from(u16::from_le_bytes([split.byte(), split.byte()]));
    // The dimensions the dictionary would supply where the parameters defer.
    let image_width = u32::from(split.byte());
    let image_height = u32::from(split.byte());
    let input = split.rest();

    let params = CcittParams {
        k,
        end_of_line: flags & 1 == 1,
        encoded_byte_align: flags & 2 == 2,
        black_is_1: flags & 4 == 4,
        columns,
        rows,
    };

    let limits = pdfrum_fuzz::limits();
    let mut diags = pdfrum_fuzz::diags();
    if let Ok(image) = pdfrum_filters::decode_ccitt(
        input,
        params,
        image_width,
        image_height,
        &limits,
        &mut diags,
    ) {
        assert!(image.bits.len() <= limits.max_decoded_stream_len);
    }
});
