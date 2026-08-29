//! The output-side security seam.
//!
//! SPEC.md §11's 2026-08-29 ruling E3: **v1 saves encrypted documents
//! decrypted.** Objects arrive here already plaintext (the parser peeled the
//! cipher off at fetch time), the `/Encrypt` dictionary is suppressed from the
//! trailer, and nothing is re-enciphered — `remove_security` semantics,
//! applied unconditionally.
//!
//! This type exists anyway, threaded through every writer as
//! `Option<&Encryptor>`, because it is where the two mandatory exemptions live
//! (a signature's `/Contents`, an XMP metadata stream) and because those
//! exemptions must be *visible in the code that writes* rather than discovered
//! when preserve-encryption lands. Today every construction site passes
//! `None`; the day `pdfrum-crypt` grows an encrypt side, only
//! [`Encryptor::encrypt`] and the trailer's `/Encrypt` branch change.

/// Enciphers the strings and stream payloads of one indirect object.
///
/// Per ISO 32000-1 §7.6.2 the key is derived per object from its number and
/// generation; the generation is always 0 here because that is what the
/// writer emits (§1.9).
#[derive(Debug, Clone, Copy)]
pub struct Encryptor {
    #[expect(
        dead_code,
        reason = "carried for the preserve-encryption path deferred past M7 (SPEC §11 E3)"
    )]
    object_number: u32,
}

impl Encryptor {
    /// An encryptor for the object numbered `num`.
    #[must_use]
    pub const fn new(object_number: u32) -> Self {
        Self { object_number }
    }

    /// Encipher one run of bytes.
    ///
    /// v1 has no encrypt side, so this is identity; see the module docs. It
    /// takes `&self` because the preserve-encryption path will need the
    /// object number and the file key, and every call site is already written
    /// as though it does.
    #[expect(
        clippy::unused_self,
        clippy::trivially_copy_pass_by_ref,
        reason = "the seam is the point: see the module docs. Taking `&self` \
                  keeps every call site written the way the \
                  preserve-encryption path will need it."
    )]
    #[must_use]
    pub fn encrypt(&self, data: &[u8]) -> Vec<u8> {
        data.to_vec()
    }
}
