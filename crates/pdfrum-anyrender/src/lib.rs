#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_cfg))]

mod area;
mod cmap;
mod faces;
mod glyphs;
mod paint;
mod prepare;
mod replay;
mod run_text;
mod sources;
mod write;

pub use area::{GlyphArea, GlyphTally};
pub use run_text::{GlyphSource, RunKey, RunText, RunTexts};
pub use sources::{EncodedImage, ImageCodec, ImageSources, Sources};
pub use write::{Page, Written, write};
