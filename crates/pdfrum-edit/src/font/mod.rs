//! Font subsetting (ISO 32000-1 §9.9), and the re-keying it forces.
//!
//! # The whole design follows from one fact about the subsetter
//!
//! `HarfBuzz`, which PDFium uses, has a `RETAIN_GIDS` mode: glyph IDs survive
//! subsetting unchanged, so `/Widths`, `/W`, the encoding CMap and
//! `/ToUnicode` all stay valid without being touched. The `subsetter` crate
//! has no such mode — it **always** produces a contiguous glyph space
//! starting at 0 with `.notdef` first, and it removes `cmap` unconditionally
//! ("CID fonts in PDF define their own cmaps").
//!
//! Everything else here is a consequence:
//!
//! 1. **Only CID fonts are subsetted.** A simple TrueType font maps codes to
//!    glyphs *through its own `cmap`*, which the subsetter removes — so a
//!    subsetted simple font would render nothing. `/FontFile` (Type 1) and
//!    `/FontFile3` are skipped as well, matching the C++.
//! 2. **`/W` is re-keyed** from old CID to new GID.
//! 3. **`/ToUnicode` is re-keyed** the same way, because under Identity-H the
//!    code *is* the CID.
//! 4. **Content-stream char codes change**, which makes subsetting
//!    inseparable from regenerating the content of every page using a
//!    subsetted font — a coupling the C++ does not have.
//!
//! [`subset`] therefore returns the map alongside the bytes; SPEC.md §11's
//! `subset(font_bytes, gids) -> Vec<u8>` is not enough once glyphs are
//! renumbered, and that widening is the §11 ruling E4.
//!
//! # Subset names
//!
//! An embedded subset is named `ABCDEF+Original`: six uppercase letters, a
//! plus, then the base name. An existing prefix is stripped before a new one
//! is added, so a font that has been subsetted twice still carries exactly
//! one tag.

mod tounicode;
mod widths;

use std::collections::BTreeMap;

use crate::error::Error;
use crate::write::id::IdSource;

pub use tounicode::to_unicode_cmap;
pub use widths::widths_array;

/// How the glyphs of a font were renumbered by subsetting.
///
/// Old glyph ID to new. Everything PDF-side that named a glyph — `/W`,
/// `/ToUnicode`, and the char codes of an Identity-H content stream — has to
/// be looked up through this.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GidMap {
    map: BTreeMap<u16, u16>,
}

impl GidMap {
    /// Build a map from pairs of (old, new).
    #[must_use]
    pub fn from_pairs(pairs: impl IntoIterator<Item = (u16, u16)>) -> Self {
        Self {
            map: pairs.into_iter().collect(),
        }
    }

    /// The new glyph ID for an old one.
    #[must_use]
    pub fn get(&self, old: u16) -> Option<u16> {
        self.map.get(&old).copied()
    }

    /// Every (old, new) pair, ascending by old ID.
    pub fn pairs(&self) -> impl Iterator<Item = (u16, u16)> + '_ {
        self.map.iter().map(|(a, b)| (*a, *b))
    }

    /// How many glyphs survived.
    #[must_use]
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Whether nothing survived.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

/// A subset font program and the renumbering it performed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subsetted {
    /// The font program.
    pub bytes: Vec<u8>,
    /// Old glyph ID to new.
    pub gid_map: GidMap,
}

/// Subset a font program to `gids`.
///
/// The returned program contains those glyphs and nothing else, renumbered
/// into a contiguous space starting at 0 with `.notdef` first. Everything
/// PDF-side that named a glyph must be looked up through
/// [`Subsetted::gid_map`].
///
/// `.notdef` (glyph 0) is always included whether or not it was asked for,
/// because a font without it is malformed.
///
/// # Errors
///
/// [`Error::Subset`] when the program cannot be parsed or subsetted — a
/// format the subsetter does not handle (CFF2 without variable-font support),
/// a truncated table directory, or a glyph ID past the end of the font.
///
/// ```
/// # fn main() -> Result<(), pdfrum_edit::Error> {
/// # let font_bytes = include_bytes!("../../tests/files/tiny.ttf");
/// let subset = pdfrum_edit::subset(font_bytes, &[3, 7])?;
/// // Glyph 0 is always kept, so the map holds three entries.
/// assert_eq!(subset.gid_map.get(0), Some(0));
/// assert!(subset.bytes.len() < font_bytes.len());
/// # Ok(())
/// # }
/// ```
pub fn subset(font_bytes: &[u8], gids: &[u16]) -> Result<Subsetted, Error> {
    // `.notdef` is not optional: a font without glyph 0 is malformed, and the
    // subsetter's own output always starts with it.
    let mut wanted: Vec<u16> = gids.to_vec();
    wanted.push(0);
    wanted.sort_unstable();
    wanted.dedup();

    let remapper = subsetter::GlyphRemapper::new_from_glyphs_sorted(&wanted);
    let bytes =
        subsetter::subset(font_bytes, 0, &remapper).map_err(|e| Error::Subset(e.to_string()))?;

    let gid_map = GidMap::from_pairs(
        wanted
            .iter()
            .filter_map(|old| remapper.get(*old).map(|new| (*old, new))),
    );
    Ok(Subsetted { bytes, gid_map })
}

/// The six-letter tag an embedded subset's name carries.
///
/// Uppercase ASCII, drawn from the save's own [`IdSource`] so a fixed save is
/// byte-reproducible.
#[must_use]
pub fn subset_tag(source: IdSource) -> [u8; 6] {
    let mut out = [b'A'; 6];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = b'A' + (source.tag_byte(i as u64) % 26);
    }
    out
}

/// `ABCDEF+Original`, with any existing tag stripped first.
#[must_use]
pub fn subset_name(base: &[u8], tag: [u8; 6]) -> Vec<u8> {
    let mut out = Vec::with_capacity(base.len() + 7);
    out.extend_from_slice(&tag);
    out.push(b'+');
    out.extend_from_slice(strip_subset_prefix(base));
    out
}

/// A font name with its subset tag removed, if it has one.
///
/// A tag is exactly six uppercase letters followed by `+`, and the name must
/// be longer than that — a name that is *only* a tag has nothing to strip.
#[must_use]
pub fn strip_subset_prefix(name: &[u8]) -> &[u8] {
    if name.len() <= 7 {
        return name;
    }
    if name.get(6) != Some(&b'+') {
        return name;
    }
    if !name
        .get(..6)
        .is_some_and(|p| p.iter().all(u8::is_ascii_uppercase))
    {
        return name;
    }
    name.get(7..).unwrap_or(name)
}

/// Whether a font program is OpenType with CFF outlines — an `OTTO` tag on
/// the *original* bytes.
///
/// It decides two things at once: the descriptor writes `/FontFile3` rather
/// than `/FontFile2`, and the descendant font is a `/CIDFontType0`.
#[must_use]
pub fn is_opentype_cff(bytes: &[u8]) -> bool {
    bytes.get(..4) == Some(b"OTTO")
}

#[cfg(test)]
mod tests {
    use super::{GidMap, is_opentype_cff, strip_subset_prefix, subset, subset_name, subset_tag};
    use crate::write::id::IdSource;

    const TINY: &[u8] = include_bytes!("../../tests/files/tiny.ttf");

    #[test]
    fn subsetting_keeps_the_asked_for_glyphs_and_notdef() {
        let out = subset(TINY, &[1]).expect("subsets");
        // `.notdef` is always present, whether or not it was asked for.
        assert_eq!(out.gid_map.get(0), Some(0));
        assert!(out.gid_map.get(1).is_some());
        assert_eq!(out.gid_map.len(), 2);
    }

    // The whole reason this crate re-keys anything: glyph IDs move.
    #[test]
    fn glyph_ids_are_renumbered_into_a_contiguous_space() {
        let out = subset(TINY, &[0, 1, 2]).expect("subsets");
        let new: Vec<u16> = out.gid_map.pairs().map(|(_, n)| n).collect();
        assert_eq!(new, vec![0, 1, 2], "contiguous from zero");
    }

    #[test]
    fn a_subset_is_smaller_than_the_original() {
        let out = subset(TINY, &[1]).expect("subsets");
        assert!(
            out.bytes.len() <= TINY.len(),
            "{} vs {}",
            out.bytes.len(),
            TINY.len()
        );
    }

    #[test]
    fn junk_is_refused_rather_than_panicking() {
        assert!(subset(b"not a font at all", &[1]).is_err());
        assert!(subset(&[], &[]).is_err());
    }

    // ReplaceExistingPrefix (:514-551): one tag, never two.
    #[test]
    fn an_existing_prefix_is_replaced_not_stacked() {
        let name = subset_name(b"AAAAAA+Arimo-Regular", *b"XXXXXX");
        assert_eq!(name, b"XXXXXX+Arimo-Regular");
        assert_eq!(name.iter().filter(|b| **b == b'+').take(2).count(), 1);
    }

    #[test]
    fn a_name_with_no_prefix_gains_one() {
        assert_eq!(
            subset_name(b"Arimo-Regular", *b"ABCDEF"),
            b"ABCDEF+Arimo-Regular"
        );
    }

    // The prefix test is exact: six *uppercase* letters and a plus.
    #[test]
    fn only_a_real_prefix_is_stripped() {
        assert_eq!(strip_subset_prefix(b"ABCDEF+Name"), b"Name");
        // Lowercase is not a tag.
        assert_eq!(strip_subset_prefix(b"abcdef+Name"), b"abcdef+Name");
        // Five letters is not a tag.
        assert_eq!(strip_subset_prefix(b"ABCDE+Name"), b"ABCDE+Name");
        // Digits are not letters.
        assert_eq!(strip_subset_prefix(b"ABC123+Name"), b"ABC123+Name");
        // No plus at all.
        assert_eq!(strip_subset_prefix(b"ABCDEFName"), b"ABCDEFName");
        // A name that is only a tag has nothing after it to keep.
        assert_eq!(strip_subset_prefix(b"ABCDEF+"), b"ABCDEF+");
        assert_eq!(strip_subset_prefix(b""), b"");
    }

    #[test]
    fn a_tag_is_six_uppercase_letters() {
        let tag = subset_tag(IdSource::Fixed([3u8; 16]));
        assert_eq!(tag.len(), 6);
        assert!(tag.iter().all(u8::is_ascii_uppercase), "{tag:?}");
    }

    // Determinism: a fixed source gives a reproducible tag.
    #[test]
    fn a_fixed_source_gives_the_same_tag_every_time() {
        let seed = IdSource::Fixed([9u8; 16]);
        assert_eq!(subset_tag(seed), subset_tag(seed));
        assert_ne!(subset_tag(seed), subset_tag(IdSource::Fixed([8u8; 16])));
    }

    // The four-byte tag test on the *original* bytes, which decides both
    // `/FontFile3` and `/CIDFontType0`.
    #[test]
    fn opentype_cff_is_an_otto_tag() {
        assert!(is_opentype_cff(b"OTTO\x00\x01"));
        assert!(!is_opentype_cff(b"\x00\x01\x00\x00"));
        assert!(!is_opentype_cff(b"true"));
        assert!(!is_opentype_cff(b"OTT"));
        assert!(!is_opentype_cff(b""));
    }

    #[test]
    fn an_empty_map_reports_itself_empty() {
        let map = GidMap::default();
        assert!(map.is_empty());
        assert_eq!(map.get(0), None);
    }
}
