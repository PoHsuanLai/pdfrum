//! The file encryption key: a short byte string that must never be logged.

use std::fmt;

/// A file encryption key of at most 32 bytes.
///
/// Inline storage, because every key in this crate is between 5 and 32 bytes
/// and heap-allocating one only adds a place for it to be copied. The
/// invariant is `len <= 32`.
///
/// [`fmt::Debug`] is implemented by hand and prints only the length: key
/// material must never reach a log line or a diagnostic entry, and the derived
/// implementation would put it in both.
#[derive(Clone, PartialEq, Eq)]
pub struct SmallKey {
    bytes: [u8; 32],
    len: u8,
}

impl SmallKey {
    /// The longest key any revision uses.
    pub const MAX_LEN: usize = 32;

    /// Take the first `len` bytes of `source` as a key, zero-filling if
    /// `source` is shorter and truncating at 32 bytes.
    #[must_use]
    pub(crate) fn from_prefix(source: &[u8], len: usize) -> Self {
        let len = len.min(Self::MAX_LEN);
        let mut bytes = [0u8; Self::MAX_LEN];
        let copied = len.min(source.len());
        if let (Some(head), Some(from)) = (bytes.get_mut(..copied), source.get(..copied)) {
            head.copy_from_slice(from);
        }
        #[expect(
            clippy::cast_possible_truncation,
            reason = "len <= 32 by the line above"
        )]
        Self {
            bytes,
            len: len as u8,
        }
    }

    /// A full 32-byte key, as revision 5 and 6 produce.
    #[must_use]
    pub(crate) const fn from_full(bytes: [u8; 32]) -> Self {
        #[expect(clippy::cast_possible_truncation, reason = "32 fits in a u8")]
        Self {
            bytes,
            len: Self::MAX_LEN as u8,
        }
    }

    /// The key bytes.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        self.bytes.get(..self.len()).unwrap_or_default()
    }

    /// The key length in bytes.
    #[must_use]
    pub fn len(&self) -> usize {
        usize::from(self.len)
    }

    /// Whether the key is empty, which no valid handler produces.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl fmt::Debug for SmallKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SmallKey(<{} bytes redacted>)", self.len())
    }
}

#[cfg(test)]
mod tests {
    use super::SmallKey;

    #[test]
    fn a_prefix_shorter_than_requested_is_zero_filled() {
        let key = SmallKey::from_prefix(b"abc", 5);
        assert_eq!(key.bytes(), b"abc\0\0");
        assert_eq!(key.len(), 5);
    }

    #[test]
    fn a_key_never_exceeds_thirty_two_bytes() {
        let key = SmallKey::from_prefix(&[0xFFu8; 64], 64);
        assert_eq!(key.len(), SmallKey::MAX_LEN);
        assert_eq!(key.bytes().len(), SmallKey::MAX_LEN);
    }

    #[test]
    fn debug_redacts_the_material() {
        let key = SmallKey::from_full([0xAB; 32]);
        let dump = format!("{key:?}");
        assert_eq!(dump, "SmallKey(<32 bytes redacted>)");
        assert!(!dump.contains("ab"), "{dump}");
        assert!(!dump.contains("171"), "{dump}");
    }

    #[test]
    fn an_empty_key_is_representable_and_says_so() {
        let key = SmallKey::from_prefix(b"", 0);
        assert!(key.is_empty());
        assert_eq!(key.bytes(), b"");
    }
}
