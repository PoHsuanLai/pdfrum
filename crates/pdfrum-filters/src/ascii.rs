//! The two text encodings, `/ASCII85Decode` and `/ASCIIHexDecode`
//! (ISO 32000-1 §7.4.2 and §7.4.3).
//!
//! Both are deliberately forgiving, and in different ways. `ASCIIHex` skips any
//! byte that is not a hex digit — not a terminator, not an error — and keeps a
//! trailing odd nibble as a byte with a zero low half. `ASCII85` stops at the
//! first byte outside its alphabet, accepts a `z` shortcut even in the middle
//! of a group where the specification forbids it, and treats the `>` of the
//! `~>` terminator as one more byte to swallow (`EI` is what ends an inline
//! image, and the page layer needs the count to find it).
//!
//! Both return the number of input bytes they consumed alongside the output.
//! Only the inline-image reader in the page layer needs that count — it has to
//! know where `EI` begins — but it is also the sharpest way to pin these
//! decoders' edge cases in a test, so it stays in the signature.

use pdfrum_common::hex_digit;

/// Bytes ISO 32000-1 §7.2.3 calls white space and both text filters skip.
/// PDFium's predicate covers only these four; a NUL or a form feed is not
/// white space to it, and so terminates ASCII85.
fn is_space(ch: u8) -> bool {
    matches!(ch, b'\r' | b'\n' | b' ' | b'\t')
}

/// Whether `ch` is one of ASCII85's 85 code characters.
fn is_a85_digit(ch: u8) -> bool {
    (b'!'..=b'u').contains(&ch)
}

/// Decode `/ASCIIHexDecode` data, returning the bytes and how much of `input`
/// was read.
///
/// Cannot fail: every byte is either a hex digit, white space, the `>`
/// terminator, or ignored.
///
/// ```
/// use pdfrum_filters::decode_ascii_hex;
///
/// // Garbage between digits is skipped, and a lone trailing nibble becomes a
/// // whole byte with a zero low half (ISO 32000-1 §7.4.2).
/// assert_eq!(decode_ascii_hex(b"12tk  \tAc>zzz"), (b"\x12\xac".to_vec(), 10));
/// assert_eq!(decode_ascii_hex(b"12A>"), (b"\x12\xa0".to_vec(), 4));
/// ```
#[must_use]
pub fn decode_ascii_hex(input: &[u8]) -> (Vec<u8>, usize) {
    let mut out = Vec::with_capacity(input.len() / 2 + 1);
    let mut high: Option<u8> = None;
    let mut consumed = input.len();

    for (i, &ch) in input.iter().enumerate() {
        if ch == b'>' {
            // The terminator is consumed along with everything before it.
            consumed = i + 1;
            break;
        }
        if is_space(ch) {
            continue;
        }
        let Some(digit) = hex_digit(ch) else {
            // Not a terminator and not an error: PDFium simply ignores it.
            continue;
        };
        match high.take() {
            None => high = Some(digit << 4),
            Some(hi) => out.push(hi | digit),
        }
    }
    // A high nibble with no low nibble keeps its byte, low half zero.
    if let Some(hi) = high {
        out.push(hi);
    }
    (out, consumed)
}

/// Decode `/ASCII85Decode` data, returning the bytes and how much of `input`
/// was read.
///
/// ```
/// use pdfrum_filters::decode_ascii85;
///
/// assert_eq!(decode_ascii85(b"FCfN8~>")?, (b"test".to_vec(), 7));
/// // White space anywhere is free, and the `~>` terminator is consumed whole.
/// assert_eq!(decode_ascii85(b"\t F C\r\n \tf N 8 ~>")?, (b"test".to_vec(), 17));
/// # Ok::<(), pdfrum_filters::Error>(())
/// ```
///
/// # Errors
///
/// [`Error::SizeOverflow`](crate::Error::SizeOverflow) when the input holds so
/// many `z` shortcuts that their four-byte expansions overflow a `usize`,
/// which needs an input larger than any real address space.
pub fn decode_ascii85(input: &[u8]) -> Result<(Vec<u8>, usize), crate::Error> {
    if input.is_empty() {
        return Ok((Vec::new(), 0));
    }

    // Sizing pass, matching PDFium's: scan to the first byte that is neither a
    // code character, a `z`, nor white space, counting `z`s on the way. The
    // `~` of `~>` is 0x7E, past `u`, so the scan stops there too.
    let mut zeroes = 0usize;
    let mut legal = 0usize;
    for &ch in input {
        if ch == b'z' {
            zeroes += 1;
        } else if !is_a85_digit(ch) && !is_space(ch) {
            break;
        }
        legal += 1;
    }
    if legal == 0 {
        return Ok((Vec::new(), 0));
    }
    let capacity = zeroes
        .checked_mul(4)
        .and_then(|z| z.checked_add((legal - zeroes) / 5 * 4 + 4))
        .ok_or(crate::Error::SizeOverflow)?;

    let mut out = Vec::with_capacity(capacity);
    let mut group = 0u32;
    let mut in_group = 0usize;
    // `pos` counts bytes *taken*, so it is already past the byte that breaks
    // the loop — which is what makes an unknown byte cost one consumed byte.
    let mut pos = 0usize;

    for &ch in input {
        pos += 1;
        if is_space(ch) {
            continue;
        }
        if ch == b'z' {
            // Four zero bytes, and the group resets even mid-group. Illegal
            // per the specification, accepted here.
            out.extend_from_slice(&[0; 4]);
            group = 0;
            in_group = 0;
            continue;
        }
        if !is_a85_digit(ch) {
            break;
        }
        // Wrapping is the C++'s `uint32_t` arithmetic: a five-character group
        // may encode a value past `u32::MAX`, and PDFium keeps the low bits.
        group = group
            .wrapping_mul(85)
            .wrapping_add(u32::from(ch) - u32::from(b'!'));
        in_group += 1;
        if in_group == 5 {
            out.extend_from_slice(&group.to_be_bytes());
            group = 0;
            in_group = 0;
        }
    }

    // A partial trailing group is padded with the highest code character and
    // yields one byte fewer than it has characters — so a group of exactly one
    // yields nothing.
    if in_group > 0 {
        for _ in in_group..5 {
            group = group.wrapping_mul(85).wrapping_add(84);
        }
        let full = group.to_be_bytes();
        out.extend_from_slice(full.get(..in_group - 1).unwrap_or(&full));
    }

    // The `~` already stopped the loop; a `>` sitting where the loop left off
    // is one more byte read. A bare `>` after a code character counts too.
    if input.get(pos) == Some(&b'>') {
        pos += 1;
    }
    Ok((out, pos))
}

#[cfg(test)]
mod tests {
    use super::{decode_ascii_hex, decode_ascii85};

    // From fpdf_parser_decode_unittest.cpp:268-294 (DecodeA85), every row.
    #[test]
    fn ascii85_reference_vectors() {
        let cases: [(&[u8], &[u8], usize); 8] = [
            (b"", b"", 0),
            (b"~>", b"", 0),
            (b"FCfN8~>", b"test", 7),
            // Anything after the terminator is neither decoded nor consumed.
            (b"FCfN8~>FCfN8", b"test", 7),
            (b"\t F C\r\n \tf N 8 ~>", b"test", 17),
            // No terminator at all: the whole input is legal and consumed.
            (b"@3B0)DJj_BF*)>@Gp#-s", b"a funny story :)", 20),
            // A three-character tail yields two bytes.
            (b"12A", b"2k", 3),
            // `v` is past `u`: the loop breaks having already taken it.
            (b"FCfN8FCfN8vw", b"testtest", 11),
        ];
        for (input, expected, consumed) in cases {
            assert_eq!(
                decode_ascii85(input).expect("no overflow"),
                (expected.to_vec(), consumed),
                "{:?}",
                String::from_utf8_lossy(input)
            );
        }
    }

    #[test]
    fn ascii85_z_expands_to_four_zeroes_anywhere() {
        assert_eq!(decode_ascii85(b"zzz").expect("ok").0, vec![0u8; 12]);
        // Mid-group `z` is illegal per ISO 32000-1 but PDFium takes it and
        // resets the group, so the two characters before it vanish.
        assert_eq!(decode_ascii85(b"FCz").expect("ok"), (vec![0, 0, 0, 0], 3));
    }

    #[test]
    fn ascii85_single_character_tail_emits_nothing() {
        // state - 1 == 0 bytes written for a one-character group.
        assert_eq!(
            decode_ascii85(b"FCfN8F").expect("ok"),
            (b"test".to_vec(), 6)
        );
    }

    #[test]
    fn ascii85_illegal_first_byte_consumes_nothing() {
        for input in [&b"\x00abc"[..], b"~>", b"\x7f"] {
            assert_eq!(decode_ascii85(input).expect("ok"), (Vec::new(), 0));
        }
    }

    #[test]
    fn ascii85_long_run_of_the_lowest_digit_is_all_zeroes() {
        let input = vec![b'!'; 5000];
        let (out, consumed) = decode_ascii85(&input).expect("no overflow");
        assert_eq!(consumed, 5000);
        assert_eq!(out.len(), 4000);
        assert!(out.iter().all(|&b| b == 0));
    }

    #[test]
    fn ascii85_treats_a_bare_greater_than_as_a_code_character() {
        // `>` is 0x3E, inside '!'..='u', so it never breaks the loop: it is a
        // code character worth 29. The terminator check that follows the loop
        // only ever sees the `>` of a `~>`, because the `~` is what stopped
        // the scan. Here the lone `>` opens a group of one, which emits
        // nothing.
        assert_eq!(
            decode_ascii85(b"FCfN8>").expect("ok"),
            (b"test".to_vec(), 6)
        );
        // Two `>` alone: a two-character group worth 29 each, padded out to
        // five with the highest digit, yields one byte.
        assert_eq!(decode_ascii85(b">>").expect("ok"), (vec![91], 2));
    }

    // From fpdf_parser_decode_unittest.cpp:296-322 (DecodeHex), every row.
    #[test]
    fn ascii_hex_reference_vectors() {
        let cases: [(&[u8], &[u8], usize); 8] = [
            (b"", b"", 0),
            (b">", b"", 1),
            (b"\t   \r\n>", b"", 7),
            (b"12Ac>zzz", b"\x12\xac", 5),
            (b"12 Ac\t02\r\nBF>zzz>", b"\x12\xac\x02\xbf", 13),
            // Odd digit count: the low nibble is zero.
            (b"12A>zzz", b"\x12\xa0", 4),
            // Non-hex bytes are skipped, not treated as terminators.
            (b"12tk  \tAc>zzz", b"\x12\xac", 10),
            (b"12AcED3c3456", b"\x12\xac\xed\x3c\x34\x56", 12),
        ];
        for (input, expected, consumed) in cases {
            assert_eq!(
                decode_ascii_hex(input),
                (expected.to_vec(), consumed),
                "{:?}",
                String::from_utf8_lossy(input)
            );
        }
    }

    #[test]
    fn ascii_hex_skips_interleaved_garbage() {
        assert_eq!(decode_ascii_hex(b"1g2h3i>"), (b"\x12\x30".to_vec(), 7));
    }

    #[test]
    fn ascii_hex_whitespace_only_decodes_to_nothing() {
        assert_eq!(decode_ascii_hex(b"  \r\n\t "), (Vec::new(), 6));
    }

    #[test]
    fn ascii_hex_accepts_both_letter_cases() {
        assert_eq!(decode_ascii_hex(b"aAbBcC>").0, b"\xaa\xbb\xcc");
    }
}
