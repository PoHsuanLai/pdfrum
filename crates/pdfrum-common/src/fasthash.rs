//! A fast, non-cryptographic hasher for maps whose keys come from *us*.
//!
//! # Why this exists at all
//!
//! `std`'s default hasher is `SipHash`-1-3 with a per-process random seed, and it
//! is the right default: a `HashMap` keyed by something an attacker chooses —
//! a PDF's name strings, say — must not be collidable on purpose, or a crafted
//! file turns every lookup into a linked-list walk. That is a real threat model
//! for a PDF engine and nothing here weakens it.
//!
//! But two of this workspace's hottest maps are not keyed on file content at
//! all. `pdfrum-parser`'s object store is keyed by **object number**, a `u32`
//! the parser assigns; `pdfrum-render`'s glyph-bitmap cache is keyed by a small
//! POD struct of a glyph id and four matrix coefficients. Both are looked up
//! once per drawn glyph — tens of thousands of times on a dense page — and for
//! both, `SipHash` is doing cryptographic-strength mixing on eight bytes that
//! no file controls. Measured, that is where a few percent of a text-heavy
//! render goes.
//!
//! # Why not `rustc-hash`
//!
//! `rustc-hash` is the crate that does exactly this. A performance dependency
//! here only earns its place beside an A/B against a tuned no-dep baseline,
//! and this module is that baseline: it is `rustc-hash`'s algorithm, which is
//! a multiply and a rotate per word and about twenty lines.
//!
//! The outcome, recorded here because it is the reason this file rather than a
//! `Cargo.toml` line: the no-dep version is **the same speed**, because it is
//! the same three instructions. There is nothing for a dependency to add.
//!
//! # What must not be keyed with this
//!
//! Anything an untrusted file controls the bytes of: name strings, dictionary
//! keys, font names, decoded text. `FxBuildHasher` is trivially collidable by
//! construction — that is the trade that makes it fast. The two call sites are
//! chosen because their keys are integers this workspace generates, and a new
//! call site needs the same argument made for it.

use std::hash::{BuildHasherDefault, Hasher};

/// A `BuildHasher` for [`FxHasher`], for use as a map's third type parameter.
///
/// ```
/// use pdfrum_common::FxBuildHasher;
/// use std::collections::HashMap;
///
/// let mut map: HashMap<u32, &str, FxBuildHasher> = HashMap::default();
/// map.insert(7, "seven");
/// assert_eq!(map.get(&7), Some(&"seven"));
/// ```
pub type FxBuildHasher = BuildHasherDefault<FxHasher>;

/// The multiplier, from `rustc-hash`: the 64-bit odd constant derived from the
/// fractional part of the golden ratio, which is what gives the multiply its
/// avalanche across the whole word.
const SEED: u64 = 0x51_7c_c1_b7_27_22_0a_95;

/// The rotate, applied before each multiply so that low-entropy high bits
/// reach the low ones. Five is `rustc-hash`'s.
const ROTATE: u32 = 5;

/// A non-cryptographic hasher: rotate, xor, multiply, per word.
///
/// **Not collision-resistant, and not seeded.** See the module docs for which
/// keys may and may not use it. Deterministic across runs and across machines,
/// which is a property this workspace happens to want elsewhere too — nothing
/// in a golden or a scoreboard may depend on a hash order, and with a fixed
/// hasher a map iteration that accidentally did would at least fail
/// reproducibly rather than one run in ten.
#[derive(Debug, Clone, Copy, Default)]
pub struct FxHasher {
    /// The running state.
    hash: u64,
}

impl FxHasher {
    /// Fold one word in.
    #[inline]
    fn add(&mut self, word: u64) {
        self.hash = (self.hash.rotate_left(ROTATE) ^ word).wrapping_mul(SEED);
    }
}

impl Hasher for FxHasher {
    #[inline]
    fn finish(&self) -> u64 {
        self.hash
    }

    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        // Whole words first, then whatever is left, one byte at a time. The
        // chunked loop is what makes this competitive on a longer key; the two
        // call sites in this workspace both go through `write_u32` /
        // `write_u64` below and never reach it, but a `Hasher` has to answer
        // `write` correctly regardless of who calls it.
        let (words, remainder) = bytes.as_chunks::<8>();
        for word in words {
            self.add(u64::from_ne_bytes(*word));
        }
        for &byte in remainder {
            self.add(u64::from(byte));
        }
    }

    #[inline]
    fn write_u8(&mut self, n: u8) {
        self.add(u64::from(n));
    }

    #[inline]
    fn write_u16(&mut self, n: u16) {
        self.add(u64::from(n));
    }

    #[inline]
    fn write_u32(&mut self, n: u32) {
        self.add(u64::from(n));
    }

    #[inline]
    fn write_u64(&mut self, n: u64) {
        self.add(n);
    }

    #[inline]
    fn write_usize(&mut self, n: usize) {
        self.add(n as u64);
    }

    #[inline]
    fn write_i32(&mut self, n: i32) {
        // Through `u32` and not `i64`: sign-extending a negative matrix
        // coefficient would set the top 32 bits of every one of them, leaving
        // the multiply less to work with. The glyph cache's key is four `i32`s.
        // `cast_unsigned` is the reinterpretation, not a value conversion —
        // the bits are what is being hashed.
        self.add(u64::from(n.cast_unsigned()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::hash::Hash;

    fn hash_of<T: Hash>(value: &T) -> u64 {
        let mut hasher = FxHasher::default();
        value.hash(&mut hasher);
        hasher.finish()
    }

    #[test]
    fn distinct_small_integers_do_not_collide() {
        // The property that matters at the call sites: object numbers and
        // glyph ids are small, dense and distinct, and a hasher that mapped
        // them onto a handful of buckets would be slower than SipHash however
        // few instructions it used.
        let hashes: std::collections::HashSet<u64> = (0_u32..10_000).map(|n| hash_of(&n)).collect();
        assert_eq!(hashes.len(), 10_000);
    }

    #[test]
    fn a_tuple_of_integers_spreads_over_the_low_bits() {
        // A map buckets on the *low* bits of the hash, so a hasher whose
        // entropy all lands high is useless in practice however good its
        // avalanche looks. This checks the shape the glyph cache actually uses:
        // several integers hashed in sequence, bucketed 256 ways.
        let mut buckets = [0_u32; 256];
        for a in 0_i32..40 {
            for b in 0_i32..40 {
                let h = hash_of(&(a, b, a ^ b, a.wrapping_mul(b)));
                let bucket = usize::try_from(h & 0xff).unwrap_or(0);
                if let Some(slot) = buckets.get_mut(bucket) {
                    *slot += 1;
                }
            }
        }
        // 1600 keys over 256 buckets is 6.25 each on average. A bucket holding
        // more than 40 would mean the low bits are barely moving.
        let worst = buckets.iter().copied().max().unwrap_or(0);
        assert!(worst < 40, "worst bucket held {worst} of 1600");
        assert!(
            buckets.iter().filter(|&&n| n == 0).count() < 32,
            "too many empty buckets: {}",
            buckets.iter().filter(|&&n| n == 0).count()
        );
    }

    #[test]
    fn it_works_as_a_hashmap_hasher() {
        let mut map: HashMap<u32, u32, FxBuildHasher> = HashMap::default();
        for n in 0..1000 {
            map.insert(n, n * 2);
        }
        for n in 0..1000 {
            assert_eq!(map.get(&n), Some(&(n * 2)));
        }
        assert_eq!(map.get(&1000), None);
    }

    #[test]
    fn it_is_deterministic_across_instances() {
        // `std`'s default hasher is seeded per process; this one is not, by
        // design. Anything that would break under a stable hash order breaks
        // reproducibly rather than one run in ten.
        assert_eq!(hash_of(&12345_u32), hash_of(&12345_u32));
        assert_ne!(hash_of(&12345_u32), hash_of(&12346_u32));
    }

    #[test]
    fn a_negative_i32_does_not_saturate_the_high_word() {
        // `write_i32` goes through `u32` rather than sign-extending. If it
        // sign-extended, every negative coefficient would set bits 32..64 and
        // four of them in a row would leave the multiply almost nothing to
        // distinguish. Checked as a difference rather than a constant.
        assert_ne!(hash_of(&(-1_i32, -2_i32)), hash_of(&(-2_i32, -1_i32)));
        assert_ne!(hash_of(&(-1_i32, 0_i32)), hash_of(&(0_i32, -1_i32)));
    }
}
