//! The `/Filter /Standard` security handler: reading `/Encrypt` and running
//! the revision 2 to 6 password algorithms (ISO 32000 §7.6.3, ISO 32000-2
//! §7.6.4).
//!
//! Two stages, kept apart. [`EncryptParams`] is a record of what the file
//! said — the only place that touches a [`Dict`] — and the algorithms below
//! are free functions over it. That split is why the algorithm tests can be
//! known-answer tests over literal parameters instead of end-to-end document
//! opens.
//!
//! Which accessor reads which key is load-bearing rather than incidental. The
//! C++ reads `/Filter` name-typed (a reference or a string there is not the
//! standard handler), `/V`, `/R`, `/P` and `/Length` through an accessor that
//! coerces any type and follows one reference, `/EncryptMetadata`
//! boolean-typed before resolving (so an `Int(1)` is not `true`), and `/CF`
//! after resolving. Files in the wild depend on each of those.

use pdfrum_object::{Dict, Name, Resolve, names};

use crate::Error;
use crate::key::SmallKey;
use crate::primitives::{
    BLOCK, aes_cbc_decrypt, aes_cbc_encrypt, md5, md5_parts, sha256, sha256_parts, sha384, sha512,
};
use crate::rc4::{rc4, rc4_in_place};

/// The 32-byte padding string every revision 2 to 4 password is padded with
/// (ISO 32000 §7.6.3.3, "Algorithm 2" step a).
pub const PAD: [u8; 32] = [
    0x28, 0xbf, 0x4e, 0x5e, 0x4e, 0x75, 0x8a, 0x41, 0x64, 0x00, 0x4e, 0x56, 0xff, 0xfa, 0x01, 0x08,
    0x2e, 0x2e, 0x00, 0xb6, 0xd0, 0x68, 0x3e, 0x80, 0x2f, 0x0c, 0xa9, 0xfe, 0x64, 0x53, 0x69, 0x7a,
];

/// The cipher a document's crypt filter resolves to.
///
/// `Aes` covers both AESV2 and AESV3: PDFium never distinguishes them by
/// name, only by whether the key is 32 bytes long.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cipher {
    /// `/StrF /Identity` — payloads pass through untouched.
    None,
    /// RC4, for `/V 1..4` without an AES crypt-filter method.
    Rc4,
    /// AES-CBC, for `/CFM /AESV2` or `/CFM /AESV3`.
    Aes,
}

impl Cipher {
    /// The spelling used in [`Error::CipherKeyLength`].
    const fn label(self) -> &'static str {
        match self {
            Self::None => "identity",
            Self::Rc4 => "RC4",
            Self::Aes => "AES",
        }
    }

    /// Whether `len` bytes is a key length this cipher accepts.
    ///
    /// RC4's floor of 5 bytes is what rejects a `/V 2 /Length 8` document:
    /// the revision-under-4 path divides by 8 without the promotion the
    /// version-4 path applies, leaving a 1-byte key.
    const fn accepts_key_len(self, len: usize) -> bool {
        match self {
            Self::None => true,
            Self::Rc4 => 5 <= len && len <= 16,
            Self::Aes => matches!(len, 16 | 24 | 32),
        }
    }
}

/// The `/Encrypt` dictionary as a record of what the file said.
///
/// Nothing here is validated against a password yet; `cipher` and `key_len`
/// are the one derived pair, because resolving them is where a malformed
/// dictionary is rejected.
#[derive(Debug, Clone)]
pub struct EncryptParams {
    /// `/V`, the algorithm version. Default 0.
    pub version: i64,
    /// `/R`, the handler revision. Default 0.
    pub revision: i64,
    /// `/P`, the permission flags, as the unsigned word they are compared as.
    /// Default `0xFFFF_FFFF`.
    pub permissions: u32,
    /// The cipher `/V`, `/CF` and `/CFM` resolve to.
    pub cipher: Cipher,
    /// The file encryption key length in bytes, 0 to 32.
    pub key_len: usize,
    /// `/EncryptMetadata`. Default `true`.
    pub encrypt_metadata: bool,
    /// `/O`, the owner password entry.
    pub o: Box<[u8]>,
    /// `/U`, the user password entry.
    pub u: Box<[u8]>,
    /// `/OE`, the owner encrypted file key (revision 5 and up).
    pub oe: Box<[u8]>,
    /// `/UE`, the user encrypted file key (revision 5 and up).
    pub ue: Box<[u8]>,
    /// `/Perms`, the encrypted permission block (revision 5 and up).
    pub perms: Box<[u8]>,
}

/// Read an `/Encrypt` dictionary into an [`EncryptParams`].
///
/// # Errors
///
/// [`Error::UnsupportedHandler`] for a `/Filter` other than `/Standard`,
/// [`Error::MismatchedCryptFilters`] when `/StmF` and `/StrF` name different
/// filters, [`Error::MissingCryptFilter`] when the named filter is not in
/// `/CF`, and [`Error::MalformedEncryptDict`] or [`Error::CipherKeyLength`]
/// when the key length does not resolve.
pub fn parse_encrypt_dict(dict: &Dict, r: &impl Resolve) -> Result<EncryptParams, Error> {
    // The parser rejects a non-standard handler on the name-typed reading; a
    // string-typed /Filter reads as absent here and so is unsupported too.
    let filter = dict.name(names::FILTER);
    if filter != Some(names::STANDARD) {
        let spelling = filter.map(|n| n.as_bytes().into()).unwrap_or_default();
        return Err(Error::UnsupportedHandler(spelling));
    }

    let version = dict.int(names::V, r).unwrap_or(0);
    let revision = dict.int(names::R, r).unwrap_or(0);
    let permissions = as_u32(dict.int(names::P, r).unwrap_or(-1));
    let encrypt_metadata = dict.bool(names::ENCRYPT_METADATA).unwrap_or(true);

    let (cipher, key_len) = resolve_cipher(dict, version, r)?;

    Ok(EncryptParams {
        version,
        revision,
        permissions,
        cipher,
        key_len,
        encrypt_metadata,
        o: byte_string(dict, names::O, r),
        u: byte_string(dict, names::U, r),
        oe: byte_string(dict, names::OE, r),
        ue: byte_string(dict, names::UE, r),
        perms: byte_string(dict, names::PERMS, r),
    })
}

/// The `/P` word as the unsigned value every comparison uses.
///
/// `/P` is written as a signed integer and compared as a `uint32`; a file may
/// also spell it as the already-unsigned `4294967232`, which the integer
/// reading has narrowed to `-64` by the time it arrives here.
fn as_u32(value: i64) -> u32 {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the C-int wrap is the semantic"
    )]
    let narrowed = value as i32;
    narrowed.cast_unsigned()
}

/// A string-valued entry as raw bytes, empty when absent.
fn byte_string(dict: &Dict, key: &Name, r: &impl Resolve) -> Box<[u8]> {
    dict.byte_string(key, r).unwrap_or_default().into()
}

/// Resolve `/V`, `/Length`, `/CF` and `/CFM` into a cipher and key length.
///
/// The version 4 branch carries two quirks that keep real files opening: a
/// `/Length` under 40 is read as *bytes* and multiplied by 8 (so a file
/// writing `/Length 16` for a 128-bit key works), and a `/CFM` PDFium does
/// not recognise leaves the cipher as RC4 rather than failing.
fn resolve_cipher(dict: &Dict, version: i64, r: &impl Resolve) -> Result<(Cipher, usize), Error> {
    let (cipher, key_bits) = if version >= 4 {
        // The class filters are compared before the dictionary is inspected,
        // so a mismatch is reported even when /CF is missing too.
        let name = crypt_filter_name(dict, r)?;
        let filters = dict.dict(names::CF, r).ok_or(Error::MalformedEncryptDict(
            "/CF is missing or not a dictionary",
        ))?;
        if name.as_bytes() == names::IDENTITY.as_bytes() {
            return Ok((Cipher::None, 0));
        }
        let filter = filters
            .dict(&name, r)
            .ok_or_else(|| Error::MissingCryptFilter(name.as_bytes().into()))?;

        // At version 4 the per-filter /Length wins, falling back to the
        // document's; from version 5 the per-filter one is ignored outright.
        let bits = if version == 4 {
            match filter.int(names::LENGTH, r).unwrap_or(0) {
                0 => dict.int(names::LENGTH, r).unwrap_or(128),
                bits => bits,
            }
        } else {
            dict.int(names::LENGTH, r).unwrap_or(256)
        };
        if bits < 0 {
            return Err(Error::MalformedEncryptDict("/Length is negative"));
        }
        let bits = if bits < 40 { bits * 8 } else { bits };

        let method = filter.byte_string(names::CFM, r).unwrap_or_default();
        let cipher = if method == b"AESV2" || method == b"AESV3" {
            Cipher::Aes
        } else {
            Cipher::Rc4
        };
        (cipher, bits)
    } else if version > 1 {
        (Cipher::Rc4, dict.int(names::LENGTH, r).unwrap_or(40))
    } else {
        // Version 1 is 40-bit RC4 by definition; its /Length is ignored.
        (Cipher::Rc4, 40)
    };

    let key_len = usize::try_from(key_bits / 8)
        .map_err(|_| Error::MalformedEncryptDict("/Length is negative"))?;
    if key_len > 32 || !cipher.accepts_key_len(key_len) {
        return Err(Error::CipherKeyLength {
            cipher: cipher.label(),
            len: key_len,
        });
    }
    Ok((cipher, key_len))
}

/// The crypt filter both `/StmF` and `/StrF` must name.
///
/// PDFium refuses a document whose two class filters differ rather than
/// keeping a cipher per class, and it does not implement the specification's
/// `/Identity` default: when both keys are absent the looked-up name is empty,
/// which is not `/Identity` and is not a key in `/CF`, so the document is
/// rejected. Both behaviors are reproduced.
fn crypt_filter_name(dict: &Dict, r: &impl Resolve) -> Result<Name, Error> {
    let stream = dict.byte_string(names::STM_F, r).unwrap_or_default();
    let string = dict.byte_string(names::STR_F, r).unwrap_or_default();
    if stream != string {
        return Err(Error::MismatchedCryptFilters);
    }
    Ok(Name::from(string.as_slice()))
}

/// Pad a password to the fixed 32 bytes every revision 2 to 4 algorithm
/// hashes (ISO 32000 §7.6.3.3, "Algorithm 2" step a).
///
/// The first bytes are the password, up to 32; the rest come from the *front*
/// of the pad, not from the pad position they sit at. A password of 32 bytes
/// or more is truncated with no padding at all, so no length is ever encoded.
fn pad_password(password: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    let taken = password.len().min(out.len());
    if let (Some(head), Some(source)) = (out.get_mut(..taken), password.get(..taken)) {
        head.copy_from_slice(source);
    }
    if let (Some(tail), Some(fill)) = (out.get_mut(taken..), PAD.get(..32 - taken)) {
        tail.copy_from_slice(fill);
    }
    out
}

/// ISO 32000 Algorithm 2 — the revision 2 to 4 file encryption key.
///
/// The MD5 order is exact and every optional piece contributes nothing at all
/// when absent, not a length marker: the padded password, `/O` verbatim at
/// whatever length the file wrote it, `/P` as a little-endian word, the first
/// `/ID` element when non-empty, and — unless `ignore_metadata` overrides it —
/// four `0xFF` bytes when revision 3 or later turned `/EncryptMetadata` off.
///
/// The revision 3 strengthening loop hashes only the first `key_len` bytes of
/// each digest while writing a full 16-byte one, fifty times.
fn file_key_r234(
    p: &EncryptParams,
    password: &[u8],
    file_id: &[u8],
    ignore_metadata: bool,
) -> SmallKey {
    let passcode = pad_password(password);
    let perm = p.permissions.to_le_bytes();
    let metadata_tag = [0xFFu8; 4];
    let revision_3_or_later = p.revision >= 3;
    let mut parts: Vec<&[u8]> = vec![&passcode, &p.o, &perm];
    if !file_id.is_empty() {
        parts.push(file_id);
    }
    if !ignore_metadata && revision_3_or_later && !p.encrypt_metadata {
        parts.push(&metadata_tag);
    }
    let mut digest = md5_parts(&parts);

    let copy_len = p.key_len.min(digest.len());
    if revision_3_or_later {
        for _ in 0..50 {
            digest = digest.get(..copy_len).map_or(digest, md5);
        }
    }
    SmallKey::from_prefix(&digest, p.key_len)
}

/// ISO 32000 Algorithms 4 and 5 — does `password` unlock the document as the
/// user password? Answers with the derived file key.
///
/// Only the first 16 bytes of `/U` are ever compared, at every revision. A
/// `/U` shorter than that is a damage guard, but one of 16 to 31 bytes is
/// accepted and zero-padded into the 32-byte working buffer.
fn check_user_password_r234(
    p: &EncryptParams,
    password: &[u8],
    file_id: &[u8],
    ignore_metadata: bool,
) -> Option<SmallKey> {
    let key = file_key_r234(p, password, file_id, ignore_metadata);
    let stored = p.u.get(..16)?;

    if p.revision == 2 {
        let encrypted = rc4(key.bytes(), &PAD);
        return (encrypted.get(..16) == Some(stored)).then_some(key);
    }

    // Revision 3 and up: undo twenty rounds of RC4 under keys derived by
    // xor-ing the file key with the round number, then compare against the
    // hash of the pad and the file id.
    let mut test = [0u8; 32];
    let copied = p.u.len().min(test.len());
    if let (Some(head), Some(source)) = (test.get_mut(..copied), p.u.get(..copied)) {
        head.copy_from_slice(source);
    }
    let mut round_key = [0u8; 32];
    for round in (0..20u8).rev() {
        for (slot, byte) in round_key.iter_mut().zip(key.bytes()) {
            *slot = byte ^ round;
        }
        rc4_in_place(round_key.get(..key.len()).unwrap_or_default(), &mut test);
    }

    let expected = if file_id.is_empty() {
        md5(&PAD)
    } else {
        md5_parts(&[&PAD, file_id])
    };
    (test.get(..16) == expected.get(..16)).then_some(key)
}

/// ISO 32000 Algorithm 7 — recover the user password from the owner password.
///
/// An `/O` shorter than 32 bytes yields an empty recovered password, which
/// then fails the user check; that guard is what keeps a truncated `/O` from
/// reading out of bounds.
///
/// The trailing strip is deliberately not a general "remove the padding": it
/// compares each tail byte against the *pad byte at the same index*, so a
/// recovered password whose own last byte happens to equal the pad byte there
/// loses one byte too many. Files were produced against this behavior.
fn recover_user_password(p: &EncryptParams, owner_password: &[u8]) -> Vec<u8> {
    let Some(stored) = p.o.get(..32) else {
        return Vec::new();
    };

    let mut digest = md5(&pad_password(owner_password));
    if p.revision >= 3 {
        // Unlike Algorithm 2's loop, this one re-hashes the whole digest.
        for _ in 0..50 {
            digest = md5(&digest);
        }
    }
    let key = SmallKey::from_prefix(&digest, p.key_len);

    let mut buf = [0u8; 32];
    buf.copy_from_slice(stored);
    if p.revision == 2 {
        rc4_in_place(key.bytes(), &mut buf);
    } else {
        let mut round_key = [0u8; 32];
        for round in (0..20u8).rev() {
            for (slot, byte) in round_key.iter_mut().zip(key.bytes()) {
                *slot = byte ^ round;
            }
            rc4_in_place(round_key.get(..key.len()).unwrap_or_default(), &mut buf);
        }
    }

    let mut len = buf.len();
    while len > 0 && PAD.get(len - 1) == buf.get(len - 1) {
        len -= 1;
    }
    buf.get(..len).unwrap_or_default().to_vec()
}

/// ISO 32000-2 Algorithm 2.A — the revision 5 and 6 password check.
///
/// Returns the 32-byte file key on success. Both `/O` and `/U` must be at
/// least 48 bytes whichever role is being checked, because the owner check
/// hashes the whole of `/U` alongside the password.
fn check_password_aes256(p: &EncryptParams, password: &[u8], owner: bool) -> Option<[u8; 32]> {
    let owner_entry: &[u8; 48] = p.o.get(..48)?.try_into().ok()?;
    let user_entry: &[u8; 48] = p.u.get(..48)?.try_into().ok()?;
    let entry = if owner { owner_entry } else { user_entry };
    let vector = owner.then_some(user_entry);

    let validation_salt: [u8; 8] = entry.get(32..40)?.try_into().ok()?;
    let key_salt: [u8; 8] = entry.get(40..48)?.try_into().ok()?;

    let hash = |salt: [u8; 8]| -> [u8; 32] {
        if p.revision >= 6 {
            revision6_hash(password, salt, vector)
        } else {
            match vector {
                Some(v) => sha256_parts(&[password, &salt, v]),
                None => sha256_parts(&[password, &salt]),
            }
        }
    };

    if entry.get(..32) != Some(hash(validation_salt).as_slice()) {
        return None;
    }

    let intermediate = hash(key_salt);
    let encrypted = if owner { &p.oe } else { &p.ue };
    let mut file_key: [u8; 32] = encrypted.get(..32)?.try_into().ok()?;
    aes_cbc_decrypt(&intermediate, &[0u8; BLOCK], &mut file_key).ok()?;

    check_perms(p, &file_key).then_some(file_key)
}

/// Validate `/Perms`, the encrypted copy of the permission word.
///
/// A short `/Perms` is zero-padded into the single block rather than
/// rejected, and the metadata comparison is deliberately one-sided: producers
/// disagree with themselves often enough that the decrypted block is treated
/// as the truth, and the only rejected combination is a block claiming
/// metadata *is* encrypted while `/EncryptMetadata` says it is not.
fn check_perms(p: &EncryptParams, file_key: &[u8; 32]) -> bool {
    if p.perms.is_empty() {
        return false;
    }
    let mut block = [0u8; BLOCK];
    let copied = p.perms.len().min(block.len());
    match (block.get_mut(..copied), p.perms.get(..copied)) {
        (Some(head), Some(source)) => head.copy_from_slice(source),
        _ => return false,
    }
    if aes_cbc_decrypt(file_key, &[0u8; BLOCK], &mut block).is_err() {
        return false;
    }

    if block.get(9..12) != Some(b"adb") {
        return false;
    }
    let Some(word) = block.get(..4).and_then(|w| <[u8; 4]>::try_from(w).ok()) else {
        return false;
    };
    if u32::from_le_bytes(word) != p.permissions {
        return false;
    }
    block.get(8) == Some(&b'F') || p.encrypt_metadata
}

/// ISO 32000-2 Algorithm 2.B — the revision 6 hardened iterated hash.
///
/// Each round encrypts sixty-four repetitions of
/// `password || K[..block_size] || vector?` under AES-128-CBC keyed by the
/// *current* `K`, then re-hashes the whole ciphertext with SHA-256, -384 or
/// -512 as selected by the ciphertext's first sixteen bytes modulo three. The
/// sixty-four-fold repetition is what guarantees the buffer is block-aligned
/// for any password length.
///
/// The loop runs at least 64 rounds and stops once the last byte of the
/// *whole* ciphertext — not of the digest — falls to `i - 32`, which caps it
/// at 287.
fn revision6_hash(password: &[u8], salt: [u8; 8], vector: Option<&[u8; 48]>) -> [u8; 32] {
    revision6_hash_counted(password, salt, vector).0
}

/// [`revision6_hash`] with the round count, so tests can assert the loop's
/// bounds without inferring them from timings.
fn revision6_hash_counted(
    password: &[u8],
    salt: [u8; 8],
    vector: Option<&[u8; 48]>,
) -> ([u8; 32], u32) {
    let mut state: Vec<u8> = match vector {
        Some(v) => sha256_parts(&[password, &salt, v]),
        None => sha256_parts(&[password, &salt]),
    }
    .to_vec();

    let mut block_size = 32usize;
    let mut round = 0u32;
    loop {
        let piece = state.get(..block_size).unwrap_or(&state);
        let mut content = Vec::with_capacity(64 * (password.len() + piece.len() + 48));
        for _ in 0..64 {
            content.extend_from_slice(password);
            content.extend_from_slice(piece);
            if let Some(v) = vector {
                content.extend_from_slice(v);
            }
        }

        // The key and IV come from the head of the current state regardless
        // of how long `block_size` has grown.
        let (Some(key), Some(iv)) = (
            state.get(..16),
            state.get(16..32).and_then(|s| <[u8; 16]>::try_from(s).ok()),
        ) else {
            return ([0u8; 32], round);
        };
        if aes_cbc_encrypt(key, &iv, &mut content).is_err() {
            return ([0u8; 32], round);
        }

        let Some(head) = content.get(..16) else {
            return ([0u8; 32], round);
        };
        state = match big_order_64_bits_mod3(head) {
            0 => {
                block_size = 32;
                sha256(&content).to_vec()
            }
            1 => {
                block_size = 48;
                sha384(&content).to_vec()
            }
            _ => {
                block_size = 64;
                sha512(&content).to_vec()
            }
        };

        round += 1;
        // The comparison is `round - 32 < last_byte`, evaluated after the
        // increment and against the last byte of the *whole* ciphertext
        // rather than of the digest. At `round >= 64` the left side is at
        // least 32, so a last byte of 255 caps the loop at 287 rounds.
        let last = u32::from(content.last().copied().unwrap_or(0));
        if round >= 64 && round.saturating_sub(32) >= last {
            break;
        }
    }

    let hash = state
        .get(..32)
        .and_then(|s| <[u8; 32]>::try_from(s).ok())
        .unwrap_or([0u8; 32]);
    (hash, round)
}

/// Fold four big-endian words of `data` modulo three.
///
/// Arithmetically this equals the byte sum modulo three, because `2^32 ≡ 1
/// (mod 3)`, but the fold is written as the C++ writes it so no equivalence
/// argument sits between the specification and the code.
fn big_order_64_bits_mod3(data: &[u8]) -> u64 {
    let mut acc = 0u64;
    for chunk in data.chunks_exact(4).take(4) {
        let word = chunk
            .try_into()
            .map_or(0, |bytes: [u8; 4]| u32::from_be_bytes(bytes));
        acc = ((acc << 32) | u64::from(word)) % 3;
    }
    acc
}

/// Which of the two password encodings unlocked a document.
///
/// PDFium retries a non-ASCII password in the other encoding, and the *save*
/// path re-encrypts with the spelling that worked. This crate only decrypts
/// (Divergence D2), so the value is reportable state rather than an input to
/// anything here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PasswordEncoding {
    /// The password bytes as supplied unlocked the document.
    #[default]
    AsGiven,
    /// Each byte was read as a Latin-1 scalar and re-encoded as UTF-8
    /// (revision 5 and up, where passwords are nominally UTF-8).
    Latin1ToUtf8,
    /// The bytes were decoded as UTF-8 and narrowed to Latin-1 (revision 2 to
    /// 4, which hash raw bytes).
    Utf8ToLatin1,
}

/// A password that unlocked a document: the key it produced, the role it
/// played, and the encoding that worked.
#[derive(Debug, Clone)]
pub(crate) struct Unlocked {
    pub key: SmallKey,
    pub encoding: PasswordEncoding,
}

/// Try `password` in the given role, retrying in the other encoding when the
/// bytes as given fail.
///
/// A pure-ASCII password is a fixed point of both conversions, so the retry is
/// skipped for one — the early return is observable as the *absence* of extra
/// work, and is what the ASCII-password fixtures pin.
pub(crate) fn try_password(
    p: &EncryptParams,
    password: &[u8],
    owner: bool,
    file_id: &[u8],
) -> Option<Unlocked> {
    if let Some(key) = check_password(p, password, owner, file_id) {
        return Some(Unlocked {
            key,
            encoding: PasswordEncoding::AsGiven,
        });
    }
    if password.is_ascii() {
        return None;
    }
    let (converted, encoding) = if p.revision >= 5 {
        (latin1_to_utf8(password), PasswordEncoding::Latin1ToUtf8)
    } else {
        (utf8_to_latin1(password), PasswordEncoding::Utf8ToLatin1)
    };
    check_password(p, &converted, owner, file_id).map(|key| Unlocked { key, encoding })
}

/// One password attempt with no encoding fallback.
///
/// Below revision 5 the user check runs twice, once honoring
/// `/EncryptMetadata` and once ignoring it, for files that wrote
/// `/EncryptMetadata false` but computed `/U` without the tag. The key kept is
/// the one the *successful* attempt derived.
fn check_password(
    p: &EncryptParams,
    password: &[u8],
    owner: bool,
    file_id: &[u8],
) -> Option<SmallKey> {
    if p.revision >= 5 {
        return check_password_aes256(p, password, owner).map(SmallKey::from_full);
    }
    let effective = if owner {
        recover_user_password(p, password)
    } else {
        password.to_vec()
    };
    check_user_password_r234(p, &effective, file_id, false)
        .or_else(|| check_user_password_r234(p, &effective, file_id, true))
}

/// Read each byte as a Latin-1 scalar and re-encode the run as UTF-8.
fn latin1_to_utf8(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    for &byte in bytes {
        let mut buf = [0u8; 4];
        out.extend_from_slice(char::from(byte).encode_utf8(&mut buf).as_bytes());
    }
    out
}

/// Decode UTF-8 the way PDFium does, then narrow each scalar to one byte.
///
/// The decoder is deliberately lenient rather than strict or replacing: a
/// stray continuation byte outside a sequence is *dropped*, a truncated
/// sequence contributes nothing, and overlong or surrogate encodings are
/// accepted as whatever they decode to. Nothing becomes `U+FFFD`. The
/// narrowing then keeps the low byte of each scalar, so `U+00E2` and `U+2AE2`
/// both narrow to `0xE2`.
fn utf8_to_latin1(bytes: &[u8]) -> Vec<u8> {
    const MAX_CODE_POINT: u32 = 0x0010_FFFF;
    let mut out = Vec::with_capacity(bytes.len());
    let mut remaining = 0u32;
    let mut code_point = 0u32;
    let emit = |cp: u32, out: &mut Vec<u8>| {
        if cp <= MAX_CODE_POINT {
            #[expect(clippy::cast_possible_truncation, reason = "narrowing is the semantic")]
            out.push(cp as u8);
        }
    };
    for &unit in bytes {
        match unit {
            0x00..=0x7F => {
                remaining = 0;
                emit(u32::from(unit), &mut out);
            }
            0x80..=0xBF => {
                if remaining > 0 {
                    remaining -= 1;
                    code_point = (code_point << 6) | u32::from(unit & 0x3F);
                    if remaining == 0 {
                        emit(code_point, &mut out);
                    }
                }
            }
            0xC0..=0xDF => {
                remaining = 1;
                code_point = u32::from(unit & 0x1F);
            }
            0xE0..=0xEF => {
                remaining = 2;
                code_point = u32::from(unit & 0x0F);
            }
            0xF0..=0xF7 => {
                remaining = 3;
                code_point = u32::from(unit & 0x07);
            }
            0xF8..=0xFF => remaining = 0,
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{
        Cipher, PasswordEncoding, big_order_64_bits_mod3, latin1_to_utf8, pad_password,
        recover_user_password, revision6_hash, revision6_hash_counted, try_password,
        utf8_to_latin1,
    };
    use crate::test_fixtures::{self, unhex};

    // ISO 32000 Algorithm 2 step a: the tail comes from the front of the pad,
    // not from the pad position it sits at.
    #[test]
    fn padding_fills_from_the_front_of_the_pad() {
        assert_eq!(pad_password(b""), super::PAD);
        let padded = pad_password(b"abc");
        assert_eq!(padded.get(..3), Some(&b"abc"[..]));
        assert_eq!(padded.get(3..), super::PAD.get(..29));
    }

    // A password of 32 bytes or more is truncated with no padding, so its
    // length is never encoded anywhere.
    #[test]
    fn a_long_password_is_truncated_to_thirty_two_bytes() {
        let long = [b'z'; 40];
        assert_eq!(pad_password(&long), [b'z'; 32]);
        assert_eq!(pad_password(&long), pad_password(&long[..32]));
    }

    // The loop runs at least 64 rounds and, since the stop test compares
    // `round - 32` against a single byte, at most 32 + 255 = 287.
    #[test]
    fn the_hardened_hash_runs_between_sixty_four_and_two_hundred_eighty_seven_rounds() {
        for seed in 0..6u8 {
            let salt = [seed; 8];
            let vector = [seed.wrapping_mul(3); 48];
            for vec in [None, Some(&vector)] {
                let (_, rounds) = revision6_hash_counted(b"password", salt, vec);
                assert!(
                    (64..=287).contains(&rounds),
                    "{rounds} rounds for seed {seed}"
                );
            }
        }
    }

    // The vector is part of the hash, so an owner check and a user check with
    // the same password and salt land on different digests.
    #[test]
    fn the_hardened_hash_depends_on_every_input() {
        let salt = [1u8; 8];
        let vector = [2u8; 48];
        let plain = revision6_hash(b"pw", salt, None);
        assert_ne!(plain, revision6_hash(b"pw", salt, Some(&vector)));
        assert_ne!(plain, revision6_hash(b"pX", salt, None));
        assert_ne!(plain, revision6_hash(b"pw", [2u8; 8], None));
        // Deterministic: the same inputs always give the same digest.
        assert_eq!(plain, revision6_hash(b"pw", salt, None));
    }

    // An empty password is legal and must not divide by zero building the
    // sixty-four repetitions.
    #[test]
    fn the_hardened_hash_accepts_an_empty_password() {
        let (hash, rounds) = revision6_hash_counted(b"", [0u8; 8], None);
        assert_ne!(hash, [0u8; 32]);
        assert!((64..=287).contains(&rounds));
    }

    #[test]
    fn mod3_fold_agrees_with_the_byte_sum() {
        for seed in 0..64u8 {
            let data: Vec<u8> = (0..16u8)
                .map(|i| i.wrapping_mul(seed).wrapping_add(i))
                .collect();
            let sum: u32 = data.iter().map(|&b| u32::from(b)).sum();
            assert_eq!(
                big_order_64_bits_mod3(&data),
                u64::from(sum % 3),
                "data {data:?}"
            );
        }
    }

    #[test]
    fn mod3_fold_reads_only_the_first_sixteen_bytes() {
        let mut data = vec![0u8; 32];
        assert_eq!(big_order_64_bits_mod3(&data), 0);
        // A change past byte 16 cannot move the result.
        if let Some(byte) = data.get_mut(20) {
            *byte = 1;
        }
        assert_eq!(big_order_64_bits_mod3(&data), 0);
        if let Some(byte) = data.get_mut(3) {
            *byte = 1;
        }
        assert_eq!(big_order_64_bits_mod3(&data), 1);
    }

    #[test]
    fn latin1_widening_is_a_scalar_per_byte() {
        assert_eq!(latin1_to_utf8(b"\xe2ge"), b"\xc3\xa2ge");
        assert_eq!(latin1_to_utf8(b"h\xf4tel"), b"h\xc3\xb4tel");
        assert_eq!(latin1_to_utf8(b"ascii"), b"ascii");
        assert!(latin1_to_utf8(b"").is_empty());
    }

    #[test]
    fn utf8_narrowing_keeps_the_low_byte() {
        assert_eq!(utf8_to_latin1(b"\xc3\xa2ge"), b"\xe2ge");
        assert_eq!(utf8_to_latin1(b"h\xc3\xb4tel"), b"h\xf4tel");
        assert_eq!(utf8_to_latin1(b"ascii"), b"ascii");
        // U+2AE2 narrows to its low byte, not to a substitution character.
        assert_eq!(utf8_to_latin1("\u{2ae2}".as_bytes()), b"\xe2");
    }

    // PDFium's decoder drops what it cannot use instead of substituting
    // U+FFFD; a lone continuation byte and a truncated sequence both vanish.
    #[test]
    fn utf8_narrowing_drops_invalid_bytes_silently() {
        assert_eq!(utf8_to_latin1(b"a\x80b"), b"ab");
        assert_eq!(utf8_to_latin1(b"a\xc3"), b"a");
        assert_eq!(utf8_to_latin1(b"\xf8\xff"), b"");
        assert_eq!(utf8_to_latin1(b"a\xc3\xa2"), b"a\xe2");
    }

    #[test]
    fn conversions_round_trip_on_latin1_text() {
        for text in [&b"\xe2ge"[..], b"h\xf4tel", b"", b"plain"] {
            assert_eq!(utf8_to_latin1(&latin1_to_utf8(text)), text);
        }
    }

    // T5 — encrypted_hello_world_r2.pdf: /V 1 forces a five-byte key
    // regardless of /Length, and both password spellings unlock it.
    #[test]
    fn revision_2_fixture() {
        let p = test_fixtures::r2();
        let id = unhex("2b778de1bcef1733b35e680882812409");
        assert_eq!((p.cipher, p.key_len), (Cipher::Rc4, 5));

        for owner_password in [&b"\xe2ge"[..], b"\xc3\xa2ge"] {
            let unlocked = try_password(&p, owner_password, true, &id)
                .unwrap_or_else(|| panic!("owner {owner_password:?}"));
            assert_eq!(unlocked.key.len(), 5);
        }
        for user_password in [&b"h\xf4tel"[..], b"h\xc3\xb4tel"] {
            assert!(
                try_password(&p, user_password, false, &id).is_some(),
                "user {user_password:?}"
            );
        }
        assert!(try_password(&p, b"tiger", true, &id).is_none());
        assert!(try_password(&p, b"tiger", false, &id).is_none());
    }

    // The encoding fallback direction flips at revision 5: below it, a UTF-8
    // password is narrowed to Latin-1.
    #[test]
    fn revision_2_records_the_encoding_that_worked() {
        let p = test_fixtures::r2();
        let id = unhex("2b778de1bcef1733b35e680882812409");
        let latin1 = try_password(&p, b"\xe2ge", true, &id).expect("latin-1 owner");
        assert_eq!(latin1.encoding, PasswordEncoding::AsGiven);
        let utf8 = try_password(&p, b"\xc3\xa2ge", true, &id).expect("utf-8 owner");
        assert_eq!(utf8.encoding, PasswordEncoding::Utf8ToLatin1);
        // Both spellings arrive at the same file key.
        assert_eq!(latin1.key.bytes(), utf8.key.bytes());
    }

    // T6 — encrypted_hello_world_r3.pdf: a 16-byte key through the fifty-round
    // strengthening loop, and a /U whose trailing sixteen bytes are zero,
    // which pins that only /U[0..16] is compared.
    #[test]
    fn revision_3_fixture() {
        let p = test_fixtures::r3();
        let id = unhex("9b744068bb5efbe920baaba6da63c2bf");
        assert_eq!((p.cipher, p.key_len), (Cipher::Rc4, 16));
        assert_eq!(p.u.get(16..), Some(&[0u8; 16][..]));

        for owner_password in [&b"\xe2ge"[..], b"\xc3\xa2ge"] {
            assert!(
                try_password(&p, owner_password, true, &id).is_some(),
                "owner {owner_password:?}"
            );
        }
        for user_password in [&b"h\xf4tel"[..], b"h\xc3\xb4tel"] {
            let unlocked = try_password(&p, user_password, false, &id)
                .unwrap_or_else(|| panic!("user {user_password:?}"));
            assert_eq!(unlocked.key.len(), 16);
        }
        assert!(try_password(&p, b"tiger", false, &id).is_none());
    }

    // T11 — a truncated /O yields an empty recovered password rather than an
    // out-of-bounds read (crbug.com/42270437).
    #[test]
    fn short_owner_entry_recovers_nothing() {
        for len in [0usize, 1, 16, 31] {
            let mut p = test_fixtures::r3();
            p.o = vec![0xAB; len].into();
            assert!(
                recover_user_password(&p, b"a").is_empty(),
                "/O of {len} bytes"
            );
            let id = unhex("9b744068bb5efbe920baaba6da63c2bf");
            assert!(try_password(&p, b"a", true, &id).is_none());
        }
    }

    // T16 — with no /ID the contribution is skipped entirely in both the key
    // derivation and the Algorithm 5 comparison hash; the fixture's passwords
    // then no longer unlock it, which is exactly what proves the id took part.
    #[test]
    fn a_missing_file_id_changes_the_derived_key() {
        let p = test_fixtures::r3();
        let id = unhex("9b744068bb5efbe920baaba6da63c2bf");
        assert!(try_password(&p, b"h\xf4tel", false, &id).is_some());
        assert!(try_password(&p, b"h\xf4tel", false, &[]).is_none());
    }
}
