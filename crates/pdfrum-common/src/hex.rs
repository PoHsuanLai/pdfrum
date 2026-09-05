//! One hexadecimal digit.
//!
//! Five grammars in this workspace read hex digits — `#xx` name escapes,
//! `<…>` hex strings, `/ASCIIHexDecode` data, a Type 1 program's hex-encoded
//! eexec portion and a ToUnicode CMap's `<…>` codes — and each used to carry
//! its own copy of this six-line function. One copy, at the bottom of the
//! graph, is the only way to keep them from drifting.

/// The value of one ASCII hexadecimal digit, either case, and `None` for any
/// other byte.
///
/// A caller that wants the permissive reading — a bad digit counts as zero —
/// spells it as `hex_digit(b).unwrap_or(0)`, so the permissiveness is visible
/// at the one place it is meant.
///
/// ```
/// use pdfrum_common::hex_digit;
///
/// assert_eq!(hex_digit(b'7'), Some(7));
/// assert_eq!(hex_digit(b'a'), Some(10));
/// assert_eq!(hex_digit(b'F'), Some(15));
/// assert_eq!(hex_digit(b'g'), None);
/// assert_eq!(hex_digit(b' '), None);
/// ```
#[must_use]
pub const fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::hex_digit;

    #[test]
    fn the_twenty_two_digits_and_nothing_else() {
        let expected: Vec<(u8, u8)> = (b'0'..=b'9')
            .zip(0..)
            .chain((b'a'..=b'f').zip(10..))
            .chain((b'A'..=b'F').zip(10..))
            .collect();
        for (byte, value) in &expected {
            assert_eq!(hex_digit(*byte), Some(*value), "{}", char::from(*byte));
        }
        let hits = (0..=u8::MAX).filter(|b| hex_digit(*b).is_some()).count();
        assert_eq!(hits, expected.len());
    }
}
