//! The two Type 1 stream ciphers (Type 1 specification §7).
//!
//! Both are the same three-line feedback cipher differing only in their seed:
//! `eexec` (seed 55665) wraps the whole private dictionary, and each individual
//! charstring is separately encrypted with seed 4330. Neither offers any
//! security — the seeds are published constants — so this module is a codec,
//! not cryptography, and lives here rather than in `pdfrum-crypt`.
//!
//! Both forms discard a number of leading plaintext bytes that were random
//! padding at encryption time: 4 for `eexec`, and `lenIV` (default 4, but the
//! Private dictionary may set anything including 0) for charstrings.

/// Seed for the `eexec` envelope around the private dictionary.
pub const EEXEC_SEED: u16 = 55665;
/// Seed for an individual charstring or subroutine.
pub const CHARSTRING_SEED: u16 = 4330;
/// Plaintext bytes `eexec` discards; fixed by the specification.
pub const EEXEC_SKIP: usize = 4;
/// Default `lenIV` when the Private dictionary does not say.
pub const DEFAULT_LEN_IV: i32 = 4;

const MULT: u16 = 52845;
const ADD: u16 = 22719;

/// Decrypt `cipher` with `seed`, dropping the first `skip` plaintext bytes.
///
/// A `skip` larger than the plaintext yields an empty result rather than
/// panicking — a `lenIV` of 16 against a 3-byte charstring is a real thing
/// broken fonts do.
///
/// ```
/// use pdfrum_type1::{decrypt, CHARSTRING_SEED};
///
/// // Round-trip: encrypting then decrypting with the same seed is identity.
/// let plain = b"\0\0\0\0hsbw-ish payload";
/// let cipher = pdfrum_type1::encrypt(plain, CHARSTRING_SEED);
/// assert_eq!(decrypt(&cipher, CHARSTRING_SEED, 4), b"hsbw-ish payload");
/// ```
#[must_use]
pub fn decrypt(cipher: &[u8], seed: u16, skip: usize) -> Vec<u8> {
    let mut r = seed;
    let mut out = Vec::with_capacity(cipher.len().saturating_sub(skip));
    for (i, &c) in cipher.iter().enumerate() {
        let plain = c ^ (r >> 8) as u8;
        r = (u16::from(c).wrapping_add(r))
            .wrapping_mul(MULT)
            .wrapping_add(ADD);
        if i >= skip {
            out.push(plain);
        }
    }
    out
}

/// The inverse of [`decrypt`], without the skip: `plain` is emitted as-is
/// through the cipher.
///
/// Exists so tests can build encrypted fixtures from readable charstrings
/// instead of hand-assembled byte tables. It is also the operation an encoder
/// would need, which is why it is public rather than `#[cfg(test)]`.
#[must_use]
pub fn encrypt(plain: &[u8], seed: u16) -> Vec<u8> {
    let mut r = seed;
    let mut out = Vec::with_capacity(plain.len());
    for &p in plain {
        let c = p ^ (r >> 8) as u8;
        out.push(c);
        r = (u16::from(c).wrapping_add(r))
            .wrapping_mul(MULT)
            .wrapping_add(ADD);
    }
    out
}

/// Turn a Private-dictionary `lenIV` into a byte count.
///
/// The value is signed in the source and fonts do write negatives; a negative
/// or absurd `lenIV` means "discard nothing", which is FreeType's reading and
/// the only one that keeps such a font renderable.
#[must_use]
pub fn len_iv_skip(len_iv: i32) -> usize {
    usize::try_from(len_iv).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::{CHARSTRING_SEED, DEFAULT_LEN_IV, EEXEC_SEED, EEXEC_SKIP, decrypt, encrypt};

    #[test]
    fn known_eexec_vector() {
        // Encrypting 32 zero bytes with the eexec seed is a fully determined
        // sequence; recomputing it here from the published recurrence pins the
        // constants against a typo in either direction.
        let cipher = encrypt(&[0u8; 32], EEXEC_SEED);
        let mut r: u16 = EEXEC_SEED;
        let expected: Vec<u8> = (0..32)
            .map(|_| {
                let c = (r >> 8) as u8; // plaintext byte is 0
                r = (u16::from(c).wrapping_add(r))
                    .wrapping_mul(52845)
                    .wrapping_add(22719);
                c
            })
            .collect();
        assert_eq!(cipher, expected);
        assert_eq!(cipher.len(), 32);
        // And the first four bytes are the padding eexec throws away.
        assert_eq!(decrypt(&cipher, EEXEC_SEED, EEXEC_SKIP), vec![0u8; 28]);
    }

    #[test]
    fn len_iv_of_zero_four_and_eight() {
        let plain: Vec<u8> = (0u8..24).collect();
        let cipher = encrypt(&plain, CHARSTRING_SEED);
        for skip in [0usize, DEFAULT_LEN_IV as usize, 8] {
            assert_eq!(
                decrypt(&cipher, CHARSTRING_SEED, skip),
                plain.get(skip..).unwrap_or_default(),
                "lenIV {skip}"
            );
        }
    }

    #[test]
    fn skip_past_the_end_is_empty_not_a_panic() {
        let cipher = encrypt(b"abc", CHARSTRING_SEED);
        assert!(decrypt(&cipher, CHARSTRING_SEED, 99).is_empty());
    }

    #[test]
    fn negative_len_iv_discards_nothing() {
        assert_eq!(super::len_iv_skip(-1), 0);
        assert_eq!(super::len_iv_skip(4), 4);
    }
}
