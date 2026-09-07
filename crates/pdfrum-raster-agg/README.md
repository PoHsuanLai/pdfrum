# pdfrum-raster-agg

Analytic scanline rasterizer. No extra rasterizer crate. Coverage is
area on a 256ths-of-a-pixel grid, `alpha = min(255, floor(coverage * 256))`,
matching PDFium's AGG path.

Part of [pdfrum](https://crates.io/crates/pdfrum). `#![forbid(unsafe_code)]`.

MIT OR Apache-2.0
