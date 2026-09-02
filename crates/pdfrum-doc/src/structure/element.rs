//! One node of the logical structure tree and the five shapes its `/K` takes.
//!
//! A structure element's kids are heterogeneous: other elements, marked
//! content on a page, marked content in a named stream, a reference to a page
//! object, or nothing at all. Which one a `/K` entry means depends on its
//! type *and* on whether it belongs to the page currently being read — the
//! same element yields different kids from different pages, which is how one
//! logical tree spans a document.

use pdfrum_object::{Dict, Name, ObjRef, Object, Resolve};

use crate::names;

/// One entry in a structure element's kid list.
///
/// A slot is always reserved, even when nothing usable sits in it, because
/// the dump walks kid indices and a missing kid contributes a skipped index
/// rather than shifting its siblings.
#[derive(Debug, Clone, PartialEq)]
pub enum Kid {
    /// Nothing usable: absent, the wrong type, or content belonging to a
    /// different page.
    Invalid,
    /// A nested structure element, identified by where it was found.
    ///
    /// Two indices live here and they are **different spaces**: `slot` is the
    /// position in the parent's `/K` array and never changes, while `linked`
    /// is an index into the tree's element table and is filled only once the
    /// upward walk actually reaches that element. A kid the walk never
    /// reached keeps `linked: None` and the dump skips it.
    Element {
        /// The element's dictionary.
        dict: Dict,
        /// Its object reference, when it has one.
        reference: Option<ObjRef>,
        /// Its position in the parent's `/K`, which distinguishes inline kids
        /// that share no object number.
        slot: usize,
        /// Its index in the tree's element table, once linked.
        linked: Option<usize>,
    },
    /// Marked content in the page's own content stream.
    PageContent {
        /// The `/MCID` this kid names.
        content_id: i64,
    },
    /// Marked content in a stream other than the page's.
    StreamContent {
        /// The stream's object number, or zero when `/Stm` is not a
        /// reference.
        stream_obj_num: u32,
        /// The `/MCID` this kid names.
        content_id: i64,
    },
    /// A reference to a page object rather than to content.
    Object {
        /// The referenced object's number, or zero when `/Obj` is not a
        /// reference.
        obj_num: u32,
    },
}

/// A logical structure element.
#[derive(Debug, Clone, PartialEq)]
pub struct StructElement {
    /// The element's own dictionary.
    pub dict: Dict,
    /// Its object reference, when it is indirect.
    pub reference: Option<ObjRef>,
    /// `/S` after the role map has been applied once, at load time. The raw
    /// `/S` is not retained.
    pub kind: Vec<u8>,
    /// The kid slots, in `/K` order.
    pub kids: Vec<Kid>,
    /// The parent element's index in the tree's element table, once the
    /// bottom-up walk has linked it.
    pub parent: Option<usize>,
}

impl StructElement {
    /// Reads an element's kids against a page.
    ///
    /// `page_obj_num` is the page being read; content kids that name a
    /// different page become `Kid::Invalid`, while element kids are never
    /// page-tested at all — that asymmetry is what lets one element tree be
    /// reachable from every page it touches.
    pub fn load_kids<R: Resolve>(dict: &Dict, page_obj_num: u32, r: &R) -> Vec<Kid> {
        // The element's own `/Pg`, which each kid may override.
        let own_page = dict
            .raw(names::PG)
            .and_then(Object::as_ref_id)
            .map_or(0, |reference| reference.num);

        let Some(kids_obj) = dict.get(names::K, r).map(|k| k.get().clone()) else {
            return Vec::new();
        };
        match &kids_obj {
            Object::Array(array) => array
                .iter()
                .enumerate()
                .map(|(slot, element)| {
                    let direct = element.resolve(r).ok().map(|d| d.get().clone());
                    load_kid(direct.as_ref(), element, own_page, page_obj_num, slot, r)
                })
                .collect(),
            other => vec![load_kid(Some(other), other, own_page, page_obj_num, 0, r)],
        }
    }

    /// Points a kid slot at an element the walk has now built.
    ///
    /// Answers whether any slot matched — a question, not a refused mutation.
    /// Matching is by object reference when both sides have one, and by
    /// dictionary equality otherwise, which is how an inline kid dictionary —
    /// having no object number to compare — still finds its slot.
    pub fn link_kid(&mut self, target: &Dict, reference: Option<ObjRef>, element: usize) -> bool {
        let mut matched = false;
        for kid in &mut self.kids {
            if let Kid::Element {
                dict,
                reference: kid_ref,
                linked,
                ..
            } = kid
            {
                let same = match (*kid_ref, reference) {
                    (Some(a), Some(b)) => a == b,
                    _ => dict == target,
                };
                if same {
                    *linked = Some(element);
                    matched = true;
                }
            }
        }
        matched
    }

    /// `/Type`, coerced from whatever the key holds. Usually `StructElem`.
    #[must_use]
    pub fn obj_type<R: Resolve>(&self, r: &R) -> Vec<u8> {
        self.dict.byte_string(names::TYPE, r).unwrap_or_default()
    }

    /// `/Alt`, the alternate description.
    #[must_use]
    pub fn alt_text<R: Resolve>(&self, r: &R) -> String {
        self.dict.text(names::ALT, r).unwrap_or_default()
    }

    /// `/ActualText`, the exact replacement text.
    #[must_use]
    pub fn actual_text<R: Resolve>(&self, r: &R) -> String {
        self.dict.text(names::ACTUAL_TEXT, r).unwrap_or_default()
    }

    /// `/E`, an abbreviation's expansion.
    #[must_use]
    pub fn expansion<R: Resolve>(&self, r: &R) -> String {
        self.dict.text(names::E, r).unwrap_or_default()
    }

    /// `/T`, the element's title.
    #[must_use]
    pub fn title<R: Resolve>(&self, r: &R) -> String {
        self.dict.text(names::T, r).unwrap_or_default()
    }

    /// `/ID`, read **without resolving** and filtered to a string.
    ///
    /// The `Option` is load-bearing: a present-but-empty `/ID` differs from
    /// an absent one at the API boundary, even though the dump prints neither.
    #[must_use]
    pub fn id(&self) -> Option<Vec<u8>> {
        self.dict
            .raw(names::ID)
            .and_then(Object::as_string)
            .map(|s| s.bytes.to_vec())
    }

    /// `/Lang`, read without resolving and filtered to a string.
    ///
    /// **Not inherited** — a row inside a table with `/Lang /hu` reports
    /// nothing of its own.
    #[must_use]
    pub fn lang(&self) -> Option<Vec<u8>> {
        self.dict
            .raw(names::LANG)
            .and_then(Object::as_string)
            .map(|s| s.bytes.to_vec())
    }

    /// The `/MCID` a kid names, or `None` for a kid that is not content.
    ///
    /// This is the **page-filtered** reading: a content kid belonging to
    /// another page came in as `Kid::Invalid` and answers `None` here. The
    /// dump uses a different, unfiltered accessor
    /// (`marked_content_id_count`).
    ///
    /// Was `-> i64` with `-1` for absence. A real `/MCID` is non-negative
    /// (ISO 32000-1 §14.7.4.2) so `-1` could not *collide*, but the file
    /// never contains it either — it was invented to mean "none", which is
    /// what `Option` is for (`docs/design/idiomatic-api.md` §C, Tier 2
    /// item 15).
    #[must_use]
    pub fn kid_content_id(&self, index: usize) -> Option<i64> {
        match self.kids.get(index) {
            Some(Kid::PageContent { content_id } | Kid::StreamContent { content_id, .. }) => {
                Some(*content_id)
            }
            _ => None,
        }
    }
}

/// Classifies one `/K` entry.
fn load_kid<R: Resolve>(
    direct: Option<&Object>,
    raw: &Object,
    own_page: u32,
    page_obj_num: u32,
    slot: usize,
    r: &R,
) -> Kid {
    let Some(direct) = direct else {
        return Kid::Invalid;
    };
    match direct {
        Object::Int(_) | Object::Real(_) => {
            if own_page == page_obj_num {
                Kid::PageContent {
                    content_id: direct.as_int().unwrap_or(0),
                }
            } else {
                Kid::Invalid
            }
        }
        Object::Dict(dict) => {
            // A kid's own `/Pg` overrides the element's, and is consulted
            // *before* the type test, so a marked-content reference is judged
            // against the page it names rather than its parent's.
            let page = dict
                .raw(names::PG)
                .and_then(Object::as_ref_id)
                .map_or(own_page, |reference| reference.num);
            let kind = dict.name(names::TYPE);
            match kind {
                Some(k) if k == names::MCR => {
                    if page != page_obj_num {
                        return Kid::Invalid;
                    }
                    Kid::StreamContent {
                        stream_obj_num: dict
                            .raw(names::STM)
                            .and_then(Object::as_ref_id)
                            .map_or(0, |reference| reference.num),
                        content_id: dict.int(names::MCID, r).unwrap_or(0),
                    }
                }
                Some(k) if k == names::OBJR => {
                    if page != page_obj_num {
                        return Kid::Invalid;
                    }
                    Kid::Object {
                        obj_num: dict
                            .raw(names::OBJ)
                            .and_then(Object::as_ref_id)
                            .map_or(0, |reference| reference.num),
                    }
                }
                // Any other `/Type`, including none, is a nested element —
                // and element kids cross pages freely.
                _ => Kid::Element {
                    dict: dict.clone(),
                    reference: raw.as_ref_id(),
                    slot,
                    linked: None,
                },
            }
        }
        _ => Kid::Invalid,
    }
}

/// The **unfiltered** marked-content count the `--show-structure` dump reads.
///
/// Deliberately different from [`StructElement::kid_content_id`]: it ignores
/// the page entirely, counts a bare number or dictionary as one, and answers
/// `None` for an absent or unusable `/K`.
///
/// Was `-> i64` answering `-1`, documented as being `-1` "so the caller's
/// `0..count` loop never runs" — a count that relies on `0..-1` being empty
/// after a cast is a sentinel wearing a count's type
/// (`docs/design/idiomatic-api.md` §C, Tier 2 item 13). `None` says the same
/// thing and the compiler enforces the check.
#[must_use]
pub(crate) fn marked_content_id_count<R: Resolve>(dict: &Dict, r: &R) -> Option<usize> {
    match dict.get(names::K, r).map(|k| k.get().clone()) {
        Some(Object::Int(_) | Object::Real(_) | Object::Dict(_)) => Some(1),
        Some(Object::Array(array)) => Some(array.len()),
        // An absent `/K` and an unusable one both mean "no marked content".
        None | Some(_) => None,
    }
}

/// The **unfiltered** marked-content identifier at one index, or `None`.
///
/// Was `-> i64` with six `-1` exits. A real `/MCID` is non-negative
/// (ISO 32000-1 §14.7.4.2), so `-1` could not collide — but the file never
/// contains it either; it was invented for absence
/// (`docs/design/idiomatic-api.md` §C, Tier 2 item 14).
#[must_use]
pub(crate) fn marked_content_id_at<R: Resolve>(dict: &Dict, index: usize, r: &R) -> Option<i64> {
    match dict.get(names::K, r).map(|k| k.get().clone()) {
        Some(obj @ (Object::Int(_) | Object::Real(_))) => {
            if index == 0 {
                obj.as_int()
            } else {
                None
            }
        }
        // A dictionary answers the same identifier at every index.
        Some(Object::Dict(dict)) => mcid_from_dict(&dict, r),
        Some(Object::Array(array)) => match array.get(index, r).map(|e| e.get().clone()) {
            Some(obj @ (Object::Int(_) | Object::Real(_))) => obj.as_int(),
            Some(Object::Dict(dict)) => mcid_from_dict(&dict, r),
            _ => None,
        },
        _ => None,
    }
}

/// A marked-content reference's identifier: `/Type` must be the **name**
/// `MCR` and `/MCID` must be a number.
fn mcid_from_dict<R: Resolve>(dict: &Dict, r: &R) -> Option<i64> {
    if dict.name(names::TYPE) != Some(names::MCR) {
        return None;
    }
    dict.get(names::MCID, r)
        .and_then(|v| v.get().as_number().and_then(Object::as_int))
}

/// Applies the tree's `/RoleMap` to a structure type, once.
#[must_use]
pub fn map_role(role_map: Option<&Dict>, kind: &[u8]) -> Vec<u8> {
    role_map
        .and_then(|map| map.name(&Name::new(kind.to_vec())))
        .filter(|mapped| !mapped.as_bytes().is_empty())
        .map_or_else(|| kind.to_vec(), |mapped| mapped.as_bytes().to_vec())
}

#[cfg(test)]
mod tests {
    use super::{Kid, StructElement, map_role, marked_content_id_at, marked_content_id_count};
    use pdfrum_object::{Array, Dict, Name, NoResolve, Object, PdfString};

    use crate::names;

    fn dict(pairs: &[(&str, Object)]) -> Dict {
        Dict::from_pairs(
            pairs
                .iter()
                .map(|(k, v)| (Name::from(*k), v.clone()))
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn a_number_kid_is_page_content_only_for_its_own_page() {
        let element = dict(&[("K", Object::Int(7))]);
        // The element has no `/Pg`, so it reads as page zero.
        assert_eq!(
            StructElement::load_kids(&element, 0, &NoResolve),
            vec![Kid::PageContent { content_id: 7 }]
        );
        assert_eq!(
            StructElement::load_kids(&element, 4, &NoResolve),
            vec![Kid::Invalid]
        );
    }

    #[test]
    fn a_dict_kid_with_no_recognized_type_is_a_nested_element() {
        let inner = dict(&[("S", Object::Name(Name::from("P")))]);
        let element = dict(&[("K", Object::Dict(inner.clone()))]);
        let kids = StructElement::load_kids(&element, 0, &NoResolve);
        assert_eq!(
            kids,
            vec![Kid::Element {
                dict: inner,
                reference: None,
                slot: 0,
                linked: None,
            }]
        );
    }

    #[test]
    fn an_element_kid_is_never_page_tested() {
        let inner = dict(&[("S", Object::Name(Name::from("P")))]);
        let element = dict(&[("K", Object::Dict(inner))]);
        // Page 9 is not this element's page, yet the kid survives.
        assert!(matches!(
            StructElement::load_kids(&element, 9, &NoResolve).as_slice(),
            [Kid::Element { .. }]
        ));
    }

    #[test]
    fn a_marked_content_reference_needs_its_page_to_match() {
        let mcr = dict(&[
            ("Type", Object::Name(names::MCR.clone())),
            ("MCID", Object::Int(3)),
        ]);
        let element = dict(&[("K", Object::Dict(mcr))]);
        assert_eq!(
            StructElement::load_kids(&element, 0, &NoResolve),
            vec![Kid::StreamContent {
                stream_obj_num: 0,
                content_id: 3,
            }]
        );
        assert_eq!(
            StructElement::load_kids(&element, 1, &NoResolve),
            vec![Kid::Invalid]
        );
    }

    #[test]
    fn the_unfiltered_count_ignores_the_page_and_answers_none_when_absent() {
        assert_eq!(marked_content_id_count(&Dict::new(), &NoResolve), None);
        assert_eq!(
            marked_content_id_count(&dict(&[("K", Object::Int(0))]), &NoResolve),
            Some(1)
        );
        assert_eq!(
            marked_content_id_count(
                &dict(&[(
                    "K",
                    Object::Array(Array::of([Object::Int(2), Object::Int(3)]))
                )]),
                &NoResolve
            ),
            Some(2)
        );
        assert_eq!(
            marked_content_id_count(
                &dict(&[("K", Object::Str(PdfString::literal(b"nope")))]),
                &NoResolve
            ),
            None
        );
    }

    #[test]
    fn a_marked_content_reference_needs_the_name_type_and_a_number_mcid() {
        let good = dict(&[(
            "K",
            Object::Dict(dict(&[
                ("Type", Object::Name(names::MCR.clone())),
                ("MCID", Object::Int(5)),
            ])),
        )]);
        assert_eq!(marked_content_id_at(&good, 0, &NoResolve), Some(5));
        // A dictionary answers at every index, not only zero.
        assert_eq!(marked_content_id_at(&good, 9, &NoResolve), Some(5));

        let wrong_type = dict(&[(
            "K",
            Object::Dict(dict(&[
                ("Type", Object::Name(Name::from("Other"))),
                ("MCID", Object::Int(5)),
            ])),
        )]);
        assert_eq!(marked_content_id_at(&wrong_type, 0, &NoResolve), None);
    }

    #[test]
    fn linking_a_kid_fills_its_element_index_and_leaves_its_slot_alone() {
        // The two indices live in different spaces, and conflating them is
        // how a kid ends up pointing back at an ancestor — which the dump
        // then follows until the stack runs out.
        let inner = dict(&[("S", Object::Name(Name::from("P")))]);
        let mut element = StructElement {
            dict: Dict::new(),
            reference: None,
            kind: b"Document".to_vec(),
            kids: StructElement::load_kids(
                &dict(&[("K", Object::Dict(inner.clone()))]),
                0,
                &NoResolve,
            ),
            parent: None,
        };
        assert!(element.link_kid(&inner, None, 7));
        assert_eq!(
            element.kids,
            vec![Kid::Element {
                dict: inner,
                reference: None,
                // Still slot zero of the parent's `/K` ...
                slot: 0,
                // ... while the element table index is the one just given.
                linked: Some(7),
            }]
        );
    }

    #[test]
    fn an_unmatched_kid_keeps_no_element_index_at_all() {
        let inner = dict(&[("S", Object::Name(Name::from("P")))]);
        let mut element = StructElement {
            dict: Dict::new(),
            reference: None,
            kind: b"Document".to_vec(),
            kids: StructElement::load_kids(&dict(&[("K", Object::Dict(inner))]), 0, &NoResolve),
            parent: None,
        };
        let stranger = dict(&[("S", Object::Name(Name::from("Span")))]);
        assert!(!element.link_kid(&stranger, None, 7));
        assert!(matches!(
            element.kids.first(),
            Some(Kid::Element { linked: None, .. })
        ));
    }

    #[test]
    fn the_role_map_replaces_a_type_only_when_it_names_a_nonempty_one() {
        let map = dict(&[("Quote", Object::Name(Name::from("BlockQuote")))]);
        assert_eq!(map_role(Some(&map), b"Quote"), b"BlockQuote");
        assert_eq!(map_role(Some(&map), b"P"), b"P");
        assert_eq!(map_role(None, b"P"), b"P");
    }
}
