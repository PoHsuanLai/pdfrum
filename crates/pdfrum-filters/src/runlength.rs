//! `/RunLengthDecode` (ISO 32000-1 §7.4.5): the one filter in this crate with
//! a hard output cap.
//!
//! The format is a sequence of length-prefixed runs. A byte `n < 128` opens a
//! literal run of `n + 1` bytes; a byte `n > 128` repeats the byte after it
//! `257 - n` times; the byte `128` ends the stream.
//!
//! Two properties make this filter's damage tolerance distinctive. The output
//! length is decided by a first pass over the run headers *alone*, so a
//! truncated run still contributes its full declared length — copied as far as
//! the input allows and zero-filled the rest of the way. And that same first
//! pass is where the 20 MiB rejection happens, before a single byte is
//! allocated, which is why a run-length bomb costs nothing to refuse.

use pdfrum_common::{DiagKind, Diagnostics, Severity};

use crate::Error;

/// The largest output `/RunLengthDecode` will produce, exclusive: a stream
/// whose runs add up to this many bytes or more is rejected outright.
///
/// PDFium's `kMaxStreamSize`. Unlike the crate-wide
/// [`Limits::max_decoded_stream_len`](pdfrum_common::Limits::max_decoded_stream_len)
/// — which we invented — this one is a rejection the oracle really performs,
/// so files exist that depend on it, exclusive bound included.
pub const RUN_LENGTH_MAX_OUTPUT: u64 = 20 * 1024 * 1024;

/// Decode `/RunLengthDecode` data, returning the bytes and how much of `input`
/// was read.
///
/// A run whose payload runs off the end of the input is not an error: the
/// bytes that exist are copied and the rest of the run is zeroes, keeping the
/// output at the length the run headers declared. Each such repair records a
/// diagnostic.
///
/// ```
/// use pdfrum_common::Diagnostics;
/// use pdfrum_filters::decode_run_length;
///
/// let mut diags = Diagnostics::default();
/// // A literal run of two bytes, then 254 -> three 'z's, then the end marker.
/// let input = [1, b'a', b'b', 254, b'z', 128];
/// assert_eq!(decode_run_length(&input, &mut diags)?, (b"abzzz".to_vec(), 6));
/// # Ok::<(), pdfrum_filters::Error>(())
/// ```
///
/// # Errors
///
/// [`Error::RunLengthTooLarge`] when the declared output reaches
/// [`RUN_LENGTH_MAX_OUTPUT`], and [`Error::SizeOverflow`] when it wraps a
/// `u32` first — PDFium checks both, in that order.
pub fn decode_run_length(input: &[u8], diags: &mut Diagnostics) -> Result<(Vec<u8>, usize), Error> {
    let size = declared_size(input)?;
    // usize on a 16-bit target could not hold this, but nothing this project
    // targets is 16-bit, and the cap above already bounds it to 20 MiB.
    let size = usize::try_from(size).map_err(|_| Error::SizeOverflow)?;

    let mut out = Vec::with_capacity(size);
    let mut i = 0usize;
    let mut truncated = false;
    while let Some(&header) = input.get(i) {
        if header == 128 {
            break;
        }
        if header < 128 {
            let want = usize::from(header) + 1;
            let available = input.get(i + 1..).unwrap_or_default();
            let take = want.min(available.len());
            out.extend_from_slice(available.get(..take).unwrap_or_default());
            // The run keeps its declared length whatever the input holds.
            out.resize(out.len() + (want - take), 0);
            if take < want {
                truncated = true;
            }
            i += want + 1;
        } else {
            // 129 repeats 128 times, 255 repeats twice, 128 never gets here.
            let count = 257 - usize::from(header);
            // A missing fill byte fills with zero rather than failing.
            let fill = input.get(i + 1).copied().unwrap_or_else(|| {
                truncated = true;
                0
            });
            out.resize(out.len() + count, fill);
            i += 2;
        }
    }
    if truncated {
        diags.record(
            Severity::Recovered,
            DiagKind::UndecodableStream,
            Some(i as u64),
        );
    }
    debug_assert_eq!(out.len(), size);
    Ok((out, (i + 1).min(input.len())))
}

/// First pass: how long the output will be, from the run headers alone.
fn declared_size(input: &[u8]) -> Result<u64, Error> {
    let mut size: u32 = 0;
    let mut i = 0usize;
    while let Some(&header) = input.get(i) {
        if header == 128 {
            break;
        }
        let (run, step) = if header < 128 {
            (u32::from(header) + 1, usize::from(header) + 2)
        } else {
            (257 - u32::from(header), 2)
        };
        // PDFium detects the wrap after the fact on a `uint32_t`; we refuse to
        // wrap in the first place, which rejects the same streams.
        size = size.checked_add(run).ok_or(Error::SizeOverflow)?;
        i += step;
    }
    let size = u64::from(size);
    if size >= RUN_LENGTH_MAX_OUTPUT {
        return Err(Error::RunLengthTooLarge { size });
    }
    Ok(size)
}

#[cfg(test)]
mod tests {
    use super::{RUN_LENGTH_MAX_OUTPUT, decode_run_length};
    use crate::Error;
    use pdfrum_common::Diagnostics;

    fn decode(input: &[u8]) -> (Vec<u8>, usize) {
        let mut diags = Diagnostics::default();
        decode_run_length(input, &mut diags).expect("decodes")
    }

    // From rle_unittest.cpp:25-32 (RLEShortInput, decode half).
    #[test]
    fn a_one_byte_literal_run_then_the_end_marker() {
        assert_eq!(decode(&[0, 1, 128]), (vec![1], 3));
    }

    // From rle_unittest.cpp:34-60: the encoder's output for this input, run
    // back through the decoder.
    #[test]
    fn mixed_literal_and_repeat_runs_round_trip() {
        let plain = [2u8, 2, 2, 2, 4, 4, 4, 4, 4, 4];
        // Four 2s, then six 4s, then the end marker.
        let encoded = [253u8, 2, 251, 4, 128];
        assert_eq!(decode(&encoded).0, plain);
    }

    #[test]
    fn a_repeat_run_of_the_maximum_length() {
        // 129 means 257 - 129 = 128 copies, the longest repeat run there is.
        let (out, consumed) = decode(&[129, b'q', 128]);
        assert_eq!(out, vec![b'q'; 128]);
        assert_eq!(consumed, 3);
    }

    #[test]
    fn a_literal_run_of_the_maximum_length() {
        // 127 means 128 literal bytes follow.
        let mut input = vec![127u8];
        input.extend(0..128u8);
        input.push(128);
        let (out, consumed) = decode(&input);
        assert_eq!(out.len(), 128);
        assert_eq!(out.first(), Some(&0));
        assert_eq!(out.last(), Some(&127));
        assert_eq!(consumed, 130);
    }

    #[test]
    fn a_truncated_literal_run_keeps_its_declared_length() {
        // Declares 128 literal bytes but supplies five.
        let input = [127u8, 1, 2, 3, 4, 5];
        let mut diags = Diagnostics::default();
        let (out, consumed) = decode_run_length(&input, &mut diags).expect("decodes");
        assert_eq!(out.len(), 128, "the pass-1 length survives");
        assert_eq!(out.get(..5), Some(&[1u8, 2, 3, 4, 5][..]));
        assert!(out.get(5..).expect("a tail").iter().all(|&b| b == 0));
        assert_eq!(consumed, 6);
        assert_eq!(diags.len(), 1, "the repair is recorded");
    }

    #[test]
    fn a_repeat_run_missing_its_fill_byte_fills_with_zero() {
        let mut diags = Diagnostics::default();
        let (out, consumed) = decode_run_length(&[250], &mut diags).expect("decodes");
        assert_eq!(out, vec![0u8; 7]);
        assert_eq!(consumed, 1);
        assert_eq!(diags.len(), 1);
    }

    #[test]
    fn a_missing_end_marker_decodes_to_the_end() {
        let input = [1u8, b'a', b'b', 0, b'c'];
        let (out, consumed) = decode(&input);
        assert_eq!(out, b"abc");
        // `min(i + 1, len)`: no end marker to account for.
        assert_eq!(consumed, input.len());
    }

    #[test]
    fn trailing_bytes_after_the_end_marker_are_not_read() {
        let (out, consumed) = decode(&[0, b'x', 128, b'j', b'u', b'n', b'k']);
        assert_eq!(out, b"x");
        assert_eq!(consumed, 3);
    }

    #[test]
    fn empty_input_decodes_to_nothing() {
        assert_eq!(decode(&[]), (Vec::new(), 0));
    }

    // The cap is exclusive: exactly 20 MiB is already too much.
    #[test]
    fn the_twenty_mebibyte_cap_rejects_at_the_boundary() {
        // 163_840 repeat runs of 128 bytes is exactly 20 MiB.
        let mut input = Vec::new();
        for _ in 0..163_840 {
            input.extend_from_slice(&[129, 0]);
        }
        let mut diags = Diagnostics::default();
        assert_eq!(
            decode_run_length(&input, &mut diags),
            Err(Error::RunLengthTooLarge {
                size: RUN_LENGTH_MAX_OUTPUT
            })
        );
    }

    #[test]
    fn one_byte_under_the_cap_still_decodes() {
        // 163_839 runs of 128, then a literal run making up the remaining 127.
        let mut input = Vec::new();
        for _ in 0..163_839 {
            input.extend_from_slice(&[129, 0]);
        }
        input.push(126);
        input.extend(std::iter::repeat_n(0u8, 127));
        input.push(128);
        let (out, _) = decode(&input);
        assert_eq!(out.len() as u64, RUN_LENGTH_MAX_OUTPUT - 1);
    }

    #[test]
    fn an_overflowing_declared_size_is_refused_before_allocating() {
        // Enough maximum-length repeat runs to wrap a u32 several times over
        // would be huge; the cap catches it first, which is the same refusal
        // without the allocation. Assert the shape, not the variant.
        let mut input = Vec::new();
        for _ in 0..200_000 {
            input.extend_from_slice(&[129, 0]);
        }
        let mut diags = Diagnostics::default();
        assert!(decode_run_length(&input, &mut diags).is_err());
    }
}
