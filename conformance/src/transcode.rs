//! Text-dump transcoding.
//!
//! The oracle writes page text with `--txt` as UTF-32LE, led by
//! a U+FEFF byte-order mark. Tier A compares text as UTF-8, so the golden
//! store holds the transcoded form and the diff is a plain byte comparison of
//! two UTF-8 buffers.

/// What went wrong decoding an oracle text dump.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TranscodeError {
    #[error("UTF-32LE dump has {len} bytes, not a multiple of 4")]
    Ragged { len: usize },
    #[error("UTF-32LE dump holds {value:#x}, not a Unicode scalar value")]
    NotAScalar { value: u32 },
}

/// Decodes an oracle `--txt` dump (UTF-32LE, optional leading BOM) to UTF-8.
///
/// Unpaired surrogates and out-of-range code points are a hard error rather
/// than a silent replacement: a text dump that cannot be decoded means the
/// oracle produced something we do not understand, and Tier A must notice.
pub fn utf32le_to_utf8(bytes: &[u8]) -> Result<String, TranscodeError> {
    if !bytes.len().is_multiple_of(4) {
        return Err(TranscodeError::Ragged { len: bytes.len() });
    }
    let mut out = String::with_capacity(bytes.len() / 4);
    for (index, unit) in bytes.chunks_exact(4).enumerate() {
        let value = u32::from_le_bytes([unit[0], unit[1], unit[2], unit[3]]);
        // Strip the leading BOM; a BOM anywhere else is a real character.
        if index == 0 && value == 0xFEFF {
            continue;
        }
        let ch = char::from_u32(value).ok_or(TranscodeError::NotAScalar { value })?;
        out.push(ch);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode(text: &str, bom: bool) -> Vec<u8> {
        let mut bytes = Vec::new();
        if bom {
            bytes.extend_from_slice(&0xFEFFu32.to_le_bytes());
        }
        for ch in text.chars() {
            bytes.extend_from_slice(&(ch as u32).to_le_bytes());
        }
        bytes
    }

    #[test]
    fn decodes_ascii_with_a_bom() {
        assert_eq!(
            utf32le_to_utf8(&encode("Hello, world!", true)).unwrap(),
            "Hello, world!"
        );
    }

    #[test]
    fn decodes_without_a_bom() {
        assert_eq!(utf32le_to_utf8(&encode("no bom", false)).unwrap(), "no bom");
    }

    #[test]
    fn decodes_non_bmp_and_cjk() {
        let text = "日本語 \u{1F600} ünïcödé";
        assert_eq!(utf32le_to_utf8(&encode(text, true)).unwrap(), text);
    }

    #[test]
    fn empty_input_decodes_to_empty_text() {
        assert_eq!(utf32le_to_utf8(&[]).unwrap(), "");
        assert_eq!(utf32le_to_utf8(&encode("", true)).unwrap(), "");
    }

    #[test]
    fn a_bom_after_the_first_unit_is_a_character() {
        let bytes = encode("a\u{FEFF}b", true);
        assert_eq!(utf32le_to_utf8(&bytes).unwrap(), "a\u{FEFF}b");
    }

    #[test]
    fn matches_the_exact_byte_layout_the_oracle_writes() {
        // `fffe 0000 4c00 0000 6900 ...` — BOM then 'L', 'i' (verified against
        // pdfium_test output for testing/resources/annots.pdf).
        let bytes = [
            0xff, 0xfe, 0x00, 0x00, 0x4c, 0x00, 0x00, 0x00, 0x69, 0x00, 0x00, 0x00,
        ];
        assert_eq!(utf32le_to_utf8(&bytes).unwrap(), "Li");
    }

    #[test]
    fn a_ragged_length_is_an_error() {
        assert_eq!(
            utf32le_to_utf8(&[0x41, 0x00, 0x00]),
            Err(TranscodeError::Ragged { len: 3 })
        );
    }

    #[test]
    fn a_surrogate_or_out_of_range_unit_is_an_error() {
        assert_eq!(
            utf32le_to_utf8(&0xD800u32.to_le_bytes()),
            Err(TranscodeError::NotAScalar { value: 0xD800 })
        );
        assert_eq!(
            utf32le_to_utf8(&0x11_0000u32.to_le_bytes()),
            Err(TranscodeError::NotAScalar { value: 0x11_0000 })
        );
    }
}
