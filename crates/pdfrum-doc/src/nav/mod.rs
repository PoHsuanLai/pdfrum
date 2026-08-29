//! Navigation: outlines, name and number trees, destinations, actions, file
//! specifications and links.

pub mod action;
pub mod dest;
pub mod filespec;
pub mod link;
pub mod name_tree;
pub mod number_tree;
pub mod open_action;
pub mod outline;

pub use action::{AActionType, Action, ActionKind, additional_action};
pub use dest::{Dest, Xyz, ZoomMode};
pub use filespec::{FileSpec, decode_file_name, encode_file_name};
pub use link::{Link, enumerate_links, link_at_point, page_links};
pub use name_tree::{NameTree, lookup_named_dest};
pub use open_action::{Hidden, hidden_by_open_action};
pub use outline::Bookmark;

/// A number tree, which has no wrapper type: the two lookups are free
/// functions over the node dictionary.
pub mod number_tree_alias {
    pub use crate::nav::number_tree::{find, lower_bound};
}

/// Re-exported so `pdfrum_doc::NumberTree::find` reads the way the name tree
/// does at the call site.
pub struct NumberTree;

impl NumberTree {
    /// The value at exactly one key. See [`number_tree::find`].
    #[must_use]
    pub fn find<R: pdfrum_object::Resolve>(
        node: &pdfrum_object::Dict,
        key: i64,
        r: &R,
        limits: &pdfrum_common::Limits,
        diags: &mut pdfrum_common::Diagnostics,
    ) -> Option<pdfrum_object::Object> {
        number_tree::find(node, key, r, limits, diags)
    }

    /// The greatest key not above one, with its value. See
    /// [`number_tree::lower_bound`].
    #[must_use]
    pub fn lower_bound<R: pdfrum_object::Resolve>(
        node: &pdfrum_object::Dict,
        key: i64,
        r: &R,
        limits: &pdfrum_common::Limits,
        diags: &mut pdfrum_common::Diagnostics,
    ) -> Option<(i64, Option<pdfrum_object::Object>)> {
        number_tree::lower_bound(node, key, r, limits, diags)
    }
}
