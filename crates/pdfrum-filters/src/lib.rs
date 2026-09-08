#![doc = include_str!("../README.md")]
// Damage tolerance is the point: a broken filter stream is not an error in the
// files this crate exists to open.
//
// The `Err` cases are a size computation that overflows, output past
// `Limits::max_decoded_stream_len`, or one of the two malformations PDFium
// itself rejects outright (the `lzw` module).
//
// A stream's `/Filter` is a chain, and `decode_chain` walks it left to right,
// handing each filter the previous one's output. It stops at the first filter
// this crate does not decode and returns `DecodeOutput::Image` with the bytes
// produced so far, which is how a `/Filter [/ASCII85Decode /DCTDecode]` image
// reaches the JPEG decoder already de-ASCII'd.
#![forbid(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_auto_cfg))]
// Every byte reaching this crate came from an untrusted file: index with
// `get()`, and size arithmetic goes through `checked_*`.
#![warn(clippy::indexing_slicing)]

mod ascii;
#[cfg(feature = "ccitt")]
mod ccitt;
mod chain;
mod error;
mod filter;
mod flate;
mod lzw;
mod predictor;
mod runlength;

pub use ascii::{decode_ascii_hex, decode_ascii85};
#[cfg(feature = "ccitt")]
pub use ccitt::{CcittImage, CcittParams, decode_ccitt};
pub use chain::{DecodedStream, decode_chain, decoder_list, validate_pipeline};
pub use error::Error;
pub use flate::{decode_flate, encode_flate};
pub use lzw::decode_lzw;
pub use predictor::{PredictorKind, PredictorParams, predictor};
pub use runlength::{RUN_LENGTH_MAX_OUTPUT, decode_run_length};

pub use filter::{DecodeOutput, Filter, NeedsImageCodec, decode};
