//! CID → Unicode over the built-in tables of the four CJK character
//! collections (ISO 32000-1 §9.10.2).
//!
//! This is the last resort of text extraction: when a CID font carries no
//! `/ToUnicode` map, the collection's own table is what turns its CIDs back
//! into text. Index 0 of every table is U+FFFD, so CID 0 — `.notdef`, and also
//! what every unmapped character code produces — extracts as the replacement
//! character rather than vanishing.

use crate::blob;
use crate::ids::{Cid, CidSet};

/// The Unicode scalar a character collection assigns to `cid`.
///
/// `CidSet::Unicode` is the identity map: the CID *is* the scalar value.
/// `CidSet::Unknown` has no table and always yields `None`, as does a CID past
/// the end of its collection's table.
pub(crate) fn unicode_from_cid(set: CidSet, cid: Cid) -> Option<char> {
    let raw = match set {
        CidSet::Unicode => u32::from(cid.0),
        CidSet::Unknown => return None,
        _ => u32::from(blob::cid2unicode(set.registry_index()?, cid.0)?),
    };
    char::from_u32(raw)
}

/// Whether a collection has a built-in CID→Unicode table at all. Only the four
/// CJK collections do; `Unicode` needs none and `Unknown` has none.
pub(crate) fn has_table(set: CidSet) -> bool {
    set.registry_index()
        .is_some_and(|reg| blob::cid2unicode_len(reg) > 0)
}

#[cfg(test)]
mod tests {
    use super::{has_table, unicode_from_cid};
    use crate::blob;
    use crate::ids::{Cid, CidSet};

    #[test]
    fn cid_zero_is_the_replacement_character_everywhere() {
        for set in [CidSet::Gb1, CidSet::Cns1, CidSet::Japan1, CidSet::Korea1] {
            assert_eq!(unicode_from_cid(set, Cid(0)), Some('\u{FFFD}'), "{set:?}");
        }
    }

    /// `kGB1CID2Unicode_5` starts `0xFFFD, 0x0020, 0x0021, …`, so the low CIDs
    /// are ASCII in order.
    #[test]
    fn gb1_low_cids_are_ascii() {
        assert_eq!(unicode_from_cid(CidSet::Gb1, Cid(1)), Some(' '));
        assert_eq!(unicode_from_cid(CidSet::Gb1, Cid(2)), Some('!'));
        assert_eq!(unicode_from_cid(CidSet::Gb1, Cid(34)), Some('A'));
    }

    #[test]
    fn unicode_collection_is_the_identity() {
        assert_eq!(unicode_from_cid(CidSet::Unicode, Cid(0x41)), Some('A'));
        assert_eq!(unicode_from_cid(CidSet::Unicode, Cid(0)), Some('\0'));
        // A lone surrogate is not a scalar value and has no `char`.
        assert_eq!(unicode_from_cid(CidSet::Unicode, Cid(0xD800)), None);
    }

    #[test]
    fn unknown_collection_has_nothing() {
        assert_eq!(unicode_from_cid(CidSet::Unknown, Cid(1)), None);
        assert!(!has_table(CidSet::Unknown));
        assert!(!has_table(CidSet::Unicode));
        for set in [CidSet::Gb1, CidSet::Cns1, CidSet::Japan1, CidSet::Korea1] {
            assert!(has_table(set), "{set:?}");
        }
    }

    #[test]
    fn a_cid_past_the_table_has_no_mapping() {
        let len = blob::cid2unicode_len(0);
        let last = u16::try_from(len - 1).unwrap();
        assert!(unicode_from_cid(CidSet::Gb1, Cid(last)).is_some());
        assert_eq!(
            unicode_from_cid(CidSet::Gb1, Cid(u16::try_from(len).unwrap())),
            None
        );
        assert_eq!(unicode_from_cid(CidSet::Gb1, Cid(u16::MAX)), None);
    }
}
