//! The objects a subsetting save writes in place of the ones the document
//! holds.
//!
//! Nothing here mutates the document. The pass builds a map from object
//! number to replacement object, and the writer's new-object loop consults it
//! per object — which is what makes a subsetting save and an ordinary one the
//! same code path with one lookup between them.
//!
//! # Four objects change, one is added, and the CID space does not move
//!
//! For each candidate: the font program becomes the subset; the root font,
//! the descendant `CIDFont` and the descriptor take the tagged name; the
//! descendant also gains a `/CIDToGIDMap`; and that map is the one object
//! this pass mints rather than replaces.
//!
//! All four are produced together or not at all. A candidate the subsetter
//! refuses, one whose subset did not shrink, and one whose chain has gone
//! missing all contribute nothing — a program with no dictionary naming it,
//! or a tagged name over untouched bytes, would each be worse than leaving
//! the font alone.
//!
//! What does **not** change is the character codes on the page, the CIDs they
//! map to, `/W`, or `/ToUnicode`. That is the whole reason for the
//! `/CIDToGIDMap`: our subsetter renumbers glyphs, and pointing the CID at
//! its new glyph through the table ISO 32000-1 §9.7.4.2 already provides
//! absorbs the renumbering at the one place it can be absorbed without
//! touching anything a content stream said. See [`super`] for why that
//! replaced the re-keying the design originally planned.

// Where the pass's shape comes from: `cpdf_fontsubsetter.cpp:126-226`. The
// `/CIDToGIDMap` reading the test's `glyph_at` reproduces is
// `cpdf_cidfont.cpp:508-518`.

use std::collections::BTreeMap;

use pdfrum_object::{ByteSpan, Dict, Name, ObjRef, Object, Resolve, Stream};

use crate::doc::EditDoc;
use crate::font::collect::Candidate;
use crate::font::{is_opentype_cff, subset, subset_name, subset_tag};
use crate::names;
use crate::write::id::IdSource;

/// Objects to write instead of the ones the document holds, by object number.
pub(crate) type Overrides = BTreeMap<u32, Object>;

/// Build the override map for one save.
///
/// `new_nums` must be sorted ascending. Objects it does not name are never
/// candidates, so an ordinary document being rewritten unchanged produces an
/// empty map and pays only for the page walk.
pub(crate) fn build(
    doc: &EditDoc<'_>,
    new_nums: &[u32],
    id_source: IdSource,
    next_number: &mut u32,
) -> Overrides {
    let mut overrides = Overrides::new();
    // The caps a save enforces are the ones the *reader* was opened with, and
    // the parser does not keep them past the load. The defaults are what
    // every other reparse in this crate uses; a document that loaded under
    // tighter ones has already had its objects capped on the way in.
    let limits = pdfrum_common::Limits::default();
    for (program, candidate) in crate::font::collect::candidates(doc, new_nums, &limits) {
        one(
            doc,
            program,
            &candidate,
            id_source,
            next_number,
            &mut overrides,
        );
    }
    overrides
}

/// Add one candidate's overrides, or none at all.
///
/// A candidate that cannot be subsetted is skipped whole — the C++'s "if the
/// subset came back empty, the original font survives unmodified". Half a
/// rewrite would be worse than none: a tagged name over an untouched program
/// is a lie, and a `/CIDToGIDMap` over the original glyph numbering points
/// every CID at the wrong glyph.
fn one(
    doc: &EditDoc<'_>,
    program: u32,
    candidate: &Candidate,
    id_source: IdSource,
    next_number: &mut u32,
    overrides: &mut Overrides,
) {
    if candidate.used_gids.is_empty() {
        return;
    }
    let Some(original) = program_bytes(doc, program) else {
        return;
    };
    // An OpenType-CFF program reaches its glyphs as `CIDFontType0`, where the
    // CID *is* the glyph index and no `/CIDToGIDMap` is consulted at all
    // (`cpdf_cidfont.cpp:508-518`). Renumbering the glyphs of one would
    // therefore need the CIDs rewritten, which is the content-stream rewrite
    // this design exists to avoid — so an `OTTO` program is left alone. The
    // C++ subsets it and switches the descendant to `CIDFontType0`; ours
    // declines, which is a narrowing (see [`super`]).
    if is_opentype_cff(&original) {
        return;
    }

    let gids: Vec<u16> = candidate.used_gids.iter().copied().collect();
    let Ok(subsetted) = subset(&original, &gids) else {
        return;
    };
    // A subset that grew is not worth the four rewritten objects it costs.
    if subsetted.bytes.len() >= original.len() {
        return;
    }

    // Every dictionary is fetched **before** anything is written, so a
    // candidate whose chain has gone missing under us contributes nothing
    // rather than a program with no dictionary pointing at it.
    let (Some(root), Some(cid_font), Some(descriptor)) = (
        fetch_dict(doc, candidate.root_font),
        fetch_dict(doc, candidate.cid_font),
        fetch_dict(doc, candidate.descriptor),
    ) else {
        return;
    };

    let name = Name::from(&subset_name(&candidate.base_name, subset_tag(id_source))[..]);

    // The `/CIDToGIDMap` is the whole renumbering, in one object: at index
    // `cid` sits the *new* glyph for the glyph `cid` used to reach. A CID the
    // page never showed maps to zero — `.notdef` — which is what a table
    // shorter than the CID space would mean anyway.
    let table = cid_to_gid_table(candidate, &subsetted.gid_map);
    let map_ref = ObjRef::new(*next_number, 0);
    *next_number = next_number.saturating_add(1);
    overrides.insert(map_ref.num, stream(Dict::new(), table));

    // The program itself. `/Length1` is the uncompressed length, which for a
    // TrueType program is the whole of it; the stream writer computes
    // `/Length` and applies the filter on the way out.
    let length1 = i64::try_from(subsetted.bytes.len()).unwrap_or(i64::MAX);
    overrides.insert(
        program,
        stream(
            Dict::from_pairs([(names::LENGTH1.clone(), Object::Int(length1))]),
            subsetted.bytes,
        ),
    );

    // The three dictionaries take the tagged name, and the descendant gains
    // the map. Everything else about them — `/Encoding`, `/DescendantFonts`,
    // `/W`, `/DW`, `/CIDSystemInfo`, `/ToUnicode` — is carried through
    // untouched, because the CID space it is all keyed by did not move.
    overrides.insert(
        candidate.root_font.num,
        Object::Dict(set(&root, names::BASE_FONT, Object::Name(name.clone()))),
    );
    let cid_font = set(&cid_font, names::BASE_FONT, Object::Name(name.clone()));
    let cid_font = set(&cid_font, names::CID_TO_GID_MAP, Object::Ref(map_ref));
    overrides.insert(candidate.cid_font.num, Object::Dict(cid_font));
    overrides.insert(
        candidate.descriptor.num,
        Object::Dict(set(&descriptor, names::FONT_NAME, Object::Name(name))),
    );
}

/// The `/CIDToGIDMap` stream body: a big-endian `u16` per CID, indexed by CID
/// (ISO 32000-1 §9.7.4.2).
///
/// Sized to the highest CID the pages actually showed. A CID past the end
/// reads as no glyph, and one inside the table but unused reads as zero —
/// both of which mean `.notdef`, which is the right answer for a glyph the
/// subset dropped.
fn cid_to_gid_table(candidate: &Candidate, map: &super::GidMap) -> Vec<u8> {
    let highest = candidate.cid_to_gid.keys().copied().max().unwrap_or(0);
    let len = usize::from(highest).saturating_add(1).saturating_mul(2);
    let mut table = vec![0u8; len];
    for (cid, old_gid) in &candidate.cid_to_gid {
        let Some(new_gid) = map.get(*old_gid) else {
            continue;
        };
        let at = usize::from(*cid).saturating_mul(2);
        if let Some(slot) = table.get_mut(at..at.saturating_add(2)) {
            slot.copy_from_slice(&new_gid.to_be_bytes());
        }
    }
    table
}

/// A stream object over owned bytes, carrying `/Length` for a reader that
/// looks at the dictionary before the writer recomputes it.
fn stream(dict: Dict, bytes: Vec<u8>) -> Object {
    let length = i64::try_from(bytes.len()).unwrap_or(i64::MAX);
    let mut dict = dict;
    dict.push(names::LENGTH.clone(), Object::Int(length));
    Object::Stream(Stream::new(dict, ByteSpan::from(bytes)))
}

/// `dict` with `key` set to `value`, replacing the first entry that names it.
///
/// [`Dict`] permits duplicate keys and `push` appends, so setting a key that
/// is already there has to rebuild rather than add.
fn set(dict: &Dict, key: &Name, value: Object) -> Dict {
    let mut out = Dict::new();
    let mut placed = false;
    for (k, v) in dict.iter() {
        if k == key {
            if !placed {
                out.push(k.clone(), value.clone());
                placed = true;
            }
            continue;
        }
        out.push(k.clone(), v.clone());
    }
    if !placed {
        out.push(key.clone(), value);
    }
    out
}

/// The dictionary `reference` names, if it is one.
fn fetch_dict(doc: &EditDoc<'_>, reference: ObjRef) -> Option<Dict> {
    doc.fetch(reference).ok()?.as_dict().cloned()
}

/// The font program's bytes with its filters undone.
fn program_bytes(doc: &EditDoc<'_>, program: u32) -> Option<Vec<u8>> {
    let object = doc.fetch(ObjRef::new(program, 0)).ok()?;
    let stream = object.as_stream()?;
    let limits = pdfrum_common::Limits::default();
    let mut diags = pdfrum_common::Diagnostics::default();
    let data = pdfrum_filters::decode_chain(stream, 0, doc, &limits, &mut diags).data;
    (!data.is_empty()).then_some(data)
}

#[cfg(test)]
mod tests {
    use super::{cid_to_gid_table, set};
    use crate::font::GidMap;
    use crate::font::collect::Candidate;
    use crate::names;
    use pdfrum_object::{Dict, Name, ObjRef, Object};
    use std::collections::{BTreeMap, BTreeSet};

    fn candidate(pairs: &[(u16, u16)]) -> Candidate {
        Candidate {
            root_font: ObjRef::new(1, 0),
            cid_font: ObjRef::new(2, 0),
            descriptor: ObjRef::new(3, 0),
            base_name: b"Test".to_vec(),
            used_gids: pairs.iter().map(|(_, gid)| *gid).collect::<BTreeSet<_>>(),
            cid_to_gid: pairs.iter().copied().collect::<BTreeMap<_, _>>(),
        }
    }

    /// Read the table back the way a reader does: two big-endian bytes at
    /// `cid * 2`, which is what `pdfrum_font::cid::glyph` does.
    fn glyph_at(table: &[u8], cid: u16) -> Option<u16> {
        let at = usize::from(cid) * 2;
        let pair = table.get(at..at + 2)?;
        Some(u16::from_be_bytes([*pair.first()?, *pair.get(1)?]))
    }

    #[test]
    fn the_table_sends_each_cid_to_its_new_glyph() {
        // CID 5 drew glyph 40 and CID 9 drew glyph 12; subsetting renumbered
        // them to 2 and 1.
        let map = GidMap::from_pairs([(0, 0), (12, 1), (40, 2)]);
        let table = cid_to_gid_table(&candidate(&[(5, 40), (9, 12)]), &map);

        assert_eq!(glyph_at(&table, 5), Some(2));
        assert_eq!(glyph_at(&table, 9), Some(1));
    }

    #[test]
    fn a_cid_the_page_never_showed_reads_as_notdef() {
        let map = GidMap::from_pairs([(0, 0), (40, 1)]);
        let table = cid_to_gid_table(&candidate(&[(5, 40)]), &map);

        // Inside the table but unused, and past its end: both `.notdef`.
        assert_eq!(glyph_at(&table, 4), Some(0));
        assert_eq!(glyph_at(&table, 6), None);
    }

    #[test]
    fn the_table_is_two_bytes_per_cid_up_to_the_highest_one_drawn() {
        let map = GidMap::from_pairs([(0, 0), (40, 1)]);
        let table = cid_to_gid_table(&candidate(&[(9, 40)]), &map);
        assert_eq!(table.len(), 20, "CIDs 0 through 9, two bytes each");
    }

    // A glyph the subsetter dropped has no entry in the map, so the CID that
    // reached it must fall back to `.notdef` rather than to a stale index.
    #[test]
    fn a_dropped_glyph_leaves_its_cid_at_notdef() {
        let map = GidMap::from_pairs([(0, 0)]);
        let table = cid_to_gid_table(&candidate(&[(3, 40)]), &map);
        assert_eq!(glyph_at(&table, 3), Some(0));
    }

    #[test]
    fn setting_a_key_that_is_already_there_replaces_it_in_place() {
        let dict = Dict::from_pairs([
            (names::TYPE.clone(), Object::Name(Name::from("Font"))),
            (names::BASE_FONT.clone(), Object::Name(Name::from("Old"))),
            (names::SUBTYPE.clone(), Object::Name(Name::from("Type0"))),
        ]);
        let out = set(
            &dict,
            names::BASE_FONT,
            Object::Name(Name::from("ABCDEF+Old")),
        );

        let keys: Vec<&Name> = out.keys().collect();
        assert_eq!(keys, vec![names::TYPE, names::BASE_FONT, names::SUBTYPE]);
        assert_eq!(
            out.name(names::BASE_FONT).map(Name::as_bytes),
            Some(&b"ABCDEF+Old"[..])
        );
    }

    #[test]
    fn setting_a_key_that_is_absent_appends_it() {
        let dict = Dict::from_pairs([(names::TYPE.clone(), Object::Name(Name::from("Font")))]);
        let out = set(&dict, names::CID_TO_GID_MAP, Object::Ref(ObjRef::new(9, 0)));
        assert_eq!(out.len(), 2);
        assert_eq!(
            out.reference(names::CID_TO_GID_MAP),
            Some(ObjRef::new(9, 0))
        );
    }

    // `Dict` permits duplicate keys, and a font dictionary that carries two
    // `/BaseFont` entries must come back with one — the tagged name, at the
    // position the first occurrence held.
    #[test]
    fn a_duplicated_key_collapses_to_one() {
        let dict = Dict::from_pairs([
            (names::BASE_FONT.clone(), Object::Name(Name::from("First"))),
            (names::TYPE.clone(), Object::Name(Name::from("Font"))),
            (names::BASE_FONT.clone(), Object::Name(Name::from("Second"))),
        ]);
        let out = set(&dict, names::BASE_FONT, Object::Name(Name::from("Tagged")));
        assert_eq!(out.len(), 2);
        assert_eq!(
            out.name(names::BASE_FONT).map(Name::as_bytes),
            Some(&b"Tagged"[..])
        );
    }
}
