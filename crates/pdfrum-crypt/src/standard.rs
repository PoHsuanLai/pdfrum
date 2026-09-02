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
    ///
    /// This is the **stream** class's cipher (`/StmF`). See [`string_cipher`].
    ///
    /// [`string_cipher`]: EncryptParams::string_cipher
    pub cipher: Cipher,
    /// The cipher `/EFF` resolves to when it differs from [`Self::cipher`]
    /// (ISO 32000-1 §7.6.5 table 20), and `None` when the embedded class uses
    /// the stream cipher — which table 20 makes the default.
    pub embedded_cipher: Option<Cipher>,
    /// `[oracle-bug]` The cipher the **string** class (`/StrF`) resolves to,
    /// independently of `/StmF`.
    ///
    /// §7.6.5 defines `/StmF` and `/StrF` as two independent entries, each
    /// defaulting to `Identity`, and says nothing forbidding them from
    /// differing. `cpdf_security_handler.cpp:305` and `:325` instead
    /// `return false` on a raw **name** inequality, so `/StmF /StdCF /StrF
    /// /StdCF2` is refused even when the two `/CF` entries are identical, and
    /// because the comparison runs *before* the default is applied, a V≥4
    /// document with **neither** entry present also fails to load. pdf.js
    /// applies the defaults and consults the two independently, with no
    /// equality check (`crypto.js:1116-1120`).
    ///
    /// Only the two values §7.6.5 makes observable at this seam are carried:
    /// a class is either the file's cipher or `Identity`. A document naming
    /// two *different non-Identity* filters would need two keys, which no
    /// corpus file does and which this record deliberately does not model —
    /// such a file resolves both classes to the stream filter's cipher.
    pub string_cipher: Cipher,
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
/// [`Error::MissingCryptFilter`] when a named filter is not in `/CF`, and
/// [`Error::MalformedEncryptDict`] or [`Error::CipherKeyLength`] when the key
/// length does not resolve. `[oracle-bug]` naming *different* filters in
/// `/StmF` and `/StrF` is **not** an error — see
/// [`EncryptParams::string_cipher`].
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

    let (cipher, string_cipher, key_len) = resolve_cipher(dict, version, r)?;
    let embedded_cipher = embedded_cipher(dict, version, cipher, r)?;

    Ok(EncryptParams {
        version,
        embedded_cipher,
        revision,
        permissions,
        cipher,
        string_cipher,
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

/// Resolve `/V`, `/Length`, `/CF` and `/CFM` into the two class ciphers and a
/// key length.
///
/// The version 4 branch carries two quirks that keep real files opening: a
/// `/Length` under 40 is read as *bytes* and multiplied by 8 (so a file
/// writing `/Length 16` for a 128-bit key works), and a `/CFM` PDFium does
/// not recognise leaves the cipher as RC4 rather than failing.
///
/// `[oracle-bug]` The stream and string classes are resolved **independently**
/// and each defaults to `/Identity` — see [`EncryptParams::string_cipher`].
fn resolve_cipher(
    dict: &Dict,
    version: i64,
    r: &impl Resolve,
) -> Result<(Cipher, Cipher, usize), Error> {
    let (cipher, string_cipher, key_bits) = if version >= 4 {
        let (stream_name, string_name) = crypt_filter_names(dict, r);
        let stream_identity = is_identity(&stream_name);
        let string_identity = is_identity(&string_name);
        if stream_identity && string_identity {
            return Ok((Cipher::None, Cipher::None, 0));
        }
        let filters = dict.dict(names::CF, r).ok_or(Error::MalformedEncryptDict(
            "/CF is missing or not a dictionary",
        ))?;
        // The non-Identity class names the filter that supplies the cipher and
        // key length; when both do and they differ, the stream's wins, which
        // is the case this record deliberately does not model.
        let name = if stream_identity {
            string_name
        } else {
            stream_name
        };
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
        let resolved = if method == b"AESV2" || method == b"AESV3" {
            Cipher::Aes
        } else {
            Cipher::Rc4
        };
        let stream = if stream_identity {
            Cipher::None
        } else {
            resolved
        };
        let string = if string_identity {
            Cipher::None
        } else {
            resolved
        };
        (stream, string, bits)
    } else if version > 1 {
        let bits = dict.int(names::LENGTH, r).unwrap_or(40);
        (Cipher::Rc4, Cipher::Rc4, bits)
    } else {
        // Version 1 is 40-bit RC4 by definition; its /Length is ignored.
        (Cipher::Rc4, Cipher::Rc4, 40)
    };

    let key_len = usize::try_from(key_bits / 8)
        .map_err(|_| Error::MalformedEncryptDict("/Length is negative"))?;
    // The key length is a property of the filter, so it is checked against
    // whichever class is not Identity.
    let effective = if cipher == Cipher::None {
        string_cipher
    } else {
        cipher
    };
    if key_len > 32 || !effective.accepts_key_len(key_len) {
        return Err(Error::CipherKeyLength {
            cipher: effective.label(),
            len: key_len,
        });
    }
    Ok((cipher, string_cipher, key_len))
}

/// The cipher an embedded-file stream is decrypted with — `/EFF`'s filter
/// (ISO 32000-1 §7.6.5 table 20), or `None` when `/EFF` is absent or names
/// the same filter the streams use.
///
/// `None` is not "no encryption": it means the embedded class needs no
/// override, and [`crate::CryptClass::Embedded`] falls back to the stream
/// cipher, which is table 20's own default for a missing `/EFF`.
///
/// Only the *cipher* can differ. §7.6.5 gives every `/CF` entry the one file
/// encryption key and a `/CFM` of its own, so a differing `/EFF` changes
/// which algorithm decrypts an embedded file, never which key.
//
// [oracle-bug] `grep '"EFF"' core/ fpdfsdk/` over the oracle returns **zero
// hits**: `/EFF` is read nowhere in PDFium. `CPDF_SecurityHandler::LoadDict`
// (cpdf_security_handler.cpp:303-311) takes one filter name and builds one
// `CPDF_CryptoHandler`, so an embedded file stream is decrypted with the
// stream filter whatever `/EFF` says — and a document whose `/EFF` names an
// AES filter while `/StmF` names an RC4 one silently produces garbage for
// every attachment. §7.6.5 table 20 defines `/EFF` as a distinct default for
// embedded file streams, independent of `/StmF`. pdf.js carries it
// separately: `crypto.js:1120` reads it with the `/StmF` default
// (`eff = dict.get("EFF") || stmf`), consults it at `:1206` and hands it to
// the cipher transform as `embeddedFilterName` at `:1336`.
fn embedded_cipher(
    dict: &Dict,
    version: i64,
    stream_cipher: Cipher,
    r: &impl Resolve,
) -> Result<Option<Cipher>, Error> {
    // Below version 4 there are no crypt filters at all, so there is nothing
    // for `/EFF` to name.
    if version < 4 {
        return Ok(None);
    }
    let Some(name) = dict.byte_string(names::EFF, r) else {
        return Ok(None);
    };
    // Table 20's default for an absent `/EFF` is `/StmF`, so naming `/StmF`'s
    // own filter is the default written out and needs no override.
    if name == dict.byte_string(names::STM_F, r).unwrap_or_default() {
        return Ok(None);
    }
    if name == names::IDENTITY.as_bytes() {
        return Ok(Some(Cipher::None));
    }
    let filters = dict.dict(names::CF, r).ok_or(Error::MalformedEncryptDict(
        "/CF is missing or not a dictionary",
    ))?;
    let Some(filter) = filters.dict(&Name::from(name.as_slice()), r) else {
        // An `/EFF` naming a filter `/CF` does not have is damage, not a
        // reason to refuse the document: the streams still decrypt. Fall back
        // to the stream cipher, which is what the absent-key default gives.
        return Ok(None);
    };
    let method = filter.byte_string(names::CFM, r).unwrap_or_default();
    let cipher = if method == b"AESV2" || method == b"AESV3" {
        Cipher::Aes
    } else {
        Cipher::Rc4
    };
    Ok((cipher != stream_cipher).then_some(cipher))
}

/// The crypt filter both `/StmF` and `/StrF` must name.
/// `/StmF` and `/StrF`, each defaulting to `/Identity`.
///
/// `[oracle-bug]` §7.6.5 table 20 defines both as independent entries whose
/// default is `Identity`. `cpdf_security_handler.cpp:305` and `:325` instead
/// compare the two raw names and `return false` on inequality — and because
/// the comparison runs *before* any default is applied, an absent entry reads
/// as the empty name, which is neither `Identity` nor a key in `/CF`, so a
/// V≥4 document with **neither** entry present is refused as well. pdf.js
/// applies the defaults and consults the two independently (`crypto.js:1116-1120`).
fn crypt_filter_names(dict: &Dict, r: &impl Resolve) -> (Name, Name) {
    let named = |key| match dict.byte_string(key, r) {
        Some(bytes) if !bytes.is_empty() => Name::from(bytes.as_slice()),
        _ => names::IDENTITY.clone(),
    };
    (named(names::STM_F), named(names::STR_F))
}

/// Whether a resolved class filter is the `Identity` filter.
fn is_identity(name: &Name) -> bool {
    name.as_bytes() == names::IDENTITY.as_bytes()
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

/// ISO 32000-2 §7.6.4.3.3 (Algorithm 2.A) step (a): the byte length a
/// revision-6 password is truncated to, **after** UTF-8 encoding.
const R6_PASSWORD_BYTES: usize = 127;

/// Which spelling of a password unlocked a document.
///
/// The authentication path tries a document's password in several spellings
/// (see `try_password`) and this records the one that worked. This crate
/// never *sets* a password — SPEC.md §3 keeps `/Encrypt` construction out of
/// scope, and the save path re-uses the file key the original password already
/// produced — so the value is reportable state rather than an input to
/// anything here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PasswordEncoding {
    /// The password bytes as supplied unlocked the document.
    #[default]
    AsGiven,
    /// The bytes were valid UTF-8, `SASLprep` (RFC 4013) changed them, and the
    /// prepared form — re-encoded as UTF-8 and cut to 127 bytes — unlocked the
    /// document. ISO 32000-2 §7.6.4.3.3's own preparation, revision 6 only.
    SaslPrepped,
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

/// Try `password` in the given role, in each spelling the format admits, and
/// return the first that authenticates.
///
/// The candidates, in order:
///
/// 1. **The specification's preparation**, revision 6 only: `SASLprep`
///    (RFC 4013), UTF-8, truncated to 127 *bytes* — ISO 32000-2 §7.6.4.3.3
///    Algorithm 2.A step (a). Skipped when the password is not valid UTF-8
///    (nothing to prepare), when `SASLprep` refuses it (a prohibited character
///    or a bidirectional violation), and when preparation is the identity, in
///    which case candidate 2 already covers it.
///
///    Revision 5 is deliberately **not** prepared. Algorithm 2.A is what
///    revision 6 is; the Adobe extension level 3 algorithm that revision 5
///    implements has no preparation step, and pdf.js draws the line at the
///    same place — `crypto.js:1142` guards the `saslPrep` call with
///    `revision === 6`, and its `algorithm === 5` branch two lines later
///    encodes UTF-8 with no preparation at all.
///
/// 2. **The bytes as given.** A file whose producer skipped the preparation
///    hashed the raw bytes, so the raw bytes must still be tried. This is
///    pdf.js's tolerance, at `crypto.js:1178-1180`, where a prepped password
///    that differs from the raw one yields *two* candidates rather than one.
///
/// 3. **PDFium's transcode**, `[oracle-bug]`. `cpdf_security_handler.cpp:425-455`
///    performs none of the specification's three steps; instead it retries a
///    non-ASCII password with a Latin-1→UTF-8 transcode (revision 5 and up) or
///    a UTF-8→Latin-1 one (revisions 2 to 4). That is not the specification and
///    it is not `PDFDocEncoding` either — the three disagree across `0x80..0x9F`
///    — but it rescues a real class of embedder mis-encoding (a host that
///    handed the library bytes in the wrong one of two encodings), no
///    independent implementation contradicts it, and by running last it can
///    only turn a failure into a success. Kept as a tolerance under PLAN.md's
///    oracle-bug rule, which obliges the correct behaviour *first*; pdf.js has
///    no equivalent (`crypto.js:1136-1152` transcodes nothing).
///
/// A pure-ASCII password is a fixed point of every one of these conversions,
/// so all three candidates collapse to one attempt — the early returns make
/// that observable as the absence of extra work, which is what the
/// ASCII-password fixtures pin.
pub(crate) fn try_password(
    p: &EncryptParams,
    password: &[u8],
    owner: bool,
    file_id: &[u8],
) -> Option<Unlocked> {
    // (1) The specification's preparation.
    if let Some(prepped) = r6_prepared(p.revision, password)
        && prepped.as_slice() != password.get(..R6_PASSWORD_BYTES).unwrap_or(password)
        && let Some(key) = check_password(p, &prepped, owner, file_id)
    {
        return Some(Unlocked {
            key,
            encoding: PasswordEncoding::SaslPrepped,
        });
    }

    // (2) The bytes as given.
    if let Some(key) = check_password(p, password, owner, file_id) {
        return Some(Unlocked {
            key,
            encoding: PasswordEncoding::AsGiven,
        });
    }

    // (3) [oracle-bug] PDFium's transcode retry, kept last as a tolerance:
    // `cpdf_security_handler.cpp:425-455` performs none of ISO 32000-2
    // §7.6.4.3.3's three preparation steps and substitutes this instead;
    // pdf.js transcodes nothing (`crypto.js:1136-1152`). Running after the
    // two correct candidates, it can only turn a failure into a success.
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

/// ISO 32000-2 §7.6.4.3.3 Algorithm 2.A step (a) applied to `password`, or
/// `None` when the revision is not 6, the bytes are not UTF-8, or `SASLprep`
/// refuses them.
///
/// The truncation cuts the **UTF-8 byte string**, not the character sequence,
/// which is what the specification says and what pdf.js does
/// (`crypto.js:896-897`, `Math.min(127, password.length)` over the already
/// encoded byte array). A multi-byte character straddling byte 127 is
/// therefore cut mid-sequence, leaving bytes that are not valid UTF-8 — and
/// that is correct, because the hash is over bytes and both implementations
/// hash the same ones.
///
/// Cutting here as well as in [`check_password`] is not redundant: it is what
/// makes the `prepped != password` test below compare the bytes that will
/// actually be hashed, so a preparation whose only effect lies past byte 127
/// does not buy a second identical attempt.
fn r6_prepared(revision: i64, password: &[u8]) -> Option<Vec<u8>> {
    if revision != 6 {
        return None;
    }
    let text = core::str::from_utf8(password).ok()?;
    let prepared = crate::saslprep::saslprep(text)?;
    let mut bytes = prepared.into_bytes();
    bytes.truncate(R6_PASSWORD_BYTES);
    Some(bytes)
}

/// One password attempt with no encoding fallback.
///
/// Below revision 5 the user check runs twice, once honoring
/// `/EncryptMetadata` and once ignoring it, for files that wrote
/// `/EncryptMetadata false` but computed `/U` without the tag. The key kept is
/// the one the *successful* attempt derived.
///
/// At revision 5 and up the password is first cut to 127 bytes — ISO 32000-2
/// §7.6.4.3.3 Algorithm 2.A step (a). The cut belongs *here* rather than to
/// one candidate because it is a property of the AES-256 hash, not of the
/// preparation: pdf.js applies it inside the key derivation, so every
/// candidate it tries is cut (`crypto.js:896-897`). PDFium applies it nowhere,
/// and hashes a 200-byte password whole — `[oracle-bug]`,
/// `cpdf_security_handler.cpp:425-455`, superseding SPEC.md §3's original
/// "passwords are NOT capped at ISO's 127 bytes" ruling.
fn check_password(
    p: &EncryptParams,
    password: &[u8],
    owner: bool,
    file_id: &[u8],
) -> Option<SmallKey> {
    if p.revision >= 5 {
        let capped = password.get(..R6_PASSWORD_BYTES).unwrap_or(password);
        return check_password_aes256(p, capped, owner).map(SmallKey::from_full);
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

    // ---- A31: ISO 32000-2 §7.6.4.3.3 password preparation ----

    // The end-to-end proof that the preparation is *required*, not merely
    // permitted: pdf.js's `saslprep-r6.pdf`, whose /U was computed from the
    // prepared spelling of `S\u{00AA}SL\u{00AD}prep`. Neither the raw bytes
    // nor either Latin-1↔UTF-8 transcode opens it, so this file fails on the
    // old behaviour and is what candidate (1) exists for.
    #[test]
    fn the_pdfjs_saslprep_fixture_needs_the_preparation() {
        let dict = test_fixtures::saslprep_r6_dict();
        let p = super::parse_encrypt_dict(&dict, &pdfrum_object::NoResolve)
            .unwrap_or_else(|e| panic!("{e:?}"));

        let raw = "S\u{00AA}SL\u{00AD}prep".as_bytes();
        let unlocked =
            try_password(&p, raw, false, &[]).unwrap_or_else(|| panic!("the prepared candidate"));
        assert_eq!(unlocked.encoding, PasswordEncoding::SaslPrepped);

        // The prepared spelling given directly opens it too, and reports
        // itself as the bytes as given — nothing was left to prepare.
        let prepped = try_password(&p, b"SaSLprep", false, &[])
            .unwrap_or_else(|| panic!("the prepared spelling"));
        assert_eq!(prepped.encoding, PasswordEncoding::AsGiven);
        assert_eq!(unlocked.key.bytes(), prepped.key.bytes());

        // And the wrong password still fails, so the ladder is not a
        // universal acceptor.
        assert!(try_password(&p, b"SASLprep", false, &[]).is_none());
    }

    // Normalisation is two-directional: either spelling of an accented
    // password opens a file keyed on the other, because NFKC sends both to
    // the same string.
    #[test]
    fn a_decomposed_and_a_composed_password_prepare_alike() {
        assert_eq!(
            super::r6_prepared(6, "cafe\u{0301}".as_bytes()),
            super::r6_prepared(6, "caf\u{00E9}".as_bytes()),
        );
        assert_eq!(
            super::r6_prepared(6, "cafe\u{0301}".as_bytes()).as_deref(),
            Some("caf\u{00E9}".as_bytes()),
        );
    }

    // Revision 5 is not prepared: Algorithm 2.A is revision 6's, and pdf.js
    // draws the same line at `crypto.js:1142`.
    #[test]
    fn only_revision_six_is_prepared() {
        let decomposed = "cafe\u{0301}".as_bytes();
        assert!(super::r6_prepared(6, decomposed).is_some());
        for revision in [2i64, 3, 4, 5] {
            assert_eq!(
                super::r6_prepared(revision, decomposed),
                None,
                "R{revision}"
            );
        }
    }

    // The truncation cuts the UTF-8 *bytes*, so a multi-byte character
    // straddling byte 127 is cut mid-sequence — which is what pdf.js does at
    // `crypto.js:896-897`, where the cut is applied to the encoded array.
    #[test]
    fn the_cut_is_at_byte_one_hundred_twenty_seven_not_at_a_character() {
        // 126 ASCII bytes then a two-byte character: byte 127 is that
        // character's lead byte, and the trail byte is dropped.
        let mut password = "a".repeat(126);
        password.push('\u{00E9}');
        let prepared =
            super::r6_prepared(6, password.as_bytes()).unwrap_or_else(|| panic!("preparable"));
        assert_eq!(prepared.len(), 127);
        assert_eq!(prepared.get(126), Some(&0xC3));
        assert!(core::str::from_utf8(&prepared).is_err());

        // And an all-ASCII password of 130 bytes keeps its first 127.
        let long = "z".repeat(130);
        let cut = super::r6_prepared(6, long.as_bytes()).unwrap_or_else(|| panic!("preparable"));
        assert_eq!(cut, "z".repeat(127).into_bytes());
    }

    // A password 130 bytes long opens a file keyed on its first 127 — the
    // truncation applies to every candidate, because it lives in the
    // revision-5-and-up check rather than in the preparation. This is what
    // supersedes SPEC.md §3's original "not capped at ISO's 127 bytes".
    #[test]
    fn a_password_past_one_hundred_twenty_seven_bytes_is_cut_for_every_candidate() {
        let dict = test_fixtures::saslprep_r6_dict();
        let p = super::parse_encrypt_dict(&dict, &pdfrum_object::NoResolve)
            .unwrap_or_else(|e| panic!("{e:?}"));
        let mut overlong = b"SaSLprep".to_vec();
        overlong.resize(200, b'!');
        // The first 127 bytes are not the password, so this must still fail —
        // the point of the assertion is that it is *the cut bytes* that are
        // hashed, which the next assertion pins from the other side.
        assert!(try_password(&p, &overlong, false, &[]).is_none());

        let mut padded = b"SaSLprep".to_vec();
        padded.resize(127, b'!');
        let short = try_password(&p, &padded, false, &[]);
        let mut long = padded.clone();
        long.resize(130, b'?');
        // Two byte strings agreeing on their first 127 bytes authenticate
        // identically.
        assert_eq!(
            short.is_some(),
            try_password(&p, &long, false, &[]).is_some()
        );
    }

    // A password SASLprep refuses skips candidate (1) and falls through to the
    // raw bytes, which is what keeps a file whose producer skipped the
    // preparation opening.
    #[test]
    fn a_prohibited_password_falls_through_to_the_raw_bytes() {
        // U+202A is table C.8; the preparation therefore yields nothing.
        assert_eq!(super::r6_prepared(6, "a\u{202A}b".as_bytes()), None);
        // Invalid UTF-8 has nothing to prepare either.
        assert_eq!(super::r6_prepared(6, b"\xe2ge"), None);

        // And the revision-6 fixture, whose passwords are the raw Latin-1
        // bytes, still opens through candidates (2) and (3).
        let dict = test_fixtures::r6_dict();
        let p = super::parse_encrypt_dict(&dict, &pdfrum_object::NoResolve)
            .unwrap_or_else(|e| panic!("{e:?}"));
        let raw = try_password(&p, b"h\xf4tel", false, &[])
            .unwrap_or_else(|| panic!("the transcode candidate"));
        assert_eq!(raw.encoding, PasswordEncoding::Latin1ToUtf8);
        let utf8 = try_password(&p, "h\u{00F4}tel".as_bytes(), false, &[])
            .unwrap_or_else(|| panic!("the bytes as given"));
        assert_eq!(utf8.encoding, PasswordEncoding::AsGiven);
    }

    // An ASCII password is a fixed point of every conversion, so the ladder
    // collapses to one attempt and reports the identity.
    #[test]
    fn an_ascii_password_reports_the_bytes_as_given() {
        let dict = test_fixtures::r6_dict();
        let p = super::parse_encrypt_dict(&dict, &pdfrum_object::NoResolve)
            .unwrap_or_else(|e| panic!("{e:?}"));
        assert!(try_password(&p, b"tiger", false, &[]).is_none());
        assert_eq!(
            super::r6_prepared(6, b"tiger").as_deref(),
            Some(&b"tiger"[..])
        );
    }
}
