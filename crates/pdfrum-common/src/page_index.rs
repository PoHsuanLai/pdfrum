//! A page's position in the document, as a type rather than a bare integer.
//!
//! It is here, at the bottom of the graph, because it appears in the public
//! signatures of six crates and none of them is below the others.

use core::fmt;

/// A zero-based page index.
///
/// `PageIndex(0)` is the first page. It is **not** a count: a three-page
/// document has `page_count() == 3` and valid indices 0, 1 and 2, and
/// `Document::page_count` deliberately stays `u32` rather than becoming a
/// one-past-the-end `PageIndex` — a count answers "how many", an index answers
/// "which one", and giving them the same type would let one be passed where
/// the other is meant, which is the whole reason this newtype exists.
///
/// Nor is it validated. Nothing stops `PageIndex::new(9000)` on a two-page
/// document; what it names is checked where it is used, and the answer there
/// is a `Result` or an `Option`. A destination that resolves to no page at all
/// is `Option<PageIndex>` and never a sentinel.
///
/// `From<u32>` exists so `impl Into<PageIndex>` arguments accept a literal:
/// `doc.page(0)` needs no wrapping at the call site.
///
/// ```
/// use pdfrum_common::PageIndex;
///
/// let first = PageIndex::new(0);
/// assert_eq!(first.get(), 0);
/// assert_eq!(first.to_string(), "0");
/// assert_eq!(PageIndex::from(4), PageIndex::new(4));
/// assert!(PageIndex::new(1) < PageIndex::new(2));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct PageIndex(u32);

impl PageIndex {
    /// The first page.
    pub const FIRST: Self = Self(0);

    /// A page index from its number.
    #[must_use]
    pub const fn new(n: u32) -> Self {
        Self(n)
    }

    /// The number back.
    ///
    /// The escape hatch for the arithmetic this type deliberately does not
    /// have: no `+`, no `-`, no `Step`, because a page index plus a page index
    /// is not a page index and the compiler should say so.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }
}

impl From<u32> for PageIndex {
    fn from(n: u32) -> Self {
        Self(n)
    }
}

impl From<PageIndex> for u32 {
    fn from(index: PageIndex) -> Self {
        index.0
    }
}

impl fmt::Display for PageIndex {
    /// The bare number, zero-based, as every message in this workspace already
    /// spells it. A user-facing "page 1 of 3" is the caller's presentation
    /// choice and is not made here.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

#[cfg(test)]
mod tests {
    use super::PageIndex;

    #[test]
    fn from_u32_round_trips_both_ways() {
        for n in [0u32, 1, 2, 41, u32::MAX] {
            let index = PageIndex::from(n);
            assert_eq!(index.get(), n);
            assert_eq!(u32::from(index), n);
            assert_eq!(index, PageIndex::new(n));
        }
    }

    #[test]
    fn display_is_the_bare_zero_based_number() {
        assert_eq!(PageIndex::new(0).to_string(), "0");
        assert_eq!(PageIndex::new(41).to_string(), "41");
        assert_eq!(format!("page {}", PageIndex::new(2)), "page 2");
    }

    #[test]
    fn first_and_default_are_page_zero() {
        assert_eq!(PageIndex::FIRST, PageIndex::new(0));
        assert_eq!(PageIndex::default(), PageIndex::FIRST);
    }

    // Ordering is the underlying number's, which is what makes a page range
    // and a sort by page mean what they say.
    #[test]
    fn ordering_is_the_numbers() {
        let mut pages = [PageIndex::new(2), PageIndex::new(0), PageIndex::new(1)];
        pages.sort_unstable();
        assert_eq!(
            pages,
            [PageIndex::new(0), PageIndex::new(1), PageIndex::new(2)]
        );
    }

    // The point of the newtype: `impl Into<PageIndex>` accepts the literal a
    // caller would have written before it existed.
    #[test]
    fn a_literal_converts_the_way_an_impl_into_argument_needs() {
        fn takes(index: impl Into<PageIndex>) -> PageIndex {
            index.into()
        }
        assert_eq!(takes(0), PageIndex::FIRST);
        assert_eq!(takes(PageIndex::new(3)), PageIndex::new(3));
    }
}
