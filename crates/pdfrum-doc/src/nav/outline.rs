//! The document outline (ISO 32000-1 §12.3.3), commonly called bookmarks.
//!
//! There is no tree object: an outline is a `/First`-and-`/Next` linked
//! structure over dictionaries, walked pre-order. The cycle guard is a
//! visited set of object references, and it is checked in **two** places —
//! on entry to a node and again on each sibling step — because a
//! self-referential root must be visited exactly once before the walk stops.

use std::collections::HashSet;

use pdfrum_common::{DiagKind, Diagnostics, Limits, Severity};
use pdfrum_object::{Dict, ObjRef, Object, Resolve, decode_text, names as obj_names};

use crate::names;
use crate::nav::action::Action;
use crate::nav::dest::Dest;

/// One outline item.
#[derive(Debug, Clone, PartialEq)]
pub struct Bookmark {
    /// The item's dictionary.
    pub dict: Dict,
    /// How deep in the outline it sits; the top level is zero.
    pub depth: usize,
}

impl Bookmark {
    /// The item's title.
    ///
    /// Two details: `/Title` is **type-filtered to a string**, so a
    /// name-valued title reads as empty; and every code unit below `0x20` is
    /// **raised to a space**, so control characters become blanks without
    /// changing the length. The second matters because titles are compared
    /// case-insensitively when searching.
    #[must_use]
    pub fn title<R: Resolve>(&self, r: &R) -> String {
        let Some(raw) = self
            .dict
            .get(obj_names::TITLE, r)
            .and_then(|v| v.get().as_string().map(|s| s.bytes.to_vec()))
        else {
            return String::new();
        };
        decode_text(&raw)
            .chars()
            .map(|c| if (c as u32) < 0x20 { ' ' } else { c })
            .collect()
    }

    /// `/Count`: positive for an open item with that many visible
    /// descendants, negative for a closed one, zero for a leaf.
    ///
    /// The sign is preserved and the value is not masked.
    #[must_use]
    pub fn count<R: Resolve>(&self, r: &R) -> i64 {
        self.dict.int(obj_names::COUNT, r).unwrap_or(0)
    }

    /// `/F`, the style flag word, **unmasked** — a file writing 15 reports
    /// 15, not the two defined bits.
    #[must_use]
    pub fn style<R: Resolve>(&self, r: &R) -> i64 {
        self.dict.int(names::F, r).unwrap_or(0)
    }

    /// `/C`, the item's colour, as three components in `[0, 1]`.
    ///
    /// The array must hold **exactly three** entries and every one must be a
    /// number inside the unit interval; anything else is no colour. Upstream
    /// dereferences a non-number here without checking, which is a latent
    /// crash — we answer `None`.
    #[must_use]
    pub fn color<R: Resolve>(&self, r: &R) -> Option<(f32, f32, f32)> {
        let array = self.dict.array(names::C, r)?;
        if array.len() != 3 {
            return None;
        }
        let component = |index: usize| {
            array
                .number_obj_at(index)
                .and_then(Object::number)
                .filter(|value| (0.0..=1.0).contains(value))
        };
        Some((component(0)?, component(1)?, component(2)?))
    }

    /// The item's action.
    #[must_use]
    pub fn action<R: Resolve>(&self, r: &R) -> Option<Action> {
        self.dict.dict(names::A, r).map(Action::new)
    }

    /// Where the item goes: `/Dest` first, the action's destination only when
    /// that yields no array.
    #[must_use]
    pub fn dest<R: Resolve>(
        &self,
        catalog: &Dict,
        r: &R,
        limits: &Limits,
        diags: &mut Diagnostics,
    ) -> Dest {
        let own = self.dict.get(names::DEST, r).map(|d| d.get().clone());
        let dest = Dest::create(catalog, own.as_ref(), r, limits, diags);
        if dest.array.is_some() {
            return dest;
        }
        self.action(r)
            .map(|action| action.dest(catalog, r, limits, diags))
            .unwrap_or_default()
    }
}

/// Walks the whole outline pre-order: each item, then its subtree, then its
/// next sibling.
#[must_use]
pub fn walk<R: Resolve>(
    catalog: &Dict,
    r: &R,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> Vec<Bookmark> {
    let mut out = Vec::new();
    let Some(outlines) = catalog.dict(names::OUTLINES, r) else {
        return out;
    };
    let mut seen = HashSet::new();
    let first = outlines.dict(names::FIRST, r);
    let first_ref = outlines.reference(names::FIRST);
    if let Some(first) = first {
        visit(&first, first_ref, 0, &mut seen, &mut out, r, limits, diags);
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn visit<R: Resolve>(
    dict: &Dict,
    reference: Option<ObjRef>,
    depth: usize,
    seen: &mut HashSet<ObjRef>,
    out: &mut Vec<Bookmark>,
    r: &R,
    limits: &Limits,
    diags: &mut Diagnostics,
) {
    if depth > limits.max_name_tree_depth as usize {
        diags.record(Severity::Suspicious, DiagKind::TreeDepthExceeded, None);
        return;
    }
    let mut node = dict.clone();
    let mut node_ref = reference;
    loop {
        if let Some(reference) = node_ref
            && !seen.insert(reference)
        {
            diags.record(Severity::Recovered, DiagKind::NavigationCycle, None);
            return;
        }
        out.push(Bookmark {
            dict: node.clone(),
            depth,
        });
        if let Some(child) = node.dict(names::FIRST, r) {
            let child_ref = node.reference(names::FIRST);
            visit(&child, child_ref, depth + 1, seen, out, r, limits, diags);
        }
        // A `/Next` pointing back at this very node ends the chain; that is
        // a self-reference test, not a structural comparison, so a duplicate
        // written inline is a distinct sibling.
        let next_ref = node.reference(names::NEXT);
        if next_ref.is_some() && next_ref == node_ref {
            return;
        }
        let Some(next) = node.dict(names::NEXT, r) else {
            return;
        };
        node = next;
        node_ref = next_ref;
    }
}

/// Finds the first item whose title matches, comparing case-insensitively.
#[must_use]
// Reached only by the tests beside it now that the module is private; the
// library compiles once without `cfg(test)`, so `dead_code` fires. §WP8's
// recurring cost — the item is pinned by a test, not unreachable.
#[allow(dead_code)]
pub(crate) fn find<R: Resolve>(
    catalog: &Dict,
    title: &str,
    r: &R,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> Option<Bookmark> {
    let wanted = title.to_lowercase();
    walk(catalog, r, limits, diags)
        .into_iter()
        .find(|bookmark| bookmark.title(r).to_lowercase() == wanted)
}

#[cfg(test)]
mod tests {
    use super::{Bookmark, find, walk};
    use pdfrum_common::{Diagnostics, Limits};
    use pdfrum_object::{Array, Dict, Name, NoResolve, Object, PdfString};

    fn dict(pairs: &[(&str, Object)]) -> Dict {
        Dict::from_pairs(
            pairs
                .iter()
                .map(|(k, v)| (Name::from(*k), v.clone()))
                .collect::<Vec<_>>(),
        )
    }

    fn bookmark(pairs: &[(&str, Object)]) -> Bookmark {
        Bookmark {
            dict: dict(pairs),
            depth: 0,
        }
    }

    #[test]
    fn a_title_is_type_filtered_and_has_its_control_characters_raised() {
        let named = bookmark(&[("Title", Object::Name(Name::from("Chapter")))]);
        assert_eq!(named.title(&NoResolve), "");

        let with_controls = bookmark(&[("Title", Object::Str(PdfString::literal(b"A\tB\x01C")))]);
        // The length is unchanged; only the code points move up.
        assert_eq!(with_controls.title(&NoResolve), "A B C");
    }

    #[test]
    fn the_count_keeps_its_sign_and_is_not_masked() {
        assert_eq!(
            bookmark(&[("Count", Object::Int(-2))]).count(&NoResolve),
            -2
        );
        assert_eq!(bookmark(&[]).count(&NoResolve), 0);
        assert_eq!(bookmark(&[("F", Object::Int(15))]).style(&NoResolve), 15);
        // A non-integer style reads as zero.
        assert_eq!(
            bookmark(&[("F", Object::Name(Name::from("x")))]).style(&NoResolve),
            0
        );
    }

    #[test]
    fn a_colour_needs_exactly_three_numbers_inside_the_unit_interval() {
        let good = bookmark(&[(
            "C",
            Object::Array(Array::of([0.1_f32, 0.2, 0.3].map(Object::from))),
        )]);
        assert_eq!(good.color(&NoResolve), Some((0.1, 0.2, 0.3)));

        for bad in [
            Object::Array(Array::of([0.1_f32, 0.2].map(Object::from))),
            Object::Array(Array::of([0.1_f32, 0.2, 0.3, 0.4].map(Object::from))),
            Object::Array(Array::of([1.5_f32, 0.2, 0.3].map(Object::from))),
            Object::Array(Array::of([-0.1_f32, 0.2, 0.3].map(Object::from))),
            Object::Int(4),
        ] {
            assert_eq!(bookmark(&[("C", bad)]).color(&NoResolve), None);
        }
    }

    #[test]
    fn a_non_numeric_component_answers_none_rather_than_crashing() {
        let malformed = bookmark(&[(
            "C",
            Object::Array(Array::of([
                Object::Name(Name::from("red")),
                Object::from(0.2_f32),
                Object::from(0.3_f32),
            ])),
        )]);
        assert_eq!(malformed.color(&NoResolve), None);
    }

    #[test]
    fn the_walk_is_pre_order_with_depth() {
        // root -> (child, sibling); child has one grandchild.
        let grandchild = dict(&[("Title", Object::Str(PdfString::literal(b"grandchild")))]);
        let child = dict(&[
            ("Title", Object::Str(PdfString::literal(b"child"))),
            ("First", Object::Dict(grandchild)),
        ]);
        let sibling = dict(&[("Title", Object::Str(PdfString::literal(b"sibling")))]);
        let root = dict(&[
            ("Title", Object::Str(PdfString::literal(b"root"))),
            ("First", Object::Dict(child)),
            ("Next", Object::Dict(sibling)),
        ]);
        let catalog = dict(&[(
            "Outlines",
            Object::Dict(dict(&[("First", Object::Dict(root))])),
        )]);
        let (l, mut d) = (Limits::default(), Diagnostics::default());
        let items = walk(&catalog, &NoResolve, &l, &mut d);
        let seen: Vec<_> = items
            .iter()
            .map(|b| (b.title(&NoResolve), b.depth))
            .collect();
        assert_eq!(
            seen,
            [
                ("root".to_owned(), 0),
                ("child".to_owned(), 1),
                ("grandchild".to_owned(), 2),
                ("sibling".to_owned(), 0),
            ]
        );
    }

    #[test]
    fn finding_a_title_ignores_case() {
        let root = dict(&[(
            "Title",
            Object::Str(PdfString::literal(b"A Good Beginning")),
        )]);
        let catalog = dict(&[(
            "Outlines",
            Object::Dict(dict(&[("First", Object::Dict(root))])),
        )]);
        let (l, mut d) = (Limits::default(), Diagnostics::default());
        assert!(find(&catalog, "a good beginning", &NoResolve, &l, &mut d).is_some());
        assert!(find(&catalog, "A BAD Beginning", &NoResolve, &l, &mut d).is_none());
    }

    #[test]
    fn an_outline_with_no_entries_walks_to_nothing() {
        let (l, mut d) = (Limits::default(), Diagnostics::default());
        assert!(walk(&Dict::new(), &NoResolve, &l, &mut d).is_empty());
        let empty = dict(&[("Outlines", Object::Dict(Dict::new()))]);
        assert!(walk(&empty, &NoResolve, &l, &mut d).is_empty());
    }
}
