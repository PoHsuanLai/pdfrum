//! Text extraction, search and link detection (SPEC.md §9).
//!
//! Turns an interpreted [`Page`] into the characters a reader can select,
//! search and copy — reading order, generated spaces, line breaks and all —
//! without ever rendering anything.
//!
//! ```no_run
//! use pdfrum_common::{Diagnostics, Limits};
//! use pdfrum_text::{ExtractOptions, extract};
//!
//! # fn demo(page: &pdfrum_page::Page, resolver: &impl pdfrum_object::Resolve) {
//! let mut diags = Diagnostics::default();
//! let text = extract(page, resolver, &ExtractOptions::default(), &Limits::default(), &mut diags);
//! println!("{text}");
//! # }
//! ```
//!
//! # There are two texts, and they are not the same sequence
//!
//! [`TextPage`] carries both, deliberately as different types, because
//! conflating them is the single easiest way to get this crate wrong:
//!
//! - [`TextPage::chars`] is the character stream — one entry per character
//!   the page draws or the extractor invents, geometry attached. It holds
//!   control characters, `\0` for an unmappable code, and a `U+0002` sentinel
//!   where a word was hyphenated across a line. **This is what a `--txt` dump
//!   emits**, unfiltered, in order.
//! - [`TextPage::search_text`] is the text a search matches and a selection
//!   copies. It drops the control characters and the placeholders, and it
//!   *expands* ligatures that the character stream keeps whole.
//!
//! On `bug_781804.pdf` the two disagree at one position: the character stream
//! holds `U+0002` where the text holds `U+00AD`. On `control_characters.pdf`
//! the stream holds two characters the text does not. Neither is a bug —
//! both are read, by different callers, and both are pinned by tests.
//! [`TextPage::runs`] maps between the two index spaces, and the two are
//! [`CharIndex`] and [`TextIndex`] in every signature that names one.
//!
//! # Everything else it does
//!
//! Reading order comes from sorting text objects by their transformed x
//! within a batch and flushing the batch when the baseline jumps. Spaces are
//! generated from geometry, never read from the content stream. Right-to-left
//! runs are reversed into logical order, and brackets in them are mirrored.
//! `/ActualText` marks replace the glyphs they cover. Web and mail addresses
//! are recognized in the result. The heuristics and their constants are
//! inventoried in `docs/design/pdfrum-text.md`; every one of them is
//! byte-exact against the oracle, so none of them is adjustable.

#![forbid(unsafe_code)]
// Every number reaching this crate came from an untrusted file, by way of the
// page interpreter: index with `get()` and do arithmetic that cannot trap.
#![warn(clippy::indexing_slicing)]

// Every module is private and the crate root is the whole surface
// (STYLE.md §4): a caller of `pdfrum-text` needs the types below, and the
// bidi resolver, the Unicode tables, the link scanners and the segment
// builder are how this crate reaches them, not what it offers.
mod bidi;
mod charinfo;
mod dedup;
mod error;
mod find;
mod index;
mod line;
mod links;
mod object;
mod orientation;
mod pipeline;
mod select;
mod unicode;

pub use charinfo::{CharBox, CharType, ObjectIndex};
pub use error::Error;
pub use find::FindOptions;
pub use index::{CharIndex, CharSegment, IndexMap, TextIndex};
pub use links::WebLink;
pub use object::TextRun;
pub use orientation::Orientation;

/// The two candidate scanners [`WebLink`] detection is built from, and the
/// range type they report.
///
/// **Not caller API**, and not a stable one: they are the crate's most
/// index-heavy code and `fuzz/fuzz_targets/text_links.rs` drives them
/// directly on arbitrary strings, which is worth more than keeping them
/// unreachable. A caller wants [`TextPage::web_links`].
#[doc(hidden)]
pub use links::{FoundLink, check_mail_link, check_web_link};

use kurbo::{Affine, Point, Rect, Size};
use pdfrum_common::{Diagnostics, Limits};
use pdfrum_object::Resolve;
use pdfrum_page::Page;
use std::ops::{Range, RangeBounds};

/// How extraction behaves.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ExtractOptions {
    /// The document's `/Root /ViewerPreferences /Direction` is `R2L`.
    ///
    /// Forces every line's overall direction right-to-left, which reverses
    /// the *order* of its direction runs. It is the only thing that flips a
    /// line — the per-line heuristic that would otherwise guess is
    /// deliberately not run. A caller reading this out of the catalog must
    /// set it, or every document with that preference extracts in the wrong
    /// order.
    pub rtl: bool,
}

/// One page's extracted text.
///
/// Cheap to clone and `Send + Sync`, so a document's pages can be extracted
/// in parallel.
#[derive(Debug, Clone, Default)]
pub struct TextPage {
    /// One entry per character, in reading order — **the `--txt` stream**.
    ///
    /// Unfiltered: control characters, `\0`, hyphen sentinels and raw
    /// character codes that are not Unicode scalars at all are all here.
    pub chars: Vec<CharBox>,
    /// The search-facing text, which is a **different sequence** from
    /// [`chars`](Self::chars): stripped of control characters and
    /// placeholders, and with ligatures expanded.
    ///
    /// Indexed by [`TextIndex`], not [`CharIndex`] — the name says which of
    /// the two spaces a number into it counts in.
    pub search_text: Vec<char>,
    /// The map between the two index spaces.
    pub runs: IndexMap,
}

#[doc(hidden)]
#[must_use]
pub fn debug_runs(page: &Page) -> Vec<TextRun> {
    object::walk(&page.objects)
}

/// Extracts one page's text (SPEC.md §9).
///
/// Infallible and never panicking: `core/fpdftext/` has no error channel at
/// all, and every place it silently drops something this records a
/// [`Diagnostic`](pdfrum_common::Diagnostic) and carries on.
#[must_use]
pub fn extract<R: Resolve>(
    page: &Page,
    resolver: &R,
    options: &ExtractOptions,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> TextPage {
    let _ = limits;
    if page.objects.is_empty() {
        return TextPage::default();
    }
    let runs = object::walk(&page.objects);
    let page_flow = orientation::page_flow(page);
    let display = display_matrix(page);

    let mut builder = pipeline::Builder::new(&runs, page_flow, display, options.rtl, resolver);
    // Object-level duplicate suppression looks back over the text objects
    // already offered, so the walk keeps them as it goes.
    let mut offered: Vec<TextRun> = Vec::new();
    for (index, run) in runs.iter().enumerate() {
        if dedup::repeats_a_predecessor(run, &offered, &builder.out.chars) {
            diags.record(
                pdfrum_common::Severity::Recovered,
                pdfrum_common::DiagKind::TextObjectDuplicate,
                None,
            );
            continue;
        }
        offered.push(run.clone());
        builder.offer(index, diags);
    }
    builder.flush(diags);
    builder.close_line();

    let out = builder.out;
    let text: Vec<char> = out
        .text
        .iter()
        .filter_map(|unit| char::from_u32(*unit))
        .collect();
    let runs = index::build(&out.chars);
    TextPage {
        chars: out.chars,
        search_text: text,
        runs,
    }
}

/// The page-space to device-space matrix the batching and one line-break
/// escape hatch measure in.
///
/// A y-flip composed with the crop box's own normalizer, which for an
/// unrotated page is `(1, 0, 0, -1, 0, height)`. A page with a zero
/// dimension gets the **zero matrix**, which collapses every position to the
/// origin — so no batch is ever split on such a page, and the escape hatch
/// never fires.
fn display_matrix(page: &Page) -> Affine {
    let (width, height) = page.display_size();
    if width <= 0.0 || height <= 0.0 {
        return Affine::new([0.0; 6]);
    }
    let normalizer = page.rotate.display_matrix(page.crop_box);
    Affine::new([1.0, 0.0, 0.0, -1.0, 0.0, height]) * normalizer
}

/// Resolves a caller's character range into the half-open `start..end` a
/// query wants, clamped to the character list.
///
/// An unbounded end is "to the end of the page", which is the shape the C++
/// spells as a negative count.
fn char_bounds(range: &impl RangeBounds<CharIndex>, total: usize) -> (usize, usize) {
    use std::ops::Bound;
    let start = match range.start_bound() {
        Bound::Included(at) => at.get(),
        Bound::Excluded(at) => at.get().saturating_add(1),
        Bound::Unbounded => 0,
    };
    let end = match range.end_bound() {
        Bound::Included(at) => at.get().saturating_add(1),
        Bound::Excluded(at) => at.get(),
        Bound::Unbounded => total,
    };
    (start, end.min(total))
}

impl TextPage {
    /// How many characters the page drew.
    #[must_use]
    pub fn char_count(&self) -> usize {
        self.chars.len()
    }

    /// A run of the search-facing text, addressed in the **character list**
    /// (`GetPageText`).
    ///
    /// The bounds are moved onto real text: a start that lands on a stripped
    /// character scans forward, an end that lands on one scans back. So
    /// asking for the fifteen characters from character 17 of
    /// `control_characters.pdf` returns `"Goodbye, world!"` even though the
    /// text itself has no character 17.
    ///
    /// This is the one place the two index spaces meet in a single call: the
    /// bounds count characters and the answer is text, which is exactly why
    /// [`runs`](Self::runs) exists.
    #[must_use]
    pub fn slice(&self, range: impl RangeBounds<CharIndex>) -> String {
        let total = self.chars.len();
        let (start, end) = char_bounds(&range, total);
        if start >= end || start >= total || self.search_text.is_empty() {
            return String::new();
        }
        let Some(text_start) = self.runs.text_index_at_or_after(CharIndex::new(start)) else {
            return String::new();
        };
        let text_end = self.runs.text_index_end(CharIndex::new(end - 1));
        if text_end <= text_start {
            return String::new();
        }
        self.search_text
            .get(text_start.get()..text_end.get())
            .unwrap_or_default()
            .iter()
            .collect()
    }

    /// Searches the page (SPEC.md §9). Match ranges are **text** offsets.
    ///
    /// ```
    /// # use pdfrum_text::{FindOptions, TextIndex, TextPage};
    /// let page = TextPage {
    ///     search_text: "Hello, world!".chars().collect(),
    ///     ..TextPage::default()
    /// };
    /// let at = TextIndex::new;
    /// let hits: Vec<_> = page.find("world", FindOptions::default()).collect();
    /// assert_eq!(hits, [at(7)..at(12)]);
    /// // The default is case-insensitive.
    /// let hits: Vec<_> = page.find("WORLD", FindOptions::default()).collect();
    /// assert_eq!(hits, [at(7)..at(12)]);
    /// ```
    pub fn find<'a>(
        &'a self,
        needle: &str,
        options: FindOptions,
    ) -> impl Iterator<Item = Range<TextIndex>> + 'a {
        find::search(&self.to_string(), needle, options)
    }

    /// Every web and mail address in the page's text (SPEC.md §9).
    ///
    /// Ranges are **character-list** indices, which is the index space the
    /// upstream API reports and is not the same one [`find`](Self::find)
    /// returns.
    #[must_use]
    pub fn web_links(&self) -> Vec<WebLink> {
        links::extract(&self.chars, &self.search_text, &index::build(&self.chars))
    }

    /// The boxes covering a run of characters, one per run sharing a text
    /// object.
    ///
    /// An unbounded end is "to the end of the page", and a range running past
    /// the end takes what is there.
    #[must_use]
    pub fn rects(&self, range: impl RangeBounds<CharIndex>) -> Vec<Rect> {
        select::rects(&self.chars, range)
    }

    /// The character under a point, or the nearest within a tolerance.
    #[must_use]
    pub fn index_at(&self, point: Point, tolerance: Size) -> Option<CharIndex> {
        select::index_at(&self.chars, point, tolerance)
    }

    /// The text inside a rectangle, with `\r\n` where the selection crosses a
    /// baseline.
    #[must_use]
    pub fn text_in_rect(&self, rect: Rect) -> String {
        select::text_in_rect(&self.chars, rect)
    }

    /// The text one text object drew.
    #[must_use]
    pub fn text_of_object(&self, object: ObjectIndex) -> String {
        select::text_of_object(&self.chars, object)
    }

    /// One character, or an error naming the bound it broke.
    ///
    /// # Errors
    ///
    /// [`Error::CharIndexOutOfRange`] when the page has no such character.
    pub fn char(&self, index: CharIndex) -> Result<&CharBox, Error> {
        self.chars
            .get(index.get())
            .ok_or(Error::CharIndexOutOfRange {
                index,
                len: self.chars.len(),
            })
    }
}

/// The whole search-facing text.
///
/// This is [`search_text`](TextPage::search_text) — what a search matches and
/// a selection copies — and **not** the character stream a `--txt` dump
/// emits, which holds control characters and placeholders this drops.
impl std::fmt::Display for TextPage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for ch in &self.search_text {
            write!(f, "{ch}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    // Test fixtures compare floats exactly where the behaviour being pinned
    // is exact.
    #![allow(
        clippy::float_cmp,
        clippy::indexing_slicing,
        reason = "test fixtures pin exact values"
    )]

    use super::*;

    #[test]
    fn an_empty_page_extracts_to_nothing() {
        let page = TextPage::default();
        assert_eq!(page.char_count(), 0);
        assert_eq!(page.to_string(), "");
        assert!(page.web_links().is_empty());
        assert!(page.rects(..).is_empty());
        assert_eq!(page.slice(..), "");
    }

    #[test]
    fn char_names_the_bound_it_broke() {
        let page = TextPage::default();
        assert_eq!(
            page.char(CharIndex::new(3)),
            Err(Error::CharIndexOutOfRange {
                index: CharIndex::new(3),
                len: 0
            })
        );
    }

    #[test]
    fn a_zero_size_page_gets_the_zero_display_matrix() {
        let mut page = Page::empty();
        page.crop_box = Rect::ZERO;
        assert_eq!(display_matrix(&page).as_coeffs(), [0.0; 6]);
    }

    #[test]
    fn an_ordinary_page_gets_a_y_flip() {
        let page = Page::empty();
        let matrix = display_matrix(&page);
        // The bottom-left of the crop box maps to the top-left of the device.
        let bottom_left = matrix * Point::new(0.0, 0.0);
        assert!((bottom_left.y - 792.0).abs() < 1e-6, "{bottom_left:?}");
        let top_left = matrix * Point::new(0.0, 792.0);
        assert!(top_left.y.abs() < 1e-6, "{top_left:?}");
    }

    #[test]
    fn the_public_types_are_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<TextPage>();
        assert_send_sync::<CharBox>();
        assert_send_sync::<WebLink>();
        assert_send_sync::<Error>();
        assert_send_sync::<IndexMap>();
    }
}
