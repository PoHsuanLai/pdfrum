//! Per-object keys and payload encryption/decryption (ISO 32000 §7.6.2,
//! Algorithm 1).
//!
//! Every string and stream in a document is enciphered under a key derived
//! from the file key and the *enclosing indirect object's* number and
//! generation — not the number of whatever nested object the string sits in.
//! AESV3 is the exception: at a 32-byte key the file key is used verbatim,
//! with no per-object derivation at all.
//!
//! # The two directions are not mirror images
//!
//! The **object key** derivation is shared: encrypt and decrypt call the same
//! three functions, which is what makes a round trip work at all.
//!
//! The **cipher** is where they part. RC4 is its own inverse, so
//! [`encrypt_rc4`] and [`decrypt_rc4`] are literally the same call. AES is
//! not: the decrypt side reproduces PDFium's streaming decoder, whose
//! one-block lag and unvalidated padding are quirks of *reading* a file
//! someone else wrote (Divergence D6). Writing one has no such history to
//! honour — we emit a fresh random IV and standard PKCS#7, which the quirky
//! reader accepts because a well-formed PKCS#7 tail is exactly the case its
//! rules were built around. See [`encrypt_aes_cbc`].

use pdfrum_object::ObjRef;

use crate::key::SmallKey;
use crate::primitives::{BLOCK, aes_cbc_decrypt, aes_cbc_encrypt, md5};
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

/// One AES initialisation vector, supplied by the caller of
/// [`crate::SecurityHandler::encrypt`].
///
/// A named type rather than a bare `[u8; 16]` because the encrypt call
/// already carries an object reference and a payload, and a bare array beside
/// those is one `&[u8]` away from being passed the payload by mistake. It
/// also gives the "where does this come from?" question somewhere to be
/// answered: nowhere in this crate, is the answer — see the crate docs.
///
/// The RC4 handlers ignore it entirely.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Iv(pub [u8; BLOCK]);

impl Iv {
    /// The vector's bytes.
    #[must_use]
    pub const fn bytes(&self) -> &[u8; BLOCK] {
        &self.0
    }
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

/// Encrypt an RC4 payload. Symmetric, so this is the same call as
/// [`decrypt_rc4`] — the two names exist so the call sites read in the
/// direction they mean.
#[must_use]
pub(crate) fn encrypt_rc4(key: &SmallKey, obj: ObjRef, data: &[u8]) -> Vec<u8> {
    rc4(&rc4_object_key(key, obj), data)
}

/// Encrypt an AESV2 payload under a per-object key, prefixing `iv`.
///
/// The object key is [`aes_v4_object_key`]'s full sixteen bytes — the same
/// derivation the decrypt side uses. The C++'s encrypt path truncates it to
/// the *file* key's length instead (`EncryptContent` calls `CRYPT_AESSetKey`
/// with `realkey.first(key_len_)`), which differs whenever the file key is
/// not sixteen bytes. It never is: AESV2 is 128-bit by definition, so the two
/// readings coincide on every real document, and at a 24-byte file key the
/// C++ would ask a 16-byte array for 24 bytes and abort. Sharing one
/// derivation is what makes encrypt-then-decrypt exact.
#[must_use]
pub(crate) fn encrypt_aes_v4(
    key: &SmallKey,
    obj: ObjRef,
    iv: &[u8; BLOCK],
    data: &[u8],
) -> Vec<u8> {
    encrypt_aes_cbc(&aes_v4_object_key(key, obj), iv, data)
}

/// Encrypt an AESV3 payload under the file key itself, prefixing `iv`.
#[must_use]
pub(crate) fn encrypt_aes_v5(key: &[u8; 32], iv: &[u8; BLOCK], data: &[u8]) -> Vec<u8> {
    encrypt_aes_cbc(key, iv, data)
}

/// Emit `iv` followed by the PKCS#7-padded, CBC-encrypted plaintext.
///
/// Standard PKCS#7 throughout: the padding is always added, so a plaintext
/// whose length is already a multiple of sixteen grows by a whole block of
/// `0x10` bytes, and the output is always `16 + 16 * ceil((n + 1) / 16)`
/// bytes. That is what makes the round trip exact against the quirky decoder
/// on the other side — its "last plaintext byte under sixteen strips that
/// many" rule and PKCS#7 agree on every value 1 through 16, and the extra
/// block is what keeps a length-16 payload from being read as a length-0 one.
///
/// A key AES cannot accept yields empty output rather than an error, matching
/// the decrypt side's contract: this crate never fails a cipher call.
#[must_use]
fn encrypt_aes_cbc(key: &[u8], iv: &[u8; BLOCK], data: &[u8]) -> Vec<u8> {
    let pad = BLOCK - data.len() % BLOCK;
    let mut body = data.to_vec();
    // `pad` is 1..=16, so the conversion never saturates.
    body.extend(std::iter::repeat_n(u8::try_from(pad).unwrap_or(0), pad));
    if aes_cbc_encrypt(key, iv, &mut body).is_err() {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(BLOCK.saturating_add(body.len()));
    out.extend_from_slice(iv);
    out.append(&mut body);
    out
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
        BLOCK, CryptClass, Iv, aes_v4_object_key, decrypt_aes_cbc, decrypt_aes_v4, decrypt_aes_v5,
        decrypt_rc4, encrypt_aes_cbc, encrypt_aes_v4, encrypt_aes_v5, encrypt_rc4, rc4_object_key,
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

    // ---- M10: the encrypt direction ----

    // The length law, which is what the writer's `/Length` depends on: a
    // vector plus a PKCS#7 pad that is *always* added.
    #[test]
    fn aes_encryption_grows_a_payload_by_a_vector_and_a_pad() {
        let k = [0x2Bu8; 16];
        for (plain, expected) in [
            (0usize, 32),
            (1, 32),
            (15, 32),
            (16, 48),
            (17, 48),
            (31, 48),
        ] {
            let out = encrypt_aes_cbc(&k, &[0u8; BLOCK], &vec![0xA5; plain]);
            assert_eq!(out.len(), expected, "{plain} bytes of plaintext");
        }
    }

    // The whole point: our own decoder — quirks and all — reads back exactly
    // what our encoder wrote, at every length.
    #[test]
    fn aes_round_trips_through_the_quirky_decoder_at_every_length() {
        let k = [0x3Cu8; 16];
        for len in 0..96usize {
            let plaintext: Vec<u8> = (0..len)
                .map(|i| u8::try_from(i % 251).unwrap_or(0))
                .collect();
            let iv = [u8::try_from(len % 256).unwrap_or(0); BLOCK];
            let sealed = encrypt_aes_cbc(&k, &iv, &plaintext);
            assert_eq!(
                decrypt_aes_cbc(&k, &sealed),
                plaintext,
                "{len} bytes did not survive"
            );
        }
    }

    // The vector really is the first sixteen bytes, and really does change
    // the ciphertext: two vectors over one plaintext share no block.
    #[test]
    fn the_vector_is_the_prefix_and_changes_every_block() {
        let k = [0x11u8; 32];
        let plaintext = vec![0u8; 3 * BLOCK];
        let first = encrypt_aes_cbc(&k, &[1u8; BLOCK], &plaintext);
        let second = encrypt_aes_cbc(&k, &[2u8; BLOCK], &plaintext);
        assert_eq!(first.get(..BLOCK), Some(&[1u8; BLOCK][..]));
        assert_eq!(second.get(..BLOCK), Some(&[2u8; BLOCK][..]));
        assert_ne!(first.get(BLOCK..), second.get(BLOCK..));
    }

    // Per-object keys really are per object, in both directions and by the
    // same derivation — which is what makes the round trip object-keyed.
    #[test]
    fn the_object_keyed_ciphers_round_trip_under_the_same_reference() {
        let k = key(16);
        let obj = ObjRef::new(12, 3);
        let payload: Vec<u8> = (0..70u8).collect();

        assert_eq!(
            decrypt_rc4(&k, obj, &encrypt_rc4(&k, obj, &payload)),
            payload
        );
        let sealed = encrypt_aes_v4(&k, obj, &Iv([9; BLOCK]).0, &payload);
        assert_eq!(decrypt_aes_v4(&k, obj, &sealed), payload);
        // A different object cannot read it.
        assert_ne!(decrypt_aes_v4(&k, ObjRef::new(13, 3), &sealed), payload);

        let file_key = [0x5Au8; 32];
        let sealed = encrypt_aes_v5(&file_key, &[3; BLOCK], &payload);
        assert_eq!(decrypt_aes_v5(&file_key, &sealed), payload);
    }

    // RC4 encrypt and decrypt are the same call, so applying either twice is
    // the identity.
    #[test]
    fn rc4_encryption_is_its_own_inverse() {
        let k = key(10);
        let obj = ObjRef::new(3, 0);
        let payload: Vec<u8> = (0..40u8).map(|i| i.wrapping_mul(7)).collect();
        assert_eq!(
            encrypt_rc4(&k, obj, &payload),
            decrypt_rc4(&k, obj, &payload)
        );
        assert!(encrypt_rc4(&k, obj, &[]).is_empty());
    }

    #[test]
    fn an_impossible_key_encrypts_to_nothing_rather_than_panicking() {
        for len in [0usize, 1, 15, 17, 31, 33] {
            assert!(encrypt_aes_cbc(&vec![0u8; len], &[0; BLOCK], b"payload").is_empty());
        }
    }

    #[test]
    fn crypt_classes_are_distinct_values() {
        assert_ne!(CryptClass::Stream, CryptClass::String);
        assert_ne!(CryptClass::String, CryptClass::Embedded);
    }
}
