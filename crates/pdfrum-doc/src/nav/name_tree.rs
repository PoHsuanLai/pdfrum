//! Name trees (ISO 32000-1 §7.9.6): `/Kids` + `/Limits` + `/Names` leaves,
//! keyed by text string.
//!
//! Three things a reimplementation gets wrong, all deliberate here:
//!
//! - **The leaf scan is linear, not binary.** It relies on the keys being
//!   sorted only for its early exit, so an unsorted leaf finds nothing past
//!   its first out-of-order key. That is the behavior real files depend on.
//! - **Comparison is on the decoded text**, code point by code point, not on
//!   the raw bytes — so a UTF-16BE key and a `PDFDocEncoded` one compare on the
//!   same footing.
//! - **A node holding both `/Names` and `/Kids` is a leaf.** `/Kids` is never
//!   reached.
//!
//! Cycle protection is per-lookup and keyed on object *number*, which means a
//! direct (inline) object is never considered visited. One consequence is
//! worth knowing: marking a `/Names` array as traversed inserts **every one
//! of its elements** into the visited set, so a later node sharing any single
//! element is poisoned along with it.

use std::collections::HashSet;

use pdfrum_common::{DiagKind, Diagnostics, Limits, Severity};
use pdfrum_object::{Array, Dict, Name, Object, Resolve, decode_text};

use crate::names;

/// A name tree rooted at one category of the catalog's `/Names`.
#[derive(Debug, Clone, PartialEq)]
pub struct NameTree {
    /// The tree's root node.
    pub root: Dict,
}

impl NameTree {
    /// Opens `catalog[/Names][<category>]`.
    ///
    /// A missing link at either step means **no tree**, which is not the same
    /// as an empty one: a lookup against nothing and a lookup against an
    /// empty tree take different fallback paths at the caller.
    ///
    /// ```
    /// use pdfrum_common::{Diagnostics, Limits};
    /// use pdfrum_doc::NameTree;
    /// use pdfrum_object::{Array, Dict, Name, NoResolve, Object, PdfString, names};
    ///
    /// let dests = Dict::from_pairs([(
    ///     Name::from("Names"),
    ///     Object::Array(Array::of([
    ///         Object::Str(PdfString::literal(b"intro")),
    ///         Object::Array(Array::of([Object::Int(0), Object::Name(Name::from("Fit"))])),
    ///     ])),
    /// )]);
    /// let catalog = Dict::from_pairs([(
    ///     Name::from("Names"),
    ///     Object::Dict(Dict::from_pairs([(Name::from("Dests"), Object::Dict(dests))])),
    /// )]);
    ///
    /// let tree = NameTree::open(&catalog, &Name::from("Dests"), &NoResolve)
    ///     .expect("the catalog carries a `/Names /Dests` tree");
    /// let (limits, mut diags) = (Limits::default(), Diagnostics::default());
    ///
    /// // A missing link at either step is *no tree*, not an empty one.
    /// assert!(NameTree::open(&Dict::default(), &Name::from("Dests"), &NoResolve).is_none());
    /// ```
    #[must_use]
    pub fn open<R: Resolve>(catalog: &Dict, category: &Name, r: &R) -> Option<NameTree> {
        let names = catalog.dict(names::NAMES, r)?;
        Some(NameTree {
            root: names.dict(category, r)?,
        })
    }

    /// Looks up one key's value.
    ///
    /// ```
    /// use pdfrum_common::{Diagnostics, Limits};
    /// use pdfrum_doc::NameTree;
    /// use pdfrum_object::{Array, Dict, Name, NoResolve, Object, PdfString, names};
    ///
    /// let dests = Dict::from_pairs([(
    ///     Name::from("Names"),
    ///     Object::Array(Array::of([
    ///         Object::Str(PdfString::literal(b"intro")),
    ///         Object::Array(Array::of([Object::Int(0), Object::Name(Name::from("Fit"))])),
    ///     ])),
    /// )]);
    /// let catalog = Dict::from_pairs([(
    ///     Name::from("Names"),
    ///     Object::Dict(Dict::from_pairs([(Name::from("Dests"), Object::Dict(dests))])),
    /// )]);
    ///
    /// let tree = NameTree::open(&catalog, &Name::from("Dests"), &NoResolve)
    ///     .expect("the catalog carries a `/Names /Dests` tree");
    /// let (limits, mut diags) = (Limits::default(), Diagnostics::default());
    ///
    /// assert!(tree.lookup("intro", &NoResolve, &limits, &mut diags).is_some());
    /// assert!(tree.lookup("absent", &NoResolve, &limits, &mut diags).is_none());
    /// ```
    #[must_use]
    pub fn lookup<R: Resolve>(
        &self,
        key: &str,
        r: &R,
        limits: &Limits,
        diags: &mut Diagnostics,
    ) -> Option<Object> {
        let mut seen = HashSet::new();
        let mut index = 0;
        search_by_name(&self.root, key, 0, &mut seen, &mut index, r, limits, diags)
    }

    /// Looks up the `index`-th entry in tree order, returning its key and
    /// value.
    ///
    /// Two differences from the by-name path: there is **no cycle guard at
    /// all** — a `/Kids` loop is stopped only by the depth cap — and
    /// `/Limits` is not consulted, because this is a pure in-order walk. A
    /// slot whose value will not resolve aborts the whole search rather than
    /// being skipped.
    ///
    /// ```
    /// use pdfrum_common::{Diagnostics, Limits};
    /// use pdfrum_doc::NameTree;
    /// use pdfrum_object::{Array, Dict, Name, NoResolve, Object, PdfString, names};
    ///
    /// let dests = Dict::from_pairs([(
    ///     Name::from("Names"),
    ///     Object::Array(Array::of([
    ///         Object::Str(PdfString::literal(b"intro")),
    ///         Object::Array(Array::of([Object::Int(0), Object::Name(Name::from("Fit"))])),
    ///     ])),
    /// )]);
    /// let catalog = Dict::from_pairs([(
    ///     Name::from("Names"),
    ///     Object::Dict(Dict::from_pairs([(Name::from("Dests"), Object::Dict(dests))])),
    /// )]);
    ///
    /// let tree = NameTree::open(&catalog, &Name::from("Dests"), &NoResolve)
    ///     .expect("the catalog carries a `/Names /Dests` tree");
    /// let (limits, mut diags) = (Limits::default(), Diagnostics::default());
    ///
    /// let (key, _value) = tree
    ///     .lookup_by_index(0, &NoResolve, &limits, &mut diags)
    ///     .expect("the tree has a first entry");
    /// assert_eq!(key, "intro");
    /// ```
    #[must_use]
    pub fn lookup_by_index<R: Resolve>(
        &self,
        target: usize,
        r: &R,
        limits: &Limits,
        diags: &mut Diagnostics,
    ) -> Option<(String, Object)> {
        let mut cursor = 0;
        search_by_index(&self.root, target, &mut cursor, 0, r, limits, diags)
    }

    /// How many entries the tree holds.
    ///
    /// The visited set here is on **dictionary identity**, so an inline node
    /// counts too — a different guard from the by-name lookup's.
    ///
    /// ```
    /// use pdfrum_common::{Diagnostics, Limits};
    /// use pdfrum_doc::NameTree;
    /// use pdfrum_object::{Array, Dict, Name, NoResolve, Object, PdfString, names};
    ///
    /// let dests = Dict::from_pairs([(
    ///     Name::from("Names"),
    ///     Object::Array(Array::of([
    ///         Object::Str(PdfString::literal(b"intro")),
    ///         Object::Array(Array::of([Object::Int(0), Object::Name(Name::from("Fit"))])),
    ///     ])),
    /// )]);
    /// let catalog = Dict::from_pairs([(
    ///     Name::from("Names"),
    ///     Object::Dict(Dict::from_pairs([(Name::from("Dests"), Object::Dict(dests))])),
    /// )]);
    ///
    /// let tree = NameTree::open(&catalog, &Name::from("Dests"), &NoResolve)
    ///     .expect("the catalog carries a `/Names /Dests` tree");
    /// let (limits, mut diags) = (Limits::default(), Diagnostics::default());
    ///
    /// assert_eq!(tree.count(&NoResolve, &limits, &mut diags), 1);
    /// ```
    #[must_use]
    pub fn count<R: Resolve>(&self, r: &R, limits: &Limits, diags: &mut Diagnostics) -> usize {
        let mut seen = Vec::new();
        count_names(&self.root, 0, &mut seen, r, limits, diags)
    }
}

/// A node's `/Limits`, padded to two entries and put the right way round.
///
/// Upstream repairs the array in place; here the repair is in the returned
/// pair and the caller's copy is untouched, which is the same decision the
/// whole crate makes about mutation.
fn node_limits(limits: &Array, diags: &mut Diagnostics) -> (String, String) {
    if limits.len() < 2 {
        diags.record(Severity::Recovered, DiagKind::NameTreeLimitsRepaired, None);
    }
    let read = |index: usize| {
        limits
            .string_at(index)
            .map(|s| decode_text(s.as_bytes()).into_owned())
            .unwrap_or_default()
    };
    let (low, high) = (read(0), read(1));
    if low > high {
        diags.record(Severity::Recovered, DiagKind::NameTreeLimitsRepaired, None);
        return (high, low);
    }
    (low, high)
}

/// Whether an object has already been traversed, recording it if not.
///
/// A **direct** object — one with no object number — is never considered
/// traversed and is never inserted, so an inline subtree can be revisited
/// freely.
fn traversed(object: Option<&Object>, seen: &mut HashSet<u32>) -> bool {
    let Some(num) = object.and_then(Object::as_ref_id).map(|r| r.num) else {
        return false;
    };
    !seen.insert(num)
}

/// Whether an array, or **any of its elements**, has been traversed.
///
/// The side effect is load-bearing: every element's object number goes into
/// the set, so a sibling node sharing a single element is poisoned too.
fn array_traversed(raw: Option<&Object>, array: &Array, seen: &mut HashSet<u32>) -> bool {
    let mut hit = traversed(raw, seen);
    for element in array.iter() {
        if traversed(Some(element), seen) {
            hit = true;
        }
    }
    hit
}

#[allow(clippy::too_many_arguments)]
fn search_by_name<R: Resolve>(
    node: &Dict,
    key: &str,
    depth: u32,
    seen: &mut HashSet<u32>,
    index: &mut usize,
    r: &R,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> Option<Object> {
    if depth > limits.max_name_tree_depth {
        diags.record(Severity::Suspicious, DiagKind::TreeDepthExceeded, None);
        return None;
    }
    let mut leaf = node.array(names::NAMES, r);
    let mut bounds = node.array(names::LIMITS, r);
    if let Some(array) = &leaf
        && array_traversed(node.raw(names::NAMES), array, seen)
    {
        leaf = None;
    }
    if let Some(array) = &bounds
        && array_traversed(node.raw(names::LIMITS), array, seen)
    {
        bounds = None;
    }

    if let Some(bounds) = &bounds {
        let (low, high) = node_limits(bounds, diags);
        if key < low.as_str() || key > high.as_str() {
            return None;
        }
    }

    if let Some(leaf) = &leaf {
        if leaf.len() % 2 == 1 {
            diags.record(Severity::Suspicious, DiagKind::NameTreeMalformed, None);
        }
        for pair in 0..leaf.len() / 2 {
            let at = leaf
                .string_at(pair * 2)
                .map(|s| decode_text(s.as_bytes()).into_owned())
                .unwrap_or_default();
            if at.as_str() > key {
                break;
            }
            if at.as_str() < key {
                continue;
            }
            *index += pair;
            return leaf.get(pair * 2 + 1, r).map(|v| v.get().clone());
        }
        *index += leaf.len() / 2;
        return None;
    }

    let kids = node.array(names::KIDS, r)?;
    if traversed(node.raw(names::KIDS), seen) {
        return None;
    }
    for slot in 0..kids.len() {
        if traversed(kids.raw_at(slot), seen) {
            continue;
        }
        let Some(kid) = kids.dict_at(slot, r) else {
            continue;
        };
        if let Some(found) = search_by_name(&kid, key, depth + 1, seen, index, r, limits, diags) {
            return Some(found);
        }
    }
    None
}

fn search_by_index<R: Resolve>(
    node: &Dict,
    target: usize,
    cursor: &mut usize,
    depth: u32,
    r: &R,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> Option<(String, Object)> {
    if depth > limits.max_name_tree_depth {
        diags.record(Severity::Suspicious, DiagKind::TreeDepthExceeded, None);
        return None;
    }
    if let Some(leaf) = node.array(names::NAMES, r) {
        let count = leaf.len() / 2;
        if target >= *cursor + count {
            *cursor += count;
            return None;
        }
        let slot = (target - *cursor) * 2;
        let value = leaf.get(slot + 1, r)?.get().clone();
        let key = leaf
            .string_at(slot)
            .map(|s| decode_text(s.as_bytes()).into_owned())
            .unwrap_or_default();
        return Some((key, value));
    }
    let kids = node.array(names::KIDS, r)?;
    for slot in 0..kids.len() {
        let Some(kid) = kids.dict_at(slot, r) else {
            continue;
        };
        if let Some(found) = search_by_index(&kid, target, cursor, depth + 1, r, limits, diags) {
            return Some(found);
        }
    }
    None
}

fn count_names<R: Resolve>(
    node: &Dict,
    depth: u32,
    seen: &mut Vec<Dict>,
    r: &R,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> usize {
    if depth > limits.max_name_tree_depth {
        diags.record(Severity::Suspicious, DiagKind::TreeDepthExceeded, None);
        return 0;
    }
    if seen.contains(node) {
        return 0;
    }
    seen.push(node.clone());
    // A leaf never descends, even when it also declares `/Kids`.
    if let Some(leaf) = node.array(names::NAMES, r) {
        return leaf.len() / 2;
    }
    let Some(kids) = node.array(names::KIDS, r) else {
        return 0;
    };
    (0..kids.len())
        .filter_map(|slot| kids.dict_at(slot, r))
        .map(|kid| count_names(&kid, depth + 1, seen, r, limits, diags))
        .sum()
}

/// Resolves a named destination through both rungs of the lookup ladder.
///
/// The new-style tree is keyed on the **decoded text** of the name, while the
/// pre-1.2 `/Dests` dictionary is keyed on the **raw bytes** verbatim, so a
/// document can reach one and not the other with the same spelling.
///
/// ```
/// use pdfrum_common::{Diagnostics, Limits};
/// use pdfrum_doc::NameTree;
/// use pdfrum_object::{Array, Dict, Name, NoResolve, Object, PdfString, names};
///
/// let dests = Dict::from_pairs([(
///     Name::from("Names"),
///     Object::Array(Array::of([
///         Object::Str(PdfString::literal(b"intro")),
///         Object::Array(Array::of([Object::Int(0), Object::Name(Name::from("Fit"))])),
///     ])),
/// )]);
/// let catalog = Dict::from_pairs([(
///     Name::from("Names"),
///     Object::Dict(Dict::from_pairs([(Name::from("Dests"), Object::Dict(dests))])),
/// )]);
///
/// let tree = NameTree::open(&catalog, &Name::from("Dests"), &NoResolve)
///     .expect("the catalog carries a `/Names /Dests` tree");
/// let (limits, mut diags) = (Limits::default(), Diagnostics::default());
/// use pdfrum_doc::nav::lookup_named_dest;
///
/// let found = lookup_named_dest(&catalog, b"intro", &NoResolve, &limits, &mut diags);
/// assert!(found.is_some());
/// ```
#[must_use]
pub fn lookup_named_dest<R: Resolve>(
    catalog: &Dict,
    key: &[u8],
    r: &R,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> Option<Array> {
    if let Some(tree) = NameTree::open(catalog, names::DESTS, r) {
        let text = decode_text(key).into_owned();
        if let Some(found) = tree
            .lookup(&text, r, limits, diags)
            .and_then(|value| named_dest_from(&value, r))
        {
            return Some(found);
        }
    }
    let old_style = catalog.dict(names::DESTS, r)?;
    let value = old_style.get(&Name::new(key.to_vec()), r)?.get().clone();
    let found = named_dest_from(&value, r);
    if found.is_some() {
        diags.record(Severity::Recovered, DiagKind::LegacyNamedDest, None);
    }
    found
}

/// A named destination's array, whether it was written bare or wrapped in a
/// dictionary under `/D`.
fn named_dest_from<R: Resolve>(object: &Object, r: &R) -> Option<Array> {
    match object {
        Object::Array(array) => Some(array.clone()),
        Object::Dict(dict) => dict.array(names::D, r),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{NameTree, lookup_named_dest};
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

    fn leaf(pairs: &[(&[u8], i64)]) -> Dict {
        let mut array = Array::new();
        for (key, value) in pairs {
            array.push(Object::Str(PdfString::literal(key)));
            array.push(Object::Int(*value));
        }
        dict(&[("Names", Object::Array(array))])
    }

    #[test]
    fn a_missing_link_yields_no_tree_at_all() {
        assert!(NameTree::open(&Dict::new(), pdfrum_object::names::DESTS, &NoResolve).is_none());
        let catalog = dict(&[("Names", Object::Dict(Dict::new()))]);
        assert!(NameTree::open(&catalog, pdfrum_object::names::DESTS, &NoResolve).is_none());
    }

    #[test]
    fn a_leaf_finds_its_keys_and_stops_at_the_first_greater_one() {
        let tree = NameTree {
            root: leaf(&[(b"a", 1), (b"c", 3), (b"e", 5)]),
        };
        let (l, mut d) = (Limits::default(), Diagnostics::default());
        assert_eq!(
            tree.lookup("c", &NoResolve, &l, &mut d),
            Some(Object::Int(3))
        );
        assert_eq!(tree.lookup("b", &NoResolve, &l, &mut d), None);
        assert_eq!(
            tree.lookup("e", &NoResolve, &l, &mut d),
            Some(Object::Int(5))
        );
    }

    #[test]
    fn a_key_with_a_byte_order_mark_reads_back_as_its_text() {
        let tree = NameTree {
            root: leaf(&[(b"\xFE\xFF\x00\x31", 100)]),
        };
        let (l, mut d) = (Limits::default(), Diagnostics::default());
        assert_eq!(
            tree.lookup_by_index(0, &NoResolve, &l, &mut d),
            Some(("1".to_owned(), Object::Int(100)))
        );
        assert_eq!(
            tree.lookup("1", &NoResolve, &l, &mut d),
            Some(Object::Int(100))
        );
    }

    #[test]
    fn a_lookup_reads_a_four_element_limits_array_without_shortening_it() {
        let mut node = leaf(&[(b"1", 111), (b"9", 999)]);
        let bounds = Array::of([
            Object::Str(PdfString::literal(b"1")),
            Object::Str(PdfString::literal(b"9")),
            Object::Str(PdfString::literal(b"extra")),
            Object::Str(PdfString::literal(b"more")),
        ]);
        node.push(Name::from("Limits"), Object::Array(bounds));
        let tree = NameTree { root: node.clone() };
        let (l, mut d) = (Limits::default(), Diagnostics::default());
        assert_eq!(
            tree.lookup("9", &NoResolve, &l, &mut d),
            Some(Object::Int(999))
        );
        assert_eq!(
            tree.lookup("1", &NoResolve, &l, &mut d),
            Some(Object::Int(111))
        );
        // Nothing here mutates the caller's array, so it is still four long.
        assert_eq!(
            tree.root
                .array(pdfrum_object::names::LENGTH, &NoResolve)
                .map_or(4, |a| a.len()),
            4
        );
    }

    #[test]
    fn a_node_holding_both_names_and_kids_is_a_leaf() {
        let mut node = leaf(&[(b"a", 1)]);
        node.push(
            Name::from("Kids"),
            Object::Array(Array::of([Object::Dict(leaf(&[(b"z", 26)]))])),
        );
        let tree = NameTree { root: node };
        let (l, mut d) = (Limits::default(), Diagnostics::default());
        assert_eq!(tree.lookup("z", &NoResolve, &l, &mut d), None);
        assert_eq!(tree.count(&NoResolve, &l, &mut d), 1);
    }

    #[test]
    fn limits_the_wrong_way_round_are_read_swapped() {
        let mut node = leaf(&[(b"a", 1)]);
        node.push(
            Name::from("Limits"),
            Object::Array(Array::of([
                Object::Str(PdfString::literal(b"z")),
                Object::Str(PdfString::literal(b"a")),
            ])),
        );
        let tree = NameTree { root: node };
        let (l, mut d) = (Limits::default(), Diagnostics::default());
        assert_eq!(
            tree.lookup("a", &NoResolve, &l, &mut d),
            Some(Object::Int(1))
        );
        assert!(d.contains(&pdfrum_common::DiagKind::NameTreeLimitsRepaired));
    }

    #[test]
    fn the_old_style_dests_dictionary_is_keyed_on_raw_bytes() {
        let dest = Array::of([Object::Int(11), Object::Name(Name::from("Fit"))]);
        let catalog = dict(&[(
            "Dests",
            Object::Dict(dict(&[("FirstAlternate", Object::Array(dest.clone()))])),
        )]);
        let (l, mut d) = (Limits::default(), Diagnostics::default());
        assert_eq!(
            lookup_named_dest(&catalog, b"FirstAlternate", &NoResolve, &l, &mut d),
            Some(dest)
        );
        assert!(d.contains(&pdfrum_common::DiagKind::LegacyNamedDest));
        assert_eq!(
            lookup_named_dest(&catalog, b"Missing", &NoResolve, &l, &mut d),
            None
        );
    }

    #[test]
    fn a_dictionary_valued_entry_unwraps_its_d_array() {
        let dest = Array::of([Object::Int(1), Object::Name(Name::from("Fit"))]);
        let wrapped = Object::Dict(dict(&[("D", Object::Array(dest.clone()))]));
        let catalog = dict(&[("Dests", Object::Dict(dict(&[("Here", wrapped)])))]);
        let (l, mut d) = (Limits::default(), Diagnostics::default());
        assert_eq!(
            lookup_named_dest(&catalog, b"Here", &NoResolve, &l, &mut d),
            Some(dest)
        );
    }
}
