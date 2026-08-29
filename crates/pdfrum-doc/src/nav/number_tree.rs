//! Number trees (ISO 32000-1 §7.9.7): `/Kids` + `/Limits` + `/Nums` leaves,
//! keyed by integer.
//!
//! Two lookups, and the second is the interesting one. [`find`] answers "the
//! value at exactly this key"; [`lower_bound`] answers "the greatest key not
//! above this one, and its value" — which is what turns a sparse table of
//! page-label ranges into a label for every page.
//!
//! Upstream neither caps the depth nor guards against a `/Kids` cycle, so a
//! self-referential tree is unbounded recursion there. Here the depth cap is
//! `Limits::max_name_tree_depth` and exceeding it answers "not found" with a
//! diagnostic.

use pdfrum_common::{DiagKind, Diagnostics, Limits, Severity};
use pdfrum_object::{Dict, Object, Resolve};

use crate::names;

/// The value a lookup found, resolved one level.
pub type Found = Object;

/// Looks up the value stored at exactly `key`.
///
/// Returns nothing when the key falls outside a node's `/Limits`, when a leaf
/// holds no matching entry, or when the depth cap is reached. A leaf never
/// falls through to `/Kids`: a node holding both is a leaf.
#[must_use]
pub fn find<R: Resolve>(
    node: &Dict,
    key: i64,
    r: &R,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> Option<Found> {
    find_at(node, key, 0, r, limits, diags)
}

fn find_at<R: Resolve>(
    node: &Dict,
    key: i64,
    depth: u32,
    r: &R,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> Option<Found> {
    if depth > limits.max_name_tree_depth {
        diags.record(Severity::Suspicious, DiagKind::TreeDepthExceeded, None);
        return None;
    }
    if let Some(bounds) = node.array(names::LIMITS, r) {
        // A missing or mistyped bound reads as zero, so a malformed array
        // prunes as if the range were exactly [0, 0].
        let low = bounds.int_at(0).unwrap_or(0);
        let high = bounds.int_at(1).unwrap_or(0);
        if key < low || key > high {
            return None;
        }
    }
    if let Some(nums) = node.array(names::NUMS, r) {
        for pair in 0..nums.len() / 2 {
            let at = nums.int_at(pair * 2).unwrap_or(0);
            if at == key {
                return nums.get(pair * 2 + 1, r).map(|v| v.get().clone());
            }
            if at > key {
                break;
            }
        }
        return None;
    }
    let kids = node.array(names::KIDS, r)?;
    for index in 0..kids.len() {
        let kid = kids.dict_at(index, r)?;
        if let Some(found) = find_at(&kid, key, depth + 1, r, limits, diags) {
            return Some(found);
        }
    }
    None
}

/// Finds the greatest key at or below `key`, with the value stored there.
///
/// Three details carry behavior:
///
/// - When a node's upper limit is at or below `key`, the search short-circuits
///   to that limit and **re-descends** for its value, which can come back
///   absent while the key is still reported. A page label built from such an
///   answer uses the key for its arithmetic and the decimal fallback for its
///   text.
/// - Both the `/Nums` scan and the `/Kids` walk run **backwards**, which is
///   what makes "greatest key not above" work without assuming the kids are
///   sorted relative to each other.
/// - A leaf never falls through to `/Kids`.
#[must_use]
pub fn lower_bound<R: Resolve>(
    node: &Dict,
    key: i64,
    r: &R,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> Option<(i64, Option<Found>)> {
    lower_bound_at(node, key, 0, r, limits, diags)
}

fn lower_bound_at<R: Resolve>(
    node: &Dict,
    key: i64,
    depth: u32,
    r: &R,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> Option<(i64, Option<Found>)> {
    if depth > limits.max_name_tree_depth {
        diags.record(Severity::Suspicious, DiagKind::TreeDepthExceeded, None);
        return None;
    }
    if let Some(bounds) = node.array(names::LIMITS, r) {
        let low = bounds.int_at(0).unwrap_or(0);
        if key < low {
            return None;
        }
        let high = bounds.int_at(1).unwrap_or(0);
        if key >= high {
            return Some((high, find_at(node, high, depth, r, limits, diags)));
        }
    }
    if let Some(nums) = node.array(names::NUMS, r) {
        for pair in (0..nums.len() / 2).rev() {
            let at = nums.int_at(pair * 2).unwrap_or(0);
            if key >= at {
                return Some((at, nums.get(pair * 2 + 1, r).map(|v| v.get().clone())));
            }
        }
        return None;
    }
    let kids = node.array(names::KIDS, r)?;
    for index in (0..kids.len()).rev() {
        let Some(kid) = kids.dict_at(index, r) else {
            continue;
        };
        if let Some(found) = lower_bound_at(&kid, key, depth + 1, r, limits, diags) {
            return Some(found);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{find, lower_bound};
    use pdfrum_common::{Diagnostics, Limits};
    use pdfrum_object::{Array, Dict, Name, NoResolve, Object};

    fn nums(pairs: &[(i64, i64)]) -> Dict {
        let mut array = Array::new();
        for &(k, v) in pairs {
            array.push(Object::Int(k));
            array.push(Object::Int(v));
        }
        Dict::from_pairs([(Name::from("Nums"), Object::Array(array))])
    }

    fn with_limits(node: Dict, low: i64, high: i64) -> Dict {
        let mut node = node;
        node.push(
            Name::from("Limits"),
            Object::Array(Array::of([Object::Int(low), Object::Int(high)])),
        );
        node
    }

    #[test]
    fn a_leaf_finds_an_exact_key_and_stops_at_a_greater_one() {
        let leaf = nums(&[(0, 10), (5, 20), (9, 30)]);
        let (l, mut d) = (Limits::default(), Diagnostics::default());
        assert_eq!(
            find(&leaf, 5, &NoResolve, &l, &mut d),
            Some(Object::Int(20))
        );
        assert_eq!(find(&leaf, 7, &NoResolve, &l, &mut d), None);
        assert_eq!(
            find(&leaf, 9, &NoResolve, &l, &mut d),
            Some(Object::Int(30))
        );
    }

    #[test]
    fn lower_bound_scans_backwards_for_the_greatest_key_not_above() {
        let leaf = nums(&[(0, 10), (5, 20), (9, 30)]);
        let (l, mut d) = (Limits::default(), Diagnostics::default());
        assert_eq!(
            lower_bound(&leaf, 7, &NoResolve, &l, &mut d),
            Some((5, Some(Object::Int(20))))
        );
        assert_eq!(
            lower_bound(&leaf, 0, &NoResolve, &l, &mut d),
            Some((0, Some(Object::Int(10))))
        );
        assert_eq!(lower_bound(&leaf, -1, &NoResolve, &l, &mut d), None);
    }

    #[test]
    fn the_upper_limit_shortcut_can_report_a_key_with_no_value() {
        // The limits promise a key of 9, but `/Nums` stops at 5, so the
        // re-descent for the value finds nothing while the key survives.
        let leaf = with_limits(nums(&[(0, 10), (5, 20)]), 0, 9);
        let (l, mut d) = (Limits::default(), Diagnostics::default());
        assert_eq!(
            lower_bound(&leaf, 42, &NoResolve, &l, &mut d),
            Some((9, None))
        );
    }

    #[test]
    fn a_leaf_never_falls_through_to_kids() {
        let mut leaf = nums(&[(0, 10)]);
        leaf.push(
            Name::from("Kids"),
            Object::Array(Array::of([Object::Dict(nums(&[(7, 70)]))])),
        );
        let (l, mut d) = (Limits::default(), Diagnostics::default());
        assert_eq!(find(&leaf, 7, &NoResolve, &l, &mut d), None);
    }

    #[test]
    fn a_malformed_limits_array_prunes_as_the_range_zero_to_zero() {
        let mut leaf = nums(&[(4, 40)]);
        leaf.push(
            Name::from("Limits"),
            Object::Array(Array::of([Object::Name(Name::from("x"))])),
        );
        let (l, mut d) = (Limits::default(), Diagnostics::default());
        assert_eq!(find(&leaf, 4, &NoResolve, &l, &mut d), None);
        assert_eq!(find(&leaf, 0, &NoResolve, &l, &mut d), None);
    }

    #[test]
    fn a_kids_cycle_terminates_at_the_depth_cap() {
        // A node whose only kid is a dictionary with the same shape, nested
        // deeper than the cap allows, stands in for a cycle: the walk stops
        // rather than recursing without bound.
        let mut node = nums(&[(0, 0)]);
        node = Dict::from_pairs([(
            Name::from("Kids"),
            Object::Array(Array::of([Object::Dict(node)])),
        )]);
        for _ in 0..64 {
            node = Dict::from_pairs([(
                Name::from("Kids"),
                Object::Array(Array::of([Object::Dict(node)])),
            )]);
        }
        let (l, mut d) = (Limits::default(), Diagnostics::default());
        assert_eq!(find(&node, 0, &NoResolve, &l, &mut d), None);
        assert!(d.contains(&pdfrum_common::DiagKind::TreeDepthExceeded));
    }
}
