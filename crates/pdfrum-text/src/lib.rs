//! Text extraction, search, and link detection for PDF pages
//! (ISO 32000-1 §14.8.2).
//!
//! Start with [`extract`] to obtain a [`TextPage`], which carries two
//! sequences that are *not* the same: the character stream
//! ([`CharIndex`]) and the search-facing text ([`TextIndex`]). See
//! [`TextPage`] for what each holds and which one a given method speaks.
//!
//! Extraction takes an *interpreted* page — the page-object graph
//! `pdfrum-page` builds — and the resolver that page's indirect objects live
//! in, so the whole call is one function over values:
//!
//! ```
//! use pdfrum_common::{Diagnostics, Limits};
//! use pdfrum_text::{CharIndex, ExtractOptions, extract};
//!
//! # fn demo(page: &pdfrum_page::Page, resolver: &impl pdfrum_object::Resolve) {
//! let mut diags = Diagnostics::default();
//! let text = extract(page, resolver, &ExtractOptions::default(), &Limits::default(), &mut diags);
//!
//! // The search-facing text, which is what `Display` writes.
//! println!("{text}");
//! // The character stream, with the geometry each glyph was drawn at.
//! for boxed in &text.chars {
//!     let _ = (boxed.unicode, boxed.char_box, boxed.font_size);
//! }
//! # }
//! ```
//!
//! The one thing to get right is **which of the two sequences a number counts
//! in**. [`TextPage::find`] answers in [`TextIndex`], [`TextPage::web_links`]
//! answers in [`CharIndex`], and [`TextPage::runs`] is the only conversion
//! between them:
//!
//! ```
//! use pdfrum_text::{CharIndex, FindOptions, TextIndex, TextPage};
//!
//! let page = TextPage {
//!     search_text: "Hello, world!".chars().collect(),
//!     ..TextPage::default()
//! };
//! let hit = page.find("world", FindOptions::default()).next().expect("a match");
//! assert_eq!(hit, TextIndex::new(7)..TextIndex::new(12));
//!
//! // A `TextIndex` is not a `CharIndex`; converting is `runs`' job, and on a
//! // page with no character stream there is nothing to convert to.
//! assert_eq!(page.runs.char_index(hit.start), None);
//! ```

#![forbid(unsafe_code)]
// Every number reaching this crate came from an untrusted file, by way of the
// page interpreter: index with `get()` and do arithmetic that cannot trap.
#![warn(clippy::indexing_slicing)]

// Every module is private and the crate root is the whole surface:
// a caller of `pdfrum-text` needs the types below, and the
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
mod word;

pub use charinfo::{CharBox, CharType, ObjectIndex};
pub use error::Error;
pub use find::FindOptions;
pub use index::{CharIndex, CharSegment, IndexMap, TextIndex};
pub use links::WebLink;
pub use object::TextRun;
pub use orientation::Orientation;
pub use word::Word;

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
use pdfrum_common::{DiagKind, Diagnostics, Limits, Operation, Severity};
use pdfrum_object::Resolve;
use pdfrum_page::Page;
use std::collections::BTreeMap;
use std::ops::{Range, RangeBounds};

/// How extraction behaves.
///
/// # Examples
///
/// ```
/// use pdfrum_text::ExtractOptions;
///
/// // The default reads direction from each line's own text.
/// assert!(!ExtractOptions::default().rtl);
///
/// // A document whose catalog says `/ViewerPreferences << /Direction /R2L >>`
/// // must be extracted with this set, or every line comes out in the wrong
/// // order.
/// let options = ExtractOptions { rtl: true };
/// assert!(options.rtl);
/// ```
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ExtractOptions {
    /// The document's `/Root /ViewerPreferences /Direction` is `R2L`.
    ///
    /// Forces every line's overall direction right-to-left, which reverses
    /// the *order* of its direction runs. A caller reading this out of the
    /// catalog must set it, or every document with that preference extracts
    /// in the wrong order.
    pub rtl: bool,
}

/// One page's extracted text (ISO 32000-1 §14.8.2).
///
/// Holds **two sequences that are not the same**, and conflating them is the
/// easiest way to get this crate wrong:
///
/// - [`chars`](Self::chars), addressed by [`CharIndex`], is every character
///   the page drew or the extractor invented, geometry attached. It keeps
///   control characters, `\0` for an unmappable code, and `U+0002` where a
///   word was hyphenated across a line.
/// - [`search_text`](Self::search_text), addressed by [`TextIndex`], is what
///   a search matches and a selection copies. It drops the control characters
///   and the placeholders, expands ligatures the character stream keeps
///   whole, and carries `U+00AD` at a hyphenated break and `U+FFFD` for an
///   unmappable code, so it can disagree with `chars` position by position.
///
/// [`runs`](Self::runs) converts between the two spaces; every signature
/// names which one it counts in. Cheap to clone and `Send + Sync`, so a
/// document's pages can be extracted in parallel.
///
/// # Examples
///
/// The fields are public, so a page can be built by hand — which is how the
/// query side is exercised without a file:
///
/// ```
/// use pdfrum_text::TextPage;
///
/// let page = TextPage {
///     search_text: "Hello, world!".chars().collect(),
///     ..TextPage::default()
/// };
/// // `Display` writes the search-facing text, never the character stream.
/// assert_eq!(page.to_string(), "Hello, world!");
/// // …which is a different sequence, and here an empty one.
/// assert_eq!(page.char_count(), 0);
/// ```
#[derive(Debug, Clone, Default)]
pub struct TextPage {
    /// Characters in reading order addressed by [`CharIndex`].
    pub chars: Vec<CharBox>,
    /// Normalized search-facing text addressed by [`TextIndex`].
    pub search_text: Vec<char>,
    /// Map between [`CharIndex`] and [`TextIndex`].
    pub runs: IndexMap,
    /// The base font name of each text object whose font has one, by the
    /// [`ObjectIndex`] its characters carry. A Type 3 font has no base name
    /// and is absent. Read through a character with
    /// [`font_name`](Self::font_name).
    pub fonts: BTreeMap<ObjectIndex, String>,
}

/// Extracts text, layout, and reading order from an interpreted page
/// (ISO 32000-1 §14.8.2).
///
/// Infallible and never panicking: every place something is silently dropped
/// records a [`Diagnostic`](pdfrum_common::Diagnostic) into `diags` and
/// carries on. A `limits.deadline` that has passed is read once, here on
/// entry — a page is the extractor's unit of work — and answers an empty
/// page with [`DiagKind::TimeLimitReached`].
///
/// # Examples
///
/// A page with nothing on it extracts to nothing, without an error and
/// without a diagnostic:
///
/// ```
/// use pdfrum_common::{Diagnostics, Limits};
/// use pdfrum_object::NoResolve;
/// use pdfrum_page::Page;
/// use pdfrum_text::{ExtractOptions, extract};
///
/// let mut diags = Diagnostics::default();
/// let text = extract(
///     &Page::empty(),
///     &NoResolve,
///     &ExtractOptions::default(),
///     &Limits::default(),
///     &mut diags,
/// );
///
/// assert_eq!(text.char_count(), 0);
/// assert_eq!(text.to_string(), "");
/// assert!(diags.entries().is_empty());
/// ```
#[must_use]
pub fn extract<R: Resolve>(
    page: &Page,
    resolver: &R,
    options: &ExtractOptions,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> TextPage {
    if limits.check_deadline(Operation::Extract).is_err() {
        diags.record(Severity::Suspicious, DiagKind::TimeLimitReached, None);
        return TextPage::default();
    }
    if page.objects.is_empty() {
        return TextPage::default();
    }
    let runs = object::walk(&page.objects);
    let page_flow = orientation::page_flow(page, &runs);
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
    let fonts = runs
        .iter()
        .filter(|run| !run.font.base_font_name().is_empty())
        .map(|run| {
            let name = String::from_utf8_lossy(run.font.base_font_name()).into_owned();
            (run.index, name)
        })
        .collect();
    let runs = index::build(&out.chars);
    TextPage {
        chars: out.chars,
        search_text: text,
        runs,
        fonts,
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
    ///
    /// This counts [`chars`](Self::chars), not
    /// [`search_text`](Self::search_text): the two sequences have different
    /// lengths, and a page can have text and no characters or the reverse.
    ///
    /// # Examples
    ///
    /// ```
    /// use pdfrum_text::TextPage;
    ///
    /// let page = TextPage {
    ///     search_text: "Hello".chars().collect(),
    ///     ..TextPage::default()
    /// };
    /// assert_eq!(page.to_string().len(), 5);
    /// assert_eq!(page.char_count(), 0);
    /// ```
    #[must_use]
    pub fn char_count(&self) -> usize {
        self.chars.len()
    }

    /// A run of the [`search_text`](Self::search_text), addressed in
    /// [`CharIndex`].
    ///
    /// The bounds are **widened onto real text**, not filtered: a start on a
    /// character the text does not hold scans forward to the next one it
    /// does, and an end on one scans back. Characters inside the range are
    /// never skipped. So on `control_characters.pdf`, asking for the fifteen
    /// characters from character 17 returns `"Goodbye, world!"` even though
    /// the text itself has no character 17.
    ///
    /// This is the one call where the two spaces of [`TextPage`] meet: the
    /// bounds count characters and the answer is text.
    ///
    /// # Examples
    ///
    /// A page holding `"Hello, world!"` on one line and `"Goodbye, world!"` on
    /// the next, with the extractor's generated `\r\n` between them:
    ///
    /// ```
    /// # use pdfrum_text::CharIndex;
    /// # use pdfrum_common::{Diagnostics, Limits};
    /// # use pdfrum_object::{Name, Object};
    /// # use pdfrum_page::{BuildContext, Resources, build_page_from_dict, parse_content};
    /// # use std::sync::Arc;
    /// # let bytes: Arc<[u8]> = Arc::from(&include_bytes!("../tests/files/hello.pdf")[..]);
    /// # let doc = pdfrum_parser::load(bytes, &pdfrum_parser::LoadOptions::default())?;
    /// # let loaded = doc.page(0)?;
    /// # let (limits, mut diags) = (Limits::default(), Diagnostics::default());
    /// # let mut content = Vec::new();
    /// # if let Some(contents) = loaded.dict.get(&Name::from("Contents"), &doc)
    /// #     && let Some(Object::Stream(stream)) = contents.as_direct() {
    /// #     content.extend_from_slice(
    /// #         &pdfrum_filters::decode_chain(stream, 0, &doc, &limits, &mut diags).data);
    /// # }
    /// # let ops = parse_content(&content, &limits, &mut diags);
    /// # let resources = Resources::for_page(
    /// #     loaded.inherited(&Name::from("Resources"), &doc)
    /// #         .and_then(|o| o.resolve(&doc).ok()?.as_dict().cloned()));
    /// # let built = build_page_from_dict(&ops, &loaded.dict, |k| loaded.inherited(k, &doc),
    /// #     &resources, &doc, &mut BuildContext::default(), &limits, &mut diags);
    /// # let page = pdfrum_text::extract(&built, &doc, &pdfrum_text::ExtractOptions::default(),
    /// #     &limits, &mut diags);
    /// let at = CharIndex::new;
    /// assert_eq!(page.slice(at(0)..at(5)), "Hello");
    /// // An unbounded end is "to the end of the page".
    /// assert_eq!(page.slice(at(15)..), "Goodbye, world!");
    /// assert_eq!(page.slice(..), "Hello, world!\r\nGoodbye, world!");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
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

    /// Searches the page.
    ///
    /// Match ranges are [`TextIndex`] offsets into
    /// [`search_text`](Self::search_text) — **not** the [`CharIndex`] space
    /// [`web_links`](Self::web_links) reports; see [`TextPage`].
    ///
    /// # Examples
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

    /// Every web and mail address in the page's text.
    ///
    /// Reported ranges are [`CharIndex`] spans into [`chars`](Self::chars) —
    /// **not** the [`TextIndex`] space [`find`](Self::find) returns. The two
    /// index spaces are different sequences; see [`TextPage`].
    /// # Examples
    ///
    /// The fixture below draws no address, so nothing is reported; a page that
    /// draws `www.example.com` reports it with an `http://` already prefixed.
    ///
    /// ```
    /// # use pdfrum_common::{Diagnostics, Limits};
    /// # use pdfrum_object::{Name, Object};
    /// # use pdfrum_page::{BuildContext, Resources, build_page_from_dict, parse_content};
    /// # use std::sync::Arc;
    /// # let bytes: Arc<[u8]> = Arc::from(&include_bytes!("../tests/files/hello.pdf")[..]);
    /// # let doc = pdfrum_parser::load(bytes, &pdfrum_parser::LoadOptions::default())?;
    /// # let loaded = doc.page(0)?;
    /// # let (limits, mut diags) = (Limits::default(), Diagnostics::default());
    /// # let mut content = Vec::new();
    /// # if let Some(contents) = loaded.dict.get(&Name::from("Contents"), &doc)
    /// #     && let Some(Object::Stream(stream)) = contents.as_direct() {
    /// #     content.extend_from_slice(
    /// #         &pdfrum_filters::decode_chain(stream, 0, &doc, &limits, &mut diags).data);
    /// # }
    /// # let ops = parse_content(&content, &limits, &mut diags);
    /// # let resources = Resources::for_page(
    /// #     loaded.inherited(&Name::from("Resources"), &doc)
    /// #         .and_then(|o| o.resolve(&doc).ok()?.as_dict().cloned()));
    /// # let built = build_page_from_dict(&ops, &loaded.dict, |k| loaded.inherited(k, &doc),
    /// #     &resources, &doc, &mut BuildContext::default(), &limits, &mut diags);
    /// # let page = pdfrum_text::extract(&built, &doc, &pdfrum_text::ExtractOptions::default(),
    /// #     &limits, &mut diags);
    /// assert!(page.web_links().is_empty());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn web_links(&self) -> Vec<WebLink> {
        links::extract(&self.chars, &self.search_text, &index::build(&self.chars))
    }

    /// The boxes covering a run of [`CharIndex`], one per run of consecutive
    /// characters sharing a text object.
    ///
    /// Generated characters and boxes under 0.01 in either dimension are
    /// skipped, and a box is pushed **unconditionally at the end** — so a run
    /// in which every character was skipped still yields one box, an all-zero
    /// rectangle. An unbounded end is "to the end of the page", and a range
    /// running past the end takes what is there.
    /// # Examples
    ///
    /// Two lines set in two different fonts are two text objects, so the whole
    /// page yields two boxes rather than one:
    ///
    /// ```
    /// # use pdfrum_text::CharIndex;
    /// # use pdfrum_common::{Diagnostics, Limits};
    /// # use pdfrum_object::{Name, Object};
    /// # use pdfrum_page::{BuildContext, Resources, build_page_from_dict, parse_content};
    /// # use std::sync::Arc;
    /// # let bytes: Arc<[u8]> = Arc::from(&include_bytes!("../tests/files/hello.pdf")[..]);
    /// # let doc = pdfrum_parser::load(bytes, &pdfrum_parser::LoadOptions::default())?;
    /// # let loaded = doc.page(0)?;
    /// # let (limits, mut diags) = (Limits::default(), Diagnostics::default());
    /// # let mut content = Vec::new();
    /// # if let Some(contents) = loaded.dict.get(&Name::from("Contents"), &doc)
    /// #     && let Some(Object::Stream(stream)) = contents.as_direct() {
    /// #     content.extend_from_slice(
    /// #         &pdfrum_filters::decode_chain(stream, 0, &doc, &limits, &mut diags).data);
    /// # }
    /// # let ops = parse_content(&content, &limits, &mut diags);
    /// # let resources = Resources::for_page(
    /// #     loaded.inherited(&Name::from("Resources"), &doc)
    /// #         .and_then(|o| o.resolve(&doc).ok()?.as_dict().cloned()));
    /// # let built = build_page_from_dict(&ops, &loaded.dict, |k| loaded.inherited(k, &doc),
    /// #     &resources, &doc, &mut BuildContext::default(), &limits, &mut diags);
    /// # let page = pdfrum_text::extract(&built, &doc, &pdfrum_text::ExtractOptions::default(),
    /// #     &limits, &mut diags);
    /// let boxes = page.rects(..);
    /// assert_eq!(boxes.len(), 2);
    /// // The first line sits below the second in page space, which is y-up.
    /// assert!(boxes[0].y1 < boxes[1].y0);
    ///
    /// // A run inside one object is one box.
    /// assert_eq!(page.rects(CharIndex::new(0)..CharIndex::new(5)).len(), 1);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn rects(&self, range: impl RangeBounds<CharIndex>) -> Vec<Rect> {
        select::rects(&self.chars, range)
    }

    /// The character under a point in page space, or the nearest within tolerance.
    /// A point inside a character's box wins outright and reports the
    /// **first** such character; failing that, and only when a tolerance is
    /// given, the nearest character within it.
    ///
    /// # Examples
    ///
    /// ```
    /// # use kurbo::{Point, Size};
    /// # use pdfrum_text::CharIndex;
    /// # use pdfrum_common::{Diagnostics, Limits};
    /// # use pdfrum_object::{Name, Object};
    /// # use pdfrum_page::{BuildContext, Resources, build_page_from_dict, parse_content};
    /// # use std::sync::Arc;
    /// # let bytes: Arc<[u8]> = Arc::from(&include_bytes!("../tests/files/hello.pdf")[..]);
    /// # let doc = pdfrum_parser::load(bytes, &pdfrum_parser::LoadOptions::default())?;
    /// # let loaded = doc.page(0)?;
    /// # let (limits, mut diags) = (Limits::default(), Diagnostics::default());
    /// # let mut content = Vec::new();
    /// # if let Some(contents) = loaded.dict.get(&Name::from("Contents"), &doc)
    /// #     && let Some(Object::Stream(stream)) = contents.as_direct() {
    /// #     content.extend_from_slice(
    /// #         &pdfrum_filters::decode_chain(stream, 0, &doc, &limits, &mut diags).data);
    /// # }
    /// # let ops = parse_content(&content, &limits, &mut diags);
    /// # let resources = Resources::for_page(
    /// #     loaded.inherited(&Name::from("Resources"), &doc)
    /// #         .and_then(|o| o.resolve(&doc).ok()?.as_dict().cloned()));
    /// # let built = build_page_from_dict(&ops, &loaded.dict, |k| loaded.inherited(k, &doc),
    /// #     &resources, &doc, &mut BuildContext::default(), &limits, &mut diags);
    /// # let page = pdfrum_text::extract(&built, &doc, &pdfrum_text::ExtractOptions::default(),
    /// #     &limits, &mut diags);
    /// // Inside the first glyph's box.
    /// assert_eq!(page.index_at(Point::new(24.0, 54.0), Size::ZERO), Some(CharIndex::new(0)));
    /// // Far from every glyph, with no tolerance to fall back on.
    /// assert_eq!(page.index_at(Point::new(500.0, 500.0), Size::ZERO), None);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn index_at(&self, point: Point, tolerance: Size) -> Option<CharIndex> {
        select::index_at(&self.chars, point, tolerance)
    }

    /// The text inside a rectangle, with `\r\n` where the selection crosses a
    /// baseline.
    /// # Examples
    ///
    /// ```
    /// # use kurbo::Rect;
    /// # use pdfrum_common::{Diagnostics, Limits};
    /// # use pdfrum_object::{Name, Object};
    /// # use pdfrum_page::{BuildContext, Resources, build_page_from_dict, parse_content};
    /// # use std::sync::Arc;
    /// # let bytes: Arc<[u8]> = Arc::from(&include_bytes!("../tests/files/hello.pdf")[..]);
    /// # let doc = pdfrum_parser::load(bytes, &pdfrum_parser::LoadOptions::default())?;
    /// # let loaded = doc.page(0)?;
    /// # let (limits, mut diags) = (Limits::default(), Diagnostics::default());
    /// # let mut content = Vec::new();
    /// # if let Some(contents) = loaded.dict.get(&Name::from("Contents"), &doc)
    /// #     && let Some(Object::Stream(stream)) = contents.as_direct() {
    /// #     content.extend_from_slice(
    /// #         &pdfrum_filters::decode_chain(stream, 0, &doc, &limits, &mut diags).data);
    /// # }
    /// # let ops = parse_content(&content, &limits, &mut diags);
    /// # let resources = Resources::for_page(
    /// #     loaded.inherited(&Name::from("Resources"), &doc)
    /// #         .and_then(|o| o.resolve(&doc).ok()?.as_dict().cloned()));
    /// # let built = build_page_from_dict(&ops, &loaded.dict, |k| loaded.inherited(k, &doc),
    /// #     &resources, &doc, &mut BuildContext::default(), &limits, &mut diags);
    /// # let page = pdfrum_text::extract(&built, &doc, &pdfrum_text::ExtractOptions::default(),
    /// #     &limits, &mut diags);
    /// // A rectangle covering only the lower line takes only its text.
    /// assert_eq!(page.text_in_rect(Rect::new(0.0, 0.0, 200.0, 70.0)), "Hello, world!");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn text_in_rect(&self, rect: Rect) -> String {
        select::text_in_rect(&self.chars, rect)
    }

    /// The text drawn by a specific text object.
    /// The [`ObjectIndex`] is the one a character carries in
    /// [`CharBox::object`], counting text objects in content order.
    ///
    /// # Examples
    ///
    /// ```
    /// # use pdfrum_text::ObjectIndex;
    /// # use pdfrum_common::{Diagnostics, Limits};
    /// # use pdfrum_object::{Name, Object};
    /// # use pdfrum_page::{BuildContext, Resources, build_page_from_dict, parse_content};
    /// # use std::sync::Arc;
    /// # let bytes: Arc<[u8]> = Arc::from(&include_bytes!("../tests/files/hello.pdf")[..]);
    /// # let doc = pdfrum_parser::load(bytes, &pdfrum_parser::LoadOptions::default())?;
    /// # let loaded = doc.page(0)?;
    /// # let (limits, mut diags) = (Limits::default(), Diagnostics::default());
    /// # let mut content = Vec::new();
    /// # if let Some(contents) = loaded.dict.get(&Name::from("Contents"), &doc)
    /// #     && let Some(Object::Stream(stream)) = contents.as_direct() {
    /// #     content.extend_from_slice(
    /// #         &pdfrum_filters::decode_chain(stream, 0, &doc, &limits, &mut diags).data);
    /// # }
    /// # let ops = parse_content(&content, &limits, &mut diags);
    /// # let resources = Resources::for_page(
    /// #     loaded.inherited(&Name::from("Resources"), &doc)
    /// #         .and_then(|o| o.resolve(&doc).ok()?.as_dict().cloned()));
    /// # let built = build_page_from_dict(&ops, &loaded.dict, |k| loaded.inherited(k, &doc),
    /// #     &resources, &doc, &mut BuildContext::default(), &limits, &mut diags);
    /// # let page = pdfrum_text::extract(&built, &doc, &pdfrum_text::ExtractOptions::default(),
    /// #     &limits, &mut diags);
    /// assert_eq!(page.text_of_object(ObjectIndex(0)), "Hello, world!");
    /// assert_eq!(page.text_of_object(ObjectIndex(1)), "Goodbye, world!");
    /// // An object the page does not have draws nothing.
    /// assert_eq!(page.text_of_object(ObjectIndex(9)), "");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn text_of_object(&self, object: ObjectIndex) -> String {
        select::text_of_object(&self.chars, object)
    }

    /// Returns the character box at the given [`CharIndex`].
    ///
    /// # Errors
    ///
    /// Returns [`Error::CharIndexOutOfRange`] when `index` is past the end of [`chars`](Self::chars).
    /// # Examples
    ///
    /// ```
    /// # use pdfrum_text::{CharIndex, CharType, Error};
    /// # use pdfrum_common::{Diagnostics, Limits};
    /// # use pdfrum_object::{Name, Object};
    /// # use pdfrum_page::{BuildContext, Resources, build_page_from_dict, parse_content};
    /// # use std::sync::Arc;
    /// # let bytes: Arc<[u8]> = Arc::from(&include_bytes!("../tests/files/hello.pdf")[..]);
    /// # let doc = pdfrum_parser::load(bytes, &pdfrum_parser::LoadOptions::default())?;
    /// # let loaded = doc.page(0)?;
    /// # let (limits, mut diags) = (Limits::default(), Diagnostics::default());
    /// # let mut content = Vec::new();
    /// # if let Some(contents) = loaded.dict.get(&Name::from("Contents"), &doc)
    /// #     && let Some(Object::Stream(stream)) = contents.as_direct() {
    /// #     content.extend_from_slice(
    /// #         &pdfrum_filters::decode_chain(stream, 0, &doc, &limits, &mut diags).data);
    /// # }
    /// # let ops = parse_content(&content, &limits, &mut diags);
    /// # let resources = Resources::for_page(
    /// #     loaded.inherited(&Name::from("Resources"), &doc)
    /// #         .and_then(|o| o.resolve(&doc).ok()?.as_dict().cloned()));
    /// # let built = build_page_from_dict(&ops, &loaded.dict, |k| loaded.inherited(k, &doc),
    /// #     &resources, &doc, &mut BuildContext::default(), &limits, &mut diags);
    /// # let page = pdfrum_text::extract(&built, &doc, &pdfrum_text::ExtractOptions::default(),
    /// #     &limits, &mut diags);
    /// let first = page.char(CharIndex::new(0))?;
    /// assert_eq!(char::from_u32(first.unicode), Some('H'));
    /// assert_eq!(first.char_type, CharType::Normal);
    /// assert_eq!(first.font_size, 12.0);
    ///
    /// // The error names both the index and the bound it broke.
    /// assert_eq!(
    ///     page.char(CharIndex::new(999)),
    ///     Err(Error::CharIndexOutOfRange { index: CharIndex::new(999), len: 30 }),
    /// );
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn char(&self, index: CharIndex) -> Result<&CharBox, Error> {
        self.chars
            .get(index.get())
            .ok_or(Error::CharIndexOutOfRange {
                index,
                len: self.chars.len(),
            })
    }

    /// The base font name of the font the character at `index` was drawn
    /// with, as the font crate normalized it — the subset tag stripped and a
    /// standard-14 alias canonicalized, so `ABCDEF+Arial,Bold` reads as
    /// `Helvetica-Bold`.
    ///
    /// `None` past the end, for a character no text object drew (every
    /// generated one), and for a font with no base name (Type 3).
    /// # Examples
    ///
    /// ```
    /// # use pdfrum_text::CharIndex;
    /// # use pdfrum_common::{Diagnostics, Limits};
    /// # use pdfrum_object::{Name, Object};
    /// # use pdfrum_page::{BuildContext, Resources, build_page_from_dict, parse_content};
    /// # use std::sync::Arc;
    /// # let bytes: Arc<[u8]> = Arc::from(&include_bytes!("../tests/files/hello.pdf")[..]);
    /// # let doc = pdfrum_parser::load(bytes, &pdfrum_parser::LoadOptions::default())?;
    /// # let loaded = doc.page(0)?;
    /// # let (limits, mut diags) = (Limits::default(), Diagnostics::default());
    /// # let mut content = Vec::new();
    /// # if let Some(contents) = loaded.dict.get(&Name::from("Contents"), &doc)
    /// #     && let Some(Object::Stream(stream)) = contents.as_direct() {
    /// #     content.extend_from_slice(
    /// #         &pdfrum_filters::decode_chain(stream, 0, &doc, &limits, &mut diags).data);
    /// # }
    /// # let ops = parse_content(&content, &limits, &mut diags);
    /// # let resources = Resources::for_page(
    /// #     loaded.inherited(&Name::from("Resources"), &doc)
    /// #         .and_then(|o| o.resolve(&doc).ok()?.as_dict().cloned()));
    /// # let built = build_page_from_dict(&ops, &loaded.dict, |k| loaded.inherited(k, &doc),
    /// #     &resources, &doc, &mut BuildContext::default(), &limits, &mut diags);
    /// # let page = pdfrum_text::extract(&built, &doc, &pdfrum_text::ExtractOptions::default(),
    /// #     &limits, &mut diags);
    /// // Two lines, two fonts.
    /// assert_eq!(page.font_name(CharIndex::new(0)), Some("Times-Roman"));
    /// assert_eq!(page.font_name(CharIndex::new(15)), Some("Helvetica"));
    ///
    /// // Character 13 is the line break the extractor generated: no text
    /// // object drew it, so it has no font.
    /// assert_eq!(page.font_name(CharIndex::new(13)), None);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn font_name(&self, index: CharIndex) -> Option<&str> {
        let object = self.chars.get(index.get())?.object?;
        self.fonts.get(&object).map(String::as_str)
    }
}

/// The words a page draws, in content order — the answer
/// `Doc.getPageNumWords` counts and `Doc.getPageNthWord` indexes into.
///
/// # A different reading of "word" from the extraction pipeline's
///
/// This walks the page's **text objects** and their raw character codes, and
/// it does not go through [`TextPage`] at all. That is deliberate rather than
/// an oversight: extraction reorders by reading order, suppresses duplicate
/// overprinted objects, inserts generated spaces and newlines and normalizes
/// what it emits, and every one of those would change the count. The
/// scripting API's word list is defined on the content stream as written.
///
/// # What separates two words
///
/// A single rule, applied per character: a character is *word-continuing*
/// when its first code unit is neither a space nor above `U+28FF`, and a run
/// of those is one word. So a character at `U+2900` or beyond — CJK, most
/// symbols — is a word of its own, and `Hello, world!` is **two** words
/// rather than four, because the comma and the exclamation mark are below the
/// threshold and continue the run.
///
/// A character whose font maps it to nothing contributes `U+0000`, which is
/// word-continuing; a space ends a word without starting one.
/// # Examples
///
/// Each text object restarts the run, so the two lines of the fixture below
/// are four words rather than two — and the comma stays attached, because it
/// is below the word-continuing cutoff:
///
/// ```
/// # use pdfrum_common::{Diagnostics, Limits};
/// # use pdfrum_object::{Name, Object};
/// # use pdfrum_page::{BuildContext, Resources, build_page_from_dict, parse_content};
/// # use std::sync::Arc;
/// # let bytes: Arc<[u8]> = Arc::from(&include_bytes!("../tests/files/hello.pdf")[..]);
/// # let doc = pdfrum_parser::load(bytes, &pdfrum_parser::LoadOptions::default())?;
/// # let loaded = doc.page(0)?;
/// # let (limits, mut diags) = (Limits::default(), Diagnostics::default());
/// # let mut content = Vec::new();
/// # if let Some(contents) = loaded.dict.get(&Name::from("Contents"), &doc)
/// #     && let Some(Object::Stream(stream)) = contents.as_direct() {
/// #     content.extend_from_slice(
/// #         &pdfrum_filters::decode_chain(stream, 0, &doc, &limits, &mut diags).data);
/// # }
/// # let ops = parse_content(&content, &limits, &mut diags);
/// # let resources = Resources::for_page(
/// #     loaded.inherited(&Name::from("Resources"), &doc)
/// #         .and_then(|o| o.resolve(&doc).ok()?.as_dict().cloned()));
/// # let built = build_page_from_dict(&ops, &loaded.dict, |k| loaded.inherited(k, &doc),
/// #     &resources, &doc, &mut BuildContext::default(), &limits, &mut diags);
/// assert_eq!(
///     pdfrum_text::words(&built),
///     ["Hello, ", "world!", "Goodbye, ", "world!"],
/// );
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[must_use]
pub fn words(page: &Page) -> Vec<String> {
    /// `IsLatinWord`: neither a space nor past the cutoff.
    fn continues_a_word(unicode: u32) -> bool {
        unicode != 0x20 && unicode <= 0x28FF
    }

    let mut out: Vec<String> = Vec::new();
    for run in object::walk(&page.objects) {
        // Each object restarts the run state, which is what makes a word
        // split across two text objects two words.
        let mut in_word = false;
        for item in &run.items {
            let mapped = run.font.unicode_from_charcode(item.code);
            // `WideString::Front()` on an empty string is `0`, and zero
            // continues a word.
            let unicode = mapped.first().map_or(0, |ch| *ch as u32);
            let continues = continues_a_word(unicode);
            if !continues || !in_word {
                in_word = continues;
                if unicode != 0x20 {
                    out.push(String::new());
                }
            }
            if let Some(word) = out.last_mut()
                && let Some(ch) = char::from_u32(unicode)
            {
                word.push(ch);
            }
        }
    }
    out
}

/// Formats the page as its [`search_text`](TextPage::search_text) — **not**
/// the [`chars`](TextPage::chars) stream, which holds the control characters
/// and placeholders this drops.
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
