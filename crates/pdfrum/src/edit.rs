//! Changing what a page draws, and writing the result out.

use kurbo::{Affine, BezPath, Rect};
use pdfrum_common::PageIndex;
use pdfrum_page::GraphicsState;
use pdfrum_page::{
    ColorSpace, Content, FillRule, IndexOutOfRange, PageObject, PathObject, TextObject, TextSegment,
};

use crate::Page;

/// One page's object graph, opened for editing.
///
/// Obtained from [`Page::edit`]. Holds the objects the page draws, in painting
/// order, plus a record of what has changed — which is what tells the save
/// which content streams to write again and which to leave alone.
///
/// # Nothing is mutated until you save
///
/// [`Page::edit`] hands back an owned graph, you change *that*, and
/// [`Document::save_pages`](crate::Document::save_pages) turns the changes
/// into replacement objects on the way out. The document is never touched,
/// which is why editing a page needs no `&mut Document` and why two threads
/// can edit two pages at once.
///
/// # Saving an edited page rewrites it, and rewriting loses things
///
/// A page whose objects you changed is written again **from the object
/// graph**, not patched. Only `rg`/`RG` colours survive, so a CMYK or
/// ICC-based fill comes back black; patterns, shadings and Type 3 text are
/// lost; text keeps only its matrix, font, render mode and strings, so
/// character and word spacing go. [`pdfrum_edit`]'s crate documentation lists
/// them in full. This applies **only to pages you edited** — every other page
/// is copied through byte-for-byte.
///
/// ```no_run
/// use pdfrum::{Document, SaveOptions};
///
/// let doc = Document::open("in.pdf")?;
/// let mut page = doc.page(0)?.edit();
/// page.remove(0);
/// doc.save_pages("out.pdf", &[page], &SaveOptions::default())?;
/// # Ok::<(), pdfrum::Error>(())
/// ```
#[derive(Debug, Clone)]
pub struct PageEdit {
    pub(crate) index: PageIndex,
    pub(crate) page: pdfrum_page::Page,
}

impl PageEdit {
    /// How many objects the page draws, including any switched off with
    /// [`PageEdit::hide`].
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
    pub fn index(&self) -> PageIndex {
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
    /// the painting order. An index equal to [`PageEdit::len`] appends.
    ///
    /// # Errors
    ///
    /// [`IndexOutOfRange`] when `index` is past the end.
    pub fn insert(&mut self, index: usize, object: PageObject) -> Result<(), IndexOutOfRange> {
        self.page.insert_object(index, object)
    }

    /// Remove the object at `index` and hand it back.
    pub fn remove(&mut self, index: usize) -> Option<PageObject> {
        self.page.remove_object(index)
    }

    /// Show the object at `index` without removing it.
    ///
    /// A hidden object keeps its place and its index, so it can be shown again
    /// — but it contributes nothing to the saved page, and reloading the saved
    /// file will not find it.
    ///
    /// # Errors
    ///
    /// [`IndexOutOfRange`] when there is no object at `index`.
    pub fn show(&mut self, index: usize) -> Result<(), IndexOutOfRange> {
        self.set_active(index, true)
    }

    /// Hide the object at `index` without removing it.
    ///
    /// See [`PageEdit::show`].
    ///
    /// # Errors
    ///
    /// [`IndexOutOfRange`] when there is no object at `index`.
    pub fn hide(&mut self, index: usize) -> Result<(), IndexOutOfRange> {
        self.set_active(index, false)
    }

    fn set_active(&mut self, index: usize, active: bool) -> Result<(), IndexOutOfRange> {
        let len = self.page.objects.len();
        let Some(object) = self.page.objects.get_mut(index) else {
            return Err(IndexOutOfRange { index, len });
        };
        object.set_active(active);
        Ok(())
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
    ///
    /// # Errors
    ///
    /// [`IndexOutOfRange`] when there is no object at `index`.
    pub fn transform(&mut self, index: usize, transform: Affine) -> Result<(), IndexOutOfRange> {
        let len = self.page.objects.len();
        let Some(object) = self.page.object_mut(index) else {
            return Err(IndexOutOfRange { index, len });
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
        Ok(())
    }

    /// Whether anything has been changed since the page was opened.
    ///
    /// A `false` here means the save will not rewrite this page's content at
    /// all, and its bytes will come through untouched.
    #[must_use]
    pub fn is_modified(&self) -> bool {
        self.page.is_dirty()
    }

    /// **Escape hatch — requires `pdfrum-page`.** The object graph
    /// underneath, for a caller reaching past this surface.
    ///
    /// Matches [`Page::objects`](crate::Page::objects) and
    /// [`Document::parser`](crate::Document::parser).
    #[must_use]
    pub fn graph(&self) -> &pdfrum_page::Page {
        &self.page
    }

    /// **Escape hatch — requires `pdfrum-page`.** The graph, mutably.
    ///
    /// Marks nothing: a caller reaching here is responsible for saying what it
    /// changed.
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

/// The RGB triple and the alpha `pdfrum-edit` writes, out of a
/// [`peniko::Color`].
///
/// The facade takes the idiomatic type and narrows on the first line, so the
/// writer's `[f32; 3]` never appears in a public signature. Two things happen
/// here.
///
/// **The components are clamped to `0..=1`.** A PDF colour operand outside
/// that range is out of gamut, and the engine clamps it on the way back out
/// (`pdfrum_page`'s `rgb_to_rgb`), so clamping here changes no rendered
/// pixel — it only means the value a caller reads back off the *saved file*
/// is the value the writer actually used, rather than one the reader would
/// clamp again.
///
/// **Alpha is kept, not dropped.** The colour itself goes out as `rg`/`RG`,
/// which carries no alpha, but constant alpha has its own home in a PDF:
/// `/ca` and `/CA` in an `/ExtGState`, which `pdfrum-edit`'s emitter already
/// writes from [`GraphicsState`]'s `fill_alpha` / `stroke_alpha`. So a
/// translucent [`peniko::Color`] produces a translucent object rather than
/// silently losing its alpha at the door.
///
/// The one consequence worth stating: **alpha 0 is transparent, not "no
/// fill".** `Color::from_rgba8(255, 0, 0, 0)` paints an invisible red fill —
/// the object is still there, still in the painting order, still saved. "No
/// fill" is spelled `None` on [`PathBuilder::fill`], and that `Option` is the
/// only thing that means it; a zero alpha is not a second spelling of it.
/// [`TextBuilder::fill`] has no `None` at all, so there alpha 0 is simply
/// invisible text.
fn rgba_of(color: peniko::Color) -> ([f32; 3], f32) {
    let [r, g, b, alpha] = color.components;
    (
        [r.clamp(0.0, 1.0), g.clamp(0.0, 1.0), b.clamp(0.0, 1.0)],
        alpha.clamp(0.0, 1.0),
    )
}

/// A filled or stroked path, ready to [`PageEdit::push`].
///
/// A plain config struct filled in with struct-update syntax, and
/// [`PathBuilder::build`] turns it into the page object. The graphics state it
/// paints under is stated in full rather than inherited, because a regenerated
/// stream restates everything from the PDF defaults anyway.
#[derive(Debug, Clone, PartialEq)]
pub struct PathBuilder {
    /// The outline, in page space.
    pub path: BezPath,
    /// The fill colour, or `None` for no fill.
    ///
    /// The alpha is honoured — it goes out as an `/ExtGState` `/ca` — so a
    /// translucent colour paints a translucent fill. Note that alpha 0 is
    /// therefore *invisible*, not *absent*: `None` is the only spelling of
    /// "do not fill".
    pub fill: Option<peniko::Color>,
    /// The stroke colour, or `None` for no stroke.
    ///
    /// The alpha is honoured as `/CA`, exactly as [`PathBuilder::fill`]'s is.
    pub stroke: Option<peniko::Color>,
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
            fill: Some(peniko::Color::BLACK),
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
        if let Some(fill) = self.fill {
            let (rgb, alpha) = rgba_of(fill);
            state.fill.set_stock(ColorSpace::DeviceRgb, &rgb);
            state.general.fill_alpha = alpha;
        }
        if let Some(stroke) = self.stroke {
            let (rgb, alpha) = rgba_of(stroke);
            state.stroke.set_stock(ColorSpace::DeviceRgb, &rgb);
            state.general.stroke_alpha = alpha;
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
/// The font is named by an indirect `/Font` dictionary: a regenerated stream
/// writes `/Name Tf` and [`pdfrum_edit::ResourceTable::realize`] allocates
/// that name for [`TextBuilder::font`]. Obtain the reference from
/// [`PageEdit::font_of`] (a font the page already has) or from
/// [`crate::DocEdit::embed_font`] / [`crate::DocEdit::standard_font`] (a font
/// this save is adding).
///
/// [`crate::ImageBuilder`] names an `/XObject` the same way.
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
    /// The fill colour.
    ///
    /// The alpha is honoured, as [`PathBuilder::fill`]'s is. There is no
    /// `None` here, so alpha 0 paints invisible text rather than no text.
    pub fill: peniko::Color,
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
            fill: peniko::Color::BLACK,
        }
    }

    /// The page object this describes.
    #[must_use]
    pub fn build(self) -> PageObject {
        let mut state = GraphicsState::default();
        let (rgb, alpha) = rgba_of(self.fill);
        state.fill.set_stock(ColorSpace::DeviceRgb, &rgb);
        state.general.fill_alpha = alpha;
        PageObject::Text(Box::new(Content::new(
            TextObject {
                segments: Box::new([TextSegment {
                    codes: self.codes.into_boxed_slice(),
                    kerning: 0.0,
                }]),
                position: self.position,
                // Size lives only in the matrix. The emitter writes `Tf` from
                // this scale and `Tm` with it divided out; putting the size
                // in `font.1` as well would scale the saved run twice. `font`
                // stays `None`: a constructed object names the dict through
                // `font_source`, and loading an unrelated face just to pass
                // the emitter's old `font: None` refusal was the wrong kind
                // of fix.
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
/// The pixels are not supplied here: an image page object names an `/XObject`,
/// and the regenerated stream writes `/Name Do`. Obtain the reference from
/// [`PageEdit::image_of`] (an image the document already holds) or from
/// [`crate::DocEdit::embed_jpeg`] / [`crate::DocEdit::embed_image`] (one this
/// save is adding).
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
    ///
    /// The image is stretched onto `rect`; nothing preserves its aspect
    /// ratio, so a caller that wants it kept sizes `rect` from
    /// [`crate::EmbeddedImage::width`] and [`crate::EmbeddedImage::height`].
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
    /// here, once. See [`PageEdit`] for what a save then does with
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
