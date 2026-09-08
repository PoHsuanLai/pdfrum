//! The hash and block-cipher primitives the security handler is built from.
//!
//! Thin, typed wrappers over RustCrypto: fixed-size digest arrays instead of
//! streaming contexts, and an AES-CBC value that owns its chaining state so a
//! multi-block stream decrypts as one call. The C++ aborts the process when a
//! key length or buffer length is wrong; every such case is a
//! [`CipherError`] here, because all of them are reachable from bytes an
//! untrusted file chose.

use aes::{Aes128, Aes192, Aes256};
use cbc::{Decryptor, Encryptor};
use cipher::{BlockModeDecrypt, BlockModeEncrypt, KeyIvInit};
use md5::{Digest, Md5};
use sha1::Sha1;
use sha2::{Sha256, Sha384, Sha512};

/// One AES block, in bytes.
pub(crate) const BLOCK: usize = 16;

/// A key or buffer length AES cannot accept.
///
/// The C++ `CHECK`s these and aborts; we report them (Divergence D4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(crate) enum CipherError {
    /// AES accepts 16-, 24- and 32-byte keys only.
    #[error("AES key length {0} bytes is not 16, 24 or 32")]
    KeyLength(usize),
    /// CBC consumes whole blocks.
    #[error("CBC input of {0} bytes is not a multiple of 16")]
    NotBlockAligned(usize),
}

/// MD5 of one buffer (RFC 1321).
#[must_use]
pub fn md5(data: &[u8]) -> [u8; 16] {
    Md5::digest(data).into()
}

/// MD5 of several buffers hashed as one stream.
///
/// Key derivation feeds an MD5 in a precise order with optional pieces; this
/// keeps the order visible at the call site instead of building a scratch
/// buffer.
#[must_use]
pub(crate) fn md5_parts(parts: &[&[u8]]) -> [u8; 16] {
    let mut hasher = Md5::new();
    for part in parts {
        hasher.update(part);
    }
    hasher.finalize().into()
}

/// SHA-1 of one buffer (FIPS 180-2).
#[must_use]
pub fn sha1(data: &[u8]) -> [u8; 20] {
    Sha1::digest(data).into()
}

/// SHA-256 of one buffer.
#[must_use]
pub(crate) fn sha256(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

/// SHA-256 of several buffers hashed as one stream.
#[must_use]
pub(crate) fn sha256_parts(parts: &[&[u8]]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part);
    }
    hasher.finalize().into()
}

/// SHA-384 of one buffer.
#[must_use]
pub(crate) fn sha384(data: &[u8]) -> [u8; 48] {
    Sha384::digest(data).into()
}

/// SHA-512 of one buffer.
#[must_use]
pub(crate) fn sha512(data: &[u8]) -> [u8; 64] {
    Sha512::digest(data).into()
}

/// AES-CBC encrypt `data` in place under `key` and `iv`.
///
/// Used only by the revision 6 hardened hash, which encrypts a buffer that is
/// a multiple of 64 bytes by construction.
pub(crate) fn aes_cbc_encrypt(
    key: &[u8],
    iv: &[u8; BLOCK],
    data: &mut [u8],
) -> Result<(), CipherError> {
    let mut blocks = split_blocks(data)?;
    match key.len() {
        16 => {
            Encryptor::<Aes128>::new_from_slices(key, iv).map(|mut c| c.encrypt_blocks(&mut blocks))
        }
        24 => {
            Encryptor::<Aes192>::new_from_slices(key, iv).map(|mut c| c.encrypt_blocks(&mut blocks))
        }
        32 => {
            Encryptor::<Aes256>::new_from_slices(key, iv).map(|mut c| c.encrypt_blocks(&mut blocks))
        }
        other => return Err(CipherError::KeyLength(other)),
    }
    .map_err(|_| CipherError::KeyLength(key.len()))?;
    join_blocks(&blocks, data);
    Ok(())
}

/// Split a block-aligned buffer into cipher blocks.
fn split_blocks(data: &[u8]) -> Result<Vec<cipher::Block<Aes128>>, CipherError> {
    if !data.len().is_multiple_of(BLOCK) {
        return Err(CipherError::NotBlockAligned(data.len()));
    }
    // A block-aligned buffer splits into whole blocks, so the remainder the
    // split also yields is empty and no block is dropped.
    Ok(data
        .as_chunks::<BLOCK>()
        .0
        .iter()
        .map(|block| (*block).into())
        .collect())
}

/// Write processed blocks back over the buffer they came from.
fn join_blocks(blocks: &[cipher::Block<Aes128>], data: &mut [u8]) {
    for (chunk, block) in data.as_chunks_mut::<BLOCK>().0.iter_mut().zip(blocks) {
        chunk.copy_from_slice(block);
    }
}

/// AES-CBC decrypt `data` in place under `key` and `iv`.
///
/// No padding is stripped: the callers here decrypt either a bare 32-byte
/// file key, a single `/Perms` block, or a document stream whose padding rule
/// is PDFium's own rather than PKCS#7's.
pub(crate) fn aes_cbc_decrypt(
    key: &[u8],
    iv: &[u8; BLOCK],
    data: &mut [u8],
) -> Result<(), CipherError> {
    let mut blocks = split_blocks(data)?;
    match key.len() {
        16 => {
            Decryptor::<Aes128>::new_from_slices(key, iv).map(|mut c| c.decrypt_blocks(&mut blocks))
        }
        24 => {
            Decryptor::<Aes192>::new_from_slices(key, iv).map(|mut c| c.decrypt_blocks(&mut blocks))
        }
        32 => {
            Decryptor::<Aes256>::new_from_slices(key, iv).map(|mut c| c.decrypt_blocks(&mut blocks))
        }
        other => return Err(CipherError::KeyLength(other)),
    }
    .map_err(|_| CipherError::KeyLength(key.len()))?;
    join_blocks(&blocks, data);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        BLOCK, CipherError, aes_cbc_decrypt, aes_cbc_encrypt, md5, md5_parts, sha1, sha256, sha384,
        sha512,
    };

    fn hex(bytes: &[u8]) -> String {
        use std::fmt::Write as _;
        bytes.iter().fold(String::new(), |mut out, b| {
            let _ = write!(out, "{b:02x}");
            out
        })
    }

    fn unhex(s: &str) -> Vec<u8> {
        s.as_bytes()
            .chunks(2)
            .filter_map(|c| std::str::from_utf8(c).ok())
            .filter_map(|c| u8::from_str_radix(c, 16).ok())
            .collect()
    }

    // T1 — RFC 1321 A.5, ported from fx_crypt_unittest.cpp:51-192.
    #[test]
    fn md5_rfc1321_suite() {
        let cases: [(&[u8], &str); 7] = [
            (b"", "d41d8cd98f00b204e9800998ecf8427e"),
            (b"a", "0cc175b9c0f1b6a831c399e269772661"),
            (b"abc", "900150983cd24fb0d6963f7d28e17f72"),
            (b"message digest", "f96b697d7cb7938d525a2f31aaf161d0"),
            (
                b"abcdefghijklmnopqrstuvwxyz",
                "c3fcd3d76192e4007dfb496cca67e13b",
            ),
            (
                b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789",
                "d174ab98d277d9f5a5611c2c9f419d9f",
            ),
            (
                b"1234567890123456789012345678901234567890\
                  1234567890123456789012345678901234567890",
                "57edf4a22be3c955ac49da2e2107b67a",
            ),
        ];
        for (input, expected) in cases {
            assert_eq!(hex(&md5(input)), expected, "md5 of {input:?}");
        }
    }

    // From fx_crypt_unittest.cpp:79-130: ten megabytes of a byte ramp, fed in
    // 4097-byte chunks so a non-power-of-two update boundary is exercised.
    #[test]
    fn md5_over_ten_megabytes_in_odd_chunks() {
        let data: Vec<u8> = (0..=10 * 1024 * 1024_usize)
            .map(|i| u8::try_from(i & 0xFF).unwrap_or(0))
            .collect();
        assert_eq!(hex(&md5(&data)), "90bd6ad90acef5adaa92203e21c7a13e");
        let chunks: Vec<&[u8]> = data.chunks(4097).collect();
        assert_eq!(hex(&md5_parts(&chunks)), "90bd6ad90acef5adaa92203e21c7a13e");
    }

    #[test]
    fn md5_parts_hashes_the_concatenation() {
        assert_eq!(md5_parts(&[b"ab", b"c"]), md5(b"abc"));
        assert_eq!(md5_parts(&[]), md5(b""));
    }

    // T2 — SHA, from fx_crypt_unittest.cpp:194-260 and :506-600.
    #[test]
    fn sha1_vectors() {
        assert_eq!(hex(&sha1(b"")), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert_eq!(
            hex(&sha1(b"abc")),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        // FIPS 180-2 A.2: two blocks.
        assert_eq!(
            hex(&sha1(
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
            )),
            "84983e441c3bd26ebaae4aa1f95129e5e54670f1"
        );
    }

    #[test]
    fn sha256_vectors() {
        assert_eq!(
            hex(&sha256(b"")),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            hex(&sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        // FIPS 180-2 B.2.
        assert_eq!(
            hex(&sha256(
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
            )),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    #[test]
    fn sha384_vectors() {
        assert_eq!(
            hex(&sha384(b"")),
            "38b060a751ac96384cd9327eb1b1e36a21fdb71114be07434c0cc7bf63f6e1da\
             274edebfe76f65fbd51ad2f14898b95b"
                .replace(char::is_whitespace, "")
        );
        assert_eq!(
            hex(&sha384(
                b"This is a simple test. To see whether it is getting correct value."
            )),
            "9554ffd389f0d642e933fe4c078119cacbb31446d8bda4f412d554037928e5dc\
             12a51be9fe59253c92305ee50e035807"
                .replace(char::is_whitespace, "")
        );
    }

    // The 112-byte cases straddle the 1024-bit variants' 112-vs-128 padding
    // rule (fx_crypt_unittest.cpp:534-548, :583-600).
    #[test]
    fn sha384_and_sha512_at_the_padding_boundary() {
        let input = [b'a'; 112];
        assert_eq!(
            hex(&sha384(&input)),
            "187d4e07cb306103c69967bf544d0dfbe904257759 9c73c330abc0cb64c61236\
             d5ed565ee19119d8c31779a38f791fcd"
                .replace(char::is_whitespace, "")
        );
        assert_eq!(
            hex(&sha512(&input)),
            "c01d080efd492776a1c43bd23dd99d0a2e626d481e16782e75d54c2503b5dc32\
             bd05f0f1ba33e568b88fd2d970929b719ecbb152f58f130a407c8830604b70ca"
                .replace(char::is_whitespace, "")
        );
    }

    #[test]
    fn sha512_vectors() {
        assert_eq!(
            hex(&sha512(b"")),
            "cf83e1357eefb8bdf1542850d66d8007d620e4050b5715dc83f4a921d36ce9ce\
             47d0d13c5d85f2b0ff8318d2877eec2f63b931bd47417a81a538327af927da3e"
                .replace(char::is_whitespace, "")
        );
        assert_eq!(
            hex(&sha512(
                b"This is a simple test. To see whether it is getting correct value."
            )),
            "86b50563a26fd6faeb9bc3bb9eb70382b650556b9069d0a7530a34ddea11cc91\
             5cc793caae30d196bed035214ac642560ca300694477cc3ed4d61031c6c058cf"
                .replace(char::is_whitespace, "")
        );
    }

    /// T3 — the `BoringSSL` NIST SP 800-38A vectors, chained so each IV is
    /// the previous ciphertext. Every case is a round trip.
    fn aes_chain(key_hex: &str, ciphertexts: [&str; 4]) {
        const PLAINTEXTS: [&str; 4] = [
            "6bc1bee22e409f96e93d7e117393172a",
            "ae2d8a571e03ac9c9eb76fac45af8e51",
            "30c81c46a35ce411e5fbc1191a0a52ef",
            "f69f2445df4f9b17ad2b417be66c3710",
        ];
        let key = unhex(key_hex);
        let mut iv: [u8; BLOCK] = unhex("000102030405060708090a0b0c0d0e0f")
            .try_into()
            .expect("16 bytes");
        for (plaintext, expected) in PLAINTEXTS.iter().zip(ciphertexts) {
            let mut buf = unhex(plaintext);
            aes_cbc_encrypt(&key, &iv, &mut buf).expect("valid key and length");
            assert_eq!(hex(&buf), expected, "key {key_hex} iv {}", hex(&iv));

            let mut back = buf.clone();
            aes_cbc_decrypt(&key, &iv, &mut back).expect("valid key and length");
            assert_eq!(hex(&back), *plaintext);

            iv = buf.try_into().expect("16 bytes");
        }
    }

    #[test]
    fn aes128_cbc_chain() {
        aes_chain(
            "2b7e151628aed2a6abf7158809cf4f3c",
            [
                "7649abac8119b246cee98e9b12e9197d",
                "5086cb9b507219ee95db113a917678b2",
                "73bed6b8e3c1743b7116e69e22229516",
                "3ff1caa1681fac09120eca307586e1a7",
            ],
        );
    }

    #[test]
    fn aes192_cbc_chain() {
        aes_chain(
            "8e73b0f7da0e6452c810f32b809079e562f8ead2522c6b7b",
            [
                "4f021db243bc633d7178183a9fa071e8",
                "b4d9ada9ad7dedf4e5e738763f69145a",
                "571b242012fb7ae07fa9baac3df102e0",
                "08b0e27988598881d920a9e64f5615cd",
            ],
        );
    }

    #[test]
    fn aes256_cbc_chain() {
        aes_chain(
            "603deb1015ca71be2b73aef0857d77811f352c073b6108d72d9810a30914dff4",
            [
                "f58c4c04d6e5f1ba779eabfb5f7bfbd6",
                "9cfc4e967edb808d679f777bc6702c7d",
                "39f23369a9d9bacfa530e26304231461",
                "b2eb05e2c39be9fcda6c19078c6a9d1b",
            ],
        );
    }

    /// A multi-block call must chain, not repeat the IV per block.
    #[test]
    fn cbc_chains_across_blocks_within_one_call() {
        let key = unhex("2b7e151628aed2a6abf7158809cf4f3c");
        let iv: [u8; BLOCK] = unhex("000102030405060708090a0b0c0d0e0f")
            .try_into()
            .expect("16 bytes");
        let mut buf = unhex("6bc1bee22e409f96e93d7e117393172aae2d8a571e03ac9c9eb76fac45af8e51");
        aes_cbc_encrypt(&key, &iv, &mut buf).expect("valid");
        assert_eq!(
            hex(&buf),
            "7649abac8119b246cee98e9b12e9197d5086cb9b507219ee95db113a917678b2"
        );
    }

    // D4 — every C++ `CHECK` becomes an error return.
    #[test]
    fn bad_key_and_buffer_lengths_report_rather_than_abort() {
        let iv = [0u8; BLOCK];
        let mut block = [0u8; BLOCK];
        for len in [0usize, 1, 15, 17, 31, 33, 64] {
            let key = vec![0u8; len];
            assert_eq!(
                aes_cbc_decrypt(&key, &iv, &mut block),
                Err(CipherError::KeyLength(len))
            );
        }
        let key = [0u8; 16];
        for len in [1usize, 15, 17, 31] {
            let mut buf = vec![0u8; len];
            assert_eq!(
                aes_cbc_encrypt(&key, &iv, &mut buf),
                Err(CipherError::NotBlockAligned(len))
            );
        }
        // An empty buffer is block-aligned and is a no-op.
        assert_eq!(aes_cbc_decrypt(&key, &iv, &mut []), Ok(()));
    }
}
