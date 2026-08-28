//! PDF stream filters (ISO 32000 §7.4) as pure functions over byte slices:
//! Flate with PNG/TIFF predictors, LZW, RunLength, ASCIIHex/ASCII85, and
//! CCITT fax. Image codecs (DCT/JPX/JBIG2) are recognized here but decoded by
//! the page layer's image path (SPEC.md §4).

#![forbid(unsafe_code)]
