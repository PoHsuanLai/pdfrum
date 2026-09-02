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
//! println!("{}", text.all_text());
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
//! - [`TextPage::text`] is the text a search matches and a selection copies.
//!   It drops the control characters and the placeholders, and it *expands*
//!   ligatures that the character stream keeps whole.
//!
//! On `bug_781804.pdf` the two disagree at one position: the character stream
//! holds `U+0002` where the text holds `U+FFFE`. On `control_characters.pdf`
//! the stream holds two characters the text does not. Neither is a bug —
//! both are read, by different callers, and both are pinned by tests.
//! [`TextPage::runs`] maps between the two index spaces.
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

pub mod bidi;
mod charinfo;
mod dedup;
mod error;
mod find;
pub mod index;
mod line;
pub mod links;
mod object;
mod orientation;
mod pipeline;
mod select;
pub mod unicode;

pub use charinfo::{CharBox, CharType, ObjectIndex};
pub use error::Error;
pub use find::FindOptions;
pub use index::{CharIndex, CharSegment};
pub use links::WebLink;
pub use object::TextRun;
pub use orientation::Orientation;

use kurbo::{Affine, Point, Rect, Size};
use pdfrum_common::{Diagnostics, Limits};
use pdfrum_object::Resolve;
use pdfrum_page::Page;
use std::ops::Range;

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
    pub text: Vec<char>,
    /// The map between the two index spaces.
    pub runs: CharIndex,
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
        text,
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

impl TextPage {
    /// How many characters the page drew.
    #[must_use]
    pub fn char_count(&self) -> usize {
        self.chars.len()
    }

    /// The whole search-facing text.
    #[must_use]
    pub fn all_text(&self) -> String {
        self.text.iter().collect()
    }

    /// A run of the search-facing text, addressed by **character** index
    /// (`GetPageText`).
    ///
    /// The bounds are moved onto real text: a start that lands on a stripped
    /// character scans forward, an end that lands on one scans back. So
    /// asking for the fifteen characters starting at character 17 of
    /// `control_characters.pdf` returns `"Goodbye, world!"` even though the
    /// text itself has no character 17.
    #[must_use]
    pub fn page_text(&self, start: usize, count: usize) -> String {
        if count == 0 || start >= self.chars.len() || self.text.is_empty() {
            return String::new();
        }
        let Some(text_start) = self.runs.text_index_at_or_after(start) else {
            return String::new();
        };
        let count = count.min(self.chars.len() - start);
        let last = start + count - 1;
        let text_end = self.runs.text_index_end(last);
        if text_end <= text_start {
            return String::new();
        }
        self.text
            .get(text_start..text_end)
            .unwrap_or_default()
            .iter()
            .collect()
    }

    /// Searches the page (SPEC.md §9). Match ranges are **text** offsets.
    ///
    /// ```
    /// # use pdfrum_text::{FindOptions, TextPage};
    /// let page = TextPage {
    ///     text: "Hello, world!".chars().collect(),
    ///     ..TextPage::default()
    /// };
    /// let hits: Vec<_> = page.find("world", FindOptions::default()).collect();
    /// assert_eq!(hits, [7..12]);
    /// // The default is case-insensitive.
    /// let hits: Vec<_> = page.find("WORLD", FindOptions::default()).collect();
    /// assert_eq!(hits, [7..12]);
    /// ```
    pub fn find<'a>(
        &'a self,
        needle: &str,
        options: FindOptions,
    ) -> impl Iterator<Item = Range<usize>> + 'a {
        find::search(&self.all_text(), needle, options)
    }

    /// Every web and mail address in the page's text (SPEC.md §9).
    ///
    /// Ranges are **character-list** indices, which is the index space the
    /// upstream API reports and is not the same one [`find`](Self::find)
    /// returns.
    #[must_use]
    pub fn web_links(&self) -> Vec<WebLink> {
        links::extract(&self.chars, &self.text, &index::build(&self.chars))
    }

    /// The boxes covering a run of characters, one per run sharing a text
    /// object. `None` for the count means "to the end".
    #[must_use]
    pub fn rects(&self, start: usize, count: Option<usize>) -> Vec<Rect> {
        select::rects(&self.chars, start, count)
    }

    /// The character under a point, or the nearest within a tolerance.
    #[must_use]
    pub fn index_at(&self, point: Point, tolerance: Size) -> Option<usize> {
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
    pub fn char_at(&self, index: usize) -> Result<&CharBox, Error> {
        self.chars.get(index).ok_or(Error::CharIndexOutOfRange {
            index,
            len: self.chars.len(),
        })
    }

    /// The character stream as UTF-32LE with a leading byte-order mark —
    /// exactly the bytes the oracle's `--txt` writes for one page.
    ///
    /// One code unit per character, **unfiltered**: this is
    /// [`chars`](Self::chars), not [`text`](Self::text), and routing it
    /// through the latter would drop the control characters and the hyphen
    /// sentinels that the goldens contain.
    ///
    /// ```
    /// # use pdfrum_text::TextPage;
    /// // A page with no text is a four-byte file holding only the mark.
    /// assert_eq!(TextPage::default().to_utf32le(), [0xFF, 0xFE, 0x00, 0x00]);
    /// ```
    #[must_use]
    pub fn to_utf32le(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity((self.chars.len() + 1) * 4);
        out.extend_from_slice(&0x0000_FEFFu32.to_le_bytes());
        for info in &self.chars {
            out.extend_from_slice(&info.unicode.to_le_bytes());
        }
        out
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
        assert_eq!(page.all_text(), "");
        assert!(page.web_links().is_empty());
        assert!(page.rects(0, None).is_empty());
        assert_eq!(page.page_text(0, 10), "");
    }

    #[test]
    fn the_utf32_dump_is_a_bare_mark_for_an_empty_page() {
        // The oracle writes a four-byte file for a page with no text, and the
        // harness's transcode reads that back as the empty string.
        assert_eq!(TextPage::default().to_utf32le(), [0xFF, 0xFE, 0x00, 0x00]);
    }

    #[test]
    fn the_utf32_dump_writes_every_character_unfiltered() {
        use kurbo::Affine;
        let unit = |unicode: u32| CharBox {
            char_type: CharType::Normal,
            unicode,
            code: None,
            origin: Point::ZERO,
            char_box: Rect::ZERO,
            loose_char_box: Rect::ZERO,
            matrix: Affine::IDENTITY,
            object: None,
            font_size: 1.0,
            angle: 0.0,
        };
        // 'a', the hyphen sentinel, 's' -- the `bug_781804.pdf` shape.
        let page = TextPage {
            chars: vec![unit(0x61), unit(0x02), unit(0x73)],
            ..TextPage::default()
        };
        assert_eq!(
            page.to_utf32le(),
            [
                0xFF, 0xFE, 0x00, 0x00, // BOM
                0x61, 0x00, 0x00, 0x00, // 'a'
                0x02, 0x00, 0x00, 0x00, // U+0002, not U+FFFE
                0x73, 0x00, 0x00, 0x00, // 's'
            ]
        );
        // And a zero is written as four zero bytes rather than skipped.
        let page = TextPage {
            chars: vec![unit(0)],
            ..TextPage::default()
        };
        assert_eq!(page.to_utf32le(), [0xFF, 0xFE, 0x00, 0x00, 0, 0, 0, 0]);
    }

    #[test]
    fn char_at_names_the_bound_it_broke() {
        let page = TextPage::default();
        assert_eq!(
            page.char_at(3),
            Err(Error::CharIndexOutOfRange { index: 3, len: 0 })
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
        assert_send_sync::<CharIndex>();
    }
}
