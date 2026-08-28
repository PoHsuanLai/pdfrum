//! The byte decoder: how a PDF string splits into character codes
//! (ISO 32000-1 §9.7.6.2), and the inverse encoder.
//!
//! The governing property is that decoding **never fails and never stalls**.
//! A byte that no codespace range accepts, or a multi-byte code cut off by the
//! end of the string, produces character code 0 — a real code, which maps to
//! CID 0 and renders as `.notdef` — rather than an error. That is what lets a
//! truncated or mislabelled string still lay out text instead of dropping the
//! whole show operator, and text extraction is measured against exactly those
//! codes.

use crate::ids::{CharCode, CodingScheme};

/// One codespace range: how wide a code is and which bytes each position may
/// hold (ISO 32000-1 §9.7.6.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CodeRange {
    /// Bytes per code, 0 through 4.
    pub(crate) char_size: u8,
    pub(crate) lower: [u8; 4],
    pub(crate) upper: [u8; 4],
}

/// Which bytes start a two-byte code under a mixed-two-byte scheme.
///
/// Indexed by the raw byte, so every lookup is total — there is no
/// out-of-range byte — which is why this is a newtype and not a bare array.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct LeadingBytes([bool; 256]);

impl LeadingBytes {
    /// A set with nothing in it.
    pub(crate) fn none() -> Self {
        Self([false; 256])
    }

    /// Add the inclusive byte range `first..=last`.
    pub(crate) fn add(&mut self, first: u8, last: u8) {
        for b in first..=last {
            if let Some(slot) = self.0.get_mut(usize::from(b)) {
                *slot = true;
            }
        }
    }

    /// Whether `b` starts a two-byte code.
    pub(crate) fn contains(&self, b: u8) -> bool {
        self.0.get(usize::from(b)).copied().unwrap_or(false)
    }

    /// Whether a code value below `0x100` collides with the set.
    fn contains_code(&self, code: u32) -> bool {
        u8::try_from(code).is_ok_and(|b| self.contains(b))
    }
}

impl std::fmt::Debug for LeadingBytes {
    /// 256 booleans make an unreadable assertion message; print the ranges.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut list = f.debug_list();
        let mut start: Option<u8> = None;
        for b in 0..=255u8 {
            match (self.contains(b), start) {
                (true, None) => start = Some(b),
                (false, Some(s)) => {
                    list.entry(&format_args!("{s:#04x}..={:#04x}", b - 1));
                    start = None;
                }
                _ => {}
            }
        }
        if let Some(s) = start {
            list.entry(&format_args!("{s:#04x}..=0xff"));
        }
        list.finish()
    }
}

/// How this CMap splits bytes into codes. Each variant carries exactly what
/// its rule needs, so a mixed-two-byte decoder without a leading-byte set —
/// representable in the C++ and reachable there only by accident — cannot be
/// built here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Decoder {
    OneByte,
    TwoBytes,
    MixedTwoBytes {
        leading: Box<LeadingBytes>,
    },
    /// The range list may be empty; see [`check_range`] for what that means.
    MixedFourBytes {
        ranges: Vec<CodeRange>,
    },
}

impl Decoder {
    pub(crate) fn scheme(&self) -> CodingScheme {
        match self {
            Self::OneByte => CodingScheme::OneByte,
            Self::TwoBytes => CodingScheme::TwoBytes,
            Self::MixedTwoBytes { .. } => CodingScheme::MixedTwoBytes,
            Self::MixedFourBytes { .. } => CodingScheme::MixedFourBytes,
        }
    }
}

/// Read one byte and advance, or yield 0 at the end of the string without
/// advancing. The asymmetry is the whole damage-tolerance story of the decoder
/// and is relied on by every arm below.
fn byte_or_zero(bytes: &[u8], offset: &mut usize) -> u8 {
    match bytes.get(*offset) {
        Some(&b) => {
            *offset += 1;
            b
        }
        None => 0,
    }
}

/// What a partial code looks like against the codespace ranges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RangeVerdict {
    /// No range can accept these bytes; the decoder gives up on this code.
    NoMatch,
    /// A prefix matches but the code is not finished; read another byte.
    NeedMore,
    /// These bytes are a complete code.
    Complete,
}

/// Classify the accumulated bytes of a code against the codespace ranges.
///
/// Ranges are consulted **last-declared first**, so a range declared later
/// shadows an earlier one that would also have matched.
///
/// The second acceptance rule is the surprising one: a range whose bytes do
/// *not* all fall inside its bounds still yields [`Complete`] as long as at
/// least the first byte matched and the input is already as wide as that
/// range. So with a range `<8140>`–`<9FFC>`, the input `81 FF` is accepted as
/// the code `0x81FF` even though `0xFF` is past the upper bound. This
/// tolerance keeps mis-encoded CJK strings decoding at the right width instead
/// of collapsing to single bytes, and it changes which CIDs come out, so it is
/// reproduced rather than tightened.
///
/// An empty range list yields [`NoMatch`] immediately, which makes every code
/// 0 — the decoder still consumes a byte per call, so iteration terminates.
fn check_range(codes: &[u8], ranges: &[CodeRange]) -> RangeVerdict {
    for range in ranges.iter().rev() {
        if usize::from(range.char_size) < codes.len() {
            continue;
        }
        let matched = codes
            .iter()
            .zip(range.lower.iter().zip(range.upper.iter()))
            .take_while(|(c, (lo, hi))| *c >= lo && *c <= hi)
            .count();
        if matched == usize::from(range.char_size) {
            return RangeVerdict::Complete;
        }
        if matched != 0 {
            return if codes.len() == usize::from(range.char_size) {
                RangeVerdict::Complete
            } else {
                RangeVerdict::NeedMore
            };
        }
    }
    RangeVerdict::NoMatch
}

/// Decode one character code, advancing `offset`.
///
/// Returns code 0 on any damage — a byte outside every codespace range, or a
/// multi-byte code truncated by the end of the string — never an error.
pub(crate) fn next_char(decoder: &Decoder, bytes: &[u8], offset: &mut usize) -> CharCode {
    let code = match decoder {
        Decoder::OneByte => u32::from(byte_or_zero(bytes, offset)),
        Decoder::TwoBytes => {
            let hi = byte_or_zero(bytes, offset);
            let lo = byte_or_zero(bytes, offset);
            u32::from(hi) * 256 + u32::from(lo)
        }
        Decoder::MixedTwoBytes { leading } => {
            let hi = byte_or_zero(bytes, offset);
            if leading.contains(hi) {
                let lo = byte_or_zero(bytes, offset);
                u32::from(hi) * 256 + u32::from(lo)
            } else {
                u32::from(hi)
            }
        }
        Decoder::MixedFourBytes { ranges } => {
            let mut codes = [0u8; 4];
            codes[0] = byte_or_zero(bytes, offset);
            let mut width = 1usize;
            loop {
                match check_range(codes.get(..width).unwrap_or(&codes), ranges) {
                    RangeVerdict::NoMatch => break 0,
                    RangeVerdict::Complete => {
                        break codes
                            .get(..width)
                            .unwrap_or(&codes)
                            .iter()
                            .fold(0u32, |acc, &b| (acc << 8) + u32::from(b));
                    }
                    RangeVerdict::NeedMore => {}
                }
                // A code that wants a fifth byte, or that runs out of string,
                // is abandoned as code 0 — and the offset is *not* advanced
                // past the end, so iteration stops here.
                if width == 4 || *offset == bytes.len() {
                    break 0;
                }
                let Some(&next) = bytes.get(*offset) else {
                    break 0;
                };
                if let Some(slot) = codes.get_mut(width) {
                    *slot = next;
                }
                *offset += 1;
                width += 1;
            }
        }
    };
    CharCode(code)
}

/// How many bytes a code of this value occupies — derived from the value, not
/// from the byte string it came from.
pub(crate) fn char_size(decoder: &Decoder, code: CharCode) -> u8 {
    match decoder {
        Decoder::OneByte => 1,
        Decoder::TwoBytes => 2,
        Decoder::MixedTwoBytes { .. } => u8::from(code.0 >= 0x100) + 1,
        Decoder::MixedFourBytes { .. } => match code.0 {
            0..0x100 => 1,
            0x100..0x1_0000 => 2,
            0x1_0000..0x100_0000 => 3,
            _ => 4,
        },
    }
}

/// Count the character codes in a byte string.
pub(crate) fn count_chars(decoder: &Decoder, bytes: &[u8]) -> usize {
    match decoder {
        Decoder::OneByte => bytes.len(),
        Decoder::TwoBytes => bytes.len().div_ceil(2),
        Decoder::MixedTwoBytes { leading } => {
            let mut count = 0usize;
            let mut i = 0usize;
            while i < bytes.len() {
                count += 1;
                // A trailing leading byte still counts as one code, matching
                // the decoder's own zero-filled second byte.
                if bytes.get(i).is_some_and(|&b| leading.contains(b)) {
                    i += 1;
                }
                i += 1;
            }
            count
        }
        Decoder::MixedFourBytes { .. } => {
            let mut count = 0usize;
            let mut offset = 0usize;
            while offset < bytes.len() {
                let before = offset;
                next_char(decoder, bytes, &mut offset);
                count += 1;
                if offset == before {
                    break;
                }
            }
            count
        }
    }
}

/// The width a four-byte-scheme encoder pads a sub-0x100 code to.
///
/// A separate walk from [`check_range`] and deliberately stricter: it accepts
/// only a *full* bounds match, and it tries the widest size first, asking "is
/// there a codespace range that would have produced this code at width 4, then
/// 3, then 2, then 1". With no ranges at all the answer is 1.
fn four_byte_width(code: u32, ranges: &[CodeRange]) -> usize {
    if ranges.is_empty() {
        return 1;
    }
    let codes = [0u8, 0, ((code >> 8) & 0xFF) as u8, code as u8];
    for offset in 0..4usize {
        let size = 4 - offset;
        for range in ranges.iter().rev() {
            if usize::from(range.char_size) < size {
                continue;
            }
            let matched = (0..size)
                .take_while(|&j| {
                    let (Some(&c), Some(&lo), Some(&hi)) = (
                        codes.get(offset + j),
                        range.lower.get(j),
                        range.upper.get(j),
                    ) else {
                        return false;
                    };
                    c >= lo && c <= hi
                })
                .count();
            if matched == usize::from(range.char_size) {
                return size;
            }
        }
    }
    1
}

/// Append a character code to a byte string in this CMap's encoding — the
/// inverse of [`next_char`], used to rebuild a searchable string.
pub(crate) fn append_char(decoder: &Decoder, out: &mut Vec<u8>, code: CharCode) {
    let c = code.0;
    match decoder {
        Decoder::OneByte => out.push(c as u8),
        Decoder::TwoBytes => {
            out.push((c / 256) as u8);
            out.push((c % 256) as u8);
        }
        Decoder::MixedTwoBytes { leading } => {
            if c < 0x100 && !leading.contains_code(c) {
                out.push(c as u8);
            } else {
                out.push((c >> 8) as u8);
                out.push(c as u8);
            }
        }
        Decoder::MixedFourBytes { ranges } => match c {
            0..0x100 => {
                let width = four_byte_width(c, ranges);
                out.resize(out.len() + width.saturating_sub(1), 0);
                out.push(c as u8);
            }
            0x100..0x1_0000 => out.extend_from_slice(&[(c >> 8) as u8, c as u8]),
            0x100_0000.. => out.extend_from_slice(&c.to_be_bytes()),
            _ => out.extend_from_slice(&[(c >> 16) as u8, (c >> 8) as u8, c as u8]),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CodeRange, Decoder, LeadingBytes, RangeVerdict, append_char, char_size, check_range,
        count_chars, next_char,
    };
    use crate::ids::CharCode;

    fn codes(decoder: &Decoder, bytes: &[u8]) -> (Vec<u32>, usize) {
        let mut out = Vec::new();
        let mut offset = 0usize;
        while offset < bytes.len() {
            let before = offset;
            out.push(next_char(decoder, bytes, &mut offset).0);
            if offset == before {
                break;
            }
        }
        (out, offset)
    }

    fn mixed_two() -> Decoder {
        let mut leading = LeadingBytes::none();
        leading.add(0xa1, 0xfe);
        Decoder::MixedTwoBytes {
            leading: Box::new(leading),
        }
    }

    fn range(size: u8, lower: [u8; 4], upper: [u8; 4]) -> CodeRange {
        CodeRange {
            char_size: size,
            lower,
            upper,
        }
    }

    #[test]
    fn one_byte_is_byte_for_byte() {
        let d = Decoder::OneByte;
        assert_eq!(codes(&d, b"ABC"), (vec![0x41, 0x42, 0x43], 3));
        assert_eq!(count_chars(&d, b"ABC"), 3);
        assert_eq!(codes(&d, b""), (vec![], 0));
        assert_eq!(count_chars(&d, b""), 0);
    }

    /// An odd trailing byte becomes a code with a zero low byte, and the
    /// offset stops at the end, so iteration ends after it.
    #[test]
    fn two_bytes_pads_an_odd_tail() {
        let d = Decoder::TwoBytes;
        assert_eq!(codes(&d, b"ABC"), (vec![0x4142, 0x4300], 3));
        assert_eq!(count_chars(&d, b"ABC"), 2);
        assert_eq!(codes(&d, b"AB"), (vec![0x4142], 2));
        assert_eq!(count_chars(&d, b"AB"), 1);
        assert_eq!(codes(&d, b""), (vec![], 0));
        assert_eq!(count_chars(&d, b""), 0);
    }

    /// `0xFF` is outside the `a1..=fe` leading set, so it is a one-byte code
    /// even though it looks like a lead byte.
    #[test]
    fn mixed_two_bytes_splits_on_the_leading_set() {
        let d = mixed_two();
        let (out, off) = codes(&d, &[0x41, 0xA1, 0xA2, 0xFF]);
        assert_eq!(out, vec![0x41, 0xA1A2, 0xFF]);
        assert_eq!(off, 4);
        assert_eq!(count_chars(&d, &[0x41, 0xA1, 0xA2, 0xFF]), 3);
    }

    /// A lead byte at the very end takes a zero second byte and consumes only
    /// the one byte it had.
    #[test]
    fn mixed_two_bytes_tolerates_a_truncated_pair() {
        let d = mixed_two();
        assert_eq!(codes(&d, &[0xA1]), (vec![0xA100], 1));
        assert_eq!(count_chars(&d, &[0xA1]), 1);
        assert_eq!(codes(&d, &[0x41, 0xA1]), (vec![0x41, 0xA100], 2));
        assert_eq!(count_chars(&d, &[0x41, 0xA1]), 2);
    }

    /// `count_chars` must agree with the number of codes the decoder yields,
    /// for every scheme and every input — the text layer counts one and
    /// iterates the other.
    #[test]
    fn count_chars_agrees_with_iteration() {
        let four = Decoder::MixedFourBytes {
            ranges: vec![
                range(1, [0x00, 0, 0, 0], [0x80, 0, 0, 0]),
                range(2, [0x81, 0x40, 0, 0], [0x9F, 0xFC, 0, 0]),
            ],
        };
        let decoders = [Decoder::OneByte, Decoder::TwoBytes, mixed_two(), four];
        let inputs: &[&[u8]] = &[
            b"",
            b"A",
            b"AB",
            b"ABC",
            &[0xA1],
            &[0xA1, 0xA2],
            &[0x81, 0x40, 0x41],
            &[0x81],
            &[0xFF, 0xFF, 0xFF],
            &[0x00, 0x81, 0x40],
        ];
        for d in &decoders {
            for input in inputs {
                let (out, _) = codes(d, input);
                assert_eq!(
                    out.len(),
                    count_chars(d, input),
                    "{d:?} over {input:?} yielded {out:?}"
                );
            }
        }
    }

    #[test]
    fn four_byte_ranges_pick_the_code_width() {
        let d = Decoder::MixedFourBytes {
            ranges: vec![
                range(1, [0x00, 0, 0, 0], [0x80, 0, 0, 0]),
                range(2, [0x81, 0x40, 0, 0], [0x9F, 0xFC, 0, 0]),
            ],
        };
        assert_eq!(codes(&d, &[0x41]), (vec![0x41], 1));
        assert_eq!(codes(&d, &[0x81, 0x40]), (vec![0x8140], 2));
        // The tolerance arm: 0xFF is past the second byte's upper bound, but
        // the input is already the range's full width, so it is accepted.
        assert_eq!(codes(&d, &[0x81, 0xFF]), (vec![0x81FF], 2));
        // A lead byte with nothing after it is abandoned as code 0.
        assert_eq!(codes(&d, &[0x81]), (vec![0], 1));
    }

    /// Later-declared ranges shadow earlier ones.
    #[test]
    fn four_byte_ranges_are_searched_backwards() {
        let d = Decoder::MixedFourBytes {
            ranges: vec![
                range(2, [0x00, 0x00, 0, 0], [0xFF, 0xFF, 0, 0]),
                range(1, [0x00, 0, 0, 0], [0xFF, 0, 0, 0]),
            ],
        };
        // The one-byte range is declared last and wins.
        assert_eq!(codes(&d, &[0x41, 0x42]), (vec![0x41, 0x42], 2));
    }

    /// An empty range list makes every code 0 while still consuming a byte
    /// per step, so iteration terminates rather than spinning.
    #[test]
    fn four_byte_with_no_ranges_yields_zeros_and_terminates() {
        let d = Decoder::MixedFourBytes { ranges: Vec::new() };
        assert_eq!(check_range(&[0x41], &[]), RangeVerdict::NoMatch);
        let (out, off) = codes(&d, &[0x41, 0x42, 0x43]);
        assert_eq!(out, vec![0, 0, 0]);
        assert_eq!(off, 3);
        assert_eq!(count_chars(&d, &[0x41, 0x42, 0x43]), 3);
    }

    /// A code that wants a fifth byte is abandoned.
    #[test]
    fn four_byte_stops_at_four() {
        let d = Decoder::MixedFourBytes {
            ranges: vec![range(4, [0x00; 4], [0xFF; 4])],
        };
        assert_eq!(codes(&d, &[1, 2, 3, 4]), (vec![0x0102_0304], 4));
        assert_eq!(codes(&d, &[1, 2, 3]), (vec![0], 3));
    }

    #[test]
    fn char_size_reads_the_value_not_the_bytes() {
        assert_eq!(char_size(&Decoder::OneByte, CharCode(0xFFFF)), 1);
        assert_eq!(char_size(&Decoder::TwoBytes, CharCode(0x1)), 2);
        let m = mixed_two();
        assert_eq!(char_size(&m, CharCode(0xFF)), 1);
        assert_eq!(char_size(&m, CharCode(0x100)), 2);
        let f = Decoder::MixedFourBytes { ranges: Vec::new() };
        for (code, want) in [
            (0xFFu32, 1u8),
            (0x100, 2),
            (0xFFFF, 2),
            (0x1_0000, 3),
            (0xFF_FFFF, 3),
            (0x100_0000, 4),
            (u32::MAX, 4),
        ] {
            assert_eq!(char_size(&f, CharCode(code)), want, "code {code:#x}");
        }
    }

    /// Re-encoding a code the decoder can actually *produce* gives back bytes
    /// the decoder reads as that same code.
    ///
    /// The qualifier matters under a mixed-two-byte scheme, where the code
    /// space has holes in both directions: a one-byte code whose value is a
    /// lead byte cannot occur (the decoder would have read a pair), and
    /// neither can a two-byte code whose high byte is *not* a lead byte (the
    /// decoder would have read two singles). Round-tripping is exact over the
    /// codes that remain; the holes are pinned by
    /// [`unencodable_mixed_two_byte_codes`].
    #[test]
    fn append_char_round_trips_through_next_char() {
        let four = Decoder::MixedFourBytes {
            ranges: vec![
                range(1, [0x00, 0, 0, 0], [0x80, 0, 0, 0]),
                range(2, [0x81, 0x40, 0, 0], [0x9F, 0xFC, 0, 0]),
            ],
        };
        for d in [Decoder::OneByte, Decoder::TwoBytes, mixed_two(), four] {
            let sample: Vec<u32> = match d {
                Decoder::OneByte => (0..=0xFFu32).collect(),
                Decoder::TwoBytes => (0..=0xFFFFu32).step_by(7).collect(),
                // A code is reachable only if its own width agrees with the
                // leading-byte set: one byte iff that byte is not a lead
                // byte, two bytes iff the high byte is. See the doc comment.
                Decoder::MixedTwoBytes { ref leading } => (0..=0xFFFFu32)
                    .step_by(11)
                    .filter(|&c| {
                        if c < 0x100 {
                            !leading.contains_code(c)
                        } else {
                            leading.contains_code(c >> 8)
                        }
                    })
                    .collect(),
                Decoder::MixedFourBytes { .. } => {
                    (0..=0x80u32).chain((0x8140..=0x9FFC).step_by(13)).collect()
                }
            };
            for code in sample {
                let mut buf = Vec::new();
                append_char(&d, &mut buf, CharCode(code));
                let mut offset = 0usize;
                let back = next_char(&d, &buf, &mut offset);
                assert_eq!(back.0, code, "{d:?} round trip {code:#x} via {buf:?}");
                assert_eq!(offset, buf.len());
            }
        }
    }

    /// The padding walk that only `append_char` uses: with a two-byte range
    /// covering `[0x00,0x00]`–`[0xFF,0xFF]`, a sub-0x100 code is written as
    /// two bytes, not one.
    #[test]
    fn four_byte_encoder_pads_narrow_codes() {
        let d = Decoder::MixedFourBytes {
            ranges: vec![range(2, [0x00, 0x00, 0, 0], [0xFF, 0xFF, 0, 0])],
        };
        let mut buf = Vec::new();
        append_char(&d, &mut buf, CharCode(0x41));
        assert_eq!(buf, vec![0x00, 0x41]);

        // With no ranges the width is 1.
        let empty = Decoder::MixedFourBytes { ranges: Vec::new() };
        let mut buf = Vec::new();
        append_char(&empty, &mut buf, CharCode(0x41));
        assert_eq!(buf, vec![0x41]);
    }

    #[test]
    fn four_byte_encoder_writes_wide_codes_big_endian() {
        let d = Decoder::MixedFourBytes { ranges: Vec::new() };
        for (code, want) in [
            (0x0102u32, vec![0x01, 0x02]),
            (0x0001_0203 & 0x00FF_FFFF, vec![0x01, 0x02, 0x03]),
            (0x0102_0304, vec![0x01, 0x02, 0x03, 0x04]),
        ] {
            let mut buf = Vec::new();
            append_char(&d, &mut buf, CharCode(code));
            assert_eq!(buf, want, "code {code:#x}");
        }
    }

    /// A mixed-two-byte code below 0x100 whose value *is* a lead byte is
    /// written as two bytes, because one byte would decode as a pair.
    ///
    /// This is the one place `char_size` and the encoder disagree, and the
    /// disagreement is in the original: `char_size` answers from the value
    /// alone (`< 0x100` is one byte) while the encoder also consults the
    /// leading-byte set. Callers that need the byte count of an encoded code
    /// must measure what `append_char` wrote, not ask `char_size`.
    #[test]
    fn mixed_two_byte_encoder_never_emits_a_bare_lead_byte() {
        let d = mixed_two();
        let mut buf = Vec::new();
        append_char(&d, &mut buf, CharCode(0xA1));
        assert_eq!(buf, vec![0x00, 0xA1]);
        assert_eq!(
            char_size(&d, CharCode(0xA1)),
            1,
            "the documented disagreement"
        );

        let mut buf = Vec::new();
        append_char(&d, &mut buf, CharCode(0x41));
        assert_eq!(buf, vec![0x41]);
        assert_eq!(char_size(&d, CharCode(0x41)), 1);
    }

    /// The two holes in a mixed-two-byte code space. Neither code can come out
    /// of the decoder, so nothing observable is lost, but the encoder accepts
    /// them and does not round-trip — pinned so nobody "fixes" it into a
    /// different byte string than the original writes.
    #[test]
    fn unencodable_mixed_two_byte_codes() {
        let d = mixed_two();

        // A one-byte code whose value is a lead byte.
        let mut buf = Vec::new();
        append_char(&d, &mut buf, CharCode(0xA5));
        assert_eq!(buf, vec![0x00, 0xA5]);
        // 0x00 is not a lead byte, so it stands alone; 0xA5 then opens a pair
        // that the end of the string closes with a zero.
        assert_eq!(codes(&d, &buf), (vec![0x00, 0xA500], 2));

        // A two-byte code whose high byte is not a lead byte.
        let mut buf = Vec::new();
        append_char(&d, &mut buf, CharCode(0x0108));
        assert_eq!(buf, vec![0x01, 0x08]);
        assert_eq!(codes(&d, &buf), (vec![0x01, 0x08], 2));
    }

    /// Everywhere else `char_size` and the encoder agree.
    #[test]
    fn char_size_matches_the_encoded_width() {
        let four = Decoder::MixedFourBytes {
            ranges: vec![range(2, [0x81, 0x40, 0, 0], [0x9F, 0xFC, 0, 0])],
        };
        for (d, sample) in [
            (Decoder::OneByte, (0..=0xFFu32).collect::<Vec<_>>()),
            (Decoder::TwoBytes, (0..=0xFFFFu32).step_by(97).collect()),
            (four, (0x8140..=0x9FFCu32).step_by(97).collect()),
        ] {
            for code in sample {
                let mut buf = Vec::new();
                append_char(&d, &mut buf, CharCode(code));
                assert_eq!(
                    usize::from(char_size(&d, CharCode(code))),
                    buf.len(),
                    "{d:?} code {code:#x}"
                );
            }
        }
    }
}
