//! Mesh shading streams — types 4 to 7's bit-packed vertex data.
//!
//! The bit widths and the decode ranges come from the input's control bytes,
//! so the fuzzer reaches every combination of coordinate width, component
//! width and flag width the format allows, plus the ones it does not.
//!
//! Property: never panics, always terminates. The bit reader returns zero
//! past the end rather than failing, so a truncated stream must stop the
//! decode rather than looping — and the capacity predicates, which divide
//! rather than multiply, are what decide that.

#![no_main]

use libfuzzer_sys::fuzz_target;
use pdfrum_page::color::ColorSpace;
use pdfrum_page::shading::{MeshParams, MeshReader};

fuzz_target!(|data: &[u8]| {
    const WIDTHS: [u32; 9] = [0, 1, 2, 3, 4, 8, 12, 16, 32];

    let mut split = pdfrum_fuzz::Split::new(data);
    let pick = |b: u8| WIDTHS[usize::from(b) % WIDTHS.len()];
    let coord_bits = pick(split.byte());
    let component_bits = pick(split.byte());
    let flag_bits = pick(split.byte());
    let components = usize::from(split.byte() % 10);
    let kind = split.byte() % 4;
    let per_row = usize::from(split.byte());
    let body = split.rest();

    let decode: Vec<f32> = (0..4 + 2 * components)
        .map(|i| if i % 2 == 0 { 0.0 } else { 255.0 })
        .collect();
    let space = ColorSpace::DeviceRgb;

    for flags in [true, false] {
        let Some(params) = MeshParams::new(
            coord_bits,
            component_bits,
            flag_bits,
            components,
            &decode,
            flags,
        ) else {
            continue;
        };
        let mut reader = MeshReader::new(body, &params, &space, &[]);
        match kind {
            0 => {
                let _ = reader.read_free_form();
            }
            1 => {
                let _ = reader.read_lattice(per_row);
            }
            2 => {
                let _ = reader.read_patches(false);
            }
            _ => {
                let _ = reader.read_patches(true);
            }
        }
    }
});
