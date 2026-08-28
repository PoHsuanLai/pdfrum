//! `/LZWDecode` (ISO 32000-1 §7.4.4.2): variable-width LZW over MSB-first
//! codes, with 256 clearing the table and 257 ending the stream.
//!
//! # `/EarlyChange` is arithmetic, not a flag
//!
//! The parameter decides *when* the code width grows, and PDFium spells it as
//! an offset added to the entry count before comparing against the width
//! boundaries — so `/EarlyChange 1` (the default, and the value every real
//! producer writes) grows the width one code before the table actually needs
//! it, at absolute code 511 rather than 512. That off-by-one is the whole
//! difference between PDF's LZW and the textbook algorithm, and it is exactly
//! the difference between `weezl`'s TIFF and non-TIFF decoders, so the
//! parameter selects between them.
//!
//! # Two rejections, everything else recovered
//!
//! A truncated stream keeps its prefix; a trailing partial code is discarded;
//! a full dictionary simply freezes and decoding continues at twelve bits.
//! Only two shapes fail: a stream opening with a dictionary code no literal
//! has defined, and — the odd one — a stream that decodes to zero bytes.
//!
//! That second rule is load-bearing rather than principled. A well-formed LZW
//! stream consisting of nothing but an end-of-data code is legal and decodes
//! to nothing, but PDFium calls it a failure, and the failure is what routes
//! the stream to the raw-bytes fallback instead of leaving a consumer holding
//! an empty buffer. Reproducing it keeps files that rely on that route
//! opening.

use pdfrum_common::{DiagKind, Diagnostics, Limits, Severity};
use weezl::{BitOrder, LzwStatus, decode::Configuration};

use crate::Error;

/// How much output to make room for per round.
const CHUNK: usize = 64 * 1024;

/// Decode `/LZWDecode` data.
///
/// `early_change` is `/EarlyChange != 0`, which defaults to **true** — both
/// when the key is missing and when there is no `/DecodeParms` at all.
///
/// ```
/// use pdfrum_common::{Diagnostics, Limits};
/// use pdfrum_filters::decode_lzw;
///
/// // A clear code, then '-' as a literal and as a growing dictionary entry,
/// // then the end code — MSB-first, starting at nine bits per code.
/// let stream = [0x80, 0x0b, 0x60, 0x50, 0x28, 0x08];
/// let mut diags = Diagnostics::default();
/// let out = decode_lzw(&stream, true, &Limits::default(), &mut diags)?;
/// assert_eq!(out, b"-----");
/// # Ok::<(), pdfrum_filters::Error>(())
/// ```
///
/// # Errors
///
/// [`Error::LzwMalformed`] for the two rejections above, and
/// [`Error::OutputTooLarge`] past `limits.max_decoded_stream_len`.
pub fn decode_lzw(
    input: &[u8],
    early_change: bool,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> Result<Vec<u8>, Error> {
    // A nine-bit initial width means a symbol size of eight, which puts the
    // clear code at 256 and the end code at 257, as PDF requires.
    let configuration = if early_change {
        Configuration::with_tiff_size_switch(BitOrder::Msb, 8)
    } else {
        Configuration::new(BitOrder::Msb, 8)
    };
    let mut decoder = configuration.build();

    let mut out: Vec<u8> = Vec::new();
    let mut consumed = 0usize;
    let mut written = 0usize;
    let mut complete = false;
    let mut rejected = false;

    loop {
        if written > limits.max_decoded_stream_len {
            return Err(Error::OutputTooLarge {
                limit: limits.max_decoded_stream_len,
            });
        }
        out.resize(written.saturating_add(CHUNK), 0);
        // Unreachable `else`: the resize above just made this range exist.
        let Some(tail) = out.get_mut(written..) else {
            break;
        };
        let result = decoder.decode_bytes(input.get(consumed..).unwrap_or_default(), tail);
        consumed = consumed.saturating_add(result.consumed_in);
        written = written.saturating_add(result.consumed_out);

        match result.status {
            Ok(LzwStatus::Done) => {
                complete = true;
                break;
            }
            // Out of input mid-code, or otherwise stuck: keep what we have.
            Ok(LzwStatus::NoProgress) => break,
            Ok(LzwStatus::Ok) => {
                if result.consumed_in == 0 && result.consumed_out == 0 {
                    break;
                }
            }
            Err(_) => {
                rejected = true;
                break;
            }
        }
    }
    out.truncate(written);

    if rejected && out.is_empty() {
        // Nothing decoded before the bad code: the stream opened with a
        // dictionary reference no literal had defined.
        return Err(Error::LzwMalformed(
            "the first code is a dictionary reference",
        ));
    }
    if out.is_empty() {
        return Err(Error::LzwMalformed("the stream decodes to no bytes"));
    }
    if out.len() > limits.max_decoded_stream_len {
        return Err(Error::OutputTooLarge {
            limit: limits.max_decoded_stream_len,
        });
    }
    if !complete {
        diags.record(
            Severity::Recovered,
            DiagKind::UndecodableStream,
            Some(consumed as u64),
        );
    }
    out.shrink_to_fit();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::decode_lzw;
    use crate::Error;
    use pdfrum_common::{Diagnostics, Limits};
    use weezl::{BitOrder, encode::Encoder};

    fn encode(payload: &[u8], early_change: bool) -> Vec<u8> {
        let mut encoder = if early_change {
            Encoder::with_tiff_size_switch(BitOrder::Msb, 8)
        } else {
            Encoder::new(BitOrder::Msb, 8)
        };
        encoder.encode(payload).expect("encodes")
    }

    fn decode(input: &[u8], early_change: bool) -> Result<(Vec<u8>, usize), Error> {
        let mut diags = Diagnostics::default();
        decode_lzw(input, early_change, &Limits::default(), &mut diags)
            .map(|out| (out, diags.len()))
    }

    /// Pack `codes` MSB-first at a fixed nine bits each.
    fn pack9(codes: &[u16]) -> Vec<u8> {
        let mut bytes = vec![0u8; codes.len() * 9 / 8 + 2];
        for (n, &code) in codes.iter().enumerate() {
            for bit in 0..9usize {
                if (code >> (8 - bit)) & 1 == 1 {
                    let at = n * 9 + bit;
                    if let Some(byte) = bytes.get_mut(at / 8) {
                        *byte |= 0x80 >> (at % 8);
                    }
                }
            }
        }
        bytes
    }

    #[test]
    fn a_complete_stream_round_trips_under_both_early_change_settings() {
        let payload: Vec<u8> = (0..3000u32).map(|i| (i % 251) as u8).collect();
        for early_change in [false, true] {
            let stream = encode(&payload, early_change);
            let (out, diags) = decode(&stream, early_change).expect("decodes");
            assert_eq!(out, payload, "early_change={early_change}");
            assert_eq!(diags, 0);
        }
    }

    // The whole point of /EarlyChange: over a stream long enough to cross the
    // 511/512 code boundary the two settings disagree, so reading one with the
    // other's rule produces something else entirely.
    #[test]
    fn early_change_changes_the_decoding_of_a_long_stream() {
        // Low-entropy data fills the dictionary quickly.
        let payload: Vec<u8> = (0..6000u32).map(|i| (i % 37) as u8).collect();
        let early = encode(&payload, true);
        let late = encode(&payload, false);
        assert_ne!(early, late, "the encodings differ past code 511");

        let (right, _) = decode(&early, true).expect("decodes");
        assert_eq!(right, payload);
        // Reading an early-change stream by the textbook rule desynchronizes.
        let wrong = decode(&early, false).map(|(out, _)| out);
        assert!(
            wrong.as_ref().map_or(true, |out| *out != payload),
            "the wrong rule must not reproduce the payload"
        );
    }

    #[test]
    fn a_truncated_stream_keeps_its_prefix_without_failing() {
        let payload: Vec<u8> = (0..3000u32).map(|i| (i % 251) as u8).collect();
        let stream = encode(&payload, true);
        let cut = stream.get(..stream.len() / 2).expect("a prefix");
        let (out, diags) = decode(cut, true).expect("truncation is never an error");
        assert!(!out.is_empty());
        assert!(payload.starts_with(&out), "the prefix is genuine");
        assert_eq!(diags, 1, "the truncation is recorded");
    }

    #[test]
    fn a_trailing_partial_code_is_discarded_silently() {
        let payload = b"the quick brown fox".to_vec();
        let mut stream = encode(&payload, true);
        // Append a single byte: fewer than nine bits of a further code.
        stream.push(0xff);
        let (out, _) = decode(&stream, true).expect("decodes");
        assert_eq!(out, payload);
    }

    // The only outright rejection PDFium performs on the code stream itself.
    #[test]
    fn a_dictionary_code_before_any_literal_is_rejected() {
        let stream = pack9(&[300]);
        assert_eq!(
            decode(&stream, true),
            Err(Error::LzwMalformed(
                "the first code is a dictionary reference"
            ))
        );
    }

    // A legal stream that decodes to nothing is a failure, which is what sends
    // it to the raw-bytes fallback.
    #[test]
    fn a_bare_end_of_data_code_is_a_failure() {
        assert_eq!(
            decode(&pack9(&[256, 257]), true),
            Err(Error::LzwMalformed("the stream decodes to no bytes"))
        );
        assert_eq!(
            decode(&[], true),
            Err(Error::LzwMalformed("the stream decodes to no bytes"))
        );
    }

    #[test]
    fn a_stream_without_a_leading_clear_code_still_decodes() {
        // PDFium's decoder has no "must start with 256" rule, and real files
        // omit it.
        let (out, _) =
            decode(&pack9(&[u16::from(b'A'), u16::from(b'B'), 257]), true).expect("decodes");
        assert_eq!(out, b"AB");
    }

    #[test]
    fn a_dictionary_filled_past_its_last_code_keeps_decoding() {
        // 30 KiB of varied data overruns the 4096-entry table several times
        // without the encoder emitting a clear.
        let payload: Vec<u8> = (0..30_000u32)
            .map(|i| (i.wrapping_mul(7) % 253) as u8)
            .collect();
        let stream = encode(&payload, true);
        let (out, diags) = decode(&stream, true).expect("a frozen table is not an error");
        assert_eq!(out, payload);
        assert_eq!(diags, 0);
    }

    #[test]
    fn the_output_limit_is_enforced() {
        let payload = vec![0u8; 4 * 1024 * 1024];
        let stream = encode(&payload, true);
        let limits = Limits {
            max_decoded_stream_len: 1024,
            ..Limits::default()
        };
        let mut diags = Diagnostics::default();
        assert_eq!(
            decode_lzw(&stream, true, &limits, &mut diags),
            Err(Error::OutputTooLarge { limit: 1024 })
        );
    }

    #[test]
    fn arbitrary_bytes_never_panic() {
        let mut diags = Diagnostics::default();
        for seed in 0..64u8 {
            let junk: Vec<u8> = (0..200u8)
                .map(|i| i.wrapping_mul(seed).wrapping_add(13))
                .collect();
            for early_change in [false, true] {
                let _ = decode_lzw(&junk, early_change, &Limits::default(), &mut diags);
            }
        }
    }
}
