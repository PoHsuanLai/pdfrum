//! The output-side security seam.
//!
//! An encrypted document saves **encrypted**, under the handler and file key
//! the original password opened it with, so the saved file opens with that
//! same password (SPEC.md §11's M10 ruling, superseding the 2026-08-29 ruling
//! E3 that v1 would save decrypted). Objects arrive here plaintext — the
//! parser peeled the cipher off at fetch time — and this is where it goes
//! back on.
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
//! AES needs a fresh vector per payload and `pdfrum-crypt` has none to give —
//! no global randomness (STYLE.md §1), no `getrandom` dependency (DEPS.md).
//! So the writer supplies them, from [`IvSource`]: a counter stirred with a
//! seed taken from the document's own bytes. Two consequences, both wanted:
//!
//! - **A save is reproducible.** The same document saved twice produces the
//!   same ciphertext, which is what lets a whole encrypted file be
//!   snapshot-tested. The C++ cannot be: its vectors come from a
//!   process-global Mersenne Twister seeded off the environment.
//! - **Vectors do not repeat within a file.** Each payload advances the
//!   counter, so no two objects share one — which is the property that
//!   actually matters for CBC. They are *unpredictable to an attacker* only
//!   as far as the file's own bytes are, which for a document being
//!   re-encrypted under a key the attacker would need anyway is the honest
//!   bar. A caller wanting more passes [`IvSource::from_seed`] with bytes of
//!   its own choosing.

use std::cell::Cell;

use pdfrum_crypt::{CryptClass, Iv, SecurityHandler};
use pdfrum_object::ObjRef;

/// Where a save's AES initialisation vectors come from.
///
/// Deliberately not `Copy`: it holds a counter, and a copy would hand two
/// call sites the same vector sequence.
#[derive(Debug)]
pub struct IvSource {
    /// Absorbed from the document's bytes; see the module docs.
    seed: u64,
    /// Advanced once per vector handed out.
    counter: Cell<u64>,
}

impl IvSource {
    /// A source seeded from the document's own bytes.
    ///
    /// The whole file is not hashed — a save must not become linear in the
    /// input a second time — so the seed absorbs the length and a sample of
    /// the head and tail. That is enough to distinguish documents; it is not
    /// meant to be unguessable, and the module docs say why.
    #[must_use]
    pub fn from_document(bytes: &[u8]) -> Self {
        const SAMPLE: usize = 64;
        let head = bytes.get(..SAMPLE.min(bytes.len())).unwrap_or_default();
        let tail = bytes
            .get(bytes.len().saturating_sub(SAMPLE)..)
            .unwrap_or_default();
        let mut seed = bytes.len() as u64;
        for byte in head.iter().chain(tail) {
            seed = seed
                .wrapping_mul(0x5851_F42D_4C95_7F2D)
                .wrapping_add(u64::from(*byte).wrapping_add(1));
        }
        Self::from_seed(seed)
    }

    /// A source from a caller's own seed, for a save that must be pinned to
    /// exact bytes.
    #[must_use]
    pub const fn from_seed(seed: u64) -> Self {
        Self {
            seed,
            counter: Cell::new(0),
        }
    }

    /// The next vector, advancing the counter.
    ///
    /// `obj` goes into the mix as well as the counter, so two saves that
    /// enumerate objects in different orders still give each object its own
    /// vector rather than one that depends on its position.
    fn next(&self, obj: ObjRef) -> Iv {
        let index = self.counter.get();
        self.counter.set(index.wrapping_add(1));

        // Absorb, then squeeze one byte per round from the high bits — the
        // same shape `write::id::mix` uses, and for the same reason: every
        // input must reach every output byte.
        let mut state = self
            .seed
            .wrapping_add(index)
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            ^ u64::from(obj.num).rotate_left(32);
        let mut out = [0u8; 16];
        for slot in &mut out {
            state = state
                .wrapping_mul(0x5851_F42D_4C95_7F2D)
                .wrapping_add(0x1405_7B7E_F767_814F);
            *slot = u8::try_from(state >> 56).unwrap_or(0);
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
        let ivs = IvSource::from_seed(1);
        let mut seen = std::collections::BTreeSet::new();
        for num in 1..40u32 {
            for _ in 0..4 {
                assert!(seen.insert(ivs.next(ObjRef::new(num, 0)).0), "{num}");
            }
        }
        assert_eq!(seen.len(), 39 * 4);
    }

    // Reproducibility: two sources built the same way agree step for step.
    #[test]
    fn one_seed_gives_one_sequence() {
        let a = IvSource::from_seed(7);
        let b = IvSource::from_seed(7);
        for num in 1..8u32 {
            assert_eq!(a.next(ObjRef::new(num, 0)), b.next(ObjRef::new(num, 0)));
        }
        // A different seed does not.
        let c = IvSource::from_seed(8);
        assert_ne!(a.next(ObjRef::new(1, 0)), c.next(ObjRef::new(1, 0)));
    }

    // Two documents differing anywhere the sample reaches get different
    // vectors, so a save never reuses another file's sequence.
    #[test]
    fn different_documents_seed_differently() {
        let a = IvSource::from_document(b"%PDF-1.7 hello");
        let b = IvSource::from_document(b"%PDF-1.7 world");
        assert_ne!(a.next(ObjRef::new(1, 0)), b.next(ObjRef::new(1, 0)));
        // And length alone is enough to separate two otherwise-equal heads.
        let c = IvSource::from_document(b"%PDF-1.7 hello!");
        assert_ne!(
            IvSource::from_document(b"%PDF-1.7 hello").next(ObjRef::new(1, 0)),
            c.next(ObjRef::new(1, 0))
        );
        // An empty document is a seed like any other, not a panic.
        let _ = IvSource::from_document(b"").next(ObjRef::new(1, 0));
    }

    // The round trip through the encryptor's own object numbering.
    #[test]
    fn an_encryptor_round_trips_under_its_object_number() {
        let h = handler();
        let ivs = IvSource::from_seed(3);
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
            ivs: IvSource::from_seed(1),
            encrypt_object: Some(9),
        };
        assert!(security.for_object(9).is_none());
        assert!(security.for_object(8).is_some());
        assert!(security.for_object(10).is_some());

        // With no encrypt object named, every object gets one.
        let security = Security {
            handler: &h,
            ivs: IvSource::from_seed(1),
            encrypt_object: None,
        };
        assert!(security.for_object(9).is_some());
    }

    // An unencrypted document's seam is the identity, which is what makes
    // "pass `None`" and "pass an Identity handler" agree.
    #[test]
    fn the_identity_handler_writes_what_it_was_given() {
        let h = SecurityHandler::Identity;
        let ivs = IvSource::from_seed(5);
        let enc = Encryptor::new(&h, &ivs, 4);
        let payload = vec![0xABu8; 33];
        assert_eq!(enc.encrypt(CryptClass::Stream, &payload), payload);
    }
}
