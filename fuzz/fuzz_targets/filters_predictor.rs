//! `predictor` — TIFF and PNG un-predictors after Flate/LZW.
//!
//! Property: never panics. A bad parameter set is `BadPredictorParams` or
//! `SizeOverflow`.

#![no_main]

use libfuzzer_sys::fuzz_target;
use pdfrum_filters::{PredictorKind, PredictorParams, predictor};

fuzz_target!(|data: &[u8]| {
    let mut split = pdfrum_fuzz::Split::new(data);

    let kind = match split.byte() % 3 {
        0 => PredictorKind::None,
        1 => PredictorKind::Tiff,
        _ => PredictorKind::Png,
    };
    // Two bytes each: small enough that legal-ish rows are reachable, wide
    // enough that the products which overflow a `u32` row size are too.
    let colors = u32::from(u16::from_le_bytes([split.byte(), split.byte()]));
    let bits_per_component = u32::from(u16::from_le_bytes([split.byte(), split.byte()]));
    let columns = u32::from(u16::from_le_bytes([split.byte(), split.byte()]));

    let params = PredictorParams {
        kind,
        colors,
        bits_per_component,
        columns,
    };
    let _ = predictor(split.rest().to_vec(), params);
});
