//! Page annotations.

pub use pdfrum_doc::{AnnotFlags, Subtype};

use crate::Document;

/// One annotation on a page (ISO 32000-1 §12.5).
///
/// Obtained from [`Page::annotations`](crate::Page::annotations). Pop-up
/// annotations are excluded from that list, matching what a reader draws:
/// a pop-up is a detail of the note it belongs to rather than a mark of its
/// own.
///
/// ```
/// let doc = pdfrum::Document::open("tests/fixtures/text_form.pdf")?;
/// let annots = doc.page(0)?.annotations();
///
/// let widget = &annots[0];
/// assert_eq!(widget.subtype(), pdfrum::Subtype::Widget);
/// assert_eq!(widget.rect().width(), 100.0);
/// assert!(!widget.is_hidden());
/// # Ok::<(), pdfrum::Error>(())
/// ```
#[derive(Debug, Clone)]
pub struct Annotation<'a> {
    pub(crate) inner: pdfrum_doc::Annotation,
    pub(crate) doc: &'a Document,
}

impl Annotation<'_> {
    /// What kind of annotation it is.
    #[must_use]
    pub fn subtype(&self) -> Subtype {
        self.inner.subtype
    }

    /// The annotation's rectangle in page space, normalized so its corners
    /// are in the usual order.
    ///
    /// A file may write `/Rect` with its corners inverted; the raw value is
    /// on [`Annotation::raw_rect`] for a caller that needs the bytes as
    /// written.
    #[must_use]
    pub fn rect(&self) -> kurbo::Rect {
        self.inner.rect.abs()
    }

    /// The annotation's `/Rect` exactly as the file wrote it — not
    /// normalized, and not corrected for an inverted ordering.
    #[must_use]
    pub fn raw_rect(&self) -> kurbo::Rect {
        self.inner.rect
    }

    /// The annotation's flag word (`/F`).
    #[must_use]
    pub fn flags(&self) -> AnnotFlags {
        self.inner.flags
    }

    /// Whether the annotation is hidden and should not be drawn at all.
    #[must_use]
    pub fn is_hidden(&self) -> bool {
        self.inner.flags.is_hidden()
    }

    /// Whether the annotation appears in a printed copy.
    #[must_use]
    pub fn prints(&self) -> bool {
        self.inner.flags.prints()
    }

    /// The annotation's text contents (`/Contents`) — the body of a sticky
    /// note, the text of a free-text annotation, the description of a stamp.
    #[must_use]
    pub fn contents(&self) -> Option<String> {
        self.dict_text("Contents")
    }

    /// The annotation's title (`/T`), which for a markup annotation is
    /// conventionally its author.
    #[must_use]
    pub fn title(&self) -> Option<String> {
        self.dict_text("T")
    }

    /// The annotation's name (`/NM`), a document-unique identifier when the
    /// file gives one.
    #[must_use]
    pub fn name(&self) -> Option<String> {
        self.dict_text("NM")
    }

    /// The annotation's modification date (`/M`), as written.
    #[must_use]
    pub fn modified(&self) -> Option<String> {
        self.dict_text("M")
    }

    /// Whether the annotation carries a drawable appearance stream.
    ///
    /// One that does not is drawn from its own properties instead, if at all;
    /// most readers, and this engine, draw nothing.
    #[must_use]
    pub fn has_appearance(&self) -> bool {
        pdfrum_doc::annot::appearance::has_appearance(&self.inner.dict, self.doc.parser())
    }

    /// The quadrilaterals a text-markup annotation covers (`/QuadPoints`) —
    /// the runs of text a highlight or underline was drawn over.
    ///
    /// Empty for an annotation that has none.
    #[must_use]
    pub fn quad_points(&self) -> Vec<kurbo::Rect> {
        let array = self.inner.quad_points.as_ref();
        let count = pdfrum_doc::annot::quad::quad_point_count(array);
        (0..count)
            .map(|index| pdfrum_doc::annot::quad::rect_from_quad_points(array, index))
            .collect()
    }

    /// The annotation's own dictionary, for the long tail of per-subtype keys
    /// this type does not surface.
    ///
    /// The escape hatch, matching [`Document::parser`](crate::Document::parser).
    #[must_use]
    pub fn dict(&self) -> &pdfrum_object::Dict {
        &self.inner.dict
    }

    fn dict_text(&self, key: &str) -> Option<String> {
        self.inner
            .dict
            .text(&pdfrum_object::Name::from(key), self.doc.parser())
            .filter(|text| !text.is_empty())
    }
}
