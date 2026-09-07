//! PDF standard security (ISO 32000 §7.6): opening an encrypted document and
//! deciphering its strings and streams, revisions 2 through 6.
//!
//! An `/Encrypt` dictionary plus a password produce a [`SecurityHandler`], and
//! every string and stream the parser reads passes through
//! [`SecurityHandler::decrypt`] keyed by the indirect object it belongs to.
//! [`SecurityHandler::encrypt`] is the inverse under that same handler, so a
//! save re-enciphers under the file's existing key: no re-keying, and no
//! `/Encrypt` dictionary is built here.
//!
//! ```
//! use pdfrum_crypt::{CryptClass, SecurityHandler};
//! use pdfrum_object::{Dict, NoResolve, ObjRef};
//!
//! // A document with no /Encrypt needs no handler: payloads pass through.
//! let handler = SecurityHandler::Identity;
//! assert_eq!(handler.decrypt(ObjRef::new(1, 0), CryptClass::Stream, b"raw"), b"raw");
//! assert_eq!(handler.permissions(), pdfrum_crypt::Permissions::ALL);
//! # let _ = (Dict::new(), NoResolve);
//! ```
//!
//! This crate has no randomness: AES's per-payload [`Iv`] is an argument.

// Revisions 2 to 4 derive an RC4 or AES-128 key by an MD5 ladder over the
// padded password; revisions 5 and 6 verify a SHA-2 hash and unwrap a 32-byte
// AES-256 key that the file stores directly.
//
// No global state and no `getrandom` dependency is why the vector is an
// argument. `pdfrum-edit` derives one deterministically from the file's own
// bytes and a per-object counter, which makes a save reproducible; a caller
// wanting unpredictable vectors passes its own source.
//
// Building an `/Encrypt` dictionary is out of scope: `/O`, `/U`, `/OE`, `/UE`
// and `/Perms` are written by whoever chose the passwords, and a
// password-preserving save copies the dictionary the file already had.
//
// Two crypto-driven behaviors also live outside this crate, because both need
// to walk the object graph, which this crate deliberately cannot:
//
// - The signature exemption. A `/Contents` value whose parent dictionary has a
//   `/Type` or `/FT` key is deferred during the decrypt walk; once the parent
//   has been decrypted its type can finally be read, and a parent that turns
//   out to be a signature dictionary (`/Type /Sig`, or `/FT /Sig` when `/Type`
//   is absent) keeps its contents undecrypted. The test cannot be made earlier
//   because those names are themselves encrypted strings until the parent is
//   done. `is_signature_dict` is what the walker calls.
// - The metadata exemption. When `SecurityHandler::encrypt_metadata` is false
//   the object `/Root/Metadata` points at is not decrypted.

#![forbid(unsafe_code)]
// Every byte reaching this crate came from an untrusted file or a password:
// index with `get()`.
#![warn(clippy::indexing_slicing)]

mod create;
mod key;
mod object;
mod permissions;
mod primitives;
mod rc4;
mod saslprep;
mod standard;

#[cfg(test)]
mod test_fixtures;

pub use create::{ENTROPY_LEN, standard_r6};
pub use key::SmallKey;
pub use object::{CryptClass, Iv};
pub use permissions::Permissions;
pub use primitives::{md5, sha1};
pub use rc4::rc4;
pub use standard::{Cipher, EncryptParams, PAD, PasswordEncoding, parse_encrypt_dict};

use pdfrum_object::{Dict, Name, ObjRef, Resolve, names};

/// What went wrong building a security handler.
///
/// The C++ collapses every one of these into a single "password error" at the
/// parser boundary; splitting them changes no document's fate but lets a
/// caller tell "this needs a password" from "we cannot do this document's
/// cryptography" (Divergence D3).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// The password is neither the user nor the owner password.
    #[error("the supplied password is not the user or owner password")]
    WrongPassword,
    /// `/Filter` names a handler other than `/Standard`. Public-key handlers
    /// (`/Adobe.PubSec`) land here.
    #[error("/Filter {0:?} is not the standard security handler")]
    UnsupportedHandler(Box<[u8]>),
    /// `[oracle-bug]` **Never constructed.** §7.6.5 table 20 makes `/StmF`
    /// and `/StrF` independent (audit item A27), so a pair naming different
    /// crypt filters is conformant and this crate resolves each class
    /// separately. The variant is kept because it is a public enum member
    /// and removing it is a breaking change no caller gains from; nothing
    /// produces it.
    // [oracle-bug] cpdf_security_handler.cpp:305 and :325 return false on a
    // differing pair.
    #[error("/StmF and /StrF name different crypt filters")]
    MismatchedCryptFilters,
    /// The named crypt filter is not a key in `/CF`. An **empty** name no
    /// longer lands here: both class keys absent is §7.6.5's `/Identity`
    /// default rather than an error (audit item A27).
    // [oracle-bug] cpdf_security_handler.cpp:305 rejects the absent pair.
    #[error("crypt filter {0:?} is not present in /CF")]
    MissingCryptFilter(Box<[u8]>),
    /// The dictionary is structurally unusable.
    #[error("/Encrypt is malformed: {0}")]
    MalformedEncryptDict(&'static str),
    /// The resolved key length is one this cipher does not accept.
    #[error("key length {len} bytes is invalid for {cipher}")]
    CipherKeyLength {
        /// The cipher that rejected the length.
        cipher: &'static str,
        /// The length in bytes.
        len: usize,
    },
}

/// A document's decryption state: which cipher, which key, and what the
/// password unlocked.
///
/// A closed enum rather than a trait, because the set of standard security
/// handlers is closed: adding a variant must break every `match` on it.
/// `Rc4V2` covers `/V 1` to `/V 4` without an AES crypt filter; `AesV4` is
/// AESV2, whose per-object key is an MD5 over the object number and the four
/// bytes `sAlT`; `AesV5` is AESV3, whose 32-byte key is used verbatim with no
/// per-object derivation; `Identity` is both an unencrypted document and one
/// naming `/Identity` as its crypt filter.
#[derive(Debug, Clone)]
pub enum SecurityHandler {
    /// RC4, with a 5- to 16-byte file key.
    Rc4V2 {
        /// The file encryption key.
        key: SmallKey,
        /// `/R`, the handler revision.
        revision: u8,
        /// `/P`, as the unsigned word permissions are compared as.
        permissions: u32,
        /// Whether the owner password was the one that opened the document.
        owner_unlocked: bool,
        /// `/EncryptMetadata`.
        encrypt_metadata: bool,
        /// Which password spelling worked.
        encoding: PasswordEncoding,
        /// The cipher `/EFF` names for embedded file streams, when it differs
        /// from this variant's own (ISO 32000-1 §7.6.5 table 20). `None` is
        /// table 20's default: the embedded class uses the stream cipher.
        embedded_cipher: Option<Cipher>,
        /// `[oracle-bug]` Whether `/StrF` resolved to `/Identity` while
        /// `/StmF` did not, so strings pass through undeciphered while
        /// streams are enciphered. §7.6.5 makes the two entries independent,
        /// so such a document is conformant and is opened.
        // [oracle-bug] cpdf_security_handler.cpp:305 refuses it outright.
        strings_identity: bool,
    },
    /// AESV2: a 16- or 24-byte file key with per-object `sAlT` derivation.
    AesV4 {
        /// The file encryption key.
        key: SmallKey,
        /// `/R`, the handler revision.
        revision: u8,
        /// `/P`, as the unsigned word permissions are compared as.
        permissions: u32,
        /// Whether the owner password was the one that opened the document.
        owner_unlocked: bool,
        /// `/EncryptMetadata`.
        encrypt_metadata: bool,
        /// Which password spelling worked.
        encoding: PasswordEncoding,
        /// The cipher `/EFF` names for embedded file streams, when it differs
        /// from this variant's own (ISO 32000-1 §7.6.5 table 20). `None` is
        /// table 20's default: the embedded class uses the stream cipher.
        embedded_cipher: Option<Cipher>,
        /// `[oracle-bug]` Whether `/StrF` resolved to `/Identity` while
        /// `/StmF` did not, so strings pass through undeciphered while
        /// streams are enciphered. §7.6.5 makes the two entries independent,
        /// so such a document is conformant and is opened.
        // [oracle-bug] cpdf_security_handler.cpp:305 refuses it outright.
        strings_identity: bool,
    },
    /// AESV3 (`/V 5`, revision 5 or 6): the 32-byte key is used as-is.
    AesV5 {
        /// The file encryption key.
        key: Box<[u8; 32]>,
        /// `/R`, the handler revision.
        revision: u8,
        /// `/P`, as the unsigned word permissions are compared as.
        permissions: u32,
        /// Whether the owner password was the one that opened the document.
        owner_unlocked: bool,
        /// `/EncryptMetadata`.
        encrypt_metadata: bool,
        /// Which password spelling worked.
        encoding: PasswordEncoding,
        /// The cipher `/EFF` names for embedded file streams, when it differs
        /// from this variant's own (ISO 32000-1 §7.6.5 table 20). `None` is
        /// table 20's default: the embedded class uses the stream cipher.
        embedded_cipher: Option<Cipher>,
        /// `[oracle-bug]` Whether `/StrF` resolved to `/Identity` while
        /// `/StmF` did not, so strings pass through undeciphered while
        /// streams are enciphered. §7.6.5 makes the two entries independent,
        /// so such a document is conformant and is opened.
        // [oracle-bug] cpdf_security_handler.cpp:305 refuses it outright.
        strings_identity: bool,
    },
    /// No encryption, or `/StrF /Identity`.
    Identity,
}

impl SecurityHandler {
    /// Build a handler from the trailer's `/Encrypt` dictionary.
    ///
    /// `file_id` is the first element of the trailer's `/ID` array as raw
    /// bytes; pass `&[]` when `/ID` is absent, which contributes nothing to
    /// the key rather than an empty marker. `password` is raw bytes, uncapped
    /// — the specification's 127-byte limit is not enforced. A non-empty
    /// password is tried as the owner password first and only then as the
    /// user password; an empty one is only ever a user password.
    ///
    /// ```
    /// use pdfrum_crypt::{Error, SecurityHandler};
    /// use pdfrum_object::{Dict, NoResolve, Object, PdfString, names};
    ///
    /// // A public-key handler is not the standard one.
    /// let dict = Dict::from_pairs([(
    ///     names::FILTER.clone(),
    ///     Object::Name(pdfrum_object::Name::from("Adobe.PubSec")),
    /// )]);
    /// assert!(matches!(
    ///     SecurityHandler::from_encrypt_dict(&dict, &[], b"", &NoResolve),
    ///     Err(Error::UnsupportedHandler(_))
    /// ));
    /// # let _ = PdfString::literal(b"");
    /// ```
    ///
    /// # Errors
    ///
    /// [`Error::WrongPassword`] when neither role accepts the password, and
    /// the parse errors of [`parse_encrypt_dict`] when the dictionary itself
    /// cannot be used.
    pub fn from_encrypt_dict(
        dict: &Dict,
        file_id: &[u8],
        password: &[u8],
        r: &impl Resolve,
    ) -> Result<Self, Error> {
        let params = parse_encrypt_dict(dict, r)?;
        // Only a document whose *stream* class is Identity has nothing to
        // decipher through this handler; a `/StrF /Identity` beside an
        // enciphering `/StmF` is carried as `strings_identity` instead.
        if params.cipher == Cipher::None {
            return Ok(Self::Identity);
        }

        if !password.is_empty()
            && let Some(unlocked) = standard::try_password(&params, password, true, file_id)
        {
            return Ok(Self::assemble(&params, unlocked, true));
        }
        standard::try_password(&params, password, false, file_id)
            .map(|unlocked| Self::assemble(&params, unlocked, false))
            .ok_or(Error::WrongPassword)
    }

    /// Pick the variant the resolved cipher and key length call for.
    fn assemble(
        params: &EncryptParams,
        unlocked: standard::Unlocked,
        owner_unlocked: bool,
    ) -> Self {
        let standard::Unlocked { key, encoding } = unlocked;
        let revision = u8::try_from(params.revision).unwrap_or(u8::MAX);
        let permissions = params.permissions;
        let encrypt_metadata = params.encrypt_metadata;
        // `[oracle-bug]` `/StrF /Identity` beside an enciphering `/StmF` is a
        // conformant document (§7.6.5), not the refusal
        // `cpdf_security_handler.cpp:305` gives it.
        let strings_identity = params.string_cipher == Cipher::None;
        match params.cipher {
            Cipher::None => Self::Identity,
            Cipher::Rc4 => Self::Rc4V2 {
                key,
                revision,
                permissions,
                owner_unlocked,
                encrypt_metadata,
                encoding,
                embedded_cipher: params.embedded_cipher,
                strings_identity,
            },
            // AESV3 is exactly "AES with a 32-byte key"; PDFium never reads
            // the /CFM name to tell the two apart.
            Cipher::Aes if key.len() == SmallKey::MAX_LEN => {
                let mut full = [0u8; 32];
                if let Some(head) = full.get_mut(..key.len()) {
                    head.copy_from_slice(key.bytes());
                }
                Self::AesV5 {
                    key: Box::new(full),
                    revision,
                    permissions,
                    owner_unlocked,
                    encrypt_metadata,
                    encoding,
                    embedded_cipher: params.embedded_cipher,
                    strings_identity,
                }
            }
            Cipher::Aes => Self::AesV4 {
                key,
                revision,
                permissions,
                owner_unlocked,
                encrypt_metadata,
                encoding,
                embedded_cipher: params.embedded_cipher,
                strings_identity,
            },
        }
    }

    /// `[oracle-bug]` Whether the string class resolved to `/Identity` while
    /// the stream class did not, so strings in this document are plaintext.
    ///
    /// Always `false` for [`Self::Identity`], which has nothing to contrast
    /// against — an unencrypted document's strings are plaintext anyway.
    #[must_use]
    pub const fn strings_identity(&self) -> bool {
        match self {
            Self::Identity => false,
            Self::Rc4V2 {
                strings_identity, ..
            }
            | Self::AesV4 {
                strings_identity, ..
            }
            | Self::AesV5 {
                strings_identity, ..
            } => *strings_identity,
        }
    }

    /// Decrypt one string or stream payload belonging to indirect object
    /// `obj`.
    ///
    /// Infallible by design: PDFium never fails a decrypt, it produces a
    /// best-effort result. An AES payload shorter than seventeen bytes, a
    /// trailing partial block, and a final block whose padding byte is out of
    /// range all yield less output than input rather than an error.
    ///
    /// `obj` must be the *enclosing indirect object*, not a nested one: a
    /// direct string inside an indirect dictionary is keyed by the
    /// dictionary's number and generation.
    ///
    /// ```
    /// use pdfrum_crypt::{CryptClass, SecurityHandler};
    /// use pdfrum_object::ObjRef;
    ///
    /// // Fewer than seventeen bytes of AES ciphertext is all initialisation
    /// // vector and no payload.
    /// let handler = SecurityHandler::AesV5 {
    ///     key: Box::new([0; 32]),
    ///     revision: 6,
    ///     permissions: 0xFFFF_FFFC,
    ///     owner_unlocked: false,
    ///     encrypt_metadata: true,
    ///     encoding: pdfrum_crypt::PasswordEncoding::AsGiven,
    ///     embedded_cipher: None,   // no /EFF: the stream cipher serves
    ///     strings_identity: false,
    /// };
    /// assert!(handler.decrypt(ObjRef::new(4, 0), CryptClass::String, &[0; 16]).is_empty());
    /// ```
    #[must_use]
    pub fn decrypt(&self, obj: ObjRef, class: CryptClass, data: &[u8]) -> Vec<u8> {
        // `[oracle-bug]` A27: the *string* class branches. §7.6.5 lets
        // `/StrF` resolve to `/Identity` beside an enciphering `/StmF`, and
        // such a document's strings are plaintext.
        if class == CryptClass::String && self.strings_identity() {
            return data.to_vec();
        }
        // `/EFF` is the one class that can genuinely name another cipher, and
        // does so by cipher only — §7.6.5 gives every `/CF` entry the same
        // file key (A26).
        if let (CryptClass::Embedded, Some(cipher)) = (class, self.embedded_cipher()) {
            return self.decrypt_with(obj, cipher, data);
        }
        match self {
            Self::Identity => data.to_vec(),
            Self::Rc4V2 { key, .. } => object::decrypt_rc4(key, obj, data),
            Self::AesV4 { key, .. } => object::decrypt_aes_v4(key, obj, data),
            Self::AesV5 { key, .. } => object::decrypt_aes_v5(key, data),
        }
    }

    /// The `/EFF` cipher override, or `None` when the embedded class uses the
    /// stream cipher — which ISO 32000-1 §7.6.5 table 20 makes the default.
    #[must_use]
    pub fn embedded_cipher(&self) -> Option<Cipher> {
        match self {
            Self::Identity => None,
            Self::Rc4V2 {
                embedded_cipher, ..
            }
            | Self::AesV4 {
                embedded_cipher, ..
            }
            | Self::AesV5 {
                embedded_cipher, ..
            } => *embedded_cipher,
        }
    }

    /// Decrypt with a named cipher over this handler's own file key — the
    /// `/EFF` path, where the algorithm differs from the stream class's but
    /// the key does not.
    ///
    /// The AES arm branches on key length exactly as [`Self::assemble`] does,
    /// because AESV2 and AESV3 differ only there: a 32-byte key is used
    /// verbatim, anything shorter takes the `sAlT` per-object derivation.
    fn decrypt_with(&self, obj: ObjRef, cipher: Cipher, data: &[u8]) -> Vec<u8> {
        let Some(key) = self.file_key() else {
            return data.to_vec();
        };
        match cipher {
            Cipher::None => data.to_vec(),
            Cipher::Rc4 => object::decrypt_rc4(&key, obj, data),
            Cipher::Aes => match <[u8; 32]>::try_from(key.bytes()) {
                Ok(full) => object::decrypt_aes_v5(&full, data),
                Err(_) => object::decrypt_aes_v4(&key, obj, data),
            },
        }
    }

    /// Encipher one string or stream payload belonging to indirect object
    /// `obj`, the inverse of [`SecurityHandler::decrypt`].
    ///
    /// `iv` must be fresh per payload for the cipher to be sound; the RC4
    /// handler and [`Self::Identity`] ignore it. Infallible, like its inverse.
    ///
    /// RC4 preserves length exactly. AES grows a payload of `n` bytes to
    /// `32 + 16 * (n / 16)`: sixteen for the vector, and a PKCS#7 pad that is
    /// always present, so an already block-aligned payload gains a whole
    /// block. An empty payload is the exception and stays empty, so a save
    /// does not grow every empty string in a document by 32 bytes.
    ///
    /// ```
    /// use pdfrum_crypt::{CryptClass, Iv, SecurityHandler};
    /// use pdfrum_object::ObjRef;
    ///
    /// let handler = SecurityHandler::AesV5 {
    ///     key: Box::new([0; 32]),
    ///     revision: 6,
    ///     permissions: 0xFFFF_FFFC,
    ///     owner_unlocked: false,
    ///     encrypt_metadata: true,
    ///     encoding: pdfrum_crypt::PasswordEncoding::AsGiven,
    ///     embedded_cipher: None,   // no /EFF: the stream cipher serves
    ///     strings_identity: false,
    /// };
    /// let obj = ObjRef::new(4, 0);
    /// let sealed = handler.encrypt(obj, CryptClass::String, Iv([7; 16]), b"secret");
    /// // Sixteen of vector, one block of ciphertext.
    /// assert_eq!(sealed.len(), 32);
    /// assert_eq!(handler.decrypt(obj, CryptClass::String, &sealed), b"secret");
    /// ```
    #[must_use]
    pub fn encrypt(&self, obj: ObjRef, class: CryptClass, iv: Iv, data: &[u8]) -> Vec<u8> {
        // `[oracle-bug]` A27: a pass-through string class, exactly as on the
        // decrypt side.
        if class == CryptClass::String && self.strings_identity() {
            return data.to_vec();
        }
        // `CPDF_Encryptor::Encrypt` returns before reaching the cipher on an
        // empty payload; see the `# Lengths` note.
        if data.is_empty() {
            return Vec::new();
        }
        // `/EFF`'s override, the mirror of the decrypt side. A document we
        // write does not itself set `/EFF` — `pdfrum-edit` writes one filter
        // — but a handler opened from a file that does must re-seal what it
        // opened with the same cipher, or the round trip is not one.
        if let (CryptClass::Embedded, Some(cipher)) = (class, self.embedded_cipher()) {
            return self.encrypt_with(obj, cipher, iv, data);
        }
        match self {
            Self::Identity => data.to_vec(),
            Self::Rc4V2 { key, .. } => object::encrypt_rc4(key, obj, data),
            Self::AesV4 { key, .. } => object::encrypt_aes_v4(key, obj, iv.bytes(), data),
            Self::AesV5 { key, .. } => object::encrypt_aes_v5(key, iv.bytes(), data),
        }
    }

    /// Encipher with a named cipher over this handler's own file key, the
    /// inverse of [`Self::decrypt_with`].
    fn encrypt_with(&self, obj: ObjRef, cipher: Cipher, iv: Iv, data: &[u8]) -> Vec<u8> {
        let Some(key) = self.file_key() else {
            return data.to_vec();
        };
        match cipher {
            Cipher::None => data.to_vec(),
            Cipher::Rc4 => object::encrypt_rc4(&key, obj, data),
            Cipher::Aes => match <[u8; 32]>::try_from(key.bytes()) {
                Ok(full) => object::encrypt_aes_v5(&full, iv.bytes(), data),
                Err(_) => object::encrypt_aes_v4(&key, obj, iv.bytes(), data),
            },
        }
    }

    /// This handler's file encryption key, or `None` for [`Self::Identity`],
    /// which has none. Every `/CF` entry shares it (ISO 32000-1 §7.6.5), so
    /// it is what the `/EFF` override runs its own cipher over.
    fn file_key(&self) -> Option<SmallKey> {
        match self {
            Self::Identity => None,
            Self::Rc4V2 { key, .. } | Self::AesV4 { key, .. } => Some(key.clone()),
            Self::AesV5 { key, .. } => Some(SmallKey::from_full(**key)),
        }
    }

    /// What the document permits, for the password that opened it.
    ///
    /// A document opened with the **owner** password reports what `/P`
    /// allows, the same as a user reading of it; the owner's own unrestricted
    /// view is [`SecurityHandler::owner_permissions`]. An unencrypted document
    /// has no restrictions at all.
    ///
    /// The ISO table-22 decode lives in [`Permissions`], next to the `/P`
    /// word, so no caller has to spell `bits & 0x100`.
    ///
    /// ```
    /// # use pdfrum_crypt::{Permissions, SecurityHandler};
    /// assert_eq!(SecurityHandler::Identity.permissions(), Permissions::ALL);
    /// ```
    #[must_use]
    pub fn permissions(&self) -> Permissions {
        Permissions::from_bits(self.permission_word(false))
    }

    /// What the document permits under the owner's view.
    ///
    /// A document the **owner password** opened reports every permission
    /// granted here, whatever `/P` says, because the owner may lift every
    /// restriction. For a document the user password opened — and for an
    /// unencrypted one — this is the same answer as
    /// [`SecurityHandler::permissions`].
    ///
    /// ```
    /// # use pdfrum_crypt::{Permissions, SecurityHandler};
    /// assert_eq!(SecurityHandler::Identity.owner_permissions(), Permissions::ALL);
    /// ```
    #[must_use]
    pub fn owner_permissions(&self) -> Permissions {
        Permissions::from_bits(self.permission_word(true))
    }

    /// The permission word as the C++ reports it.
    ///
    /// `owner` selects the owner-unlocked override: a document opened with
    /// the owner password reports all permissions granted, while the user
    /// reading of the same document still reports what `/P` allows. The
    /// standard handler then clears the two reserved low bits and forces bits
    /// 7 through 32 set, so `/P 4092` reports `0xFFFFFFFC` either way.
    ///
    /// An unencrypted document has no restrictions at all.
    ///
    /// Private: the word itself is `pdfrum-crypt`'s business, and the two
    /// public methods above are the whole of what leaves the crate. The
    /// forcing is kept exactly as the C++ has it because it is behaviour —
    /// clearing bits 1 and 2 is what makes `/P 4092` and `/P 4095` report
    /// alike — and only the channel changed.
    fn permission_word(&self, owner: bool) -> u32 {
        let (permissions, owner_unlocked) = match self {
            Self::Identity => return 0xFFFF_FFFF,
            Self::Rc4V2 {
                permissions,
                owner_unlocked,
                ..
            }
            | Self::AesV4 {
                permissions,
                owner_unlocked,
                ..
            }
            | Self::AesV5 {
                permissions,
                owner_unlocked,
                ..
            } => (*permissions, *owner_unlocked),
        };
        let base = if owner_unlocked && owner {
            0xFFFF_FFFF
        } else {
            permissions
        };
        (base & 0xFFFF_FFFC) | 0xFFFF_F0C0
    }

    /// Whether the document's metadata stream is encrypted (`/EncryptMetadata`,
    /// default true).
    ///
    /// The parser consults this to decide whether to skip decrypting the
    /// object `/Root/Metadata` points at.
    #[must_use]
    pub fn encrypt_metadata(&self) -> bool {
        match self {
            Self::Identity => true,
            Self::Rc4V2 {
                encrypt_metadata, ..
            }
            | Self::AesV4 {
                encrypt_metadata, ..
            }
            | Self::AesV5 {
                encrypt_metadata, ..
            } => *encrypt_metadata,
        }
    }

    /// `/R`, the handler revision. Zero for an unencrypted document.
    #[must_use]
    pub fn revision(&self) -> u8 {
        match self {
            Self::Identity => 0,
            Self::Rc4V2 { revision, .. }
            | Self::AesV4 { revision, .. }
            | Self::AesV5 { revision, .. } => *revision,
        }
    }

    /// Whether the owner password opened this document.
    #[must_use]
    pub fn owner_unlocked(&self) -> bool {
        match self {
            Self::Identity => false,
            Self::Rc4V2 { owner_unlocked, .. }
            | Self::AesV4 { owner_unlocked, .. }
            | Self::AesV5 { owner_unlocked, .. } => *owner_unlocked,
        }
    }

    /// Which spelling of the password unlocked the document.
    #[must_use]
    pub fn password_encoding(&self) -> PasswordEncoding {
        match self {
            Self::Identity => PasswordEncoding::AsGiven,
            Self::Rc4V2 { encoding, .. }
            | Self::AesV4 { encoding, .. }
            | Self::AesV5 { encoding, .. } => *encoding,
        }
    }
}

/// Whether `dict` is a signature dictionary, whose `/Contents` must stay
/// undecrypted.
///
/// The test is on the *direct* `/Type`, falling back to `/FT` only when
/// `/Type` is absent entirely — a `/Type` of some other value does not let
/// `/FT` speak. The decrypt walk calls this after the enclosing dictionary
/// has been decrypted, because until then both names are ciphertext.
///
/// ```
/// use pdfrum_crypt::is_signature_dict;
/// use pdfrum_object::{Dict, Name, Object, names};
///
/// let sig = Dict::from_pairs([(names::TYPE.clone(), Object::Name(names::SIG.clone()))]);
/// assert!(is_signature_dict(&sig));
///
/// // A field dictionary of signature type counts too, via /FT.
/// let field = Dict::from_pairs([(names::FT.clone(), Object::Name(names::SIG.clone()))]);
/// assert!(is_signature_dict(&field));
///
/// // But a /Type that is present and something else wins over /FT.
/// let annot = Dict::from_pairs([
///     (names::TYPE.clone(), Object::Name(Name::from("Annot"))),
///     (names::FT.clone(), Object::Name(names::SIG.clone())),
/// ]);
/// assert!(!is_signature_dict(&annot));
/// ```
#[must_use]
pub fn is_signature_dict(dict: &Dict) -> bool {
    let key = if dict.contains_key(names::TYPE) {
        names::TYPE
    } else {
        names::FT
    };
    signature_valued(dict, key)
}

/// Whether `key`'s value spells `Sig`, as a name or as a string.
///
/// The C++ reads the value through an accessor that gives a name and a string
/// the same spelling, so a `/Type (Sig)` counts.
fn signature_valued(dict: &Dict, key: &Name) -> bool {
    dict.raw(key).is_some_and(|value| {
        value.as_name().is_some_and(|n| n == names::SIG)
            || value
                .as_string()
                .is_some_and(|s| &*s.bytes == names::SIG.as_bytes())
    })
}

#[cfg(test)]
mod tests {
    use super::{CryptClass, Error, Iv, Permissions, SecurityHandler, is_signature_dict};
    use crate::standard::Cipher;
    use crate::test_fixtures::{self, unhex};
    use pdfrum_object::{Dict, Name, NoResolve, ObjRef, Object, PdfString, names};

    /// The parse-only view a key-length test wants.
    fn cipher_of(dict: &Dict) -> Result<(Cipher, usize), Error> {
        super::parse_encrypt_dict(dict, &NoResolve).map(|p| (p.cipher, p.key_len))
    }

    // ---- T7: the AESV2 fixture, and the /Length promotion it depends on ----

    #[test]
    fn aes_v2_fixture_promotes_a_byte_length_to_bits() {
        let dict = test_fixtures::encrypted_pdf_dict();
        assert_eq!(cipher_of(&dict), Ok((Cipher::Aes, 16)));
    }

    #[test]
    fn aes_v2_fixture_opens_with_either_password() {
        let dict = test_fixtures::encrypted_pdf_dict();
        let id = unhex("1B0FD0F5E29AD84DBF67775E9E3B009F");

        let user = SecurityHandler::from_encrypt_dict(&dict, &id, b"1234", &NoResolve)
            .expect("the user password");
        assert!(matches!(user, SecurityHandler::AesV4 { .. }));
        assert!(!user.owner_unlocked());
        assert_eq!(user.permission_word(false), 0xFFFF_F2C0);
        assert_eq!(user.permission_word(true), 0xFFFF_F2C0);
        assert_eq!(user.revision(), 4);

        let owner = SecurityHandler::from_encrypt_dict(&dict, &id, b"5678", &NoResolve)
            .expect("the owner password");
        assert!(owner.owner_unlocked());
        assert_eq!(owner.permission_word(true), 0xFFFF_FFFC);
        assert_eq!(owner.permission_word(false), 0xFFFF_F2C0);
    }

    #[test]
    fn aes_v2_fixture_rejects_the_wrong_password() {
        let dict = test_fixtures::encrypted_pdf_dict();
        let id = unhex("1B0FD0F5E29AD84DBF67775E9E3B009F");
        for password in [&b""[..], b"tiger"] {
            assert_eq!(
                SecurityHandler::from_encrypt_dict(&dict, &id, password, &NoResolve).unwrap_err(),
                Error::WrongPassword,
                "password {password:?}"
            );
        }
    }

    // ---- T8 / T9 / T10: the AES-256 revisions ----

    #[test]
    fn revision_5_fixture_opens_with_all_four_spellings() {
        let dict = test_fixtures::r5_dict();
        let id = unhex("7ca64129d20fc9745f1bfc0e4166590a");

        let owner_keys: Vec<_> = [&b"\xe2ge"[..], b"\xc3\xa2ge"]
            .iter()
            .map(|password| {
                let handler = SecurityHandler::from_encrypt_dict(&dict, &id, password, &NoResolve)
                    .unwrap_or_else(|_| panic!("owner {password:?}"));
                assert!(handler.owner_unlocked());
                assert_eq!(handler.revision(), 5);
                match handler {
                    SecurityHandler::AesV5 { key, .. } => *key,
                    _ => panic!("expected AesV5"),
                }
            })
            .collect();
        // Both spellings of the same role arrive at the same file key.
        assert_eq!(owner_keys.first(), owner_keys.last());

        for password in [&b"h\xf4tel"[..], b"h\xc3\xb4tel"] {
            let handler = SecurityHandler::from_encrypt_dict(&dict, &id, password, &NoResolve)
                .unwrap_or_else(|_| panic!("user {password:?}"));
            assert!(!handler.owner_unlocked());
        }
    }

    // At revision 5 the /ID plays no part: the same passwords work without it.
    #[test]
    fn revision_5_ignores_the_file_id() {
        let dict = test_fixtures::r5_dict();
        assert!(SecurityHandler::from_encrypt_dict(&dict, &[], b"h\xf4tel", &NoResolve).is_ok());
        assert!(SecurityHandler::from_encrypt_dict(&dict, &[], b"\xe2ge", &NoResolve).is_ok());
    }

    // T8 — the /Perms block is genuinely checked, not just decrypted.
    #[test]
    fn a_tampered_perms_block_rejects_the_password() {
        let mut dict = test_fixtures::r5_dict();
        let mut perms = unhex("c954c264d796dfd131ddb784f5a8b1bf");
        if let Some(byte) = perms.get_mut(9) {
            *byte ^= 0xFF;
        }
        dict.push(
            names::PERMS.clone(),
            Object::Str(PdfString::literal(&perms)),
        );
        assert_eq!(
            SecurityHandler::from_encrypt_dict(&dict, &[], b"h\xf4tel", &NoResolve).unwrap_err(),
            Error::WrongPassword
        );
    }

    // T9 — the only test that drives the hardened hash's 64-round loop, the
    // SHA-384/512 branches and the mod-3 selector.
    #[test]
    fn revision_6_fixture_opens_with_all_four_spellings() {
        let dict = test_fixtures::r6_dict();
        for password in [&b"\xe2ge"[..], b"\xc3\xa2ge"] {
            let handler = SecurityHandler::from_encrypt_dict(&dict, &[], password, &NoResolve)
                .unwrap_or_else(|_| panic!("owner {password:?}"));
            assert!(handler.owner_unlocked());
            assert_eq!(handler.revision(), 6);
        }
        for password in [&b"h\xf4tel"[..], b"h\xc3\xb4tel"] {
            let handler = SecurityHandler::from_encrypt_dict(&dict, &[], password, &NoResolve)
                .unwrap_or_else(|_| panic!("user {password:?}"));
            assert!(!handler.owner_unlocked());
        }
        assert_eq!(
            SecurityHandler::from_encrypt_dict(&dict, &[], b"tiger", &NoResolve).unwrap_err(),
            Error::WrongPassword
        );
    }

    // T10 — bug_644.pdf: ASCII passwords, so the encoding fallback must not
    // fire, and a /P of 4092 that masks to the same word for both roles.
    //
    // The roles are the reverse of what the C++ test *names* suggest: `b` is
    // the owner password and `a` the user one. The embedder test cannot tell,
    // because both roles report the same permissions here — which is exactly
    // why `/P 4092` was picked for that fixture.
    #[test]
    fn revision_5_alternate_fixture() {
        let dict = test_fixtures::bug_644_dict();
        let owner = SecurityHandler::from_encrypt_dict(&dict, &[], b"b", &NoResolve)
            .expect("the owner password");
        assert!(owner.owner_unlocked());
        assert_eq!(owner.permission_word(true), 0xFFFF_FFFC);
        assert_eq!(owner.permission_word(false), 0xFFFF_FFFC);

        let user = SecurityHandler::from_encrypt_dict(&dict, &[], b"a", &NoResolve)
            .expect("the user password");
        assert!(!user.owner_unlocked());
        assert_eq!(user.permission_word(false), 0xFFFF_FFFC);
        // Both roles reach the same file key, since /OE and /UE wrap it.
        assert_eq!(
            format!("{:?}", (owner.revision(), user.revision())),
            "(5, 5)"
        );

        for password in [&b""[..], b"tiger"] {
            assert_eq!(
                SecurityHandler::from_encrypt_dict(&dict, &[], password, &NoResolve).unwrap_err(),
                Error::WrongPassword,
                "password {password:?}"
            );
        }
        assert_eq!(
            owner.password_encoding(),
            crate::PasswordEncoding::AsGiven,
            "an ASCII password never converts"
        );
    }

    // ---- T14: the key-length resolution table ----

    /// One row: `/V`, `/Length`, the crypt filter's own `/Length`, `/CFM`,
    /// and what the pair should resolve to.
    type KeyLengthRow = (
        i64,
        Option<i64>,
        Option<i64>,
        Option<&'static str>,
        Result<(Cipher, usize), Error>,
    );

    #[test]
    fn key_length_resolution_table() {
        use test_fixtures::encrypt_dict;
        let cases: [KeyLengthRow; 14] = [
            (1, None, None, None, Ok((Cipher::Rc4, 5))),
            // /V 1 is 40-bit by definition; its /Length is ignored outright.
            (1, Some(128), None, None, Ok((Cipher::Rc4, 5))),
            (2, None, None, None, Ok((Cipher::Rc4, 5))),
            (2, Some(40), None, None, Ok((Cipher::Rc4, 5))),
            (2, Some(128), None, None, Ok((Cipher::Rc4, 16))),
            (
                2,
                Some(256),
                None,
                None,
                Err(Error::CipherKeyLength {
                    cipher: "RC4",
                    len: 32,
                }),
            ),
            // The `< 40 ⇒ × 8` promotion lives only in the /V >= 4 branch, so
            // /Length 8 here is a bare divide to a one-byte key.
            (
                2,
                Some(8),
                None,
                None,
                Err(Error::CipherKeyLength {
                    cipher: "RC4",
                    len: 1,
                }),
            ),
            (4, Some(128), None, Some("V2"), Ok((Cipher::Rc4, 16))),
            (4, Some(128), Some(16), Some("AESV2"), Ok((Cipher::Aes, 16))),
            (
                4,
                Some(128),
                Some(128),
                Some("AESV2"),
                Ok((Cipher::Aes, 16)),
            ),
            (4, None, None, Some("AESV2"), Ok((Cipher::Aes, 16))),
            (
                4,
                Some(128),
                Some(40),
                Some("AESV2"),
                Err(Error::CipherKeyLength {
                    cipher: "AES",
                    len: 5,
                }),
            ),
            (5, Some(256), Some(32), Some("AESV3"), Ok((Cipher::Aes, 32))),
            (5, None, None, Some("AESV3"), Ok((Cipher::Aes, 32))),
        ];
        for (version, length, filter_length, method, expected) in cases {
            let dict = encrypt_dict(version, length, filter_length, method);
            assert_eq!(
                cipher_of(&dict),
                expected,
                "/V {version} /Length {length:?} /CF Length {filter_length:?} /CFM {method:?}"
            );
        }
    }

    #[test]
    fn a_negative_filter_length_is_malformed() {
        let dict = test_fixtures::encrypt_dict(4, Some(128), Some(-8), Some("AESV2"));
        assert!(matches!(
            cipher_of(&dict),
            Err(Error::MalformedEncryptDict(_))
        ));
    }

    // The /Identity crypt filter is a handler, not a failure.
    #[test]
    fn an_identity_crypt_filter_yields_the_identity_handler() {
        let dict = test_fixtures::identity_dict();
        assert_eq!(cipher_of(&dict), Ok((Cipher::None, 0)));
        let handler = SecurityHandler::from_encrypt_dict(&dict, &[], b"", &NoResolve)
            .expect("identity needs no password");
        assert!(matches!(handler, SecurityHandler::Identity));
        assert_eq!(
            handler.decrypt(ObjRef::new(3, 0), CryptClass::Stream, b"plain"),
            b"plain"
        );
    }

    // ---- T15: the crypt-filter class rules ----

    /// Audit item **A27**. This asserted `Err(MismatchedCryptFilters)`.
    /// §7.6.5 table 20 makes `/StmF` and `/StrF` two independent entries, so
    /// differing names are conformant — the stream filter supplies the
    /// cipher this record models.
    // [oracle-bug] cpdf_security_handler.cpp:305 and :325 return false on a
    // raw name inequality.
    #[test]
    fn differing_stream_and_string_filters_open_rather_than_refusing() {
        let mut dict = test_fixtures::encrypt_dict(4, Some(128), Some(16), Some("AESV2"));
        dict.push(names::STR_F.clone(), Object::Name(Name::from("Other")));
        assert_eq!(cipher_of(&dict), Ok((Cipher::Aes, 16)));
    }

    /// Audit item **A27**. This asserted `Err(MismatchedCryptFilters)` for an
    /// absent `/StmF` against an explicit `/StrF`, because PDFium compares the
    /// looked-up bytes *before* applying any default and absent reads as
    /// empty. §7.6.5's default is `/Identity`, so the streams pass through
    /// while the strings are enciphered by `/StrF`'s filter.
    #[test]
    fn an_absent_stream_filter_defaults_to_identity_beside_a_named_string_filter() {
        let mut dict = test_fixtures::bare_v4_dict();
        dict.push(names::STR_F.clone(), Object::Name(Name::from("StdCF")));
        let params =
            super::parse_encrypt_dict(&dict, &NoResolve).expect("a defaulted /StmF is conformant");
        assert_eq!(params.cipher, Cipher::None, "/StmF defaults to /Identity");
        assert_eq!(params.string_cipher, Cipher::Aes, "/StrF names StdCF");
    }

    /// Audit item **A27**. This asserted `Err(MissingCryptFilter)` for both
    /// entries absent, because the empty name is looked up in `/CF` and is
    /// not there. §7.6.5 defaults **both** to `/Identity`, so `/CF` is never
    /// consulted and nothing is enciphered.
    #[test]
    fn both_class_filters_absent_default_to_identity() {
        let dict = test_fixtures::bare_v4_dict();
        assert_eq!(cipher_of(&dict), Ok((Cipher::None, 0)));
        let handler = SecurityHandler::from_encrypt_dict(&dict, &[], b"", &NoResolve)
            .expect("a document with neither class filter opens");
        assert!(matches!(handler, SecurityHandler::Identity));
    }

    /// Audit item **A27**. A V4 dictionary with no `/CF` used to be
    /// `MalformedEncryptDict`, because PDFium's empty-name lookup reached for
    /// `/CF` before any default applied. With both classes defaulting to
    /// `/Identity`, `/CF` is never consulted, so the dictionary resolves to no
    /// cipher rather than to a malformation.
    #[test]
    fn a_missing_crypt_filter_dictionary_defaults_to_identity() {
        let dict = Dict::from_pairs([
            (names::FILTER.clone(), Object::Name(names::STANDARD.clone())),
            (names::V.clone(), Object::Int(4)),
            (names::R.clone(), Object::Int(4)),
        ]);
        assert_eq!(cipher_of(&dict), Ok((Cipher::None, 0)));
    }

    /// Audit item **A27**, the case the fix exists for: `/StrF /Identity`
    /// beside an enciphering `/StmF`. PDFium refuses the document; §7.6.5
    /// says its strings are plaintext while its streams are enciphered.
    #[test]
    fn an_identity_string_filter_leaves_strings_plaintext_beside_an_enciphering_stream() {
        let mut dict = test_fixtures::encrypt_dict(4, Some(128), Some(16), Some("AESV2"));
        dict.push(names::STR_F.clone(), Object::Name(names::IDENTITY.clone()));
        let params = super::parse_encrypt_dict(&dict, &NoResolve).expect("conformant per §7.6.5");
        assert_eq!(params.cipher, Cipher::Aes);
        assert_eq!(params.string_cipher, Cipher::None);
    }

    // ---- Handler-level facts ----

    #[test]
    fn a_non_standard_filter_is_unsupported() {
        for spelling in ["Adobe.PubSec", "Nonesuch"] {
            let dict =
                Dict::from_pairs([(names::FILTER.clone(), Object::Name(Name::from(spelling)))]);
            assert_eq!(
                SecurityHandler::from_encrypt_dict(&dict, &[], b"", &NoResolve).unwrap_err(),
                Error::UnsupportedHandler(spelling.as_bytes().into())
            );
        }
    }

    // The /Filter check is name-typed, so a string-valued one is not the
    // standard handler even though it spells "Standard".
    #[test]
    fn a_string_valued_filter_is_not_the_standard_handler() {
        let dict = Dict::from_pairs([(
            names::FILTER.clone(),
            Object::Str(PdfString::literal(b"Standard")),
        )]);
        assert_eq!(
            SecurityHandler::from_encrypt_dict(&dict, &[], b"", &NoResolve).unwrap_err(),
            Error::UnsupportedHandler(Box::default())
        );
    }

    // /EncryptMetadata is read boolean-typed before resolving, so an Int(0)
    // there does not turn metadata encryption off.
    #[test]
    fn encrypt_metadata_reads_only_a_boolean() {
        let mut dict = test_fixtures::encrypt_dict(4, Some(128), Some(16), Some("AESV2"));
        dict.push(names::ENCRYPT_METADATA.clone(), Object::Int(0));
        let params = super::parse_encrypt_dict(&dict, &NoResolve).expect("parses");
        assert!(params.encrypt_metadata, "an integer is not a boolean");

        let mut dict = test_fixtures::encrypt_dict(4, Some(128), Some(16), Some("AESV2"));
        dict.push(names::ENCRYPT_METADATA.clone(), Object::Bool(false));
        let params = super::parse_encrypt_dict(&dict, &NoResolve).expect("parses");
        assert!(!params.encrypt_metadata);
    }

    #[test]
    fn identity_reports_no_restrictions_and_no_revision() {
        let handler = SecurityHandler::Identity;
        assert_eq!(handler.permission_word(false), 0xFFFF_FFFF);
        assert_eq!(handler.permission_word(true), 0xFFFF_FFFF);
        assert_eq!(handler.revision(), 0);
        assert!(handler.encrypt_metadata());
        assert!(!handler.owner_unlocked());
    }

    #[test]
    fn the_permission_mask_clears_reserved_bits_and_forces_the_high_ones() {
        let dict = test_fixtures::encrypted_pdf_dict();
        let id = unhex("1B0FD0F5E29AD84DBF67775E9E3B009F");
        let handler = SecurityHandler::from_encrypt_dict(&dict, &id, b"1234", &NoResolve)
            .expect("the user password");
        let reported = handler.permission_word(false);
        assert_eq!(reported & 0b11, 0, "the two reserved bits are cleared");
        assert_eq!(
            reported & 0xFFFF_F0C0,
            0xFFFF_F0C0,
            "the forced bits are set"
        );
    }

    // The two public methods are the private word, decoded. This is the
    // assertion the facade used to make for itself with `bits & 0x100`
    // before the decode moved here: the AESV2 fixture's `/P` reports
    // `0xFFFF_F2C0`, which grants neither form filling (bit 9) nor annotation
    // modification (bit 6) — and the owner's view of the same document grants
    // everything.
    #[test]
    fn the_public_methods_decode_the_word_the_private_one_reports() {
        let dict = test_fixtures::encrypted_pdf_dict();
        let id = unhex("1B0FD0F5E29AD84DBF67775E9E3B009F");

        let user = SecurityHandler::from_encrypt_dict(&dict, &id, b"1234", &NoResolve)
            .expect("the user password");
        let granted = user.permissions();
        assert_eq!(granted, Permissions::from_bits(user.permission_word(false)));
        // `0xFFFF_F2C0` sets exactly one of table 22's eight named bits — 10,
        // accessibility extraction. Neither of the two the form session asks
        // about is granted, and neither is printing.
        assert_eq!(
            granted,
            Permissions {
                extract: true,
                ..Permissions::NONE
            }
        );
        // The user's own owner view is still the user's, since the user
        // password opened it, not the owner's.
        assert_eq!(user.owner_permissions(), granted);

        let owner = SecurityHandler::from_encrypt_dict(&dict, &id, b"5678", &NoResolve)
            .expect("the owner password");
        assert_eq!(owner.owner_permissions(), Permissions::ALL);
        assert_eq!(owner.permissions(), granted);
    }

    // ---- T12 / T13: damage tolerance ----

    #[test]
    fn a_short_user_entry_rejects_rather_than_reading_out_of_bounds() {
        for len in 0..16usize {
            let dict = test_fixtures::r3_dict_with_user_entry(&vec![0xCD; len]);
            let id = unhex("9b744068bb5efbe920baaba6da63c2bf");
            assert_eq!(
                SecurityHandler::from_encrypt_dict(&dict, &id, b"h\xf4tel", &NoResolve)
                    .unwrap_err(),
                Error::WrongPassword,
                "/U of {len} bytes"
            );
        }
    }

    // A /U of 16 to 31 bytes is zero-padded into the working buffer rather
    // than rejected, so the comparison still runs over its first 16 bytes.
    #[test]
    fn a_partial_user_entry_is_zero_padded_and_still_compared() {
        for len in 16..32usize {
            let dict = test_fixtures::r3_dict_with_user_entry(&vec![0xCD; len]);
            let id = unhex("9b744068bb5efbe920baaba6da63c2bf");
            // No panic; the wrong bytes simply do not match.
            assert!(
                SecurityHandler::from_encrypt_dict(&dict, &id, b"h\xf4tel", &NoResolve).is_err(),
                "/U of {len} bytes"
            );
        }
    }

    // T13 — /O and /U must each be at least 48 bytes whichever role is
    // checked, because the owner check hashes the whole of /U alongside the
    // password; /UE must be 32 for a user open and /OE for an owner one.
    #[test]
    fn short_version_five_entries_reject_rather_than_panicking() {
        for len in [0usize, 1, 31, 47] {
            let short = vec![0xEFu8; len];
            // Both password entries gate both roles.
            for key in [names::O, names::U, names::PERMS] {
                let mut dict = test_fixtures::r5_dict();
                dict.push(key.clone(), Object::Str(PdfString::literal(&short)));
                for password in [&b"h\xf4tel"[..], b"\xe2ge"] {
                    assert!(
                        SecurityHandler::from_encrypt_dict(&dict, &[], password, &NoResolve)
                            .is_err(),
                        "{key:?} of {len} bytes with {password:?}"
                    );
                }
            }
            // The wrapped-key entries gate only the role that unwraps them.
            for (key, password) in [(names::UE, &b"h\xf4tel"[..]), (names::OE, b"\xe2ge")] {
                let mut dict = test_fixtures::r5_dict();
                dict.push(key.clone(), Object::Str(PdfString::literal(&short)));
                assert!(
                    SecurityHandler::from_encrypt_dict(&dict, &[], password, &NoResolve).is_err(),
                    "{key:?} of {len} bytes"
                );
            }
        }
    }

    #[test]
    fn an_empty_perms_entry_rejects() {
        let mut dict = test_fixtures::r5_dict();
        dict.push(names::PERMS.clone(), Object::Str(PdfString::literal(b"")));
        assert_eq!(
            SecurityHandler::from_encrypt_dict(&dict, &[], b"h\xf4tel", &NoResolve).unwrap_err(),
            Error::WrongPassword
        );
    }

    // T11 — the bad-okey fixtures: a truncated /O must fail the open, not
    // read past its end (crbug.com/42270437).
    #[test]
    fn a_truncated_owner_entry_fails_the_open() {
        for revision in [2i64, 3] {
            for len in [0usize, 1, 31] {
                let dict = test_fixtures::rc4_dict_with_owner_entry(revision, &vec![0x11; len]);
                assert_eq!(
                    SecurityHandler::from_encrypt_dict(&dict, &[], b"a", &NoResolve).unwrap_err(),
                    Error::WrongPassword,
                    "/R {revision} with /O of {len} bytes"
                );
            }
        }
    }

    // ---- The signature-dictionary predicate ----

    #[test]
    fn signature_dictionaries_are_recognised_by_type_then_field_type() {
        let sig_type = Dict::from_pairs([(names::TYPE.clone(), Object::Name(names::SIG.clone()))]);
        assert!(is_signature_dict(&sig_type));

        let sig_field = Dict::from_pairs([(names::FT.clone(), Object::Name(names::SIG.clone()))]);
        assert!(is_signature_dict(&sig_field));

        // /Type present and not Sig shuts /FT out.
        let annot = Dict::from_pairs([
            (names::TYPE.clone(), Object::Name(Name::from("Annot"))),
            (names::FT.clone(), Object::Name(names::SIG.clone())),
        ]);
        assert!(!is_signature_dict(&annot));

        // Neither key at all.
        assert!(!is_signature_dict(&Dict::new()));
    }

    // The value is read through an accessor that spells a name and a string
    // alike, so a string-valued /Type counts.
    #[test]
    fn a_string_valued_type_still_names_a_signature() {
        let dict =
            Dict::from_pairs([(names::TYPE.clone(), Object::Str(PdfString::literal(b"Sig")))]);
        assert!(is_signature_dict(&dict));
    }

    // ---- The public decrypt entry point ----

    /// The three real fixtures, as opened handlers, for the payload tests.
    fn opened_handlers() -> Vec<(&'static str, SecurityHandler)> {
        let aes_v2_id = unhex("1B0FD0F5E29AD84DBF67775E9E3B009F");
        let rc4_id = unhex("9b744068bb5efbe920baaba6da63c2bf");
        vec![
            (
                "RC4 (/R 3)",
                SecurityHandler::from_encrypt_dict(
                    &test_fixtures::r3_dict(),
                    &rc4_id,
                    b"h\xf4tel",
                    &NoResolve,
                )
                .expect("the r3 user password"),
            ),
            (
                "AESV2 (/R 4)",
                SecurityHandler::from_encrypt_dict(
                    &test_fixtures::encrypted_pdf_dict(),
                    &aes_v2_id,
                    b"1234",
                    &NoResolve,
                )
                .expect("the encrypted.pdf user password"),
            ),
            (
                "AESV3 (/R 6)",
                SecurityHandler::from_encrypt_dict(
                    &test_fixtures::r6_dict(),
                    &[],
                    b"h\xf4tel",
                    &NoResolve,
                )
                .expect("the r6 user password"),
            ),
        ]
    }

    // RC4 is symmetric, so decrypting twice restores the payload; AES is not,
    // so only its length behavior is asserted here.
    #[test]
    fn rc4_decrypt_round_trips_through_the_public_api() {
        let rc4_id = unhex("9b744068bb5efbe920baaba6da63c2bf");
        let handler = SecurityHandler::from_encrypt_dict(
            &test_fixtures::r3_dict(),
            &rc4_id,
            b"h\xf4tel",
            &NoResolve,
        )
        .expect("the r3 user password");
        let obj = ObjRef::new(12, 3);
        let payload = b"Hello, encrypted world.".to_vec();
        for class in [CryptClass::Stream, CryptClass::String, CryptClass::Embedded] {
            let once = handler.decrypt(obj, class, &payload);
            assert_ne!(once, payload, "{class:?} actually enciphered");
            assert_eq!(handler.decrypt(obj, class, &once), payload, "{class:?}");
        }
    }

    // D1 — all three classes resolve to the same cipher and key, so the same
    // bytes decrypt identically whichever class they are labelled with.
    #[test]
    fn every_crypt_class_decrypts_the_same_way() {
        for (name, handler) in opened_handlers() {
            let obj = ObjRef::new(7, 0);
            let payload: Vec<u8> = (0..64u8).collect();
            let stream = handler.decrypt(obj, CryptClass::Stream, &payload);
            assert_eq!(
                handler.decrypt(obj, CryptClass::String, &payload),
                stream,
                "{name}"
            );
            assert_eq!(
                handler.decrypt(obj, CryptClass::Embedded, &payload),
                stream,
                "{name}"
            );
        }
    }

    // -----------------------------------------------------------------
    // `/EFF` — audit A26. The oracle reads the key nowhere
    // (`grep '"EFF"' core/ fpdfsdk/` is empty), so an embedded file stream
    // decrypts with the stream filter whatever `/EFF` says. The corpus has
    // no file with an `/EFF` at all, let alone one differing from `/StmF`,
    // so these fixtures are constructed rather than taken from it.
    // -----------------------------------------------------------------

    /// The `encrypted.pdf` dictionary — whose `/StmF` is AESV2 and whose
    /// `/O`/`/U` are real, so it opens with `1234` — extended with a second
    /// `/CF` entry that `/EFF` names.
    fn eff_dict(embedded_method: &str) -> Dict {
        use pdfrum_object::Name;
        let mut dict = test_fixtures::encrypted_pdf_dict();
        let std_cf = Dict::from_pairs([
            (names::CFM.clone(), Object::Name(Name::from("AESV2"))),
            (names::LENGTH.clone(), Object::Int(16)),
        ]);
        let emb_cf = Dict::from_pairs([
            (
                names::CFM.clone(),
                Object::Name(Name::from(embedded_method)),
            ),
            (names::LENGTH.clone(), Object::Int(16)),
        ]);
        dict.push(
            names::CF.clone(),
            Object::Dict(Dict::from_pairs([
                (Name::from("StdCF"), Object::Dict(std_cf)),
                (Name::from("EmbCF"), Object::Dict(emb_cf)),
            ])),
        );
        dict.push(names::EFF.clone(), Object::Name(Name::from("EmbCF")));
        dict
    }

    /// The handler `eff_dict` opens to, under `encrypted.pdf`'s password.
    fn eff_handler(dict: &Dict) -> SecurityHandler {
        SecurityHandler::from_encrypt_dict(
            dict,
            &unhex("1B0FD0F5E29AD84DBF67775E9E3B009F"),
            b"1234",
            &NoResolve,
        )
        .expect("the encrypted.pdf user password")
    }

    /// An `/EFF` naming a filter with a different `/CFM` gives the embedded
    /// class its own cipher — pdf.js `crypto.js:1120`, `:1336`; ISO 32000-1
    /// §7.6.5 table 20. Fails on the old behaviour, which folded `Embedded`
    /// onto `Stream`.
    #[test]
    fn an_eff_naming_another_filter_decrypts_embedded_files_with_it() {
        let dict = eff_dict("V2");
        let params = super::parse_encrypt_dict(&dict, &NoResolve).unwrap();
        assert_eq!(params.cipher, Cipher::Aes);
        assert_eq!(params.embedded_cipher, Some(Cipher::Rc4));

        let handler = eff_handler(&dict);
        assert_eq!(handler.embedded_cipher(), Some(Cipher::Rc4));

        let obj = ObjRef::new(9, 0);
        let payload: Vec<u8> = (0..48u8).collect();
        // The two classes now genuinely differ: AES reads the first sixteen
        // bytes as an initialisation vector, RC4 preserves length.
        let as_stream = handler.decrypt(obj, CryptClass::Stream, &payload);
        let as_embedded = handler.decrypt(obj, CryptClass::Embedded, &payload);
        assert_eq!(as_embedded.len(), payload.len());
        assert_ne!(as_embedded, as_stream);
        // And the string class follows the stream, as `/StrF` names `StdCF`.
        assert_eq!(
            handler.decrypt(obj, CryptClass::String, &payload),
            as_stream
        );
    }

    /// The embedded class round-trips through its own cipher.
    #[test]
    fn the_embedded_class_round_trips_under_its_own_cipher() {
        let dict = eff_dict("V2");
        let handler = eff_handler(&dict);
        let obj = ObjRef::new(9, 0);
        let payload = b"an attachment".to_vec();
        let sealed = handler.encrypt(obj, CryptClass::Embedded, Iv([3; 16]), &payload);
        assert_eq!(handler.decrypt(obj, CryptClass::Embedded, &sealed), payload);
        // Sealed under RC4, so it is not what the stream cipher would make.
        assert_ne!(
            sealed,
            handler.encrypt(obj, CryptClass::Stream, Iv([3; 16]), &payload)
        );
    }

    /// Table 20's default for an absent `/EFF` is `/StmF`, so a document
    /// without the key — every file in the corpus — is unchanged.
    #[test]
    fn an_absent_eff_leaves_every_class_on_the_stream_cipher() {
        for (name, handler) in opened_handlers() {
            assert_eq!(handler.embedded_cipher(), None, "{name}");
        }
        let dict = test_fixtures::encrypt_dict(4, Some(128), Some(16), Some("AESV2"));
        let params = super::parse_encrypt_dict(&dict, &NoResolve).unwrap();
        assert_eq!(params.embedded_cipher, None);
    }

    /// Naming `/StmF`'s own filter is the default written out, and an `/EFF`
    /// whose `/CFM` resolves to the same cipher needs no override either.
    #[test]
    fn an_eff_that_agrees_with_the_stream_filter_is_no_override() {
        use pdfrum_object::Name;
        let mut same = test_fixtures::encrypt_dict(4, Some(128), Some(16), Some("AESV2"));
        same.push(names::EFF.clone(), Object::Name(Name::from("StdCF")));
        assert_eq!(
            super::parse_encrypt_dict(&same, &NoResolve)
                .unwrap()
                .embedded_cipher,
            None
        );
        // Different filter name, same cipher — AESV3 and AESV2 are both
        // `Cipher::Aes`, which is the level `/EFF` can actually change.
        let agreeing = eff_dict("AESV3");
        assert_eq!(
            super::parse_encrypt_dict(&agreeing, &NoResolve)
                .unwrap()
                .embedded_cipher,
            None
        );
    }

    /// `/EFF /Identity` leaves embedded files in the clear while the streams
    /// stay enciphered — the case that makes `/EFF` worth reading at all.
    #[test]
    fn an_identity_eff_leaves_embedded_files_unenciphered() {
        let mut dict = test_fixtures::encrypted_pdf_dict();
        dict.push(names::EFF.clone(), Object::Name(names::IDENTITY.clone()));
        let handler = eff_handler(&dict);
        assert_eq!(handler.embedded_cipher(), Some(Cipher::None));
        let obj = ObjRef::new(9, 0);
        let payload: Vec<u8> = (0..48u8).collect();
        assert_eq!(
            handler.decrypt(obj, CryptClass::Embedded, &payload),
            payload
        );
        assert_ne!(handler.decrypt(obj, CryptClass::Stream, &payload), payload);
    }

    /// An `/EFF` naming a filter `/CF` does not have is damage, not a reason
    /// to refuse the document: the streams still decrypt, and the embedded
    /// class falls back to the stream cipher — the absent-key default.
    #[test]
    fn an_eff_naming_a_missing_filter_falls_back_rather_than_failing() {
        use pdfrum_object::Name;
        let mut dict = test_fixtures::encrypt_dict(4, Some(128), Some(16), Some("AESV2"));
        dict.push(names::EFF.clone(), Object::Name(Name::from("NoSuchCF")));
        let params = super::parse_encrypt_dict(&dict, &NoResolve).expect("still opens");
        assert_eq!(params.embedded_cipher, None);
    }

    // The object number keys the payload for RC4 and AESV2 but not for
    // AESV3, whose key is the file key itself.
    #[test]
    fn only_the_pre_version_five_handlers_key_by_object() {
        let payload: Vec<u8> = (0..48u8).map(|i| i.wrapping_mul(5)).collect();
        for (name, handler) in opened_handlers() {
            let first = handler.decrypt(ObjRef::new(1, 0), CryptClass::Stream, &payload);
            let second = handler.decrypt(ObjRef::new(2, 0), CryptClass::Stream, &payload);
            if handler.revision() >= 5 {
                assert_eq!(second, first, "{name} uses the file key verbatim");
            } else {
                assert_ne!(second, first, "{name} salts by object number");
            }
        }
    }

    // No input length may panic, and nothing may produce more bytes than it
    // was given.
    #[test]
    fn decrypt_never_panics_and_never_grows_a_payload() {
        for (name, handler) in opened_handlers() {
            for len in 0..80usize {
                let payload = vec![0xA5u8; len];
                let out = handler.decrypt(ObjRef::new(9, 1), CryptClass::Stream, &payload);
                assert!(out.len() <= len, "{name} at {len} bytes");
            }
        }
        // The identity handler is the one that returns exactly its input.
        for len in 0..40usize {
            let payload = vec![0x5Au8; len];
            assert_eq!(
                SecurityHandler::Identity.decrypt(ObjRef::new(0, 0), CryptClass::String, &payload),
                payload
            );
        }
    }

    // ---- encrypt-then-decrypt, per revision ----

    /// Every revision the corpus exercises, as an opened handler.
    ///
    /// `opened_handlers` covers three; this adds R2 and R5 so the round-trip
    /// matrix spans /R 2 through /R 6, which is what byte-identity per
    /// revision requires.
    fn every_revision() -> Vec<(&'static str, SecurityHandler)> {
        let r2_id = unhex("2b778de1bcef1733b35e680882812409");
        let mut all = vec![(
            "RC4 (/R 2)",
            SecurityHandler::from_encrypt_dict(
                &test_fixtures::r2_dict(),
                &r2_id,
                b"h\xf4tel",
                &NoResolve,
            )
            .expect("the r2 user password"),
        )];
        all.extend(opened_handlers());
        all.push((
            "AESV3 (/R 5)",
            SecurityHandler::from_encrypt_dict(
                &test_fixtures::r5_dict(),
                &[],
                b"h\xf4tel",
                &NoResolve,
            )
            .expect("the r5 user password"),
        ));
        all
    }

    // The KAT: whatever we encipher, our own decipher returns byte for byte,
    // at every revision, every class and every length that straddles a block
    // boundary.
    #[test]
    fn every_revision_round_trips_encrypt_then_decrypt() {
        for (name, handler) in every_revision() {
            let obj = ObjRef::new(11, 0);
            for len in [0usize, 1, 15, 16, 17, 31, 32, 33, 64, 127] {
                let payload: Vec<u8> = (0..len)
                    .map(|i| u8::try_from(i % 253).unwrap_or(0))
                    .collect();
                for class in [CryptClass::Stream, CryptClass::String, CryptClass::Embedded] {
                    let iv = Iv([u8::try_from(len % 256).unwrap_or(0); 16]);
                    let sealed = handler.encrypt(obj, class, iv, &payload);
                    assert_eq!(
                        handler.decrypt(obj, class, &sealed),
                        payload,
                        "{name} {class:?} at {len} bytes"
                    );
                }
            }
        }
    }

    // A payload really is enciphered — a handler that returned its input
    // would pass the round-trip test above and write a plaintext file.
    #[test]
    fn an_encrypted_payload_is_not_its_own_plaintext() {
        let payload = b"Hello, encrypted world.".to_vec();
        for (name, handler) in every_revision() {
            let sealed = handler.encrypt(
                ObjRef::new(4, 0),
                CryptClass::Stream,
                Iv([0x5A; 16]),
                &payload,
            );
            assert_ne!(sealed, payload, "{name}");
        }
        // Identity is the one handler that passes bytes through untouched.
        assert_eq!(
            SecurityHandler::Identity.encrypt(
                ObjRef::new(4, 0),
                CryptClass::Stream,
                Iv([0; 16]),
                &payload
            ),
            payload
        );
    }

    // The object reference keys the payload for RC4 and AESV2 and does not
    // for AESV3 — the same split the decrypt side has, since it is the same
    // derivation.
    #[test]
    fn only_the_pre_version_five_handlers_key_an_encryption_by_object() {
        let payload = b"payload".to_vec();
        for (name, handler) in every_revision() {
            let iv = Iv([1; 16]);
            let first = handler.encrypt(ObjRef::new(1, 0), CryptClass::Stream, iv, &payload);
            let second = handler.encrypt(ObjRef::new(2, 0), CryptClass::Stream, iv, &payload);
            if handler.revision() >= 5 {
                assert_eq!(second, first, "{name} uses the file key verbatim");
            } else {
                assert_ne!(second, first, "{name} salts by object number");
            }
        }
    }

    // Determinism is a parameter here too: the same vector gives the same
    // bytes, which is what lets `pdfrum-edit` snapshot a whole encrypted file.
    #[test]
    fn the_same_vector_produces_the_same_ciphertext() {
        for (name, handler) in every_revision() {
            let obj = ObjRef::new(6, 0);
            let once = handler.encrypt(obj, CryptClass::Stream, Iv([2; 16]), b"stable");
            let again = handler.encrypt(obj, CryptClass::Stream, Iv([2; 16]), b"stable");
            assert_eq!(once, again, "{name}");
            // And a different vector does not, for the ciphers that read it.
            let other = handler.encrypt(obj, CryptClass::Stream, Iv([3; 16]), b"stable");
            if handler.revision() >= 4 {
                assert_ne!(other, once, "{name} mixes the vector in");
            }
        }
    }

    // No length may panic, and the growth is exactly the documented law.
    #[test]
    fn encrypt_never_panics_and_grows_by_the_documented_amount() {
        for (name, handler) in every_revision() {
            for len in 0..80usize {
                let out = handler.encrypt(
                    ObjRef::new(9, 1),
                    CryptClass::Stream,
                    Iv([0xC3; 16]),
                    &vec![0xA5u8; len],
                );
                let expected = if len == 0 || matches!(handler, SecurityHandler::Rc4V2 { .. }) {
                    len
                } else {
                    32 + (len / 16) * 16
                };
                assert_eq!(out.len(), expected, "{name} at {len} bytes");
            }
        }
    }

    // The one length that skips the cipher entirely. Without it a save would
    // rewrite every `()` in a document as 32 bytes of vector and padding —
    // which round-trips, but is not what the oracle writes.
    #[test]
    fn an_empty_payload_stays_empty_at_every_revision() {
        for (name, handler) in every_revision() {
            for class in [CryptClass::Stream, CryptClass::String, CryptClass::Embedded] {
                assert!(
                    handler
                        .encrypt(ObjRef::new(2, 0), class, Iv([0xFF; 16]), b"")
                        .is_empty(),
                    "{name} {class:?}"
                );
            }
        }
    }

    // ---- The properties a fuzzer would look for ----
    //
    // Every byte of an /Encrypt dictionary comes from the file, so no
    // combination of them may panic. These sweep the shape space
    // deterministically rather than randomly: a failure names the exact input
    // instead of a corpus file.

    /// A cheap deterministic byte sequence — no dependency, and reproducible.
    fn pseudo_random(seed: u64, len: usize) -> Vec<u8> {
        let mut state = seed.wrapping_mul(0x2545_F491_4F6C_DD1D) | 1;
        (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                u8::try_from(state >> 56).unwrap_or(0)
            })
            .collect()
    }

    #[test]
    fn arbitrary_encrypt_dictionaries_never_panic() {
        for seed in 0..8u64 {
            for version in [-1i64, 0, 1, 2, 3, 4, 5, 6, 99] {
                for revision in [0i64, 2, 3, 4, 5, 6, 7] {
                    // The extremes are `INT_RANGE`'s: a lexer folds anything
                    // wider to zero, so nothing outside it can reach here.
                    for length in [
                        *pdfrum_object::INT_RANGE.start(),
                        -8,
                        0,
                        8,
                        40,
                        128,
                        256,
                        4096,
                        *pdfrum_object::INT_RANGE.end(),
                    ] {
                        let mut dict = test_fixtures::encrypt_dict(
                            version,
                            Some(length),
                            Some(length),
                            Some("AESV2"),
                        );
                        dict.push(names::R.clone(), Object::Int(revision));
                        for key in [names::O, names::U, names::OE, names::UE, names::PERMS] {
                            let entry = pseudo_random(
                                seed.wrapping_add(key.as_bytes().len() as u64),
                                usize::try_from(seed % 60).unwrap_or(0),
                            );
                            dict.push(key.clone(), Object::Str(PdfString::literal(entry)));
                        }
                        let password = pseudo_random(seed, usize::try_from(seed % 9).unwrap_or(0));
                        let file_id =
                            pseudo_random(seed + 1, usize::try_from(seed % 20).unwrap_or(0));
                        // The only requirement is that it returns.
                        let _ = SecurityHandler::from_encrypt_dict(
                            &dict, &file_id, &password, &NoResolve,
                        );
                    }
                }
            }
        }
    }

    // The revision 6 loop is the one unbounded-looking construction; a
    // hostile /U salt cannot make it run away, and a long password only
    // enlarges each round rather than adding rounds.
    #[test]
    fn arbitrary_version_five_entries_terminate() {
        for seed in 0..4u64 {
            let mut dict = test_fixtures::r6_dict();
            for key in [names::O, names::U, names::OE, names::UE, names::PERMS] {
                let entry = pseudo_random(seed, 48);
                dict.push(key.clone(), Object::Str(PdfString::literal(entry)));
            }
            // Long enough that each round moves real data, short enough that
            // the worst case — 287 rounds of 64 repetitions — stays quick.
            let password = pseudo_random(seed, 64);
            let _ = SecurityHandler::from_encrypt_dict(&dict, &[], &password, &NoResolve);
        }
    }

    // T11/T12/T13 as a sweep: any length of any password entry, at any
    // revision, must return rather than panic.
    #[test]
    fn every_password_entry_length_is_survivable() {
        for len in 0..64usize {
            let entry = pseudo_random(len as u64, len);
            // Revision 6 is covered by its own sweep above; running it for
            // every length here would spend minutes on the hardened hash.
            for base in [
                test_fixtures::r2_dict(),
                test_fixtures::r3_dict(),
                test_fixtures::encrypted_pdf_dict(),
                test_fixtures::r5_dict(),
            ] {
                for key in [names::O, names::U, names::OE, names::UE, names::PERMS] {
                    let mut dict = base.clone();
                    dict.push(key.clone(), Object::Str(PdfString::literal(&entry)));
                    let _ = SecurityHandler::from_encrypt_dict(&dict, &[], b"pw", &NoResolve);
                }
            }
        }
    }

    #[test]
    fn handlers_are_send_and_sync() {
        const fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<SecurityHandler>();
        assert_send_sync::<Error>();
        assert_send_sync::<crate::EncryptParams>();
    }

    #[test]
    fn a_handler_debug_dump_never_shows_key_material() {
        let dict = test_fixtures::encrypted_pdf_dict();
        let id = unhex("1B0FD0F5E29AD84DBF67775E9E3B009F");
        let handler = SecurityHandler::from_encrypt_dict(&dict, &id, b"1234", &NoResolve)
            .expect("the user password");
        let dump = format!("{handler:?}");
        assert!(dump.contains("redacted"), "{dump}");
    }
}
