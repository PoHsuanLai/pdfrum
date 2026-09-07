//! Mesh shading streams — types 4–7 bit-packed vertex data.
//!
//! Property: never panics; a truncated stream stops rather than looping.

#![no_main]

use libfuzzer_sys::fuzz_target;
use pdfrum_page::ColorSpace;
use pdfrum_page::{MeshParams, MeshReader, ShadingKind};

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

    // The lattice type is the one that reads no edge flags, so both branches
    // of `MeshParams`' flag-width validation are exercised.
    for params_kind in [ShadingKind::FreeFormMesh, ShadingKind::LatticeMesh] {
        let Some(params) = MeshParams::new(
            coord_bits,
            component_bits,
            flag_bits,
            components,
            &decode,
            params_kind,
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
                let _ = reader.read_patches(ShadingKind::CoonsMesh);
            }
            _ => {
                let _ = reader.read_patches(ShadingKind::TensorMesh);
            }
        }
    }
});
