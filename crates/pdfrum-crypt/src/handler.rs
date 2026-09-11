//! The standard security handler: which cipher, which key, and what the
//! password unlocked.

use pdfrum_object::{Dict, Name, ObjRef, Resolve, names};
use zeroize::Zeroize;

use crate::key::SmallKey;
use crate::object::{self, CryptClass, Iv};
use crate::permissions::Permissions;
use crate::standard::{self, Cipher, EncryptParams, PasswordEncoding, parse_encrypt_dict};

/// What went wrong building a security handler.
///
/// The C++ collapses every one of these into a single "password error" at the
/// parser boundary; splitting them changes no document's fate but lets a
/// caller tell "this needs a password" from "we cannot do this document's
/// cryptography".
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The password is neither the user nor the owner password.
    #[error("the supplied password is not the user or owner password")]
    WrongPassword,
    /// The operating system's cryptographic generator is unavailable, so no
    /// file key can be minted. Raised only when creating an encrypted file;
    /// opening one needs no randomness.
    #[error("the operating system's random generator is unavailable")]
    NoEntropy,
    /// `/Filter` names a handler other than `/Standard`. Public-key handlers
    /// (`/Adobe.PubSec`) land here.
    #[error("/Filter {0:?} is not the standard security handler")]
    UnsupportedHandler(Box<[u8]>),
    /// `[oracle-bug]` **Never constructed.** §7.6.5 table 20 makes `/StmF`
    /// and `/StrF` independent, so a pair naming different crypt filters is
    /// conformant and this crate resolves each class separately. The variant
    /// is kept because it is a public enum member and removing it is a
    /// breaking change no caller gains from; nothing produces it.
    // [oracle-bug] cpdf_security_handler.cpp:305 and :325 return false on a
    // differing pair.
    #[error("/StmF and /StrF name different crypt filters")]
    MismatchedCryptFilters,
    /// The named crypt filter is not a key in `/CF`. An **empty** name is
    /// §7.6.5's `/Identity` default rather than an error.
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

impl Drop for SecurityHandler {
    fn drop(&mut self) {
        if let Self::AesV5 { key, .. } = self {
            key.zeroize();
        }
    }
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
        // `[oracle-bug]`: the *string* class branches. §7.6.5 lets
        // `/StrF` resolve to `/Identity` beside an enciphering `/StmF`, and
        // such a document's strings are plaintext.
        if class == CryptClass::String && self.strings_identity() {
            return data.to_vec();
        }
        // `/EFF` is the one class that can genuinely name another cipher, and
        // does so by cipher only — §7.6.5 gives every `/CF` entry the same
        // file key.
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
        // `[oracle-bug]`: a pass-through string class, exactly as on the
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
    pub(crate) fn permission_word(&self, owner: bool) -> u32 {
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
                .is_some_and(|s| s.as_bytes() == names::SIG.as_bytes())
    })
}
