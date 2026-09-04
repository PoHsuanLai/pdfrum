//! The file identifier (ISO 32000-1 §14.4).
//!
//! `/ID` is a two-element array. The **first** element is the document's
//! permanent identity, minted once and preserved across every save the file
//! ever sees. The **second** changes on each save, so two files sharing an
//! `ID[0]` but differing in `ID[1]` are recognisably versions of one document.
//!
//! # Randomness is a parameter here, not a global
//!
//! The C++ draws both elements from a process-global Mersenne Twister, which
//! makes its output unreproducible. This workspace takes no global state and
//! a non-reproducible writer cannot be snapshot-tested, so the source is an
//! explicit [`IdSource`]: [`IdSource::Random`] by default (matching the C++'s
//! observable behavior — fresh bytes per save), [`IdSource::Fixed`] for tests
//! and for byte-reproducible output.
//!
//! # The five branches
//!
//! | the input had | `ID[0]` | `ID[1]` |
//! |---|---|---|
//! | an `/ID` with a first element | that element, preserved | fresh random |
//! | an `/ID`, incremental save, encrypted, with a second element | preserved | **preserved** |
//! | an `/ID` with no first element | fresh random | fresh random |
//! | no `/ID` at all | fresh random | **a copy of `ID[0]`** |
//! | no `/ID` at all, encrypted at revision 2 or 3 | fresh random | copy of `ID[0]`, **and the key is rebuilt** |
//!
//! Row 2 exists because R2/R3 key derivation mixes `ID[0]` in: changing `ID[1]`
//! on an incremental save would be harmless, but the C++ preserves it and a
//! file's already-written ciphertext is what makes that the safe choice.
//!
//! Row 5 is the interesting one. A document with no `/ID` that *is* encrypted
//! at revision 2 or 3 has just had a fresh `ID[0]` minted — and since R2/R3
//! derive the file key from `ID[0]`, the old key is no longer derivable. The
//! C++ answers by rebuilding the security handler from the new ID, which sets
//! `security_changed_`, which in turn **forces a full save**: appending
//! freshly-keyed objects after ciphertext under the old key would produce a
//! file no reader could open. That interlock is reported here as
//! [`FileId::rekeyed`] so the caller can honour it.

use std::hash::{BuildHasher, RandomState};

use pdfrum_object::{Array, Dict, Object, PdfString, names};

/// How many bytes each `/ID` element carries — 16, written as 32 hex digits.
const ID_LEN: usize = 16;

/// Where the bytes of a fresh `/ID` element come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IdSource {
    /// Fresh bytes per save, the way a real writer behaves.
    #[default]
    Random,
    /// A fixed seed, so the same input saves to the same bytes. Also seeds
    /// the six-letter subset tag, for the same reason.
    Fixed([u8; ID_LEN]),
}

impl IdSource {
    /// Sixty-four bytes for a new encryption's file key and salts: fresh
    /// from the OS for [`IdSource::Random`], a fixed derivation of the seed
    /// for [`IdSource::Fixed`], so a reproducible save stays reproducible.
    #[must_use]
    pub fn entropy(self) -> [u8; pdfrum_crypt::ENTROPY_LEN] {
        let mut out = [0u8; pdfrum_crypt::ENTROPY_LEN];
        for (i, chunk) in out.chunks_mut(8).enumerate() {
            let index = u64::try_from(i).unwrap_or(u64::MAX);
            let word = match self {
                Self::Random => RandomState::new().hash_one(index ^ 0x5EC7_E7A1),
                Self::Fixed(seed) => {
                    // A hash chain over the seed and the chunk index; the
                    // same seed always gives the same 64 bytes.
                    let mut h = 0xcbf2_9ce4_8422_2325u64;
                    for &b in seed.iter().chain(&index.to_le_bytes()) {
                        h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
                    }
                    h
                }
            };
            chunk.copy_from_slice(&word.to_le_bytes());
        }
        out
    }

    /// A sequence of `ID_LEN` bytes. `nonce` distinguishes the two elements
    /// of one array, so a fixed source still produces two different values
    /// where the rules call for two.
    fn bytes(self, nonce: u64) -> [u8; ID_LEN] {
        match self {
            Self::Fixed(seed) => mix(&seed, nonce),
            // `RandomState` is std's per-process randomness, seeded by the OS
            // — no global of ours, and no dependency added for it.
            Self::Random => {
                let a = RandomState::new().hash_one(nonce);
                let b = RandomState::new().hash_one(nonce.wrapping_add(0x9E37_79B9));
                let mut out = [0u8; ID_LEN];
                for (slot, byte) in out
                    .iter_mut()
                    .zip(a.to_le_bytes().into_iter().chain(b.to_le_bytes()))
                {
                    *slot = byte;
                }
                out
            }
        }
    }

    /// A deterministic byte for the subset tag's letter `index` (D1/D4:
    /// subsetting shares the save's seed so a fixed save is fully
    /// reproducible).
    #[must_use]
    pub fn tag_byte(self, index: u64) -> u8 {
        match self {
            Self::Fixed(seed) => mix(&seed, TAG_NONCE ^ index).first().copied().unwrap_or(0),
            Self::Random => {
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "any byte of the hash is as good as any other"
                )]
                {
                    RandomState::new().hash_one(index) as u8
                }
            }
        }
    }
}

/// Nonce separating subset-tag bytes from `/ID` elements drawn from the same
/// fixed seed.
const TAG_NONCE: u64 = 0x5375_6273_6574_0000;

/// Stir a seed with a nonce into `ID_LEN` bytes.
///
/// Not a cryptographic function and not meant to be: `/ID` needs to be
/// *distinct*, not unguessable, and the C++'s Mersenne Twister is no
/// stronger. What it *does* need is for the whole seed to reach every output
/// byte — absorbing the seed before emitting anything is what makes the first
/// byte differ between two seeds, which a per-byte mix would not (the first
/// output would then depend on one seed byte, and a six-letter subset tag is
/// six such bytes).
fn mix(seed: &[u8; ID_LEN], nonce: u64) -> [u8; ID_LEN] {
    // Absorb: every seed byte and the nonce go into the state first.
    let mut state = nonce ^ 0x243F_6A88_85A3_08D3;
    for byte in seed {
        state = state
            .wrapping_mul(0x5851_F42D_4C95_7F2D)
            .wrapping_add(u64::from(*byte).wrapping_add(1));
    }

    // Squeeze: one byte of output per round, from the high bits, which are
    // the ones a multiplicative step actually stirs.
    let mut out = [0u8; ID_LEN];
    for slot in &mut out {
        state = state
            .wrapping_mul(0x5851_F42D_4C95_7F2D)
            .wrapping_add(0x1405_7B7E_F767_814F);
        *slot = u8::try_from(state >> 56).unwrap_or(0);
    }
    out
}

/// The `/ID` array a save will write, and whether minting it invalidated the
/// document's encryption key.
#[derive(Debug, Clone, PartialEq)]
pub struct FileId {
    /// The two-element array, both elements hex strings of 32 digits.
    pub array: Array,
    /// Row 5 of the table above: the file key was rebuilt from a fresh `ID[0]`,
    /// so the original bytes can no longer be appended to and the save must
    /// be a full one.
    pub rekeyed: bool,
}

/// What the save needs to know about the document to decide `/ID`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct IdContext<'a> {
    /// The trailer's own `/ID`, if it had one.
    pub(crate) old: Option<&'a Array>,
    /// The `/Encrypt` dictionary, if the file declared one.
    pub(crate) encrypt: Option<&'a Dict>,
    /// Whether this save is an incremental one.
    pub(crate) incremental: bool,
}

/// Build the `/ID` array for one save.
pub(crate) fn build(ctx: IdContext<'_>, source: IdSource) -> FileId {
    let fresh = |nonce: u64| Object::Str(PdfString::hex(source.bytes(nonce)));

    let Some(old) = ctx.old else {
        // No `/ID` at all: both elements are the same fresh value, and the
        // R2/R3 rekey may follow.
        let first = fresh(0);
        return FileId {
            array: Array::of([first.clone(), first]),
            rekeyed: needs_rekey(ctx.encrypt),
        };
    };

    // ID[0] is the document's identity: preserved whenever it exists.
    let first = old
        .raw_at(0)
        .filter(|o| o.as_string().is_some())
        .cloned()
        .unwrap_or_else(|| fresh(0));

    let second = old.raw_at(1).filter(|o| o.as_string().is_some());
    // An incremental save of an encrypted document keeps ID[1], because the
    // ciphertext already in the file was keyed with it.
    if ctx.incremental
        && ctx.encrypt.is_some()
        && let Some(second) = second
    {
        return FileId {
            array: Array::of([first, second.clone()]),
            rekeyed: false,
        };
    }

    FileId {
        array: Array::of([first, fresh(1)]),
        rekeyed: false,
    }
}

/// Does minting a fresh `ID[0]` invalidate this handler's key?
///
/// Only for the standard handler at revision 2 or 3, whose key derivation
/// mixes the first `/ID` element in. Revision 4 and up derive from `/O` and
/// `/U` alone, so a new `/ID` costs them nothing.
fn needs_rekey(encrypt: Option<&Dict>) -> bool {
    let Some(dict) = encrypt else {
        return false;
    };
    let revision = dict.direct_int(names::R).unwrap_or(0);
    (revision == 2 || revision == 3) && dict.name(names::FILTER) == Some(names::STANDARD)
}

#[cfg(test)]
mod tests {
    use super::{FileId, IdContext, IdSource, build};
    use pdfrum_object::{Array, Dict, Object, PdfString, names};

    fn hex(s: &str) -> Object {
        Object::Str(PdfString::hex(s.as_bytes()))
    }

    fn ctx<'a>(
        old: Option<&'a Array>,
        encrypt: Option<&'a Dict>,
        incremental: bool,
    ) -> IdContext<'a> {
        IdContext {
            old,
            encrypt,
            incremental,
        }
    }

    fn seed() -> IdSource {
        IdSource::Fixed([7u8; 16])
    }

    fn elements(id: &FileId) -> (Vec<u8>, Vec<u8>) {
        let get = |i: usize| {
            id.array
                .string_at(i)
                .map(|s| s.bytes.to_vec())
                .unwrap_or_default()
        };
        (get(0), get(1))
    }

    // Bug873: ID[0] is the document's permanent identity.
    #[test]
    fn the_first_element_is_preserved_when_the_file_had_one() {
        let old = Array::of([hex("keepme"), hex("changeme")]);
        let id = build(ctx(Some(&old), None, false), seed());
        let (first, second) = elements(&id);
        assert_eq!(first, b"keepme");
        assert_ne!(second, b"changeme", "the second element is regenerated");
        assert_eq!(second.len(), 16);
        assert!(!id.rekeyed);
    }

    #[test]
    fn a_document_with_no_id_gets_two_identical_elements() {
        let id = build(ctx(None, None, false), seed());
        let (first, second) = elements(&id);
        assert_eq!(first, second);
        assert_eq!(first.len(), 16);
        assert!(!id.rekeyed);
    }

    // The one case that preserves ID[1]: the ciphertext already written was
    // keyed with it.
    #[test]
    fn an_incremental_encrypted_save_keeps_the_second_element() {
        let old = Array::of([hex("keepme"), hex("alsokeep")]);
        let encrypt = Dict::from_pairs([(names::R.clone(), Object::Int(4))]);
        let id = build(ctx(Some(&old), Some(&encrypt), true), seed());
        let (first, second) = elements(&id);
        assert_eq!(first, b"keepme");
        assert_eq!(second, b"alsokeep");
    }

    #[test]
    fn a_full_encrypted_save_still_regenerates_the_second_element() {
        let old = Array::of([hex("keepme"), hex("changeme")]);
        let encrypt = Dict::from_pairs([(names::R.clone(), Object::Int(4))]);
        let id = build(ctx(Some(&old), Some(&encrypt), false), seed());
        assert_ne!(elements(&id).1, b"changeme");
    }

    // Row 5: no /ID plus R2/R3 standard handler means the key is gone.
    #[test]
    fn no_id_plus_revision_three_forces_a_rekey() {
        for revision in [2i64, 3] {
            let encrypt = Dict::from_pairs([
                (names::R.clone(), Object::Int(revision)),
                (names::FILTER.clone(), Object::Name(names::STANDARD.clone())),
            ]);
            let id = build(ctx(None, Some(&encrypt), false), seed());
            assert!(id.rekeyed, "revision {revision} derives its key from /ID");
        }
    }

    #[test]
    fn revision_four_and_up_survive_a_fresh_id() {
        for revision in [4i64, 5, 6] {
            let encrypt = Dict::from_pairs([
                (names::R.clone(), Object::Int(revision)),
                (names::FILTER.clone(), Object::Name(names::STANDARD.clone())),
            ]);
            assert!(!build(ctx(None, Some(&encrypt), false), seed()).rekeyed);
        }
    }

    // A non-standard handler is not rekeyed however low its revision reads.
    #[test]
    fn a_non_standard_handler_is_never_rekeyed() {
        let encrypt = Dict::from_pairs([
            (names::R.clone(), Object::Int(2)),
            (
                names::FILTER.clone(),
                Object::Name(pdfrum_object::Name::from("Custom")),
            ),
        ]);
        assert!(!build(ctx(None, Some(&encrypt), false), seed()).rekeyed);
    }

    // Determinism is what makes snapshot tests of whole files possible.
    #[test]
    fn a_fixed_source_produces_the_same_id_every_time() {
        let a = build(ctx(None, None, false), seed());
        let b = build(ctx(None, None, false), seed());
        assert_eq!(a, b);
    }

    #[test]
    fn different_seeds_produce_different_ids() {
        let a = build(ctx(None, None, false), IdSource::Fixed([1u8; 16]));
        let b = build(ctx(None, None, false), IdSource::Fixed([2u8; 16]));
        assert_ne!(a, b);
    }

    // Both elements of an array drawn from one seed must differ, or a save
    // would claim ID[0] == ID[1] where the rules say otherwise.
    #[test]
    fn the_two_elements_of_one_array_differ() {
        let old = Array::of([hex("keepme")]);
        let id = build(ctx(Some(&old), None, false), seed());
        let (first, second) = elements(&id);
        assert_ne!(first, second);
    }

    // A random source really is random.
    #[test]
    fn a_random_source_differs_between_saves() {
        let a = build(ctx(None, None, false), IdSource::Random);
        let b = build(ctx(None, None, false), IdSource::Random);
        assert_ne!(a, b);
    }

    // The array's elements are written hex, so the trailer reads
    // `/ID[<32 hex><32 hex>]` — 16 bytes each, 32 digits each.
    #[test]
    fn elements_are_sixteen_bytes_spelled_as_hex() {
        let id = build(ctx(None, None, false), seed());
        for i in 0..2 {
            let s = id.array.string_at(i).expect("a string");
            assert!(s.hex, "the trailer spells /ID in hex");
            assert_eq!(s.bytes.len(), 16);
        }
    }

    // A non-string first element is not an identity worth keeping.
    #[test]
    fn a_junk_first_element_is_replaced() {
        let old = Array::of([Object::Int(5), hex("second")]);
        let id = build(ctx(Some(&old), None, false), seed());
        assert_eq!(elements(&id).0.len(), 16);
    }

    #[test]
    fn tag_bytes_are_stable_under_a_fixed_seed() {
        let s = seed();
        let first: Vec<u8> = (0..6).map(|i| s.tag_byte(i)).collect();
        let again: Vec<u8> = (0..6).map(|i| s.tag_byte(i)).collect();
        assert_eq!(first, again);
    }

    // The whole seed must reach every output byte. An earlier mix folded
    // seed byte *i* into output byte *i* only, so two seeds differing past
    // the sixth byte produced identical six-letter subset tags — two fonts
    // with the same tag in one file.
    #[test]
    fn every_seed_byte_reaches_every_output_byte() {
        let base = [0u8; 16];
        let tag_of = |s: IdSource| -> Vec<u8> { (0..6).map(|i| s.tag_byte(i)).collect() };
        let reference = tag_of(IdSource::Fixed(base));

        for position in 0..16 {
            let mut altered = base;
            if let Some(slot) = altered.get_mut(position) {
                *slot = 0xFF;
            }
            assert_ne!(
                tag_of(IdSource::Fixed(altered)),
                reference,
                "changing seed byte {position} must change the tag"
            );
        }
    }

    // The same, for /ID: a seed differing anywhere gives a different array.
    #[test]
    fn every_seed_byte_reaches_the_id() {
        let base = [0u8; 16];
        let reference = build(ctx(None, None, false), IdSource::Fixed(base));
        for position in 0..16 {
            let mut altered = base;
            if let Some(slot) = altered.get_mut(position) {
                *slot = 0xFF;
            }
            assert_ne!(
                build(ctx(None, None, false), IdSource::Fixed(altered)),
                reference,
                "changing seed byte {position} must change /ID"
            );
        }
    }
}
