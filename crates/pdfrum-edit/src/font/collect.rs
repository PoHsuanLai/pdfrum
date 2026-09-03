//! Finding the fonts a save may subset, and the glyphs each one still needs
//! (`cpdf_fontsubsetter.cpp:233-328`).
//!
//! A candidate is a font **new in this save** that a text operator on some
//! page actually shows through. Both halves matter: an old font is shared
//! with bytes we are not rewriting, and a new font nothing draws with has no
//! used-glyph set to subset down to.
//!
//! # Why this walks operators rather than page objects
//!
//! The C++ calls `page->ParseContent()` and iterates `CPDF_PageObject`s,
//! because that is the only view it has. We have a cheaper one that answers
//! exactly the same question: [`pdfrum_page::parse_content`] gives the
//! operator list, and `Tf` plus the show operators carry the font resource
//! and the character codes between them. Building the page-object graph
//! instead would decode every image and evaluate every shading on the page to
//! learn nothing more.
//!
//! The one thing the operator view costs is the *inline* font: a `Tf` naming
//! a `/Font` entry that is a direct dictionary rather than a reference has no
//! object number, so it can never be new, so it is never a candidate — which
//! is the same answer the C++ reaches through `GetFontDict()->GetObjNum()`
//! being zero.
//!
//! # Text inside a form `XObject` is not reached, and the C++ does not reach
//! it either
//!
//! Only the page's own `/Contents` is scanned. A `Do` naming a form is not
//! followed, so a glyph shown only from inside one is not in the used set —
//! and a font used *only* there is not a candidate at all, which is the safe
//! half of the gap.
//!
//! The unsafe half would be a font used both on the page and inside a form:
//! the form's glyphs would be dropped from the subset. **The C++ has exactly
//! the same hole.** `CollectSubsetCandidatesFromPage` iterates the page's
//! object list and asks each for `AsText()`; a form arrives as a
//! `CPDF_FormObject`, which is not a text object, so its contents are never
//! visited (`cpdf_fontsubsetter.cpp:237-241`). Matched rather than fixed, and
//! recorded here because it is a real limit of the option in both
//! implementations.

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};

use pdfrum_common::{Diagnostics, Limits};
use pdfrum_font::{Font, FontCache};
use pdfrum_object::{Dict, Name, ObjRef, Object, Resolve};
use pdfrum_page::Op;

use crate::doc::EditDoc;
use crate::names;

/// How deep a `/Kids` chain may go before the walk gives up.
///
/// The parser's own page walk stops on an ancestor cycle; this is the second
/// guard, for a tree that is acyclic but absurdly deep.
const MAX_TREE_DEPTH: u32 = 64;

/// One font the save may subset, with everything the override pass needs.
///
/// Keyed by the font *program* stream, as the C++ keys `candidates_`: two
/// `/Font` dictionaries sharing one `/FontFile2` must produce one subset, not
/// two conflicting rewrites of the same object.
#[derive(Debug)]
pub(crate) struct Candidate {
    /// The `/Type0` font dictionary.
    pub(crate) root_font: ObjRef,
    /// The descendant `CIDFont` dictionary.
    pub(crate) cid_font: ObjRef,
    /// The `/FontDescriptor`.
    pub(crate) descriptor: ObjRef,
    /// `/BaseFont`, as it stands before a subset tag is minted.
    pub(crate) base_name: Vec<u8>,
    /// The glyphs text on some page still shows, in the font program's own
    /// numbering.
    pub(crate) used_gids: BTreeSet<u16>,
    /// Which CID each of those glyphs was reached through.
    ///
    /// The subset keeps the CID space untouched and rewrites `/CIDToGIDMap`
    /// instead, so this is what that table is built from — and it is why `/W`
    /// and `/ToUnicode`, both keyed by CID, need no re-keying at all.
    pub(crate) cid_to_gid: BTreeMap<u16, u16>,
}

/// Every font this save may subset, keyed by its font-program object number.
///
/// `new_nums` must be sorted ascending; it is searched by binary search, the
/// same requirement `GenerateObjectOverrides` documents.
pub(crate) fn candidates(
    doc: &EditDoc<'_>,
    new_nums: &[u32],
    limits: &Limits,
) -> BTreeMap<u32, Candidate> {
    let mut found = BTreeMap::new();
    if new_nums.is_empty() {
        return found;
    }
    let cache = FontCache::new();
    for page in pages(doc) {
        scan_page(doc, &page, new_nums, limits, &cache, &mut found);
    }
    found
}

/// Every page dictionary in the document, in tree order.
///
/// The walk is over the [`EditDoc`] rather than the base document on purpose:
/// a page imported into this save exists only in the overlay, and it is
/// precisely the pages whose fonts are new that this pass is looking for.
fn pages(doc: &EditDoc<'_>) -> Vec<Dict> {
    let Some(root) = doc.base().trailer().reference(names::ROOT) else {
        return Vec::new();
    };
    let Some(catalog) = fetch_dict(doc, root) else {
        return Vec::new();
    };
    let Some(tree) = catalog.reference(names::PAGES) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    descend(doc, tree, 0, &mut seen, &mut out);
    out
}

/// Depth-first through `/Kids`, collecting leaves.
///
/// `seen` is the cycle guard and is global rather than per-path: a node
/// reached twice would contribute its subtree twice, and for a *set* of used
/// glyphs the second visit can only repeat what the first found.
fn descend(
    doc: &EditDoc<'_>,
    node: ObjRef,
    depth: u32,
    seen: &mut BTreeSet<u32>,
    out: &mut Vec<Dict>,
) {
    if depth > MAX_TREE_DEPTH || !seen.insert(node.num) {
        return;
    }
    let Some(dict) = fetch_dict(doc, node) else {
        return;
    };
    let Some(Object::Array(kids)) = dict
        .get(names::KIDS, doc)
        .map(pdfrum_object::Resolved::into_owned)
    else {
        // No `/Kids` at all is a leaf, whatever its `/Type` claims — a page
        // missing `/Type /Page` still draws.
        out.push(dict);
        return;
    };
    for kid in kids.iter() {
        if let Object::Ref(r) = kid {
            descend(doc, *r, depth.saturating_add(1), seen, out);
        }
    }
}

/// The dictionary `reference` names, if it is one.
fn fetch_dict(doc: &EditDoc<'_>, reference: ObjRef) -> Option<Dict> {
    doc.fetch(reference).ok()?.as_dict().cloned()
}

/// Add everything one page's text shows to `found`.
fn scan_page(
    doc: &EditDoc<'_>,
    page: &Dict,
    new_nums: &[u32],
    limits: &Limits,
    cache: &FontCache,
    found: &mut BTreeMap<u32, Candidate>,
) {
    let Some(fonts) = inherited_fonts(doc, page) else {
        return;
    };
    let bytes = content_bytes(doc, page);
    if bytes.is_empty() {
        return;
    }
    let mut diags = Diagnostics::default();
    let ops = pdfrum_page::parse_content(&bytes, limits, &mut diags);

    // Resolved once per distinct resource name, and the loaded font kept: a
    // page that sets the same font before every one of a thousand show
    // operators must not load it a thousand times, and `FontCache` is an
    // identity dispenser rather than a cache.
    let mut loaded: BTreeMap<Name, Option<Selected>> = BTreeMap::new();
    let mut current: Option<Selected> = None;
    for op in &ops {
        match op {
            Op::SetFont(name, _) => {
                if !loaded.contains_key(name) {
                    let hit = admit(doc, &fonts, name, new_nums, limits, cache, found);
                    loaded.insert(name.clone(), hit);
                }
                current = loaded.get(name).and_then(Clone::clone);
            }
            Op::ShowText(s) | Op::NextLineShowText(s) | Op::SetSpacingShowText(_, _, s) => {
                add_used(current.as_ref(), &s.bytes, found);
            }
            Op::ShowTextAdjusted(array) => {
                for item in &array.items {
                    if let pdfrum_page::TextItem::Show(codes) = item {
                        add_used(current.as_ref(), codes, found);
                    }
                }
            }
            _ => {}
        }
    }
}

/// The font a `Tf` selected, once it is known to be a candidate.
///
/// The loaded [`Font`] travels with the program number because decoding a
/// show operator needs it, and loading it per operator is what this exists to
/// avoid.
#[derive(Clone)]
struct Selected {
    /// The font-program object number the candidate is keyed by.
    program: u32,
    /// The loaded font, for [`Font::decode`].
    font: std::sync::Arc<Font>,
}

/// Decide whether the font `name` resolves to is a subset candidate, entering
/// it in `found` if so, and answer with its font-program object number.
///
/// This is `CollectSubsetCandidatesFromPage`'s filter ladder, in its order:
/// a new root font, a descriptor, and a `/FontFile2`. Two rungs are ours
/// alone and both are cited in [`super`]: the font must be `/Type0`, because
/// a subsetted simple font would have lost the `cmap` it maps codes through;
/// and its descendant must be `CIDFontType2`, because only a TrueType-outline
/// program is reached through the `/CIDToGIDMap` this pass rewrites.
fn admit(
    doc: &EditDoc<'_>,
    fonts: &Dict,
    name: &Name,
    new_nums: &[u32],
    limits: &Limits,
    cache: &FontCache,
    found: &mut BTreeMap<u32, Candidate>,
) -> Option<Selected> {
    // An inline font dictionary has no object number, so it cannot be new.
    let root_ref = fonts.reference(name)?;
    let is_new = |reference: ObjRef| new_nums.binary_search(&reference.num).is_ok();
    if !is_new(root_ref) {
        return None;
    }
    let root = fetch_dict(doc, root_ref)?;
    if root.name(names::SUBTYPE)?.as_bytes() != b"Type0" {
        return None;
    }

    let descendants = root.array(names::DESCENDANT_FONTS, doc)?;
    let cid_ref = descendants.reference_at(0)?;
    let cid_font = fetch_dict(doc, cid_ref)?;
    if cid_font.name(names::SUBTYPE)?.as_bytes() != b"CIDFontType2" {
        return None;
    }

    let descriptor_ref = cid_font.reference(names::FONT_DESCRIPTOR)?;
    let descriptor = fetch_dict(doc, descriptor_ref)?;
    let program = descriptor.reference(names::FONT_FILE2)?;

    // Every object the override pass will rewrite must be new, not just the
    // root font the C++ checks (`:262-265`). It checks one because it only
    // ever runs over fonts *it* created, where the whole chain is new by
    // construction; we run over whatever a caller imported, and an old
    // dictionary sharing this program would be left pointing at a subset
    // built for somebody else's glyph set. **Deliberate divergence**, and a
    // free one: the import path copies the whole chain into fresh numbers,
    // so nothing this stage would otherwise have subsetted is lost.
    if !is_new(cid_ref) || !is_new(descriptor_ref) || !is_new(program) {
        return None;
    }

    // A font whose dictionary will not load as a Type 0 is no use to the
    // subsetter, and finding that out here keeps the override pass free of
    // the question.
    let mut diags = Diagnostics::default();
    let font = pdfrum_font::load(&root, doc, cache, limits, &mut diags)?;
    if !matches!(font, Font::Type0(_)) {
        return None;
    }

    // The candidate is keyed by the program, so a second `/Font` reaching the
    // same one contributes its glyphs to the entry the first made.
    if let Entry::Vacant(slot) = found.entry(program.num) {
        slot.insert(Candidate {
            root_font: root_ref,
            cid_font: cid_ref,
            descriptor: descriptor_ref,
            base_name: root
                .name(names::BASE_FONT)
                .map(|n| n.as_bytes().to_vec())
                .unwrap_or_default(),
            used_gids: BTreeSet::new(),
            cid_to_gid: BTreeMap::new(),
        });
    }
    Some(Selected {
        program: program.num,
        font: std::sync::Arc::new(font),
    })
}

/// Record the glyphs `codes` shows through the font currently set.
///
/// `AddUsedText` (`:303-328`) does the same thing through
/// `GlyphFromCharCode`; [`Font::decode`] is that ladder, and it also hands
/// back the CID, which is what our `/CIDToGIDMap` rewrite needs and the C++'s
/// retained-GID subset does not.
fn add_used(current: Option<&Selected>, codes: &[u8], found: &mut BTreeMap<u32, Candidate>) {
    let Some(selected) = current else {
        return;
    };
    let Some(candidate) = found.get_mut(&selected.program) else {
        return;
    };
    for item in selected.font.decode(codes) {
        // `has_glyph` false is the C++'s `gid == -1`: nothing to keep.
        if let (Some(gid), Some(cid)) = (item.glyph(), item.cid) {
            candidate.used_gids.insert(gid.0);
            candidate.cid_to_gid.insert(cid.0, gid.0);
        }
    }
}

/// The page's `/Font` resource dictionary, following `/Parent` for a page
/// that does not state `/Resources` itself (ISO 32000-1 §7.7.3.4).
fn inherited_fonts(doc: &EditDoc<'_>, page: &Dict) -> Option<Dict> {
    let mut node = page.clone();
    for _ in 0..MAX_TREE_DEPTH {
        if let Some(resources) = node.dict(names::RESOURCES, doc) {
            return resources.dict(names::FONT, doc);
        }
        node = node.dict(names::PARENT, doc)?;
    }
    None
}

/// The page's content, with the elements of a `/Contents` array joined by a
/// space each.
///
/// The separator is not cosmetic: an element may end mid-token, and the join
/// is what stops the next element's first byte from extending it.
fn content_bytes(doc: &EditDoc<'_>, page: &Dict) -> Vec<u8> {
    let mut out = Vec::new();
    match page
        .get(names::CONTENTS, doc)
        .map(pdfrum_object::Resolved::into_owned)
    {
        Some(Object::Stream(s)) => {
            out.extend_from_slice(&decode(doc, &s));
        }
        Some(Object::Array(a)) => {
            for element in a.iter() {
                if let Some(s) = element
                    .resolve(doc)
                    .ok()
                    .and_then(|r| r.as_stream().cloned())
                {
                    out.extend_from_slice(&decode(doc, &s));
                    out.push(b' ');
                }
            }
        }
        _ => {}
    }
    out
}

/// A content stream's bytes with its filters undone.
fn decode(doc: &EditDoc<'_>, stream: &pdfrum_object::Stream) -> Vec<u8> {
    let limits = Limits::default();
    let mut diags = Diagnostics::default();
    pdfrum_filters::decode_chain(stream, 0, doc, &limits, &mut diags).data
}
