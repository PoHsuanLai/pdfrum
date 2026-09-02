//! Navigation: outlines, name and number trees, destinations, actions, file
//! specifications and links.

mod action;
mod dest;
mod filespec;
pub mod link;
mod name_tree;
pub(crate) mod number_tree;
pub mod open_action;
pub mod outline;

pub use action::{AActionType, Action, ActionKind, additional_action};
pub use dest::{Dest, Xyz, ZoomMode};
pub use filespec::{FileSpec, decode_file_name, encode_file_name};
pub use link::{Link, page_links};
pub use name_tree::{NameTree, lookup_named_dest};
pub use open_action::{Hidden, hidden_by_open_action};
pub use outline::Bookmark;
