//! The document outline (ISO 32000-1 §12.3.3) — bookmarks — on the write
//! side.
//!
//! The reader hands back a flat list carrying a depth, because that is what a
//! caller walking bookmarks wants. Writing takes the same shape: a caller
//! describes the tree as a list of items with depths and this builds the
//! doubly-linked `/First` `/Last` `/Next` `/Prev` `/Parent` structure a PDF
//! actually holds, which is tedious and easy to get subtly wrong by hand.
//!
//! Until this existed, nothing in the writer touched `/Outlines` at all: a
//! merge or a split dropped every bookmark in the document silently.

use pdfrum_object::{Array, Dict, Name, ObjRef, Object, PdfString, Resolve, encode_text};

use crate::names;

use crate::{annot::AnnotGoToView, doc::EditDoc, error::Error};

/// An outline write either applies or names why it could not.
type Result<T> = core::result::Result<T, Error>;

/// Where a bookmark sends the reader.
#[derive(Debug, Clone, PartialEq)]
pub enum BookmarkTarget {
    /// A page in this document, with a view — written as `/Dest`.
    Page {
        /// The page's object reference.
        page: ObjRef,
        /// How to display it.
        view: AnnotGoToView,
    },
    /// A destination the document names in `/Names /Dests` — written as a
    /// `/Dest` naming the string, which the reader resolves through the name
    /// tree.
    Named(String),
    /// Nothing: a heading that groups its children without jumping anywhere.
    None,
}

/// One bookmark, as a caller describes it.
///
/// `depth` is what the reader's [`Bookmark::depth`](pdfrum_doc::nav::Bookmark)
/// answers: zero is a top-level item, and an item deeper than the one before
/// it is that item's child. A depth that jumps by more than one is clamped to
/// one deeper, since there is no honest tree that shape describes.
#[derive(Debug, Clone, PartialEq)]
pub struct BookmarkSpec {
    /// The visible text, written as `/Title`.
    pub title: String,
    /// How deep the item sits; zero is top level.
    pub depth: usize,
    /// Where it points.
    pub target: BookmarkTarget,
    /// `/C`, the item's colour as three components in `[0, 1]`.
    pub color: Option<(f32, f32, f32)>,
    /// `/F` style bits: 1 italic, 2 bold.
    pub style: Option<i64>,
    /// Whether the item shows its children when the outline opens.
    ///
    /// A closed item writes a negative `/Count`, which is how a reader tells
    /// "collapsed" from "has no children".
    pub open: bool,
}

impl BookmarkSpec {
    /// A top-level bookmark pointing nowhere.
    #[must_use]
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            depth: 0,
            target: BookmarkTarget::None,
            color: None,
            style: None,
            open: true,
        }
    }

    /// Sets how deep the item sits; zero is top level.
    #[must_use]
    pub fn depth(mut self, depth: usize) -> Self {
        self.depth = depth;
        self
    }

    /// Points the item at a page.
    #[must_use]
    pub fn page(mut self, page: ObjRef, view: AnnotGoToView) -> Self {
        self.target = BookmarkTarget::Page { page, view };
        self
    }

    /// Points the item at a name the document already carries.
    #[must_use]
    pub fn named(mut self, name: impl Into<String>) -> Self {
        self.target = BookmarkTarget::Named(name.into());
        self
    }

    /// Sets `/C`, the item's colour.
    #[must_use]
    pub fn color(mut self, rgb: (f32, f32, f32)) -> Self {
        self.color = Some(rgb);
        self
    }

    /// Sets `/F`: 1 italic, 2 bold.
    #[must_use]
    pub fn style(mut self, style: i64) -> Self {
        self.style = Some(style);
        self
    }

    /// Whether the item's children show when the outline opens.
    #[must_use]
    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }
}

/// Replaces the document's outline with `items`.
///
/// The list is a pre-order walk: each item's `depth` places it under the
/// nearest earlier item one level shallower, exactly as
/// [`Document::outline`](pdfrum_doc) reports it. An empty list removes the
/// outline.
///
/// `/Count` is written the way a reader expects: a **positive** count of
/// visible descendants on an open item, the same count **negated** on a
/// closed one, and no entry at all on a leaf. The catalog's `/Outlines` gets
/// the total of its open descendants.
///
/// # Errors
///
/// [`Error::NoDestinationCatalog`] when the document has no catalog to hold
/// the outline.
///
/// ```
/// use std::sync::Arc;
/// use pdfrum_edit::{BookmarkSpec, EditDoc, SaveOptions, save, set_outline};
/// use pdfrum_parser::{LoadOptions, load};
///
/// let bytes: Arc<[u8]> = Arc::from(&include_bytes!("../tests/files/hello.pdf")[..]);
/// let doc = load(bytes, &LoadOptions::default())?;
/// let mut edit = EditDoc::new(&doc);
/// set_outline(
///     &mut edit,
///     &[
///         BookmarkSpec::new("Chapter 1"),
///         BookmarkSpec::new("Section 1.1").depth(1),
///         BookmarkSpec::new("Chapter 2"),
///     ],
/// )?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn set_outline(dest: &mut EditDoc<'_>, items: &[BookmarkSpec]) -> Result<()> {
    let Some(root) = dest.base().trailer().reference(names::ROOT) else {
        return Err(Error::NoDestinationCatalog);
    };
    let Some(mut catalog) = dest
        .fetch(root)
        .ok()
        .and_then(|object| object.as_dict().cloned())
    else {
        return Err(Error::NoDestinationCatalog);
    };

    if items.is_empty() {
        catalog.remove(names::OUTLINES);
        dest.replace(root, Object::Dict(catalog));
        return Ok(());
    }

    // Reserve every object first: an item's `/Parent`, `/Next` and `/Prev`
    // name objects that do not exist yet, so the references have to be in
    // hand before any dictionary can be filled in.
    let refs: Vec<ObjRef> = items
        .iter()
        .map(|_| dest.add(Object::Dict(Dict::new())))
        .collect();
    let outlines_ref = dest.add(Object::Dict(Dict::new()));

    let tree = Tree::of(items);
    // `refs` and `tree.nodes` are both built from `items`, so every index
    // below is in range — `get` rather than `[]` so the code says so rather
    // than relying on the reader to check.
    let reference_of = |index: usize| refs.get(index).copied();
    for (index, item) in items.iter().enumerate() {
        let (Some(node), Some(self_ref)) = (tree.nodes.get(index), reference_of(index)) else {
            continue;
        };
        let dict = item_dict(item, node, index, &tree, items, outlines_ref, reference_of);
        dest.replace(self_ref, Object::Dict(dict));
    }

    let mut outlines = Dict::new();
    outlines.insert(names::TYPE.clone(), Object::Name(Name::from("Outlines")));
    if let (Some(first), Some(last)) = (
        tree.roots.first().copied().and_then(reference_of),
        tree.roots.last().copied().and_then(reference_of),
    ) {
        outlines.insert(names::FIRST.clone(), Object::Ref(first));
        outlines.insert(names::LAST.clone(), Object::Ref(last));
    }
    outlines.insert(names::COUNT.clone(), Object::Int(tree.visible_total(items)));
    dest.replace(outlines_ref, Object::Dict(outlines));

    catalog.insert(names::OUTLINES.clone(), Object::Ref(outlines_ref));
    dest.replace(root, Object::Dict(catalog));
    Ok(())
}

/// One item's place in the tree, by index into the caller's list.
#[derive(Default)]
struct Node {
    parent: Option<usize>,
    prev: Option<usize>,
    next: Option<usize>,
    first: Option<usize>,
    last: Option<usize>,
    children: Vec<usize>,
}

/// The links a flat depth-tagged list implies.
struct Tree {
    nodes: Vec<Node>,
    roots: Vec<usize>,
}

impl Tree {
    /// Derives parent, sibling and child links from the items' depths.
    ///
    /// A depth that jumps by more than one is treated as one deeper than the
    /// item before it: there is no tree in which a child is three levels below
    /// its parent, and guessing an intermediate is worse than flattening.
    fn of(items: &[BookmarkSpec]) -> Self {
        let mut nodes: Vec<Node> = (0..items.len()).map(|_| Node::default()).collect();
        let mut roots = Vec::new();
        // The item most recently seen at each depth, so a new item can find
        // its parent and its previous sibling in one pass.
        let mut at_depth: Vec<usize> = Vec::new();

        for (index, item) in items.iter().enumerate() {
            let depth = item.depth.min(at_depth.len());
            at_depth.truncate(depth);

            // `link` records a sibling pair in both directions; every index
            // here came from this same walk, so a miss is impossible and
            // silently skipping one is the safe reading of `get_mut`.
            let link = |prev: usize, next: usize, nodes: &mut Vec<Node>| {
                if let Some(node) = nodes.get_mut(next) {
                    node.prev = Some(prev);
                }
                if let Some(node) = nodes.get_mut(prev) {
                    node.next = Some(next);
                }
            };

            if depth == 0 {
                if let Some(&prev) = roots.last() {
                    link(prev, index, &mut nodes);
                }
                roots.push(index);
            } else if let Some(&parent) = at_depth.get(depth - 1) {
                if let Some(node) = nodes.get_mut(index) {
                    node.parent = Some(parent);
                }
                let last_child = nodes.get(parent).and_then(|p| p.children.last().copied());
                if let Some(prev) = last_child {
                    link(prev, index, &mut nodes);
                }
                if let Some(node) = nodes.get_mut(parent) {
                    node.children.push(index);
                }
            }
            at_depth.push(index);
        }

        for node in &mut nodes {
            node.first = node.children.first().copied();
            node.last = node.children.last().copied();
        }
        Self { nodes, roots }
    }

    /// How many descendants of `index` a reader would show — children, plus
    /// the descendants of each open child.
    fn visible_descendants(&self, index: usize, items: &[BookmarkSpec]) -> i64 {
        let mut total = 0;
        let Some(node) = self.nodes.get(index) else {
            return 0;
        };
        for &child in &node.children {
            total += 1;
            if items.get(child).is_some_and(|item| item.open) {
                total += self.visible_descendants(child, items);
            }
        }
        total
    }

    /// The same count for the outline as a whole.
    fn visible_total(&self, items: &[BookmarkSpec]) -> i64 {
        let mut total = 0;
        for &root in &self.roots {
            total += 1;
            if items.get(root).is_some_and(|item| item.open) {
                total += self.visible_descendants(root, items);
            }
        }
        total
    }
}

/// One outline item's dictionary: its title, its links to the items around
/// it, and whatever presentation the caller asked for.
fn item_dict(
    item: &BookmarkSpec,
    node: &Node,
    index: usize,
    tree: &Tree,
    items: &[BookmarkSpec],
    outlines_ref: ObjRef,
    reference_of: impl Fn(usize) -> Option<ObjRef>,
) -> Dict {
    let mut dict = Dict::new();
    dict.insert(
        names::TITLE.clone(),
        Object::Str(PdfString::literal(encode_text(&item.title))),
    );
    dict.insert(
        names::PARENT.clone(),
        Object::Ref(node.parent.and_then(&reference_of).unwrap_or(outlines_ref)),
    );
    if let Some(prev) = node.prev.and_then(&reference_of) {
        dict.insert(names::PREV.clone(), Object::Ref(prev));
    }
    if let Some(next) = node.next.and_then(&reference_of) {
        dict.insert(names::NEXT.clone(), Object::Ref(next));
    }
    if let (Some(first), Some(last)) = (
        node.first.and_then(&reference_of),
        node.last.and_then(&reference_of),
    ) {
        dict.insert(names::FIRST.clone(), Object::Ref(first));
        dict.insert(names::LAST.clone(), Object::Ref(last));
        // A leaf writes no `/Count` at all; only an item with children states
        // one, negated when it is closed.
        let visible = tree.visible_descendants(index, items);
        dict.insert(
            names::COUNT.clone(),
            Object::Int(if item.open { visible } else { -visible }),
        );
    }
    match &item.target {
        BookmarkTarget::Page { page, view } => {
            dict.insert(
                names::DEST.clone(),
                Object::Array(crate::annot::goto_dest_array(*page, *view)),
            );
        }
        BookmarkTarget::Named(name) => {
            dict.insert(
                names::DEST.clone(),
                Object::Str(PdfString::literal(name.as_bytes())),
            );
        }
        BookmarkTarget::None => {}
    }
    if let Some((r, g, b)) = item.color {
        dict.insert(
            names::C.clone(),
            Object::Array(Array::of([
                Object::Real(r.clamp(0.0, 1.0)),
                Object::Real(g.clamp(0.0, 1.0)),
                Object::Real(b.clamp(0.0, 1.0)),
            ])),
        );
    }
    if let Some(style) = item.style {
        dict.insert(names::F.clone(), Object::Int(style));
    }
    dict
}
