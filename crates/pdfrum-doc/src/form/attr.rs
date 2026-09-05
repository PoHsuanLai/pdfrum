//! The inherited-attribute walk and the fully-qualified field name.
//!
//! A form field's attributes are looked up on the field itself and then up
//! its `/Parent` chain, which is how a group of radio buttons shares one
//! `/FT` and one `/Ff`. Upstream has **no cycle guard** here — a `/Parent`
//! loop is stopped only by the depth cap — and a non-dictionary parent ends
//! the walk.

use pdfrum_common::{DiagKind, Diagnostics, Limits, Severity};
use pdfrum_object::{Dict, Name, Object, Resolve};

use crate::names;

/// Looks an attribute up on a field and then up its ancestors.
///
/// The value is resolved one level, so an attribute written as a reference
/// comes back as what it points at.
///
/// ```
/// use pdfrum_common::{Diagnostics, Limits};
/// use pdfrum_doc::form::field_attr;
/// use pdfrum_object::{Dict, Name, NoResolve, Object};
///
/// // `/Ff` on the parent is inherited by the child that omits it.
/// let parent = Dict::from_pairs([(Name::from("Ff"), Object::Int(1))]);
/// let child = Dict::from_pairs([(Name::from("Parent"), Object::Dict(parent))]);
///
/// let (limits, mut diags) = (Limits::default(), Diagnostics::default());
/// let found = field_attr(&child, &Name::from("Ff"), &NoResolve, &limits, &mut diags);
/// assert_eq!(found, Some(Object::Int(1)));
/// ```
#[must_use]
pub fn field_attr<R: Resolve>(
    dict: &Dict,
    key: &Name,
    r: &R,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> Option<Object> {
    let mut level = Some(dict.clone());
    let mut depth = 0;
    while let Some(node) = level {
        if depth > limits.max_name_tree_depth {
            diags.record(Severity::Suspicious, DiagKind::TreeDepthExceeded, None);
            return None;
        }
        if let Some(found) = node.get(key, r) {
            return Some(found.get().clone());
        }
        level = node.dict(names::PARENT, r);
        depth += 1;
    }
    None
}

/// A field's fully-qualified name: its ancestors' `/T` values and its own,
/// joined with dots from the root down.
///
/// Two behaviors that look like bugs and are not:
///
/// - A level with an **empty or absent `/T` is skipped without a separator**,
///   so a grandparent `a`, an unnamed parent and a child `b` give `a.b`
///   rather than `a..b`.
/// - The cycle check runs **after** advancing to the parent, so the node that
///   closes a cycle is processed once more than a check-before-advance guard
///   would allow. A four-node ring therefore reports three components,
///   rotated differently depending on where the walk started.
///
/// ```
/// use pdfrum_doc::form::full_name;
/// use pdfrum_object::{Dict, Name, NoResolve, Object, PdfString};
///
/// let t = |s: &[u8]| (Name::from("T"), Object::Str(PdfString::literal(s)));
///
/// // An unnamed level is skipped without a separator: `a.b`, not `a..b`.
/// let root = Dict::from_pairs([t(b"a")]);
/// let middle = Dict::from_pairs([(Name::from("Parent"), Object::Dict(root))]);
/// let leaf = Dict::from_pairs([t(b"b"), (Name::from("Parent"), Object::Dict(middle))]);
/// assert_eq!(full_name(&leaf, &NoResolve), "a.b");
/// ```
#[must_use]
pub fn full_name<R: Resolve>(dict: &Dict, r: &R) -> String {
    let mut full = String::new();
    let mut visited: Vec<Dict> = Vec::new();
    let mut level = Some(dict.clone());

    while let Some(node) = level {
        visited.push(node.clone());
        let short = node.text(names::T, r).unwrap_or_default();
        if !short.is_empty() {
            full = if full.is_empty() {
                short
            } else {
                format!("{short}.{full}")
            };
        }
        level = node.dict(names::PARENT, r);
        // Checked here, after the step, which is what lets the entry node be
        // seen twice.
        if let Some(next) = &level
            && visited.contains(next)
        {
            break;
        }
    }
    full
}

#[cfg(test)]
mod tests {
    use super::{field_attr, full_name};
    use pdfrum_common::{Diagnostics, Limits};
    use pdfrum_object::{Dict, Name, NoResolve, Object, PdfString};

    fn dict(pairs: &[(&str, Object)]) -> Dict {
        Dict::from_pairs(
            pairs
                .iter()
                .map(|(k, v)| (Name::from(*k), v.clone()))
                .collect::<Vec<_>>(),
        )
    }

    fn named(text: &str) -> Object {
        Object::Str(PdfString::literal(text.as_bytes()))
    }

    #[test]
    fn an_attribute_is_found_on_an_ancestor() {
        let parent = dict(&[("FT", Object::Name(Name::from("Btn")))]);
        let child = dict(&[("Parent", Object::Dict(parent))]);
        let (l, mut d) = (Limits::default(), Diagnostics::default());
        assert_eq!(
            field_attr(&child, &Name::from("FT"), &NoResolve, &l, &mut d),
            Some(Object::Name(Name::from("Btn")))
        );
        assert_eq!(
            field_attr(&child, &Name::from("Ff"), &NoResolve, &l, &mut d),
            None
        );
    }

    #[test]
    fn the_fields_own_value_wins_over_its_parents() {
        let parent = dict(&[("Ff", Object::Int(1))]);
        let child = dict(&[("Ff", Object::Int(2)), ("Parent", Object::Dict(parent))]);
        let (l, mut d) = (Limits::default(), Diagnostics::default());
        assert_eq!(
            field_attr(&child, &Name::from("Ff"), &NoResolve, &l, &mut d),
            Some(Object::Int(2))
        );
    }

    #[test]
    fn a_non_dictionary_parent_ends_the_walk() {
        let child = dict(&[("Parent", Object::Int(4))]);
        let (l, mut d) = (Limits::default(), Diagnostics::default());
        assert_eq!(
            field_attr(&child, &Name::from("FT"), &NoResolve, &l, &mut d),
            None
        );
    }

    #[test]
    fn a_level_with_no_name_is_skipped_without_a_separator() {
        let grandparent = dict(&[("T", named("a"))]);
        let parent = dict(&[("Parent", Object::Dict(grandparent))]);
        let child = dict(&[("T", named("b")), ("Parent", Object::Dict(parent))]);
        assert_eq!(full_name(&child, &NoResolve), "a.b");
    }

    #[test]
    fn a_field_with_no_ancestors_is_its_own_name() {
        assert_eq!(full_name(&dict(&[("T", named("foo"))]), &NoResolve), "foo");
        assert_eq!(full_name(&Dict::new(), &NoResolve), "");
    }

    #[test]
    fn a_two_level_name_reads_root_first() {
        let parent = dict(&[("T", named("bar"))]);
        let child = dict(&[("T", named("foo")), ("Parent", Object::Dict(parent))]);
        assert_eq!(full_name(&child, &NoResolve), "bar.foo");
    }

    #[test]
    fn a_cycle_is_cut_after_the_entry_node_is_seen_once_more() {
        // A ring of four inline dictionaries, three of which are named. The
        // walk stops one step later than a naive guard would, which is what
        // gives three components rather than two.
        //
        // Building it inline means each node is a *value*, so the ring is
        // expressed by nesting: root -> d1 -> d2 -> d3 -> (root again).
        let root_again = dict(&[("T", named("foo"))]);
        let d3 = dict(&[("T", named("qux")), ("Parent", Object::Dict(root_again))]);
        let d2 = dict(&[("Parent", Object::Dict(d3))]);
        let d1 = dict(&[("T", named("bar")), ("Parent", Object::Dict(d2))]);
        let root = dict(&[("T", named("foo")), ("Parent", Object::Dict(d1))]);
        assert_eq!(full_name(&root, &NoResolve), "foo.qux.bar.foo");
    }
}
