//! The page-range grammar viewers accept: `"1,3-5"`.
//!
//! # It is not the grammar you would design
//!
//! Two rules surprise everyone who reads the output before the code.
//!
//! **All spaces are stripped globally, including inside numbers.** So
//! `"5  0, 1-2"` becomes `"50,1-2"` and selects page 50, then 1, then 2 —
//! not page 5. The C++'s own unit test annotates this behavior with a
//! literal `// ???`, and it is pinned here because files and command lines in
//! the wild depend on `"1- 4"` and `"1 -4"` both meaning `1-4`.
//!
//! **Failure is all-or-nothing.** One bad entry discards the entire range,
//! rather than being skipped. `"1,2,clams"` selects nothing at all.
//!
//! Duplicates and descending order are both legal: `"1-4,3-6"` really does
//! select eight pages with two repeats, and `"2,1"` selects page 2 before
//! page 1. The result is a *sequence*, not a set.

use pdfrum_common::PageIndex;

use crate::error::Error;

/// A parsed page range: zero-based page indices, in the order named, with
/// duplicates kept.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PageRange(Vec<PageIndex>);

impl PageRange {
    /// Every page of a document of `count` pages, in order.
    #[must_use]
    pub fn all(count: u32) -> Self {
        Self((0..count).map(PageIndex::from).collect())
    }

    /// A range naming exactly these zero-based indices.
    #[must_use]
    pub fn of(indices: impl IntoIterator<Item = impl Into<PageIndex>>) -> Self {
        Self(indices.into_iter().map(Into::into).collect())
    }

    /// Parse `"1,3-5"` against a document of `count` pages.
    ///
    /// Numbers in the text are **one**-based; the indices returned are
    /// zero-based.
    ///
    /// # Errors
    ///
    /// [`Error::BadPageRange`] for anything the grammar rejects — an illegal
    /// character, a page number outside `1..=count`, a descending range, a
    /// three-part entry, or an empty entry. One bad entry fails the whole
    /// string.
    ///
    /// ```
    /// use pdfrum_edit::PageRange;
    ///
    /// let range = PageRange::parse("1,3-5", 10)?;
    /// assert_eq!(range.indices().iter().map(|p| p.get()).collect::<Vec<_>>(), [0, 2, 3, 4]);
    /// // Order is preserved and duplicates are kept.
    /// let dupes = PageRange::parse("2,1,1", 10)?;
    /// assert_eq!(dupes.indices().iter().map(|p| p.get()).collect::<Vec<_>>(), [1, 0, 0]);
    /// // One bad entry discards everything.
    /// assert!(PageRange::parse("1,clams", 10).is_err());
    /// # Ok::<(), pdfrum_edit::Error>(())
    /// ```
    pub fn parse(text: &str, count: u32) -> Result<Self, Error> {
        // The only legal characters. Anything else fails the whole string —
        // there is no "skip the junk" reading.
        if !text
            .bytes()
            .all(|b| b == b' ' || b.is_ascii_digit() || b == b'-' || b == b',')
        {
            return Err(Error::BadPageRange);
        }
        // Spaces go everywhere, including from inside numbers. This is what
        // makes "5  0" mean 50.
        let stripped: String = text.chars().filter(|c| *c != ' ').collect();
        if stripped.is_empty() {
            return Ok(Self(Vec::new()));
        }

        let mut out = Vec::new();
        for entry in stripped.split(',') {
            let mut parts = entry.split('-');
            let first = parts.next().unwrap_or_default();
            let second = parts.next();
            // Three parts is not a range, it is a malformed one.
            if parts.next().is_some() {
                return Err(Error::BadPageRange);
            }

            match second {
                None => {
                    // An empty entry reads as page 0, which is never valid —
                    // so ",1", "1," and ",," all fail here.
                    let n = number(first);
                    if n == 0 || n > count {
                        return Err(Error::BadPageRange);
                    }
                    out.push(PageIndex::new(n - 1));
                }
                Some(second) => {
                    let (a, b) = (number(first), number(second));
                    // `a` is never bounds-checked against `count` directly;
                    // `a <= b <= count` bounds it transitively.
                    if a == 0 || b == 0 || a > b || b > count {
                        return Err(Error::BadPageRange);
                    }
                    out.extend(((a - 1)..b).map(PageIndex::from));
                }
            }
        }
        Ok(Self(out))
    }

    /// The zero-based indices, in the order named.
    #[must_use]
    pub fn indices(&self) -> &[PageIndex] {
        &self.0
    }

    /// How many pages the range names, duplicates counted.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the range names no pages.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// A decimal number, saturating rather than overflowing, with an empty or
/// unparseable string reading as zero — which every caller then rejects.
fn number(text: &str) -> u32 {
    let mut out: u32 = 0;
    for b in text.bytes() {
        let Some(digit) = (b as char).to_digit(10) else {
            return 0;
        };
        out = out.saturating_mul(10).saturating_add(digit);
    }
    out
}

#[cfg(test)]
mod tests {
    use pdfrum_common::PageIndex;

    use super::PageRange;

    fn parse(text: &str, count: u32) -> Option<Vec<u32>> {
        PageRange::parse(text, count)
            .ok()
            .map(|r| r.indices().iter().map(|p| p.get()).collect())
    }

    // cpdfsdk_helpers_unittest.cpp:51-93, the succeeding cases.
    #[test]
    fn simple_ranges_expand_inclusively() {
        assert_eq!(parse("1", 10), Some(vec![0]));
        assert_eq!(parse("1-1", 10), Some(vec![0]));
        assert_eq!(parse("1-4", 10), Some(vec![0, 1, 2, 3]));
        assert_eq!(parse("1,3-5", 10), Some(vec![0, 2, 3, 4]));
        assert_eq!(parse("10", 10), Some(vec![9]));
    }

    // Space stripping, including inside numbers — the `// ???` behavior.
    #[test]
    fn spaces_are_stripped_from_inside_numbers() {
        assert_eq!(parse("1- 4", 4), Some(vec![0, 1, 2, 3]));
        assert_eq!(parse("1 -4", 4), Some(vec![0, 1, 2, 3]));
        assert_eq!(parse(" 1 - 4 ", 4), Some(vec![0, 1, 2, 3]));
        // Two digits separated by spaces are one number.
        assert_eq!(parse("5  0, 1-2 ", 100), Some(vec![49, 0, 1]));
    }

    // The result is a sequence: duplicates and descending order are legal.
    #[test]
    fn duplicates_and_order_are_preserved() {
        assert_eq!(parse("1-4,3-6", 10), Some(vec![0, 1, 2, 3, 2, 3, 4, 5]));
        assert_eq!(parse("2,1", 10), Some(vec![1, 0]));
        assert_eq!(parse("1,1,1,1", 10), Some(vec![0, 0, 0, 0]));
    }

    // The failing cases, all-or-nothing.
    #[test]
    fn the_grammar_rejects() {
        for bad in [
            "clams", // not a number at all
            "0",     // page numbers are one-based
            "42",    // past the end
            "1-2-",  // three parts
            "1-2-3",
            ",1", // empty entry
            "1,",
            ",,",
            "1-", // empty end
            "-1", // empty start
            "-,0,,,1-",
            "1-2,,,,3-4",
            "4-1", // descending
            "1-5", // end past the count
            "1;2", // illegal character
            "1.2",
            "a",
        ] {
            assert_eq!(parse(bad, 4), None, "{bad:?} must fail");
        }
    }

    // One bad entry discards everything, including the good entries before it.
    #[test]
    fn one_bad_entry_discards_the_whole_string() {
        assert_eq!(parse("1,2,clams", 10), None);
        assert_eq!(parse("1,2,99", 10), None);
    }

    #[test]
    fn an_empty_string_names_no_pages() {
        assert_eq!(parse("", 10), Some(vec![]));
        assert_eq!(parse("   ", 10), Some(vec![]));
    }

    // On a one-page document, only "1" and "1-1" work.
    #[test]
    fn a_one_page_document_accepts_only_page_one() {
        assert_eq!(parse("1", 1), Some(vec![0]));
        assert_eq!(parse("1-1", 1), Some(vec![0]));
        assert_eq!(parse("2", 1), None);
        assert_eq!(parse("1-2", 1), None);
    }

    #[test]
    fn all_names_every_page_in_order() {
        assert_eq!(
            PageRange::all(3).indices(),
            &[PageIndex::new(0), PageIndex::new(1), PageIndex::new(2)]
        );
        assert!(PageRange::all(0).is_empty());
    }

    #[test]
    fn a_huge_number_saturates_rather_than_wrapping() {
        assert_eq!(parse("99999999999999", 10), None);
    }
}
