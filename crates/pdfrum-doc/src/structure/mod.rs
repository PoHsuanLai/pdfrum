//! The logical structure tree (ISO 32000-1 §14.7), read **per page and
//! bottom-up**.
//!
//! The shape is unusual and it matters. Nothing here walks `/StructTreeRoot`
//! `/K` downward. Instead the page's `/StructParents` key indexes the
//! `/ParentTree`, which yields the elements that own this page's marked
//! content; each of those walks *up* its `/P` chain until it reaches the root,
//! building the ancestors it passes and linking itself into each one's kid
//! list. Elements on other pages are never visited, which is why the same
//! logical tree looks different from each page.
//!
//! Two consequences worth expecting:
//!
//! - The top-level slot list is sized from the root's `/K` but filled only by
//!   the upward walk, so a document can report more top-level children than
//!   it can produce. The dump skips the empty slots.
//! - Top-level linking matches **direct references only**, so an inline
//!   dictionary written straight into `/K` is never linked, however well
//!   formed it is.

mod dump;
mod element;

pub use dump::render as dump_tree;
pub(crate) use element::Kid;
pub use element::StructElement;

use pdfrum_common::{DiagKind, Diagnostics, Limits, Severity};
use pdfrum_object::{Dict, Object, Resolve};

use crate::names;
use crate::nav::number_tree;

/// One page's view of the document's structure tree.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StructTree {
    /// Every element reached from this page, in discovery order. Kid and
    /// parent links are indices into this table.
    pub elements: Vec<StructElement>,
    /// The root's `/K` slots. Sized from `/K`, filled only where the upward
    /// walk reached a top-level element, so a `None` is normal.
    pub top: Vec<Option<usize>>,
}

impl StructTree {
    /// Builds the tree for one page, or nothing when the document is not
    /// tagged.
    ///
    /// "Tagged" is `catalog[/MarkInfo][/Marked]` read as a **non-zero
    /// integer**, and a Boolean has an integer reading of zero or one — so
    /// the spec-conforming `/Marked true` works, and so does a file writing
    /// `/Marked 1`.
    #[must_use]
    pub fn load_page<R: Resolve>(
        catalog: &Dict,
        page: &Dict,
        page_obj_num: u32,
        r: &R,
        limits: &Limits,
        diags: &mut Diagnostics,
    ) -> Option<StructTree> {
        if !is_tagged(catalog, r) {
            return None;
        }
        let root = catalog.dict(names::STRUCT_TREE_ROOT, r);
        let mut tree = StructTree::default();
        let Some(root) = root else {
            return Some(tree);
        };
        let role_map = root.dict(names::ROLE_MAP, r);

        let Some(kids) = root.get(names::K, r).map(|k| k.get().clone()) else {
            return Some(tree);
        };
        tree.top = match &kids {
            Object::Dict(_) => vec![None],
            Object::Array(array) => vec![None; array.len()],
            _ => return Some(tree),
        };

        let Some(parent_tree) = root.dict(names::PARENT_TREE, r) else {
            return Some(tree);
        };
        let parents_id = page.int(names::STRUCT_PARENTS, r).unwrap_or(-1);
        if parents_id < 0 {
            return Some(tree);
        }
        let entry = number_tree::find(&parent_tree, parents_id, r, limits, diags);
        let Some(Object::Array(owners)) = entry else {
            return Some(tree);
        };

        let mut build = Build {
            root: &root,
            root_kids: &kids,
            role_map: role_map.as_ref(),
            page_obj_num,
            memo: Vec::new(),
        };
        for index in 0..owners.len() {
            if let Some(dict) = owners.dict_at(index, r) {
                let reference = owners.reference_at(index);
                build.add_page_node(
                    &mut tree,
                    Node {
                        dict: &dict,
                        reference,
                    },
                    0,
                    r,
                    limits,
                    diags,
                );
            }
        }
        Some(tree)
    }
}

/// Whether the catalog claims the document is tagged.
fn is_tagged<R: Resolve>(catalog: &Dict, r: &R) -> bool {
    catalog
        .dict(names::MARK_INFO, r)
        .and_then(|info| info.int(names::MARKED, r))
        .is_some_and(|marked| marked != 0)
}

/// One node the upward walk is about to build: the dictionary and, when it
/// has one, the reference that named it.
#[derive(Debug, Clone, Copy)]
struct Node<'a> {
    dict: &'a Dict,
    reference: Option<pdfrum_object::ObjRef>,
}

/// Scratch state for the upward walk.
struct Build<'a> {
    root: &'a Dict,
    root_kids: &'a Object,
    role_map: Option<&'a Dict>,
    page_obj_num: u32,
    /// Elements already built, keyed by the dictionary that produced them so
    /// a shared ancestor is visited once.
    memo: Vec<(Dict, usize)>,
}

impl Build<'_> {
    /// Builds `dict`'s element and everything above it, returning its index.
    fn add_page_node<R: Resolve>(
        &mut self,
        tree: &mut StructTree,
        node: Node,
        depth: u32,
        r: &R,
        limits: &Limits,
        diags: &mut Diagnostics,
    ) -> Option<usize> {
        let Node { dict, reference } = node;
        if depth > limits.max_name_tree_depth {
            diags.record(Severity::Suspicious, DiagKind::TreeDepthExceeded, None);
            return None;
        }
        if let Some((_, index)) = self.memo.iter().find(|(seen, _)| seen == dict) {
            return Some(*index);
        }

        let kind = dict
            .name(names::S)
            .map_or_else(Vec::new, |s| s.as_bytes().to_vec());
        let element = StructElement {
            dict: dict.clone(),
            reference,
            kind: element::map_role(self.role_map, &kind),
            kids: StructElement::load_kids(dict, self.page_obj_num, r),
            parent: None,
        };
        let index = tree.elements.len();
        tree.elements.push(element);
        self.memo.push((dict.clone(), index));

        // A structure element names its parent `/P`, not `/Parent` — the
        // latter is the page tree's key and is absent here.
        let parent = dict.dict(names::P, r);
        let is_top = match &parent {
            None => true,
            Some(p) => p.name(names::TYPE) == Some(names::STRUCT_TREE_ROOT),
        };
        if is_top {
            if !self.add_top_level_node(tree, dict, reference, index) {
                self.forget(tree, dict, index, diags);
            }
            return Some(index);
        }

        let Some(parent) = parent else {
            return Some(index);
        };
        let parent_ref = dict.reference(names::P);
        let parent_index = self.add_page_node(
            tree,
            Node {
                dict: &parent,
                reference: parent_ref,
            },
            depth + 1,
            r,
            limits,
            diags,
        )?;
        let linked = tree
            .elements
            .get_mut(parent_index)
            .is_some_and(|p| p.link_kid(dict, reference, index));
        if !linked {
            self.forget(tree, dict, index, diags);
            return Some(index);
        }
        if let Some(child) = tree.elements.get_mut(index) {
            child.parent = Some(parent_index);
        }
        Some(index)
    }

    /// Drops an element from the memo so a later path can rebuild it.
    ///
    /// The element itself stays in the table — nothing else holds an index
    /// into it, and removing it would shift every index above.
    fn forget(
        &mut self,
        tree: &mut StructTree,
        dict: &Dict,
        index: usize,
        diags: &mut Diagnostics,
    ) {
        self.memo.retain(|(seen, at)| seen != dict || *at != index);
        let _ = tree;
        diags.record(Severity::Recovered, DiagKind::StructElementDropped, None);
    }

    /// Files an element into the root's `/K` slots.
    ///
    /// A dictionary-valued `/K` fills slot zero and then **falls through** to
    /// the array branch, which finds no array and reports success — that is
    /// why a single top-level element works at all. The array branch matches
    /// only direct references, so an inline kid is never filed.
    fn add_top_level_node(
        &self,
        tree: &mut StructTree,
        dict: &Dict,
        reference: Option<pdfrum_object::ObjRef>,
        index: usize,
    ) -> bool {
        let _ = self.root;
        match self.root_kids {
            Object::Dict(root_kid) => {
                if root_kid != dict {
                    return false;
                }
                if let Some(slot) = tree.top.first_mut() {
                    *slot = Some(index);
                }
                // No array follows, so the fall-through reports success.
                true
            }
            Object::Array(array) => {
                let mut saved = false;
                for (slot, item) in array.iter().enumerate() {
                    if let (Some(item_ref), Some(reference)) = (item.as_ref_id(), reference)
                        && item_ref.num == reference.num
                        && let Some(cell) = tree.top.get_mut(slot)
                    {
                        *cell = Some(index);
                        saved = true;
                    }
                }
                saved
            }
            _ => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::StructTree;
    use pdfrum_common::{Diagnostics, Limits};
    use pdfrum_object::{Dict, Name, NoResolve, Object};

    fn dict(pairs: &[(&str, Object)]) -> Dict {
        Dict::from_pairs(
            pairs
                .iter()
                .map(|(k, v)| (Name::from(*k), v.clone()))
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn an_untagged_document_has_no_tree_at_all() {
        let (l, mut d) = (Limits::default(), Diagnostics::default());
        assert!(
            StructTree::load_page(&Dict::new(), &Dict::new(), 0, &NoResolve, &l, &mut d).is_none()
        );
    }

    #[test]
    fn the_marked_flag_reads_as_an_integer_whichever_way_it_is_written() {
        let (l, mut d) = (Limits::default(), Diagnostics::default());
        for marked in [Object::Bool(true), Object::Int(1)] {
            let catalog = dict(&[("MarkInfo", Object::Dict(dict(&[("Marked", marked)])))]);
            assert!(
                StructTree::load_page(&catalog, &Dict::new(), 0, &NoResolve, &l, &mut d).is_some()
            );
        }
        // Zero, false, and a non-numeric value all leave it untagged.
        for marked in [
            Object::Bool(false),
            Object::Int(0),
            Object::Name(Name::from("yes")),
        ] {
            let catalog = dict(&[("MarkInfo", Object::Dict(dict(&[("Marked", marked)])))]);
            assert!(
                StructTree::load_page(&catalog, &Dict::new(), 0, &NoResolve, &l, &mut d).is_none()
            );
        }
    }

    #[test]
    fn a_tagged_document_with_no_root_yields_an_empty_tree() {
        let catalog = dict(&[(
            "MarkInfo",
            Object::Dict(dict(&[("Marked", Object::Int(1))])),
        )]);
        let (l, mut d) = (Limits::default(), Diagnostics::default());
        let tree = StructTree::load_page(&catalog, &Dict::new(), 0, &NoResolve, &l, &mut d);
        assert_eq!(tree, Some(StructTree::default()));
    }
}
