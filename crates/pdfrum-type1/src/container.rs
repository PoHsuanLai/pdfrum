//! Getting from a `/FontFile` blob to two byte ranges: the ASCII clear portion
//! and the still-encrypted private portion.
//!
//! A Type 1 font program arrives in one of three wrappers, and the font dict's
//! `/Length1` `/Length2` `/Length3` cannot be trusted to describe them —
//! PDFium ignores those keys entirely and so do we, sniffing the bytes instead.
//!
//! - **PFB**: a chain of `[0x80, type, len:u32le, data…]` records, type 1 for
//!   ASCII text, 2 for binary, 3 for end-of-file. The clear part is the leading
//!   text record; the binary records concatenated are the eexec ciphertext.
//! - **PFA**: plain ASCII beginning `%!PS-AdobeFont` or `%!FontType1`, with the
//!   private portion written as hexadecimal after the `eexec` keyword.
//! - **bare**: neither marker. Treated as PFA-shaped, because that is what a
//!   stripped-down embedded font usually is.
//!
//! The output is deliberately owned rather than borrowed: PFB's binary
//! segments are non-contiguous and PFA's hex needs decoding, so a `&[u8]` view
//! of the ciphertext does not exist in the general case.

use crate::error::Error;
use pdfrum_common::{DiagKind, Diagnostics, Severity};

/// Which wrapper the bytes turned out to be in. Reported so callers (and
/// tests) can tell a genuine PFA from a bare program that merely parses like
/// one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Container {
    /// Segmented binary container (`0x80 0x01 …`).
    Pfb,
    /// ASCII container with a `%!PS-AdobeFont` or `%!FontType1` banner.
    Pfa,
    /// No recognisable banner; read as if it were PFA.
    Bare,
}

/// A font program split into its two halves, ready for `eexec` decryption.
#[derive(Debug, Clone)]
pub struct Split {
    /// Which wrapper this came out of.
    pub container: Container,
    /// The cleartext PostScript preamble: everything up to and including the
    /// `eexec` keyword's whitespace.
    pub clear: Vec<u8>,
    /// The still-encrypted private portion, hex-decoded if it was hex.
    pub cipher: Vec<u8>,
}

const PFB_MARKER: u8 = 0x80;
const PFB_TEXT: u8 = 1;
const PFB_BINARY: u8 = 2;
const PFB_EOF: u8 = 3;

/// ISO 32000-1 table 127 `/Length1` `/Length2` `/Length3` for a Type 1
/// `/FontFile` stream.
///
/// For a PFB these are the concatenated bodies of the leading text segment,
/// the binary `eexec` segments, and the trailing text (`cleartomark`)
/// segment. For a PFA or a bare program, `/Length1` is the clear preamble,
/// `/Length2` the decoded ciphertext, and `/Length3` is 0 when no trailer
/// can be split out.
///
/// The oracle's `LoadFontDesc` (`fpdfsdk/fpdf_edittext.cpp:166-170`) never
/// writes these three keys — a TODO, and a file that ISO 32000-1 §9.9
/// table 127 will not accept as a Type 1 program. Callers that embed a
/// Type 1 program should use this instead.
#[must_use]
pub fn font_file_lengths(bytes: &[u8]) -> (u32, u32, u32) {
    if bytes.first() == Some(&PFB_MARKER) {
        return pfb_lengths(bytes);
    }
    let mut diags = Diagnostics::default();
    match split(bytes, &mut diags) {
        Ok(s) => (len_u32(s.clear.len()), len_u32(s.cipher.len()), 0),
        Err(_) => (len_u32(bytes.len()), 0, 0),
    }
}

fn len_u32(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// Walk PFB records for the three table-127 lengths, counting truncated
/// bodies the same way [`split_pfb`] keeps them.
fn pfb_lengths(bytes: &[u8]) -> (u32, u32, u32) {
    let mut length1 = 0u32;
    let mut length2 = 0u32;
    let mut length3 = 0u32;
    let mut seen_binary = false;
    let mut at = 0usize;
    while at < bytes.len() {
        let Some(header) = bytes.get(at..at.saturating_add(6)) else {
            break;
        };
        if header.first().copied() != Some(PFB_MARKER) {
            break;
        }
        match header.get(1).copied() {
            Some(k @ (PFB_TEXT | PFB_BINARY)) => {
                let declared = le_u32(header.get(2..6).unwrap_or_default()) as usize;
                let body_at = at.saturating_add(6);
                let body = bytes
                    .get(body_at..body_at.saturating_add(declared))
                    .unwrap_or_else(|| bytes.get(body_at..).unwrap_or_default());
                let n = len_u32(body.len());
                if k == PFB_TEXT {
                    if seen_binary {
                        length3 = length3.saturating_add(n);
                    } else {
                        length1 = length1.saturating_add(n);
                    }
                } else {
                    seen_binary = true;
                    length2 = length2.saturating_add(n);
                }
                at = body_at.saturating_add(body.len());
                if body.len() < declared {
                    break;
                }
            }
            _ => break,
        }
    }
    (length1, length2, length3)
}

/// Sniff the container and split the program.
///
/// Damage tolerance mirrors what a Type 1 rasterizer has to survive in the
/// wild: a PFB whose final segment length overruns the blob is truncated to
/// what is there, a missing type-3 terminator is not an error, and a PFA whose
/// hex runs into `0000…0000 cleartomark` simply stops at the first non-hex
/// byte. Each of those records a diagnostic.
///
/// # Errors
///
/// [`Error::Empty`] for an empty blob, [`Error::PfbSegment`] for a PFB whose
/// *first* segment header is unusable (past that point truncation is a
/// recovery, not a failure), and [`Error::NoEexec`] when no private portion can
/// be located at all.
pub fn split(bytes: &[u8], diags: &mut Diagnostics) -> Result<Split, Error> {
    if bytes.is_empty() {
        return Err(Error::Empty);
    }
    if bytes.first() == Some(&PFB_MARKER) {
        return split_pfb(bytes, diags);
    }
    let container = if banner(bytes) {
        Container::Pfa
    } else {
        Container::Bare
    };
    split_ascii(bytes, container, diags)
}

/// Whether the blob opens with one of the two ASCII Type 1 banners. The scan
/// tolerates leading whitespace, which real files do carry.
fn banner(bytes: &[u8]) -> bool {
    let start = bytes
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    let rest = bytes.get(start..).unwrap_or_default();
    rest.starts_with(b"%!PS-AdobeFont") || rest.starts_with(b"%!FontType1")
}

/// Walk the `[0x80, type, len:u32le]` record chain.
fn split_pfb(bytes: &[u8], diags: &mut Diagnostics) -> Result<Split, Error> {
    let mut clear = Vec::new();
    let mut cipher = Vec::new();
    let mut at = 0usize;
    let mut first = true;

    while at < bytes.len() {
        let Some(header) = bytes.get(at..at.saturating_add(6)) else {
            // A trailing stub too short to be a header. Real files end on a
            // type-3 record; this one did not.
            diags.record(
                Severity::Suspicious,
                DiagKind::Type1PfbTruncated,
                Some(at as u64),
            );
            break;
        };
        let (marker, kind) = (header.first().copied(), header.get(1).copied());
        if marker != Some(PFB_MARKER) {
            if first {
                return Err(Error::PfbSegment { at });
            }
            diags.record(
                Severity::Suspicious,
                DiagKind::Type1PfbTruncated,
                Some(at as u64),
            );
            break;
        }
        match kind {
            Some(PFB_EOF) => break,
            Some(k @ (PFB_TEXT | PFB_BINARY)) => {
                let declared = le_u32(header.get(2..6).unwrap_or_default()) as usize;
                let body_at = at.saturating_add(6);
                let body = if let Some(b) = bytes.get(body_at..body_at.saturating_add(declared)) {
                    b
                } else {
                    // Length overruns the blob: keep what is there.
                    diags.record(
                        Severity::Recovered,
                        DiagKind::Type1PfbTruncated,
                        Some(at as u64),
                    );
                    bytes.get(body_at..).unwrap_or_default()
                };
                if k == PFB_TEXT {
                    // Only the preamble matters; the trailing 512 zeros and
                    // `cleartomark` are a PostScript ritual, not font data,
                    // and they arrive *after* the binary segments.
                    if cipher.is_empty() {
                        clear.extend_from_slice(body);
                    }
                } else {
                    cipher.extend_from_slice(body);
                }
                at = body_at.saturating_add(body.len());
                if body.len() < declared {
                    break;
                }
            }
            _ => {
                if first {
                    return Err(Error::PfbSegment { at });
                }
                diags.record(
                    Severity::Suspicious,
                    DiagKind::Type1PfbTruncated,
                    Some(at as u64),
                );
                break;
            }
        }
        first = false;
    }

    if cipher.is_empty() {
        // A PFB whose text segment nevertheless holds an inline `eexec`
        // section — malformed, but recoverable by reading it as PFA.
        if let Ok(mut ascii) = split_ascii(&clear, Container::Pfb, diags) {
            ascii.container = Container::Pfb;
            return Ok(ascii);
        }
        return Err(Error::NoEexec);
    }
    Ok(Split {
        container: Container::Pfb,
        clear,
        cipher,
    })
}

/// Find `eexec` in an ASCII program and take everything after it, hex-decoding
/// when the tail is hex.
fn split_ascii(
    bytes: &[u8],
    container: Container,
    diags: &mut Diagnostics,
) -> Result<Split, Error> {
    let key = find_eexec(bytes).ok_or(Error::NoEexec)?;
    let clear = bytes.get(..key).unwrap_or_default().to_vec();
    let tail = bytes.get(key..).unwrap_or_default();

    // The private portion is hex when its first four *significant* bytes are
    // all hex digits — the test FreeType uses, and it is reliable because a
    // binary section's first four bytes are random ciphertext.
    let significant: Vec<u8> = tail
        .iter()
        .copied()
        .filter(|b| !b.is_ascii_whitespace())
        .take(4)
        .collect();
    let is_hex = significant.len() == 4 && significant.iter().all(u8::is_ascii_hexdigit);

    let cipher = if is_hex {
        hex_decode(tail, diags)
    } else {
        tail.to_vec()
    };
    Ok(Split {
        container,
        clear,
        cipher,
    })
}

/// Locate the byte just past the `eexec` keyword and its following
/// end-of-line.
///
/// The keyword is matched only at a token boundary, so `eexec` inside a
/// comment or a `(string)` does not trigger. Exactly one EOL is consumed —
/// `\r\n`, `\r` or `\n` — plus any run of spaces or tabs before it, matching
/// what the Type 1 specification says the interpreter does.
fn find_eexec(bytes: &[u8]) -> Option<usize> {
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes.get(i).copied() {
            // Skip a `%` comment to end of line.
            Some(b'%') => {
                while i < bytes.len() && !matches!(bytes.get(i), Some(b'\r' | b'\n')) {
                    i = i.saturating_add(1);
                }
            }
            // Skip a `(…)` string, honouring nesting and backslash escapes.
            Some(b'(') => {
                let mut depth = 1usize;
                i = i.saturating_add(1);
                while i < bytes.len() && depth > 0 {
                    match bytes.get(i).copied() {
                        Some(b'\\') => i = i.saturating_add(1),
                        Some(b'(') => depth = depth.saturating_add(1),
                        Some(b')') => depth = depth.saturating_sub(1),
                        _ => {}
                    }
                    i = i.saturating_add(1);
                }
            }
            _ => {
                if bytes.get(i..i.saturating_add(5)) == Some(b"eexec".as_slice())
                    && before_is_boundary(bytes, i)
                    && after_is_boundary(bytes, i.saturating_add(5))
                {
                    return Some(skip_one_eol(bytes, i.saturating_add(5)));
                }
                i = i.saturating_add(1);
            }
        }
    }
    None
}

fn before_is_boundary(bytes: &[u8], at: usize) -> bool {
    at == 0
        || at
            .checked_sub(1)
            .and_then(|p| bytes.get(p))
            .is_some_and(|b| b.is_ascii_whitespace() || *b == b'/')
}

fn after_is_boundary(bytes: &[u8], at: usize) -> bool {
    bytes.get(at).is_none_or(u8::is_ascii_whitespace)
}

/// Consume trailing blanks then exactly one line ending.
fn skip_one_eol(bytes: &[u8], mut at: usize) -> usize {
    while matches!(bytes.get(at), Some(b' ' | b'\t')) {
        at = at.saturating_add(1);
    }
    match bytes.get(at) {
        Some(b'\r') => {
            at = at.saturating_add(1);
            if bytes.get(at) == Some(&b'\n') {
                at = at.saturating_add(1);
            }
        }
        Some(b'\n') => at = at.saturating_add(1),
        _ => {}
    }
    at
}

/// Decode hex until the first byte that is neither a hex digit nor whitespace.
fn hex_decode(bytes: &[u8], diags: &mut Diagnostics) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() / 2);
    let mut high: Option<u8> = None;
    for (i, b) in bytes.iter().enumerate() {
        if b.is_ascii_whitespace() {
            continue;
        }
        let Some(nibble) = hex_value(*b) else {
            if i.saturating_add(1) < bytes.len() {
                diags.record(
                    Severity::Recovered,
                    DiagKind::Type1HexTruncated,
                    Some(i as u64),
                );
            }
            break;
        };
        match high.take() {
            None => high = Some(nibble),
            Some(h) => out.push((h << 4) | nibble),
        }
    }
    out
}

fn hex_value(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn le_u32(b: &[u8]) -> u32 {
    let g = |i: usize| u32::from(b.get(i).copied().unwrap_or(0));
    g(0) | (g(1) << 8) | (g(2) << 16) | (g(3) << 24)
}

#[cfg(test)]
#[allow(
    clippy::indexing_slicing,
    clippy::float_cmp,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::similar_names
)]
mod tests {
    use super::{Container, split};
    use pdfrum_common::{DiagKind, Diagnostics};

    /// A minimal PFB: text segment, binary segment, EOF.
    fn pfb(text: &[u8], binary: &[u8]) -> Vec<u8> {
        let mut v = vec![0x80, 1];
        v.extend_from_slice(&(text.len() as u32).to_le_bytes());
        v.extend_from_slice(text);
        v.extend_from_slice(&[0x80, 2]);
        v.extend_from_slice(&(binary.len() as u32).to_le_bytes());
        v.extend_from_slice(binary);
        v.extend_from_slice(&[0x80, 3]);
        v
    }

    #[test]
    fn pfb_and_pfa_agree() {
        let mut d = Diagnostics::default();
        let clear = b"%!PS-AdobeFont-1.0: T 1\n/FontName /T def\ncurrentfile eexec\n";
        let binary = b"\x01\x02\x03\x04rest-of-the-private-dict";

        let from_pfb = split(&pfb(clear, binary), &mut d).unwrap();
        assert_eq!(from_pfb.container, Container::Pfb);
        assert_eq!(from_pfb.clear, clear);
        assert_eq!(from_pfb.cipher, binary);

        // The same font written as PFA: the binary section spelled in hex.
        let mut pfa = clear.to_vec();
        for b in binary {
            pfa.extend_from_slice(format!("{b:02X}").as_bytes());
        }
        let from_pfa = split(&pfa, &mut d).unwrap();
        assert_eq!(from_pfa.container, Container::Pfa);
        assert_eq!(from_pfa.clear, clear);
        assert_eq!(from_pfa.cipher, binary);
    }

    #[test]
    fn truncated_pfb_segment_keeps_what_it_has() {
        // Eat the two-byte EOF record and four bytes of the binary payload,
        // so the declared length of 8 overruns what is there.
        let mut good = pfb(b"%!PS-AdobeFont\n", b"abcdefgh");
        good.truncate(good.len() - 6);
        let mut d = Diagnostics::default();
        let s = split(&good, &mut d).unwrap();
        assert_eq!(s.cipher, b"abcd");
        assert!(d.contains(&DiagKind::Type1PfbTruncated));
    }

    #[test]
    fn bare_program_reads_as_pfa_shaped() {
        let mut d = Diagnostics::default();
        let s = split(
            b"/FontName /T def\ncurrentfile eexec\n\x01\x02\x03\x04tail",
            &mut d,
        )
        .unwrap();
        assert_eq!(s.container, Container::Bare);
        assert_eq!(s.cipher, b"\x01\x02\x03\x04tail");
    }

    #[test]
    fn eexec_in_a_comment_or_string_is_not_the_keyword() {
        let mut d = Diagnostics::default();
        // The payload's first four bytes are not all hex digits, so it is read
        // as binary rather than hex-decoded.
        let s = split(
            b"%!PS-AdobeFont\n% eexec here\n(eexec there) def\ncurrentfile eexec\r\n\x01\x02\x03\x04real",
            &mut d,
        )
        .unwrap();
        assert_eq!(s.cipher, b"\x01\x02\x03\x04real");
        assert!(s.clear.ends_with(b"eexec\r\n"));
    }

    #[test]
    fn hex_stops_at_the_first_non_hex_byte() {
        let mut d = Diagnostics::default();
        let s = split(b"%!FontType1\neexec\n4142 4344 zz9999", &mut d).unwrap();
        assert_eq!(s.cipher, b"ABCD");
        assert!(d.contains(&DiagKind::Type1HexTruncated));
    }
}
