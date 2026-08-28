//! Per-object keys and payload decryption (ISO 32000 §7.6.2, Algorithm 1).
//!
//! Every string and stream in a document is decrypted under a key derived
//! from the file key and the *enclosing indirect object's* number and
//! generation — not the number of whatever nested object the string sits in.
//! AESV3 is the exception: at a 32-byte key the file key is used verbatim,
//! with no per-object derivation at all.

use pdfrum_object::ObjRef;

use crate::key::SmallKey;
use crate::primitives::{BLOCK, aes_cbc_decrypt, md5};
use crate::rc4::rc4;

/// Which crypt filter class a payload belongs to.
///
/// PDFium refuses documents whose `/StmF` and `/StrF` differ and never reads
/// `/EFF` at all, so all three classes resolve to one cipher and one key.
/// The distinction is kept because it is the specification's, and because an
/// `/EFF` implementation would otherwise need a signature change (Divergence
/// D1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CryptClass {
    /// A stream's data, governed by `/StmF`.
    Stream,
    /// A string object, governed by `/StrF`.
    String,
    /// An embedded file stream, nominally governed by `/EFF`.
    Embedded,
}

/// The four ASCII bytes AESV2 appends before hashing the object key.
const AES_SALT: [u8; 4] = *b"sAlT";

/// Algorithm 1's scratch buffer: the file key, then three bytes of object
/// number and two of generation, all little-endian and all truncated.
///
/// The truncation is real behavior, not an oversight to guard against: an
/// object number of 2^24 or more wraps, so two such objects share a key.
fn salted(key: &SmallKey, obj: ObjRef) -> ([u8; 48], usize) {
    let mut scratch = [0u8; 48];
    let key_len = key.len().min(scratch.len());
    if let (Some(head), Some(from)) = (scratch.get_mut(..key_len), key.bytes().get(..key_len)) {
        head.copy_from_slice(from);
    }
    let num = obj.num.to_le_bytes();
    let generation = obj.generation.to_le_bytes();
    for (offset, byte) in num
        .iter()
        .take(3)
        .chain(generation.iter().take(2))
        .enumerate()
    {
        if let Some(slot) = scratch.get_mut(key_len + offset) {
            *slot = *byte;
        }
    }
    (scratch, key_len)
}

/// The RC4 object key: MD5 of the salted scratch, capped at sixteen bytes.
///
/// The cap is why a 16-byte file key yields a 16-byte object key rather than
/// the 21 bytes the scratch length would suggest.
#[must_use]
fn rc4_object_key(key: &SmallKey, obj: ObjRef) -> Vec<u8> {
    let (scratch, key_len) = salted(key, obj);
    let digest = md5(scratch.get(..key_len + 5).unwrap_or(&scratch));
    let len = (key_len + 5).min(digest.len());
    digest.get(..len).unwrap_or(&digest).to_vec()
}

/// The AESV2 object key: MD5 of the salted scratch plus `sAlT`, all sixteen
/// bytes of it.
///
/// The C++'s encrypt path truncates this to the file key length while its
/// decrypt path does not; since AESV2 is always a 16-byte key in practice the
/// two agree, and we implement the decrypt reading.
#[must_use]
fn aes_v4_object_key(key: &SmallKey, obj: ObjRef) -> [u8; 16] {
    let (mut scratch, key_len) = salted(key, obj);
    for (offset, byte) in AES_SALT.iter().enumerate() {
        if let Some(slot) = scratch.get_mut(key_len + 5 + offset) {
            *slot = *byte;
        }
    }
    md5(scratch.get(..key_len + 9).unwrap_or(&scratch))
}

/// Decrypt an RC4 payload. Symmetric, so this is just the cipher.
#[must_use]
pub(crate) fn decrypt_rc4(key: &SmallKey, obj: ObjRef, data: &[u8]) -> Vec<u8> {
    rc4(&rc4_object_key(key, obj), data)
}

/// Decrypt an AESV2 payload under a per-object key.
#[must_use]
pub(crate) fn decrypt_aes_v4(key: &SmallKey, obj: ObjRef, data: &[u8]) -> Vec<u8> {
    decrypt_aes_cbc(&aes_v4_object_key(key, obj), data)
}

/// Decrypt an AESV3 payload under the file key itself.
#[must_use]
pub(crate) fn decrypt_aes_v5(key: &[u8; 32], data: &[u8]) -> Vec<u8> {
    decrypt_aes_cbc(key, data)
}

/// Strip the leading initialisation vector and CBC-decrypt the rest,
/// reproducing PDFium's buffering rules exactly.
///
/// The rules are quirks of a streaming decoder that buffers one block behind
/// its input, restated here as an output rule (Divergence D6):
///
/// - the first sixteen bytes are the IV and are consumed, never emitted, so
///   fewer than seventeen bytes of input produce nothing at all;
/// - a block is emitted only once more input has arrived after it, which
///   leaves the final block for the padding step;
/// - that final block is emitted minus its last plaintext byte's worth of
///   padding when that byte is under sixteen, and dropped entirely otherwise.
///   Nothing checks that the padding bytes agree with the count, and a last
///   byte of zero keeps the whole block — neither of which strict PKCS#7
///   would do;
/// - a trailing partial block never fills, so it is discarded — but the full
///   block before it *was* followed by input and so is emitted.
///
/// A key AES cannot accept, or a chaining failure, yields empty output rather
/// than an error: PDFium never fails a decrypt, it produces a best-effort
/// result and lets the consumer see a short or empty payload.
#[must_use]
fn decrypt_aes_cbc(key: &[u8], data: &[u8]) -> Vec<u8> {
    let Some(iv) = data
        .get(..BLOCK)
        .and_then(|s| <[u8; BLOCK]>::try_from(s).ok())
    else {
        return Vec::new();
    };
    let body = data.get(BLOCK..).unwrap_or_default();
    let whole = body.len() - body.len() % BLOCK;
    let Some(mut out) = body.get(..whole).map(<[u8]>::to_vec) else {
        return Vec::new();
    };
    if aes_cbc_decrypt(key, &iv, &mut out).is_err() {
        return Vec::new();
    }

    if whole < body.len() {
        // A partial tail arrived after the last full block, so that block was
        // emitted and only the tail is lost.
        return out;
    }
    // No tail: the last block is still in the lag buffer and gets the
    // padding treatment.
    let Some(pad) = out.last().copied() else {
        return out;
    };
    if usize::from(pad) >= BLOCK {
        out.truncate(out.len() - BLOCK);
    } else {
        out.truncate(out.len() - usize::from(pad));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{
        BLOCK, CryptClass, aes_v4_object_key, decrypt_aes_cbc, decrypt_aes_v5, decrypt_rc4,
        rc4_object_key,
    };
    use crate::key::SmallKey;
    use crate::primitives::aes_cbc_encrypt;
    use pdfrum_object::ObjRef;

    fn key(len: usize) -> SmallKey {
        SmallKey::from_prefix(&(0..32u8).collect::<Vec<_>>(), len)
    }

    // T17 — the RC4 object-key length is min(key_len + 5, 16), so a 16-byte
    // file key produces a 16-byte object key, not a 21-byte one.
    #[test]
    fn rc4_object_key_length_is_capped_at_sixteen() {
        assert_eq!(rc4_object_key(&key(5), ObjRef::new(1, 0)).len(), 10);
        assert_eq!(rc4_object_key(&key(10), ObjRef::new(1, 0)).len(), 15);
        assert_eq!(rc4_object_key(&key(16), ObjRef::new(1, 0)).len(), 16);
    }

    // T17 — only three bytes of object number and two of generation take
    // part, so numbers 2^24 apart collide.
    #[test]
    fn object_numbers_contribute_three_bytes() {
        let k = key(16);
        assert_eq!(
            rc4_object_key(&k, ObjRef::new(1, 0)),
            rc4_object_key(&k, ObjRef::new(0x0100_0001, 0))
        );
        assert_ne!(
            rc4_object_key(&k, ObjRef::new(1, 0)),
            rc4_object_key(&k, ObjRef::new(2, 0))
        );
        assert_ne!(
            rc4_object_key(&k, ObjRef::new(1, 0)),
            rc4_object_key(&k, ObjRef::new(1, 1))
        );
    }

    #[test]
    fn generation_contributes_two_bytes() {
        let k = key(16);
        assert_ne!(
            rc4_object_key(&k, ObjRef::new(1, 0x0100)),
            rc4_object_key(&k, ObjRef::new(1, 0))
        );
    }

    // T17 — AESV2 hashes the scratch plus "sAlT" and keeps all sixteen bytes.
    #[test]
    fn aes_v4_object_key_is_a_full_digest() {
        let derived = aes_v4_object_key(&key(16), ObjRef::new(1, 0));
        assert_eq!(derived.len(), 16);
        // Hand-computed: the scratch is key || 01 00 00 00 00 || 73 41 6C 54,
        // hashed over its first key_len + 9 = 25 bytes.
        let mut scratch = Vec::new();
        scratch.extend_from_slice(key(16).bytes());
        scratch.extend_from_slice(&[1, 0, 0, 0, 0]);
        scratch.extend_from_slice(b"sAlT");
        assert_eq!(scratch.len(), 25);
        assert_eq!(derived, crate::primitives::md5(&scratch));
    }

    // T17 — AESV3 ignores the object entirely.
    #[test]
    fn aes_v5_uses_the_file_key_verbatim() {
        let file_key = [7u8; 32];
        let payload = encrypted(&file_key, b"hello");
        let first = decrypt_aes_v5(&file_key, &payload);
        assert_eq!(first, b"hello");
        // Not keyed by an object at all: the same bytes decrypt the same way
        // whichever object they came from.
        assert_eq!(decrypt_aes_v5(&file_key, &payload), first);
    }

    /// Build an IV-prefixed, PKCS#7-padded ciphertext the way a writer would.
    fn encrypted(key: &[u8], plaintext: &[u8]) -> Vec<u8> {
        let iv = [0x5Au8; BLOCK];
        let pad = BLOCK - plaintext.len() % BLOCK;
        let mut body = plaintext.to_vec();
        body.extend(std::iter::repeat_n(u8::try_from(pad).unwrap_or(0), pad));
        aes_cbc_encrypt(key, &iv, &mut body).expect("valid key");
        let mut out = iv.to_vec();
        out.extend_from_slice(&body);
        out
    }

    // T18 — the acceptance table for the buffering rules.
    #[test]
    fn aes_lengths_under_seventeen_bytes_yield_nothing() {
        let k = [0u8; 16];
        for len in 0..=BLOCK {
            assert!(
                decrypt_aes_cbc(&k, &vec![0xAA; len]).is_empty(),
                "{len} bytes"
            );
        }
    }

    #[test]
    fn a_round_trip_recovers_the_plaintext() {
        let k = [3u8; 16];
        for len in [0usize, 1, 15, 16, 17, 31, 32, 100] {
            let plaintext: Vec<u8> = (0..len)
                .map(|i| u8::try_from(i % 251).unwrap_or(0))
                .collect();
            assert_eq!(
                decrypt_aes_cbc(&k, &encrypted(&k, &plaintext)),
                plaintext,
                "{len} bytes"
            );
        }
    }

    /// Encrypt one block of chosen plaintext so its last byte can be dialled.
    fn one_block_with_last(key: &[u8], last: u8) -> Vec<u8> {
        let iv = [0u8; BLOCK];
        let mut block = [0u8; BLOCK];
        if let Some(slot) = block.last_mut() {
            *slot = last;
        }
        let mut body = block.to_vec();
        aes_cbc_encrypt(key, &iv, &mut body).expect("valid key");
        let mut out = iv.to_vec();
        out.extend_from_slice(&body);
        out
    }

    // A final byte of 16 or more drops the whole block; a byte of zero keeps
    // it, which strict PKCS#7 would reject.
    #[test]
    fn the_final_block_pad_byte_decides_how_much_survives() {
        let k = [9u8; 16];
        assert!(decrypt_aes_cbc(&k, &one_block_with_last(&k, 0x10)).is_empty());
        assert!(decrypt_aes_cbc(&k, &one_block_with_last(&k, 0xFF)).is_empty());
        assert_eq!(
            decrypt_aes_cbc(&k, &one_block_with_last(&k, 0)).len(),
            BLOCK
        );
        assert_eq!(decrypt_aes_cbc(&k, &one_block_with_last(&k, 1)).len(), 15);
        assert_eq!(decrypt_aes_cbc(&k, &one_block_with_last(&k, 15)).len(), 1);
    }

    // Nothing validates that the padding bytes agree with the count.
    #[test]
    fn inconsistent_padding_is_accepted() {
        let k = [9u8; 16];
        // A block ending in 4 but whose preceding bytes are not 4s.
        assert_eq!(decrypt_aes_cbc(&k, &one_block_with_last(&k, 4)).len(), 12);
    }

    // T18 — a partial tail is discarded, but the full block before it was
    // followed by input and so survives, unstripped.
    #[test]
    fn a_partial_tail_is_dropped_and_the_block_before_it_is_kept() {
        let k = [4u8; 16];
        let mut payload = one_block_with_last(&k, 3);
        assert_eq!(decrypt_aes_cbc(&k, &payload).len(), 13);
        payload.extend_from_slice(&[0xEE; 5]);
        // 16 IV + 16 data + 5 tail: the data block is emitted whole.
        assert_eq!(decrypt_aes_cbc(&k, &payload).len(), BLOCK);
    }

    #[test]
    fn two_blocks_plus_a_tail_keep_both_blocks() {
        let k = [4u8; 16];
        let iv = [0u8; BLOCK];
        let mut body = vec![0u8; 2 * BLOCK];
        aes_cbc_encrypt(&k, &iv, &mut body).expect("valid key");
        let mut payload = iv.to_vec();
        payload.extend_from_slice(&body);
        payload.extend_from_slice(&[0x11; 7]);
        assert_eq!(decrypt_aes_cbc(&k, &payload).len(), 2 * BLOCK);
    }

    // T19 — RC4 has no length transformation at all.
    #[test]
    fn rc4_preserves_length_and_round_trips() {
        let k = key(16);
        let obj = ObjRef::new(5, 0);
        assert!(decrypt_rc4(&k, obj, &[]).is_empty());
        let data: Vec<u8> = (0..77u8).collect();
        let once = decrypt_rc4(&k, obj, &data);
        assert_eq!(once.len(), data.len());
        assert_eq!(decrypt_rc4(&k, obj, &once), data);
    }

    #[test]
    fn a_key_aes_cannot_accept_yields_empty_output_rather_than_a_panic() {
        for len in [0usize, 1, 15, 17, 31, 33] {
            let bad = vec![0u8; len];
            assert!(
                decrypt_aes_cbc(&bad, &[0xAA; 48]).is_empty(),
                "{len}-byte key"
            );
        }
    }

    #[test]
    fn crypt_classes_are_distinct_values() {
        assert_ne!(CryptClass::Stream, CryptClass::String);
        assert_ne!(CryptClass::String, CryptClass::Embedded);
    }
}
