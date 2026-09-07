//! `/FlateDecode` (ISO 32000-1 §7.4.4): zlib-wrapped deflate.
//!
//! # Inflate's return value is a loop-exit condition, not an error
//!
//! This is the single most load-bearing damage-tolerance behavior in the
//! crate. PDFium drives zlib in a loop and looks at `inflate`'s return value
//! only to decide whether to go round again; a data error, a buffer error and
//! a clean end of stream all leave the loop the same way, and whatever bytes
//! were produced before that point are returned as a **success**.
//!
//! So `b"preposterous nonsense"` — whose first two bytes are not a zlib header
//! — decodes to nothing, having read two bytes, without failing. Empty output
//! then trips the stream accessor's last fallback, which hands the *compressed*
//! bytes to the consumer. That chain, corrupt stream to raw bytes, is why the
//! oracle opens files other readers reject, and [`decode_chain`](crate::decode_chain)
//! depends on this function never returning `Err` for malformed input.

use miniz_oxide::inflate::stream::{InflateState, inflate};
use miniz_oxide::{DataFormat, MZFlush, MZStatus};
use pdfrum_common::{DiagKind, Diagnostics, Limits, Severity};

use crate::Error;

/// Largest buffer to grow the output by in one step.
///
/// PDFium's `kMaxInitialAllocSize`, decimal ten million — not ten mebibytes,
/// and not a cap on the output: it only bounds how much is allocated at a
/// time, so a stream larger than this simply takes more rounds.
const MAX_CHUNK: usize = 10_000_000;

/// Inflate `input`, returning whatever decompressed before the stream ended,
/// went wrong, or stopped making progress.
///
/// `estimated_size` sizes the first allocation and nothing else; pass 0 when
/// the decoded length is unknown, `/Length1 + /Length2 + /Length3` for an
/// embedded font, or `pitch * height` for an image.
///
/// Malformed input never fails here: a truncated or corrupt stream yields the
/// prefix that inflated, with a diagnostic. Trailing garbage past the
/// end-of-stream marker yields the full content and no diagnostic.
///
/// ```
/// use pdfrum_common::{Diagnostics, Limits};
/// use pdfrum_filters::decode_flate;
///
/// let mut diags = Diagnostics::default();
/// // Not a zlib stream at all: two bytes read, nothing produced, no error.
/// let out = decode_flate(b"preposterous nonsense", 0, &Limits::default(), &mut diags)?;
/// assert!(out.is_empty());
/// assert_eq!(diags.len(), 1);
/// # Ok::<(), pdfrum_filters::Error>(())
/// ```
///
/// # Errors
///
/// [`Error::OutputTooLarge`] alone, when the stream has more to say than
/// `limits.max_decoded_stream_len` bytes. The buffer never grows past that
/// bound, so the error costs no more memory than the bound allows.
pub fn decode_flate(
    input: &[u8],
    estimated_size: usize,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> Result<Vec<u8>, Error> {
    if input.is_empty() {
        return Ok(Vec::new());
    }

    let chunk = if estimated_size != 0 {
        estimated_size
    } else {
        input.len().saturating_mul(2)
    }
    .clamp(1, MAX_CHUNK);

    let mut state = InflateState::new_boxed(DataFormat::Zlib);
    let mut out: Vec<u8> = Vec::new();
    let mut consumed = 0usize;
    let mut finished = false;

    loop {
        let filled = out.len();
        // The budget is a ceiling on the buffer, not just on the result: growing
        // past it and truncating afterwards would let a bomb allocate the very
        // memory the limit exists to deny. Room of zero means the previous round
        // filled the buffer exactly and the stream still wants more.
        let room = limits.max_decoded_stream_len.saturating_sub(filled);
        if room == 0 {
            return Err(Error::OutputTooLarge {
                limit: limits.max_decoded_stream_len,
            });
        }
        out.resize(filled.saturating_add(chunk.min(room)), 0);
        // Unreachable `else`: the resize above just made this range exist.
        let Some(tail) = out.get_mut(filled..) else {
            break;
        };
        let result = inflate(
            &mut state,
            input.get(consumed..).unwrap_or_default(),
            tail,
            MZFlush::None,
        );
        consumed = consumed.saturating_add(result.bytes_consumed);
        out.truncate(filled.saturating_add(result.bytes_written));

        match result.status {
            Ok(MZStatus::StreamEnd) => {
                finished = true;
                break;
            }
            // No input read and no output written means zlib is stuck: either
            // the input ran out mid-stream, or it can make no further sense of
            // what is left.
            Ok(_) if result.bytes_consumed == 0 && result.bytes_written == 0 => break,
            Ok(_) => {}
            Err(_) => break,
        }
    }

    if !finished {
        // The stream did not reach its end marker: it is truncated, corrupt,
        // or was never deflate to begin with. All three keep their prefix.
        diags.record(
            Severity::Recovered,
            DiagKind::UndecodableStream,
            Some(consumed as u64),
        );
    }
    out.shrink_to_fit();
    Ok(out)
}

/// Compression level the writer deflates at.
///
/// PDFium calls zlib's `compress()`, which is `Z_DEFAULT_COMPRESSION` — level
/// 6. Matching it keeps our output sizes in the same neighbourhood as the
/// oracle's without any claim of byte-identity (the two deflate
/// implementations differ regardless).
const ENCODE_LEVEL: u8 = 6;

/// Deflate `input` into a zlib stream, the payload a `/FlateDecode` stream
/// declares.
///
/// The inverse of [`decode_flate`] up to the compressor's choices: the bytes
/// differ from zlib's for the same input, the decoded content does not. Used
/// only by the writer (`pdfrum-edit`); nothing on the read path compresses.
///
/// ```
/// use pdfrum_common::{Diagnostics, Limits};
/// use pdfrum_filters::{decode_flate, encode_flate};
///
/// let round = decode_flate(
///     &encode_flate(b"round trip"),
///     0,
///     &Limits::default(),
///     &mut Diagnostics::default(),
/// )?;
/// assert_eq!(round, b"round trip");
/// # Ok::<(), pdfrum_filters::Error>(())
/// ```
#[must_use]
pub fn encode_flate(input: &[u8]) -> Vec<u8> {
    miniz_oxide::deflate::compress_to_vec_zlib(input, ENCODE_LEVEL)
}

#[cfg(test)]
mod tests {
    use super::{decode_flate, encode_flate};
    use crate::Error;
    use pdfrum_common::{Diagnostics, Limits};

    fn decode(input: &[u8]) -> (Vec<u8>, usize) {
        let mut diags = Diagnostics::default();
        let out = decode_flate(input, 0, &Limits::default(), &mut diags).expect("never fails");
        (out, diags.len())
    }

    /// A real zlib stream of `payload`, built with `miniz_oxide`'s compressor so
    /// the tests do not depend on a checked-in blob.
    fn compress(payload: &[u8]) -> Vec<u8> {
        miniz_oxide::deflate::compress_to_vec_zlib(payload, 6)
    }

    // From flatemodule_unittest.cpp:16-46 (FlateDecode), the short vectors.
    // PDFium reports these as successes with the given consumed counts; we
    // keep the output half, which is what our API exposes.
    #[test]
    fn reference_vectors() {
        let cases: [(&[u8], &[u8]); 6] = [
            (b"", b""),
            // Not a zlib header: two bytes read, nothing inflated, success.
            (b"preposterous nonsense", b""),
            (&[0x78, 0x9c, 0x03, 0x00, 0x00, 0x00, 0x00, 0x01], b""),
            (
                &[0x78, 0x9c, 0x53, 0x00, 0x00, 0x00, 0x21, 0x00, 0x21],
                b" ",
            ),
            (
                &[
                    0x78, 0x9c, 0x33, 0x34, 0x32, 0x06, 0x00, 0x01, 0x2d, 0x00, 0x97,
                ],
                b"123",
            ),
            (
                &[0x78, 0x9c, 0x63, 0xf8, 0x0f, 0x00, 0x01, 0x01, 0x01, 0x00],
                b"\x00\xff",
            ),
        ];
        for (input, expected) in cases {
            assert_eq!(decode(input).0, expected, "{input:02x?}");
        }
    }

    // The long vector from flatemodule_unittest.cpp:25-35: a real page content
    // stream, multi-block, 96 compressed bytes to 111 decoded ones.
    #[test]
    fn a_real_content_stream_inflates_whole() {
        let compressed: [u8; 96] = [
            0x78, 0x9c, 0x33, 0x54, 0x30, 0x00, 0x42, 0x5d, 0x43, 0x05, 0x23, 0x4b, 0x05, 0x73,
            0x33, 0x63, 0x85, 0xe4, 0x5c, 0x2e, 0x90, 0x80, 0xa9, 0xa9, 0xa9, 0x82, 0xb9, 0xb1,
            0xa9, 0x42, 0x51, 0x2a, 0x57, 0xb8, 0x42, 0x1e, 0x57, 0x21, 0x92, 0xa0, 0x89, 0x9e,
            0xb1, 0xa5, 0x09, 0x92, 0x84, 0x9e, 0x85, 0x81, 0x81, 0x25, 0xd8, 0x14, 0x24, 0x26,
            0xd0, 0x18, 0x43, 0x05, 0x10, 0x0c, 0x72, 0x57, 0x80, 0x30, 0x8a, 0xd2, 0xb9, 0xf4,
            0xdd, 0x0d, 0x14, 0xd2, 0x8b, 0xc1, 0x46, 0x99, 0x59, 0x1a, 0x2b, 0x58, 0x1a, 0x9a,
            0x83, 0x8c, 0x49, 0xe3, 0x0a, 0x04, 0x42, 0x00, 0x37, 0x4c, 0x1b, 0x42,
        ];
        let expected = concat!(
            "1 0 0 -1 29 763 cm\n0 0 555 735 re\nW n\nq\n0 0 555 734.394 re\n",
            "W n\nq\n0.8009 0 0 0.8009 0 0 cm\n1 1 1 RG 1 1 1 rg\n/G0 gs\n",
            "0 0 693 917 re\nf\nQ\nQ\n"
        );
        let (out, diags) = decode(&compressed);
        assert_eq!(out, expected.as_bytes());
        assert_eq!(diags, 0, "a complete stream needs no recovery");
    }

    #[test]
    fn a_complete_stream_round_trips_without_a_diagnostic() {
        let payload: Vec<u8> = (0..1000u32).map(|i| (i % 251) as u8).collect();
        let (out, diags) = decode(&compress(&payload));
        assert_eq!(out, payload);
        assert_eq!(diags, 0);
    }

    #[test]
    fn trailing_garbage_after_the_end_marker_is_invisible() {
        let payload: Vec<u8> = (0..1000u32).map(|i| (i % 251) as u8).collect();
        let mut stream = compress(&payload);
        stream.extend_from_slice(b"garbage");
        let (out, diags) = decode(&stream);
        assert_eq!(out, payload, "the full payload survives");
        assert_eq!(diags, 0, "the garbage sits past the end marker");
    }

    #[test]
    fn truncation_yields_a_monotonically_growing_prefix() {
        let payload: Vec<u8> = (0..1000u32).map(|i| (i % 251) as u8).collect();
        let stream = compress(&payload);
        let mut previous = 0usize;
        for cut in [0usize, 1, 2, 3, 10, stream.len() / 2, stream.len() - 1] {
            let slice = stream.get(..cut).expect("a prefix");
            let mut diags = Diagnostics::default();
            let out = decode_flate(slice, 0, &Limits::default(), &mut diags)
                .expect("truncation is never an error");
            assert!(
                out.len() >= previous,
                "cut {cut}: {} bytes after {previous}",
                out.len()
            );
            assert!(payload.starts_with(&out), "cut {cut} is a real prefix");
            previous = out.len();
            if cut > 0 {
                assert_eq!(diags.len(), 1, "cut {cut} records its truncation");
            }
        }
    }

    #[test]
    fn a_corrupt_header_byte_produces_nothing_and_a_diagnostic() {
        let payload = vec![7u8; 1000];
        let mut stream = compress(&payload);
        *stream.first_mut().expect("a header") ^= 0xff;
        let (out, diags) = decode(&stream);
        assert!(out.is_empty());
        assert_eq!(diags, 1);
    }

    #[test]
    fn a_corrupt_byte_mid_stream_keeps_the_prefix_and_never_panics() {
        let payload: Vec<u8> = (0..4000u32).map(|i| (i % 251) as u8).collect();
        let stream = compress(&payload);
        for offset in [3usize, 7, stream.len() / 3, stream.len() / 2] {
            let mut damaged = stream.clone();
            if let Some(byte) = damaged.get_mut(offset) {
                *byte ^= 0x5a;
            }
            let mut diags = Diagnostics::default();
            let out = decode_flate(&damaged, 0, &Limits::default(), &mut diags)
                .expect("corruption is never an error");
            assert!(out.len() <= payload.len() + 4096, "no runaway output");
        }
    }

    #[test]
    fn a_zip_bomb_is_refused_at_the_configured_limit() {
        // Ten mebibytes of zeroes compresses to a few kilobytes.
        let bomb = compress(&vec![0u8; 10 * 1024 * 1024]);
        let limits = Limits {
            max_decoded_stream_len: 1024 * 1024,
            ..Limits::default()
        };
        let mut diags = Diagnostics::default();
        assert_eq!(
            decode_flate(&bomb, 0, &limits, &mut diags),
            Err(Error::OutputTooLarge { limit: 1024 * 1024 })
        );
    }

    // A stream whose decoded size sits far above the budget must not allocate
    // its way there first: the ceiling bounds the buffer, not merely the value
    // that comes back.
    #[test]
    fn the_buffer_never_grows_past_the_ceiling() {
        let bomb = compress(&vec![0u8; 8 * 1024 * 1024]);
        for limit in [0usize, 1, 100, 4096, 65_536] {
            let limits = Limits {
                max_decoded_stream_len: limit,
                ..Limits::default()
            };
            let mut diags = Diagnostics::default();
            assert_eq!(
                decode_flate(&bomb, 20_000_000, &limits, &mut diags),
                Err(Error::OutputTooLarge { limit }),
                "limit {limit}"
            );
        }
    }

    // A payload that fits exactly is not a bomb: the budget is inclusive, so
    // the last byte of room is usable rather than the trigger for a refusal.
    #[test]
    fn a_payload_of_exactly_the_ceiling_decodes() {
        let payload = vec![9u8; 4096];
        let limits = Limits {
            max_decoded_stream_len: payload.len(),
            ..Limits::default()
        };
        let mut diags = Diagnostics::default();
        let out = decode_flate(&compress(&payload), 0, &limits, &mut diags).expect("it fits");
        assert_eq!(out, payload);
    }

    // The writer's half : whatever the
    // compressor chooses, decode_flate must read it back exactly.
    #[test]
    fn encode_round_trips_through_decode() {
        for payload in [
            Vec::new(),
            b"round trip".to_vec(),
            vec![0u8; 100_000],
            (0..30_000u32).map(|i| (i % 251) as u8).collect(),
        ] {
            let mut diags = Diagnostics::default();
            let out = decode_flate(&encode_flate(&payload), 0, &Limits::default(), &mut diags)
                .expect("our own output decodes");
            assert_eq!(out, payload);
            assert_eq!(diags.len(), 0, "our own output is never damaged");
        }
    }

    #[test]
    fn encode_shrinks_compressible_input() {
        let payload = vec![b'a'; 50_000];
        assert!(encode_flate(&payload).len() < payload.len() / 10);
    }

    #[test]
    fn the_estimated_size_hint_does_not_change_the_output() {
        let payload: Vec<u8> = (0..5000u32).map(|i| (i % 97) as u8).collect();
        let stream = compress(&payload);
        for hint in [0usize, 1, 5000, 20_000_000] {
            let mut diags = Diagnostics::default();
            let out = decode_flate(&stream, hint, &Limits::default(), &mut diags).expect("decodes");
            assert_eq!(out, payload, "hint {hint}");
        }
    }
}
