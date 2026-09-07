//! The output-side security seam.
//!
//! An encrypted document saves **encrypted**, under the handler and file key
//! the original password opened it with, so the saved file opens with that
//! same password. Objects arrive here plaintext — the parser peeled the
//! cipher off at fetch time — and this is where it goes back on.
//!
//! [`SaveOptions::remove_security`](crate::SaveOptions::remove_security)
//! remains the explicit way to drop it: it suppresses `/Encrypt` from the
//! trailer and passes `None` here, so nothing is enciphered.
//!
//! # Three things are never enciphered
//!
//! - **The `/Encrypt` dictionary itself.** ISO 32000-1 §7.6.1: a reader has
//!   to read `/O`, `/U` and `/Perms` before it has a key. The C++ tests for it
//!   by pointer identity against the object it is writing; we test by object
//!   number, which is the same test once the dictionary has one.
//! - **A signature's `/Contents`.** It covers a byte range of the finished
//!   file, so re-enciphering it would invalidate the signature.
//! - **An XMP metadata stream's payload.** ISO 32000-1 §14.3.2 requires the
//!   packet be readable by a consumer holding neither the file key nor a
//!   flate decoder. Note this is unconditional in the C++ writer — it never
//!   consults `/EncryptMetadata`, so a document declaring `true` still gets a
//!   plaintext metadata stream on save. The stream's *dictionary* is still
//!   enciphered; only the payload is exempt.
//!
//! The trailer is a fourth, but it never reaches this type: it is not an
//! indirect object, and `write_classic` passes `None` outright.
//!
//! # Where the initialisation vectors come from
//!
//! AES-CBC needs a vector per payload, and under AESV3 (ISO 32000-2 §7.6.5.3)
//! the file key enciphers every object verbatim, with no per-object
//! derivation. The vector is therefore the only thing separating two
//! ciphertexts under one key, and it has to satisfy both of CBC's
//! requirements:
//!
//! - **Unique within a save.** Two payloads sharing a key and a vector leak
//!   the XOR of their first blocks to anyone holding the ciphertext, key or
//!   no key. [`IvSource`] mixes a per-save secret with the object number and
//!   a counter, so each payload gets its own vector however the writer
//!   enumerates objects.
//! - **Unpredictable.** The per-save secret is 32 bytes drawn from the
//!   operating system, so a vector cannot be recomputed from anything the
//!   file itself exposes — its length, its head, its tail, or a sibling file
//!   built from the same template.
//!
//! An encrypted save is therefore not byte-reproducible: reproducible
//! ciphertext is reproducible secrets. [`crate::IdSource::Fixed`] pins
//! everything a save writes in the clear, which is what reproducibility is
//! for.

use std::cell::Cell;

use pdfrum_crypt::{CryptClass, Iv, SecurityHandler};
use pdfrum_object::ObjRef;
use sha2::{Digest, Sha256};

use crate::Error;

/// Where a save's AES initialisation vectors come from.
///
/// Deliberately neither `Copy` nor `Clone`: it holds a counter, and a copy
/// would hand two call sites the same vector sequence. Its `Debug` shows the
/// counter and withholds the secret, which a log has no use for.
pub struct IvSource {
    /// Thirty-two bytes from the operating system, drawn once per save. The
    /// vectors are unpredictable exactly because this is.
    secret: [u8; 32],
    /// Advanced once per vector handed out.
    counter: Cell<u64>,
}

impl std::fmt::Debug for IvSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IvSource")
            .field("counter", &self.counter.get())
            .finish_non_exhaustive()
    }
}

impl IvSource {
    /// A source keyed by fresh operating-system randomness.
    ///
    /// # Errors
    ///
    /// [`Error::NoEntropy`] when the platform's generator is unavailable.
    pub fn from_os() -> Result<Self, Error> {
        let mut secret = [0u8; 32];
        getrandom::fill(&mut secret).map_err(|_| Error::NoEntropy)?;
        Ok(Self {
            secret,
            counter: Cell::new(0),
        })
    }

    /// The next vector, advancing the counter.
    ///
    /// `obj` goes into the mix as well as the counter, so two saves that
    /// enumerate objects in different orders still give each object its own
    /// vector rather than one that depends on its position.
    fn next(&self, obj: ObjRef) -> Iv {
        let index = self.counter.get();
        self.counter.set(index.wrapping_add(1));

        // SHA-256 over the secret and the pair that makes this payload
        // unique. Absorbing before emitting is what carries every input into
        // every output byte; the vector is the leading half of the digest.
        let mut hash = Sha256::new();
        hash.update(self.secret);
        hash.update(index.to_le_bytes());
        hash.update(obj.num.to_le_bytes());
        hash.update(obj.generation.to_le_bytes());
        let digest = hash.finalize();
        let mut out = [0u8; 16];
        for (slot, byte) in out.iter_mut().zip(digest) {
            *slot = byte;
        }
        Iv(out)
    }
}

/// Enciphers the strings and stream payloads of one indirect object.
///
/// Per ISO 32000-1 §7.6.2 the key is derived per object from its number and
/// generation; the generation is always 0 here because that is what the
/// writer emits (§1.9), which is also what the C++ passes.
///
/// Borrowed rather than owned so one handler and one vector source serve a
/// whole save, with only the object number changing per object.
#[derive(Debug, Clone, Copy)]
pub struct Encryptor<'a> {
    handler: &'a SecurityHandler,
    ivs: &'a IvSource,
    object_number: u32,
}

impl<'a> Encryptor<'a> {
    /// An encryptor for the object numbered `num`.
    #[must_use]
    pub const fn new(handler: &'a SecurityHandler, ivs: &'a IvSource, object_number: u32) -> Self {
        Self {
            handler,
            ivs,
            object_number,
        }
    }

    /// Whether this document's metadata stream is enciphered along with
    /// everything else (`/EncryptMetadata`, default true).
    ///
    /// Read by the stream encoder, which owns the exemption; see its docs for
    /// why we consult the flag where the C++ writer does not.
    #[must_use]
    pub fn encrypts_metadata(&self) -> bool {
        self.handler.encrypt_metadata()
    }

    /// Encipher one run of bytes as a string or a stream payload.
    ///
    /// The class never changes the result — PDFium refuses a document whose
    /// `/StmF` and `/StrF` differ — but naming it keeps the call site honest
    /// about what it is writing.
    #[must_use]
    pub fn encrypt(&self, class: CryptClass, data: &[u8]) -> Vec<u8> {
        let obj = ObjRef::new(self.object_number, 0);
        self.handler.encrypt(obj, class, self.ivs.next(obj), data)
    }
}

/// Everything a save needs to re-encipher what it writes.
///
/// Held by [`crate::save`] for the length of one save and handed to each
/// object in turn. `None` for an unencrypted document and for a
/// `remove_security` save, which is what makes "pass `None` and nothing is
/// enciphered" the whole of the plaintext path.
#[derive(Debug)]
pub(crate) struct Security<'a> {
    /// The handler the original password opened the document with.
    pub(crate) handler: &'a SecurityHandler,
    /// The vector source for this save.
    pub(crate) ivs: IvSource,
    /// The object number the `/Encrypt` dictionary will be written as — the
    /// one object that is never enciphered.
    pub(crate) encrypt_object: Option<u32>,
}

impl Security<'_> {
    /// The encryptor for object `num`, or `None` when that object is the
    /// `/Encrypt` dictionary.
    #[must_use]
    pub(crate) fn for_object(&self, num: u32) -> Option<Encryptor<'_>> {
        if self.encrypt_object == Some(num) {
            return None;
        }
        Some(Encryptor::new(self.handler, &self.ivs, num))
    }
}

#[cfg(test)]
mod tests {
    use super::{Encryptor, IvSource, Security};
    use pdfrum_crypt::{CryptClass, SecurityHandler};
    use pdfrum_object::ObjRef;

    fn handler() -> SecurityHandler {
        SecurityHandler::AesV5 {
            key: Box::new([0x42; 32]),
            revision: 6,
            permissions: 0xFFFF_FFFC,
            owner_unlocked: false,
            encrypt_metadata: true,
            encoding: pdfrum_crypt::PasswordEncoding::AsGiven,
            // No /EFF, so the embedded class takes the stream cipher —
            // ISO 32000-1 §7.6.5 table 20's own default.
            embedded_cipher: None,
            strings_identity: false,
        }
    }

    // The property CBC actually needs: no two payloads in one save share a
    // vector, whether they belong to one object or to many.
    #[test]
    fn vectors_never_repeat_within_one_save() {
        let ivs = IvSource::from_os().unwrap();
        let mut seen = std::collections::BTreeSet::new();
        for num in 1..40u32 {
            for _ in 0..4 {
                assert!(seen.insert(ivs.next(ObjRef::new(num, 0)).0), "{num}");
            }
        }
        assert_eq!(seen.len(), 39 * 4);
    }

    // Unpredictability: two sources drawn from the OS share nothing, so a
    // vector cannot be recovered from another save of the same document.
    #[test]
    fn two_sources_never_agree() {
        let a = IvSource::from_os().unwrap();
        let b = IvSource::from_os().unwrap();
        for num in 1..8u32 {
            assert_ne!(a.next(ObjRef::new(num, 0)), b.next(ObjRef::new(num, 0)));
        }
    }

    // The round trip through the encryptor's own object numbering.
    #[test]
    fn an_encryptor_round_trips_under_its_object_number() {
        let h = handler();
        let ivs = IvSource::from_os().unwrap();
        let enc = Encryptor::new(&h, &ivs, 12);
        let payload = b"the quick brown fox".to_vec();
        let sealed = enc.encrypt(CryptClass::String, &payload);
        assert_ne!(sealed, payload);
        assert_eq!(
            h.decrypt(ObjRef::new(12, 0), CryptClass::String, &sealed),
            payload
        );
    }

    // The encrypt dictionary is the one object that gets no encryptor.
    #[test]
    fn the_encrypt_dictionary_is_never_given_an_encryptor() {
        let h = handler();
        let security = Security {
            handler: &h,
            ivs: IvSource::from_os().unwrap(),
            encrypt_object: Some(9),
        };
        assert!(security.for_object(9).is_none());
        assert!(security.for_object(8).is_some());
        assert!(security.for_object(10).is_some());

        // With no encrypt object named, every object gets one.
        let security = Security {
            handler: &h,
            ivs: IvSource::from_os().unwrap(),
            encrypt_object: None,
        };
        assert!(security.for_object(9).is_some());
    }

    // An unencrypted document's seam is the identity, which is what makes
    // "pass `None`" and "pass an Identity handler" agree.
    #[test]
    fn the_identity_handler_writes_what_it_was_given() {
        let h = SecurityHandler::Identity;
        let ivs = IvSource::from_os().unwrap();
        let enc = Encryptor::new(&h, &ivs, 4);
        let payload = vec![0xABu8; 33];
        assert_eq!(enc.encrypt(CryptClass::Stream, &payload), payload);
    }
}
