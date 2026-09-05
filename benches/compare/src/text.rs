//! Text comparison: the oracle's UTF-32LE page text decoded, then three
//! readings of "the same": byte-exact, whitespace-normalized, and a token
//! F1 for the distribution between those two.

use serde::{Deserialize, Serialize};

/// One page's text verdict against the oracle.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TextDiff {
    /// The two strings are identical.
    pub exact: bool,
    /// Identical after every run of whitespace collapses to one space and
    /// the ends are trimmed.
    pub normalized: bool,
    /// F1 over the multiset of whitespace-separated tokens, 0..=1.
    pub token_f1: f64,
    /// Characters the engine produced.
    pub len: usize,
    /// Characters the oracle produced.
    pub oracle_len: usize,
}

/// `pdfium_test --txt` writes UTF-32LE with a BOM.
pub fn decode_utf32le(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() / 4);
    for chunk in bytes.chunks_exact(4) {
        let code = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        if code == 0xFEFF {
            continue;
        }
        out.push(char::from_u32(code).unwrap_or(char::REPLACEMENT_CHARACTER));
    }
    out
}

/// Whitespace runs to one space, ends trimmed.
pub fn normalize(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// F1 over token multisets; two empty texts score 1.
pub fn token_f1(a: &str, b: &str) -> f64 {
    let mut counts: std::collections::HashMap<&str, (usize, usize)> =
        std::collections::HashMap::new();
    for token in a.split_whitespace() {
        counts.entry(token).or_default().0 += 1;
    }
    for token in b.split_whitespace() {
        counts.entry(token).or_default().1 += 1;
    }
    let (mut common, mut total_a, mut total_b) = (0usize, 0usize, 0usize);
    for (na, nb) in counts.values() {
        common += na.min(nb);
        total_a += na;
        total_b += nb;
    }
    if total_a == 0 && total_b == 0 {
        return 1.0;
    }
    if common == 0 {
        return 0.0;
    }
    let precision = common as f64 / total_b as f64;
    let recall = common as f64 / total_a as f64;
    2.0 * precision * recall / (precision + recall)
}

/// Scores `candidate` against `oracle`.
pub fn compare(oracle: &str, candidate: &str) -> TextDiff {
    TextDiff {
        exact: oracle == candidate,
        normalized: normalize(oracle) == normalize(candidate),
        token_f1: token_f1(oracle, candidate),
        len: candidate.chars().count(),
        oracle_len: oracle.chars().count(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_utf32le_and_drops_the_bom() {
        let bytes = [0xFF, 0xFE, 0, 0, b'H', 0, 0, 0, b'i', 0, 0, 0];
        assert_eq!(decode_utf32le(&bytes), "Hi");
    }

    #[test]
    fn normalization_is_line_ending_and_run_insensitive() {
        assert!(compare("a\r\nb  c", "a\nb c").normalized);
        assert!(!compare("a\r\nb  c", "a\nb c").exact);
    }

    #[test]
    fn token_f1_is_symmetric_and_bounded() {
        assert!((token_f1("a b c", "a b c") - 1.0).abs() < f64::EPSILON);
        assert!((token_f1("a b c", "c b a") - 1.0).abs() < f64::EPSILON);
        assert!((token_f1("a b c d", "a b") - 2.0 / 3.0).abs() < 1e-9);
        assert!((token_f1("", "") - 1.0).abs() < f64::EPSILON);
        assert!(token_f1("a", "") < f64::EPSILON);
    }
}
