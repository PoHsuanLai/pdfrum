//! Changing what a page draws, and writing the result out.
//!
//! # Nothing is mutated until you save
//!
//! [`Document`](crate::Document) is a shared, `Sync`, lazily-caching reader, and every other
//! crate borrows from that shape. So editing follows the same pattern form
//! filling does: [`Page::edit`] hands back an owned [`PageEdit`] holding this
//! page's object graph, you change *that*, and
//! [`Document::save_pages`](crate::Document::save_pages) turns the changes into replacement objects on the
//! way out. The document itself is never touched, which is why editing a page
//! does not need a `&mut Document` and why two threads can edit two pages at
//! once.
//!
//! ```no_run
//! use pdfrum::{Document, SaveOptions};
//!
//! let doc = Document::open("in.pdf")?;
//! let mut page = doc.page(0)?.edit();
//! page.remove(0);
//! doc.save_pages("out.pdf", &[page], &SaveOptions::default())?;
//! # Ok::<(), pdfrum::Error>(())
//! ```
//!
//! # Saving an edited page rewrites it, and rewriting loses things
//!
//! A page whose objects you changed is written again **from the object
//! graph**, not patched. That regeneration reproduces what PDFium's does,
//! including what PDFium's drops: only `rg`/`RG` colours survive, so a CMYK or
//! ICC-based fill comes back black; patterns, shadings and Type 3 text are
//! lost entirely; and text keeps only its matrix, font, render mode and
//! strings, so character and word spacing go. The losses are listed in full in
//! `pdfrum_edit::content`.
//!
//! Two consequences worth stating plainly. First, this applies **only to pages
//! you edited** — every other page of the document is copied through
//! byte-for-byte, and a document you edit nothing in saves exactly as it would
//! have without this module. Second, the losses are matched to the oracle
//! deliberately rather than tolerated: a regenerated page of ours is compared
//! against a regenerated page of PDFium's, and emitting *more* than it does
//! would fail that comparison as surely as emitting less.

use kurbo::{Affine, BezPath, Rect};
use pdfrum_page::state::GraphicsState;
use pdfrum_page::{ColorSpace, Content, FillRule, PageObject, PathObject, TextObject, TextSegment};

use crate::Page;

/// One page's object graph, opened for editing.
///
/// Obtained from [`Page::edit`]. Holds the objects the page draws, in painting
/// order, plus a record of what has changed — which is what tells the save
/// which content streams to write again and which to leave alone.
///
/// ```
/// let doc = pdfrum::Document::open("tests/fixtures/hello_world.pdf")?;
/// let page = doc.page(0)?.edit();
/// assert_eq!(page.len(), 2);
/// assert!(!page.is_modified());
/// # Ok::<(), pdfrum::Error>(())
/// ```
#[derive(Debug, Clone)]
pub struct PageEdit {
    pub(crate) index: u32,
    pub(crate) page: pdfrum_page::Page,
}

impl PageEdit {
    /// How many objects the page draws, including any switched off with
    /// [`PageEdit::set_visible`].
    #[must_use]
    pub fn len(&self) -> usize {
        self.page.objects().len()
    }

    /// Whether the page draws nothing at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.page.objects().is_empty()
    }

    /// The zero-based index of the page being edited.
    #[must_use]
    pub fn index(&self) -> u32 {
        self.index
    }

    /// The objects, in painting order.
    #[must_use]
    pub fn objects(&self) -> &[PageObject] {
        self.page.objects()
    }

    /// The object at `index`, for a caller who wants to change it in place.
    ///
    /// Taking this reference *is* the edit — the object is marked changed on
    /// the way out, so its content stream is rewritten whether or not you go
    /// on to touch it. Read with [`PageEdit::objects`] when you only want to
    /// look.
    pub fn object_mut(&mut self, index: usize) -> Option<&mut PageObject> {
        self.page.object_mut(index)
    }

    /// Append an object, drawn last and therefore on top.
    pub fn push(&mut self, object: PageObject) {
        self.page.push_object(object);
    }

    /// Insert an object at `index`, pushing the ones there and after later in
    /// the painting order. Returns `false` when `index` is past the end.
    pub fn insert(&mut self, index: usize, object: PageObject) -> bool {
        self.page.insert_object(index, object)
    }

    /// Remove the object at `index` and hand it back.
    pub fn remove(&mut self, index: usize) -> Option<PageObject> {
        self.page.remove_object(index)
    }

    /// Show or hide the object at `index` without removing it.
    ///
    /// A hidden object keeps its place and its index, so it can be shown again
    /// — but it contributes nothing to the saved page, and reloading the saved
    /// file will not find it. Returns `false` when there is no such object.
    pub fn set_visible(&mut self, index: usize, visible: bool) -> bool {
        let Some(object) = self.page.objects.get_mut(index) else {
            return false;
        };
        object.set_active(visible);
        true
    }

    /// Whether the object at `index` is drawn.
    #[must_use]
    pub fn is_visible(&self, index: usize) -> Option<bool> {
        self.page.objects().get(index).map(PageObject::is_active)
    }

    /// Move the object at `index` by `transform`.
    ///
    /// The transform is applied *before* the object's existing one, so
    /// `Affine::translate((10.0, 0.0))` moves it ten points right in page
    /// space whatever it was already doing.
    pub fn transform(&mut self, index: usize, transform: Affine) -> bool {
        let Some(object) = self.page.object_mut(index) else {
            return false;
        };
        match object {
            PageObject::Path(p) => p.object.matrix = transform * p.object.matrix,
            PageObject::Text(t) => {
                t.object.matrix = transform * t.object.matrix;
                t.object.position = transform * t.object.position;
            }
            PageObject::Image(i) => i.object.matrix = transform * i.object.matrix,
            PageObject::Shading(s) => s.object.matrix = transform * s.object.matrix,
            PageObject::Form(f) => f.object.matrix = transform * f.object.matrix,
        }
        true
    }

    /// Whether anything has been changed since the page was opened.
    ///
    /// A `false` here means the save will not rewrite this page's content at
    /// all, and its bytes will come through untouched.
    #[must_use]
    pub fn is_modified(&self) -> bool {
        self.page.is_dirty()
    }

    /// The object graph underneath, for a caller reaching past this surface
    /// into `pdfrum-page`.
    #[must_use]
    pub fn graph(&self) -> &pdfrum_page::Page {
        &self.page
    }

    /// The graph, mutably. Marks nothing: a caller reaching here is
    /// responsible for saying what it changed.
    pub fn graph_mut(&mut self) -> &mut pdfrum_page::Page {
        &mut self.page
    }

    /// The page's `/Resources` as it stands in the document, which the
    /// regenerator consults for the names already in use.
    pub(crate) fn resources(&self, doc: &crate::Document) -> pdfrum_object::Dict {
        doc.inner
            .page(self.index)
            .ok()
            .and_then(|page| {
                page.inherited(pdfrum_object::names::RESOURCES, &doc.inner)?
                    .resolve(&doc.inner)
                    .ok()?
                    .as_dict()
                    .cloned()
            })
            .unwrap_or_default()
    }
}

/// A filled or stroked path, ready to [`PageEdit::push`].
///
/// A plain config struct filled in with struct-update syntax, and
/// [`PathBuilder::build`] turns it into the page object. The graphics state it
/// paints under is stated in full rather than inherited, because a regenerated
/// stream restates everything from the PDF defaults anyway.
///
/// ```
/// use pdfrum::{PathBuilder, kurbo::Rect};
///
/// let object = PathBuilder {
///     fill: Some([1.0, 0.0, 0.0]),
///     ..PathBuilder::rect(Rect::new(10.0, 10.0, 60.0, 40.0))
/// }
/// .build();
/// assert!(matches!(object, pdfrum::PageObject::Path(_)));
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct PathBuilder {
    /// The outline, in page space.
    pub path: BezPath,
    /// The fill colour as RGB in 0..=1, or `None` for no fill.
    pub fill: Option<[f32; 3]>,
    /// The stroke colour as RGB in 0..=1, or `None` for no stroke.
    pub stroke: Option<[f32; 3]>,
    /// The stroke width in page units.
    pub line_width: f32,
    /// Whether the fill uses the even-odd rule rather than the nonzero one.
    pub even_odd: bool,
    /// A further transform on the path, composed after it.
    pub matrix: Affine,
}

impl Default for PathBuilder {
    fn default() -> Self {
        Self {
            path: BezPath::new(),
            fill: Some([0.0, 0.0, 0.0]),
            stroke: None,
            line_width: 1.0,
            even_odd: false,
            matrix: Affine::IDENTITY,
        }
    }
}

impl PathBuilder {
    /// A rectangle, filled black.
    #[must_use]
    pub fn rect(rect: Rect) -> Self {
        let mut path = BezPath::new();
        path.move_to((rect.x0, rect.y0));
        path.line_to((rect.x1, rect.y0));
        path.line_to((rect.x1, rect.y1));
        path.line_to((rect.x0, rect.y1));
        path.close_path();
        Self {
            path,
            ..Self::default()
        }
    }

    /// The page object this describes.
    #[must_use]
    pub fn build(self) -> PageObject {
        let mut state = GraphicsState::default();
        if let Some(rgb) = self.fill {
            state.fill.set_stock(ColorSpace::DeviceRgb, &rgb);
        }
        if let Some(rgb) = self.stroke {
            state.stroke.set_stock(ColorSpace::DeviceRgb, &rgb);
        }
        state.stroke_params.width = self.line_width;
        let fill_rule = match (self.fill.is_some(), self.even_odd) {
            (false, _) => FillRule::None,
            (true, false) => FillRule::Winding,
            (true, true) => FillRule::EvenOdd,
        };
        PageObject::Path(Box::new(Content::new(
            PathObject {
                path: self.path,
                matrix: self.matrix,
                fill_rule,
                stroke: self.stroke.is_some(),
            },
            state,
        )))
    }
}

/// A run of text, ready to [`PageEdit::push`].
///
/// The font is named by the `/Font` resource entry it already has on the page:
/// a regenerated stream writes `/Name Tf`, so the font must be something the
/// page's resources can reach. [`PageEdit::font_of`] finds one from an
/// existing text object, which is the reliable way to get a reference that
/// resolves.
#[derive(Debug, Clone, PartialEq)]
pub struct TextBuilder {
    /// The character codes, in the font's own encoding.
    pub codes: Vec<u8>,
    /// The `/Font` resource this page reaches the font through.
    pub font: pdfrum_object::ObjRef,
    /// The font size in page units.
    pub size: f32,
    /// Where the baseline starts, in page space.
    pub position: kurbo::Point,
    /// How the glyphs are painted.
    pub render_mode: pdfrum_page::TextRenderMode,
    /// The fill colour as RGB in 0..=1.
    pub fill: [f32; 3],
}

impl TextBuilder {
    /// A run of `codes` in the font `font` names, at `size`, starting at the
    /// origin and painted black.
    #[must_use]
    pub fn new(codes: impl Into<Vec<u8>>, font: pdfrum_object::ObjRef, size: f32) -> Self {
        Self {
            codes: codes.into(),
            font,
            size,
            position: kurbo::Point::ZERO,
            render_mode: pdfrum_page::TextRenderMode::Fill,
            fill: [0.0, 0.0, 0.0],
        }
    }

    /// The page object this describes.
    #[must_use]
    pub fn build(self) -> PageObject {
        let mut state = GraphicsState::default();
        state.fill.set_stock(ColorSpace::DeviceRgb, &self.fill);
        PageObject::Text(Box::new(Content::new(
            TextObject {
                segments: Box::new([TextSegment {
                    codes: self.codes.into_boxed_slice(),
                    kerning: 0.0,
                }]),
                position: self.position,
                // The glyph matrix carries the size, as the interpreter builds
                // it: the emitter writes `Tf` and `Tm` separately and the
                // matrix it writes is this one with the translation dropped.
                matrix: Affine::scale(f64::from(self.size)),
                font: None,
                font_source: Some(self.font),
                render_mode: self.render_mode,
                type3_metrics: std::collections::BTreeMap::new(),
            },
            state,
        )))
    }
}

/// An image placement, ready to [`PageEdit::push`].
///
/// The pixels are not supplied here: an image page object names an `/XObject`
/// the file already holds, and the regenerated stream writes `/Name Do`. So
/// this places an image the document has — one found on another page, or one
/// added to the document beforehand — rather than encoding a new one.
#[derive(Debug, Clone, PartialEq)]
pub struct ImageBuilder {
    /// The image `XObject`.
    pub source: pdfrum_object::ObjRef,
    /// Where it lands: the matrix mapping the unit square onto the page.
    ///
    /// `Affine::new([w, 0.0, 0.0, h, x, y])` places a `w` by `h` image with
    /// its lower-left corner at `(x, y)`.
    pub matrix: Affine,
}

impl ImageBuilder {
    /// Place `source` in the rectangle `rect`.
    #[must_use]
    pub fn at(source: pdfrum_object::ObjRef, rect: Rect) -> Self {
        Self {
            source,
            matrix: Affine::new([rect.width(), 0.0, 0.0, rect.height(), rect.x0, rect.y0]),
        }
    }

    /// The page object this describes.
    ///
    /// The pixels are a one-by-one placeholder: a regenerated stream writes a
    /// `Do` naming the source, and never looks at them. Rendering the edited
    /// graph before saving would show the placeholder rather than the image,
    /// so render the *saved* file to see the result.
    #[must_use]
    pub fn build(self) -> PageObject {
        let placeholder = pdfrum_page::ImageData {
            width: 1,
            height: 1,
            pixels: pdfrum_page::Pixels::Gray8(Box::new([0])),
            mask: None,
            matte: None,
            interpolate: false,
        };
        PageObject::Image(Box::new(Content::new(
            pdfrum_page::ImageObject {
                image: std::sync::Arc::new(placeholder),
                matrix: self.matrix,
                is_mask: false,
                oc: None,
                source: Some(self.source),
            },
            GraphicsState::default(),
        )))
    }
}

impl PageEdit {
    /// The `/Font` resource the text object at `index` uses, for a
    /// [`TextBuilder`] that wants to write in the same font.
    ///
    /// `None` when there is no such object, when it is not text, or when its
    /// font was written inline in the resource dictionary and so has no object
    /// to name.
    #[must_use]
    pub fn font_of(&self, index: usize) -> Option<pdfrum_object::ObjRef> {
        match self.page.objects().get(index)? {
            PageObject::Text(text) => text.object.font_source,
            _ => None,
        }
    }

    /// The image `XObject` the object at `index` draws, for an
    /// [`ImageBuilder`] that wants to place the same image again.
    ///
    /// `None` for anything that is not an image, and for an inline image,
    /// which has no indirect object to name.
    #[must_use]
    pub fn image_of(&self, index: usize) -> Option<pdfrum_object::ObjRef> {
        match self.page.objects().get(index)? {
            PageObject::Image(image) => image.object.source,
            _ => None,
        }
    }
}

impl Page<'_> {
    /// Open this page's object graph for editing.
    ///
    /// Interpreting the content stream is the expensive part and it happens
    /// here, once. See the [module docs](self) for what a save then does with
    /// the result.
    ///
    /// ```
    /// let doc = pdfrum::Document::open("tests/fixtures/hello_world.pdf")?;
    /// let mut page = doc.page(0)?.edit();
    ///
    /// // Two text objects; drop the second.
    /// assert_eq!(page.len(), 2);
    /// assert!(page.remove(1).is_some());
    /// assert_eq!(page.len(), 1);
    /// assert!(page.is_modified());
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    #[must_use]
    pub fn edit(&self) -> PageEdit {
        PageEdit {
            index: self.index,
            page: self.build_for_edit(&mut pdfrum_page::BuildContext::new()),
        }
    }
}
