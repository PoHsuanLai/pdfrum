//! Page labels (ISO 32000-1 §12.4.2) on the write side.
//!
//! `/PageLabels` is a number tree from a **starting** page index to a
//! labelling rule, and every page from there until the next entry follows that
//! rule. So the labels a document shows are not a list one per page — they are
//! a handful of ranges, which is why this writes ranges rather than labels.
//!
//! The reader's half is [`pdfrum_doc::page_label`], and the two agree on the
//! fallback: a page inside the document with no rule covering it gets its
//! one-based index as a decimal.

use pdfrum_common::PageIndex;
use pdfrum_object::{Array, Dict, Object, PdfString, Resolve, encode_text, names};

use crate::{doc::EditDoc, error::Error};

/// A label write either applies or names why it could not.
type Result<T> = core::result::Result<T, Error>;

/// How the numeric part of a label is written.
///
/// An absent style is its own thing rather than a default: a rule with a
/// prefix and no style labels every page in its range with that prefix alone,
/// which is how a run of unnumbered front matter is spelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PageLabelStyle {
    /// No numeric part at all — the prefix is the whole label.
    #[default]
    None,
    /// `1`, `2`, `3` (`/D`).
    Decimal,
    /// `I`, `II`, `III` (`/R`).
    UpperRoman,
    /// `i`, `ii`, `iii` (`/r`).
    LowerRoman,
    /// `A`, …, `Z`, `AA` (`/A`).
    UpperLetters,
    /// `a`, …, `z`, `aa` (`/a`).
    LowerLetters,
}

impl PageLabelStyle {
    /// The `/S` name, or nothing when the rule writes no numeric part.
    fn key(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::Decimal => Some("D"),
            Self::UpperRoman => Some("R"),
            Self::LowerRoman => Some("r"),
            Self::UpperLetters => Some("A"),
            Self::LowerLetters => Some("a"),
        }
    }
}

/// One labelling rule, and the page it starts at.
///
/// ```
/// use pdfrum_edit::{PageLabelRange, PageLabelStyle};
///
/// // Front matter in lower-case roman, then the body restarting at 1.
/// let front = PageLabelRange::new(0u32, PageLabelStyle::LowerRoman);
/// let body = PageLabelRange::new(4u32, PageLabelStyle::Decimal).prefix("Part 1-");
/// assert_eq!(front.start().get(), 0);
/// assert_eq!(body.start().get(), 4);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageLabelRange {
    start: PageIndex,
    style: PageLabelStyle,
    prefix: Option<String>,
    first: Option<i64>,
}

impl PageLabelRange {
    /// A rule that takes effect at `start` and runs until the next one.
    #[must_use]
    pub fn new(start: impl Into<PageIndex>, style: PageLabelStyle) -> Self {
        Self {
            start: start.into(),
            style,
            prefix: None,
            first: None,
        }
    }

    /// The page this rule takes effect at.
    #[must_use]
    pub fn start(&self) -> PageIndex {
        self.start
    }

    /// `/P`: text placed before the numeric part of every label in the range.
    #[must_use]
    pub fn prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = Some(prefix.into());
        self
    }

    /// `/St`: the number the first page of the range is labelled with.
    ///
    /// Defaults to 1, which is what makes a second range *restart* rather than
    /// continue — the usual reason a document has more than one.
    #[must_use]
    pub fn first(mut self, first: i64) -> Self {
        self.first = Some(first);
        self
    }

    /// The rule as its `/PageLabels` value.
    fn to_dict(&self) -> Dict {
        let mut dict = Dict::new();
        if let Some(style) = self.style.key() {
            dict.insert(
                names::S.clone(),
                Object::Name(pdfrum_object::Name::from(style.as_bytes())),
            );
        }
        if let Some(prefix) = &self.prefix {
            dict.insert(
                crate::names::P.clone(),
                Object::Str(PdfString::literal(encode_text(prefix))),
            );
        }
        if let Some(first) = self.first {
            dict.insert(crate::names::ST.clone(), Object::Int(first));
        }
        dict
    }
}

/// Sets the document's page labels, replacing whatever it had.
///
/// `ranges` are sorted by starting page and de-duplicated — a later range
/// starting at the same page as an earlier one wins — so a caller may pass
/// them in any order. An empty slice **removes** `/PageLabels`, which returns
/// the document to labelling every page with its one-based index.
///
/// The tree is written flat, as one `/Nums` array. A number tree may be split
/// into `/Kids` leaves, and the reader walks either; splitting pays off at
/// thousands of entries, and a document with thousands of *distinct labelling
/// rules* is not a document anyone has.
///
/// # Errors
///
/// [`Error::NoDestinationCatalog`] when the document has no catalog to hold
/// the tree.
///
/// ```
/// use std::sync::Arc;
/// use pdfrum_edit::{
///     EditDoc, PageLabelRange, PageLabelStyle, SaveOptions, save, set_page_labels,
/// };
/// use pdfrum_parser::{LoadOptions, load};
///
/// let bytes: Arc<[u8]> = Arc::from(&include_bytes!("../tests/files/hello.pdf")[..]);
/// let doc = load(bytes, &LoadOptions::default())?;
/// let mut edit = EditDoc::new(&doc);
///
/// set_page_labels(
///     &mut edit,
///     &[PageLabelRange::new(0u32, PageLabelStyle::LowerRoman)],
/// )?;
///
/// let mut out = Vec::new();
/// save(&edit, &SaveOptions::default(), &mut out)?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn set_page_labels(dest: &mut EditDoc<'_>, ranges: &[PageLabelRange]) -> Result<()> {
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

    if ranges.is_empty() {
        catalog.remove(names::PAGE_LABELS);
        dest.replace(root, Object::Dict(catalog));
        return Ok(());
    }

    // A stable sort keeps ranges that share a starting page in the order the
    // caller gave them, so "the later one wins" is decided by skipping any
    // range that a range further along the slice supersedes.
    let mut ranges: Vec<&PageLabelRange> = ranges.iter().collect();
    ranges.sort_by_key(|range| range.start.get());

    let mut nums = Array::default();
    for (index, range) in ranges.iter().enumerate() {
        let superseded = ranges
            .get(index + 1..)
            .is_some_and(|rest| rest.iter().any(|other| other.start == range.start));
        if superseded {
            continue;
        }
        nums.push(Object::Int(i64::from(range.start.get())));
        nums.push(Object::Dict(range.to_dict()));
    }

    let tree = Dict::from_pairs([(crate::names::NUMS.clone(), Object::Array(nums))]);
    catalog.insert(names::PAGE_LABELS.clone(), Object::Dict(tree));
    dest.replace(root, Object::Dict(catalog));
    Ok(())
}
