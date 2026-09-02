//! One page: its geometry, its pixels, its text and its annotations.

use kurbo::Affine;
use pdfrum_common::Diagnostics;
use pdfrum_object::Name;
use pdfrum_page::BuildContext;
use pdfrum_parser::PageDict;
use pdfrum_render::RenderCaches;

use crate::{
    Annotation, Document, Pixmap, RasterBackend, RenderOptions, RenderSession, Result, TextPage,
    VelloCpuBackend,
};

/// One page of a [`Document`].
///
/// Cheap to obtain and cheap to hold: constructing a `Page` reads its
/// dictionary and its inherited attributes, which is a handful of dictionary
/// lookups. The expensive work — interpreting the content stream into a
/// drawable object graph — happens in [`Page::render`] and [`Page::text`],
/// each of which does it once for the call.
///
/// ```
/// let doc = pdfrum::Document::open("tests/fixtures/hello_world.pdf")?;
/// let page = doc.page(0)?;
/// assert_eq!((page.width(), page.height()), (200.0, 200.0));
/// # Ok::<(), pdfrum::Error>(())
/// ```
#[derive(Debug, Clone)]
pub struct Page<'a> {
    pub(crate) doc: &'a Document,
    pub(crate) dict: PageDict,
    pub(crate) index: u32,
    media_box: kurbo::Rect,
    crop_box: kurbo::Rect,
    rotation: Rotation,
}

impl<'a> Page<'a> {
    pub(crate) fn load(doc: &'a Document, index: u32) -> Result<Page<'a>> {
        let dict = doc.inner.page(index)?;
        let mut diags = Diagnostics::default();
        let (media_box, crop_box) = pdfrum_page::derive_boxes(
            &dict.dict,
            |key| dict.inherited(key, &doc.inner),
            &doc.inner,
            &mut diags,
        );
        doc.note(&diags);
        let rotation = Rotation::from(pdfrum_page::Rotation::from_degrees(
            dict.inherited(&Name::from("Rotate"), &doc.inner)
                .as_ref()
                .and_then(pdfrum_object::Object::as_int)
                .unwrap_or(0),
        ));
        Ok(Page {
            doc,
            dict,
            index,
            media_box,
            crop_box,
            rotation,
        })
    }

    /// The page's zero-based index in the document.
    #[must_use]
    pub fn index(&self) -> u32 {
        self.index
    }

    /// The page's displayed width in points, after rotation.
    ///
    /// A quarter-turned page is as wide as its crop box is tall, which is
    /// what a viewer shows and what a render is sized from.
    #[must_use]
    pub fn width(&self) -> f64 {
        self.display_size().0
    }

    /// The page's displayed height in points, after rotation.
    #[must_use]
    pub fn height(&self) -> f64 {
        self.display_size().1
    }

    fn display_size(&self) -> (f64, f64) {
        let (w, h) = (self.crop_box.width(), self.crop_box.height());
        match self.rotation {
            Rotation::Quarter | Rotation::ThreeQuarter => (h, w),
            Rotation::None | Rotation::Half => (w, h),
        }
    }

    /// The page's `/MediaBox` — the sheet it was laid out on
    /// (ISO 32000-1 §14.11.2).
    #[must_use]
    pub fn media_box(&self) -> kurbo::Rect {
        self.media_box
    }

    /// The page's `/CropBox`, already intersected with the media box — the
    /// region a viewer displays.
    #[must_use]
    pub fn crop_box(&self) -> kurbo::Rect {
        self.crop_box
    }

    /// The page's `/Rotate`, normalized to a quarter turn.
    #[must_use]
    pub fn rotation(&self) -> Rotation {
        self.rotation
    }

    /// Renders the page to a pixel buffer.
    ///
    /// The image is sized by [`RenderOptions::transform`]: the default
    /// identity transform gives one pixel per PDF point, and
    /// `Affine::scale(2.0)` gives a 2x image. The page's own rotation and
    /// crop are applied for you.
    ///
    /// # Errors
    ///
    /// [`Error::Render`](crate::Error::Render) when the resulting target would be empty or larger
    /// than the rasterizer's 65535-pixel limit. Content that will not draw is
    /// *not* an error: it is recorded as a diagnostic and the rest of the
    /// page still renders.
    ///
    /// ```
    /// use pdfrum::{Document, RenderOptions};
    /// use pdfrum::kurbo::Affine;
    ///
    /// let doc = Document::open("tests/fixtures/hello_world.pdf")?;
    /// let page = doc.page(0)?;
    ///
    /// let pixmap = page.render(&RenderOptions::default())?;
    /// assert_eq!((pixmap.width(), pixmap.height()), (200, 200));
    ///
    /// // Twice the size, same page.
    /// let big = page.render(&RenderOptions {
    ///     transform: Affine::scale(2.0),
    ///     ..RenderOptions::default()
    /// })?;
    /// assert_eq!((big.width(), big.height()), (400, 400));
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    pub fn render(&self, options: &RenderOptions) -> Result<Pixmap> {
        self.render_on(&VelloCpuBackend::new(), options)
    }

    /// Renders the page on a rasterizer you name.
    ///
    /// [`Page::render`] is this with [`VelloCpuBackend`] — the facade's
    /// default, and the one rasterizer `cargo add pdfrum` pulls in. Any other
    /// backend is a direct dependency of yours, named here:
    /// `pdfrum-raster-tinyskia`'s `TinySkiaBackend` for a determinism
    /// cross-check, `pdfrum-raster-agg`'s `AggBackend` when you want edges
    /// that match PDFium's, `pdfrum-raster-vello`'s `VelloBackend` for the
    /// GPU. Nothing about the page changes: a backend rasterizes paths and
    /// images, it does not interpret PDF.
    ///
    /// Added 2026-09-02, replacing `RenderOptions::backend` and the facade's
    /// `Backend` enum. Const generics were considered and rejected for the
    /// job: a `const` parameter cannot carry a `wgpu` device, so a
    /// const-selected backend could never name the GPU one.
    ///
    /// # Errors
    ///
    /// As [`Page::render`].
    ///
    /// ```
    /// use pdfrum::{Document, RenderOptions, VelloCpuBackend};
    ///
    /// let doc = Document::open("tests/fixtures/hello_world.pdf")?;
    /// let page = doc.page(0)?;
    ///
    /// let pixmap = page.render_on(&VelloCpuBackend::new(), &RenderOptions::default())?;
    /// assert_eq!((pixmap.width(), pixmap.height()), (200, 200));
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    pub fn render_on<B: RasterBackend>(
        &self,
        backend: &B,
        options: &RenderOptions,
    ) -> Result<Pixmap> {
        let mut ctx = BuildContext::new();
        self.render_with_on(backend, options, &mut ctx)
    }

    /// Renders the page reusing a caller-owned [`BuildContext`].
    ///
    /// The context carries the font, colorspace, function and image caches. A
    /// caller rendering many pages of one document should thread one through
    /// them all — decoding a shared image or parsing a shared font once per
    /// document instead of once per page is the difference between a fast
    /// walk and a slow one.
    ///
    /// It is `&mut` and not shareable, so under rayon each worker keeps its
    /// own; see the crate docs.
    ///
    /// # Errors
    ///
    /// As [`Page::render`].
    pub fn render_with(&self, options: &RenderOptions, ctx: &mut BuildContext) -> Result<Pixmap> {
        self.render_with_on(&VelloCpuBackend::new(), options, ctx)
    }

    /// [`Page::render_with`] on a rasterizer you name, as
    /// [`Page::render_on`].
    ///
    /// # Errors
    ///
    /// As [`Page::render`].
    pub fn render_with_on<B: RasterBackend>(
        &self,
        backend: &B,
        options: &RenderOptions,
        ctx: &mut BuildContext,
    ) -> Result<Pixmap> {
        self.paint(backend, options, ctx, &mut RenderCaches::new())
    }

    /// Renders the page reusing a caller-owned [`RenderSession`] — both the
    /// build caches and the glyph cache.
    ///
    /// [`Page::render_with`] threads the resources a page is *built* from,
    /// which is most of the win but not all of it: the rasterizer still
    /// flattens every glyph outline afresh for every page. A session carries
    /// that cache too, so a run over many pages of one document flattens each
    /// glyph once.
    ///
    /// It is `&mut` and not shareable, so under rayon each worker keeps its
    /// own; see the crate docs. On the caveat about type-3 snapping and a
    /// warm cache, see [`RenderSession`].
    ///
    /// ```
    /// use pdfrum::{Document, RenderOptions, RenderSession};
    ///
    /// let doc = Document::open("tests/fixtures/bookmarks.pdf")?;
    /// let mut session = RenderSession::new();
    /// let rendered: Vec<_> = doc
    ///     .pages()
    ///     .map(|page| page.render_session(&RenderOptions::default(), &mut session))
    ///     .collect::<Result<_, _>>()?;
    /// assert_eq!(rendered.len(), 2);
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    ///
    /// # Errors
    ///
    /// As [`Page::render`].
    pub fn render_session(
        &self,
        options: &RenderOptions,
        session: &mut RenderSession,
    ) -> Result<Pixmap> {
        self.render_session_on(&VelloCpuBackend::new(), options, session)
    }

    /// [`Page::render_session`] on a rasterizer you name, as
    /// [`Page::render_on`].
    ///
    /// The session's glyph cache holds *flattened outlines*, which are the
    /// engine's own and not a backend's, so one session serves a run that
    /// changes backend between pages — the cross-check case.
    ///
    /// # Errors
    ///
    /// As [`Page::render`].
    pub fn render_session_on<B: RasterBackend>(
        &self,
        backend: &B,
        options: &RenderOptions,
        session: &mut RenderSession,
    ) -> Result<Pixmap> {
        self.paint(backend, options, &mut session.build, &mut session.caches)
    }

    /// The device box this page's images should be decoded against
    /// (SPEC.md §7, `[spec]` 2026-08-31).
    ///
    /// The oracle's `max_size_required` is the **render device's own
    /// dimensions** — `CPDF_ImageRenderer::StartLoadDIBBase` fills it from
    /// `GetRenderDevice()->GetWidth()/GetHeight()`
    /// (`cpdf_imagerenderer.cpp:74-77`) — not the rectangle an individual image
    /// lands in. So this is a property of the render target, one value for the
    /// whole page, and it can be computed before a single object is
    /// interpreted: the display size is fixed by the crop box and `/Rotate`
    /// alone.
    ///
    /// The truncation, and the reason the box is the *page's* rather than any
    /// image's, are both in [`pdfrum_page::RequestedSize::for_device`].
    fn decode_target(&self, options: &RenderOptions) -> pdfrum_page::RequestedSize {
        let (pw, ph) = self.display_size();
        let corners = options
            .transform
            .transform_rect_bbox(kurbo::Rect::new(0.0, 0.0, pw, ph));
        pdfrum_page::RequestedSize::for_device(corners.width(), corners.height())
    }

    /// The one render body, parameterised by which rasterizer draws and which
    /// caches it borrows.
    fn paint<B: RasterBackend>(
        &self,
        backend: &B,
        options: &RenderOptions,
        ctx: &mut BuildContext,
        caches: &mut RenderCaches,
    ) -> Result<Pixmap> {
        // Set before the build, because the build is what decodes the images.
        // Restored afterwards so a caller threading one context through a
        // render and then a `Page::objects` call does not silently inherit this
        // page's device box.
        let previous = std::mem::replace(&mut ctx.decode_target, self.decode_target(options));
        let mut page = self.build(ctx);
        if options.annotations {
            // An annotation's appearance form is drawn *into* the page graph
            // rather than composited over the finished image, because that is
            // where it belongs: an appearance is page content that happens to
            // be stored beside the page. Only the render path wants it —
            // extraction reads the content stream's own text.
            let mut diags = Diagnostics::default();
            pdfrum_doc::annot_render::overlay(
                &mut page,
                &self.dict.dict,
                &self.doc.catalog(),
                &self.doc.inner,
                ctx,
                &self.doc.limits,
                &mut diags,
            );
            self.doc.note(&diags);
        }
        ctx.decode_target = previous;
        let page = page;
        let inner = options.to_inner();
        let mut diags = Diagnostics::default();
        let pixmap =
            pdfrum_render::render_page_with_caches(&page, &inner, backend, caches, &mut diags);
        // Recorded whether or not the render succeeded: a page too large to
        // rasterize may still have reported damage on the way there.
        self.doc.note(&diags);
        Ok(pixmap?)
    }

    /// Extracts the page's text.
    ///
    /// The returned [`TextPage`] carries the characters in reading order and
    /// answers search, selection and link queries over them.
    ///
    /// ```
    /// let doc = pdfrum::Document::open("tests/fixtures/hello_world.pdf")?;
    /// let text = doc.page(0)?.text();
    /// assert!(text.all_text().contains("Hello, world!"));
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    #[must_use]
    pub fn text(&self) -> TextPage {
        let mut ctx = BuildContext::new();
        self.text_with(&mut ctx)
    }

    /// Extracts the page's text reusing a caller-owned [`BuildContext`], as
    /// [`Page::render_with`].
    #[must_use]
    pub fn text_with(&self, ctx: &mut BuildContext) -> TextPage {
        let page = self.build(ctx);
        let mut diags = Diagnostics::default();
        let options = pdfrum_text::ExtractOptions {
            rtl: self.doc.reads_right_to_left(),
        };
        let text = pdfrum_text::extract(
            &page,
            &self.doc.inner,
            &options,
            &self.doc.limits,
            &mut diags,
        );
        self.doc.note(&diags);
        text
    }

    /// Extracts the page's text reusing a [`RenderSession`]'s build caches,
    /// so one session serves a run that both renders and extracts.
    ///
    /// Extraction never touches the glyph cache — it reads the content
    /// stream's own text and never rasterizes — so this is exactly
    /// [`Page::text_with`] over `session.build`, offered so a caller holding a
    /// session need not reach into it.
    #[must_use]
    pub fn text_session(&self, session: &mut RenderSession) -> TextPage {
        self.text_with(&mut session.build)
    }

    /// The page's annotations, in `/Annots` order, with pop-ups excluded.
    ///
    /// ```
    /// let doc = pdfrum::Document::open("tests/fixtures/text_form.pdf")?;
    /// let annots = doc.page(0)?.annotations();
    /// assert_eq!(annots.len(), 1);
    /// assert_eq!(annots[0].subtype(), pdfrum::Subtype::Widget);
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    #[must_use]
    pub fn annotations(&self) -> Vec<Annotation<'a>> {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "the annotation list places synthesized pop-ups relative to the \
                      page width, which upstream measures in f32; a page wider than \
                      f32 can express is not one this changes the behaviour of"
        )]
        let width = self.crop_box.width() as f32;
        pdfrum_doc::annot::AnnotList::load(&self.dict.dict, width, &self.doc.inner)
            .annots
            .into_iter()
            .map(|inner| Annotation {
                inner,
                doc: self.doc,
            })
            .collect()
    }

    /// The page's link annotations, with the destination or action each one
    /// carries (ISO 32000-1 §12.5.6.5).
    #[must_use]
    pub fn links(&self) -> Vec<pdfrum_doc::Link> {
        pdfrum_doc::nav::link::page_links(&self.dict.dict, &self.doc.inner)
            .into_iter()
            .flatten()
            .collect()
    }

    /// The interpreted page-object graph, for a caller who wants the drawing
    /// operations themselves rather than pixels or text.
    ///
    /// The escape hatch onto `pdfrum-page`, matching
    /// [`Document::parser`](crate::Document::parser).
    #[must_use]
    pub fn objects(&self) -> pdfrum_page::Page {
        self.build(&mut BuildContext::new())
    }

    /// Interpret the content stream into a page-object graph.
    ///
    /// The one place this crate assembles anything, and it is assembly rather
    /// than logic: `/Contents` is a stream or an array of streams, and the
    /// array's members are joined with one space each — including after the
    /// last, which is what terminates a stream ending mid-token.
    fn build(&self, ctx: &mut BuildContext) -> pdfrum_page::Page {
        let mut diags = Diagnostics::default();
        let bytes = self.content_bytes(&mut diags);
        let ops = pdfrum_page::parse_content(&bytes, &self.doc.limits, &mut diags);
        let resources = pdfrum_page::Resources::for_page(
            self.dict
                .inherited(&Name::from("Resources"), &self.doc.inner)
                .and_then(|object| object.resolve(&self.doc.inner).ok()?.as_dict().cloned()),
        );
        let page = pdfrum_page::build_page_from_dict(
            &ops,
            &self.dict.dict,
            |key| self.dict.inherited(key, &self.doc.inner),
            &resources,
            &self.doc.inner,
            ctx,
            &self.doc.limits,
            &mut diags,
        );
        // Where most of a document's lazy damage surfaces: a `/Length` the
        // filter chain had to work around, a font that had to be substituted.
        self.doc.note(&diags);
        page
    }

    fn content_bytes(&self, diags: &mut Diagnostics) -> Vec<u8> {
        self.content_segments(diags).0
    }

    /// The content bytes, and where each `/Contents` element's bytes end.
    ///
    /// The joined buffer is what the interpreter reads — an element ending
    /// mid-token is continued by the next, which is why they are joined rather
    /// than parsed apart — and the ends are what lets the editor say which
    /// element each object came from.
    fn content_segments(&self, diags: &mut Diagnostics) -> (Vec<u8>, Vec<usize>) {
        let r = &self.doc.inner;
        let Some(contents) = self.dict.dict.get(&Name::from("Contents"), r) else {
            return (Vec::new(), Vec::new());
        };
        let mut out = Vec::new();
        let mut ends = Vec::new();
        let mut push = |object: &pdfrum_object::Object,
                        out: &mut Vec<u8>,
                        ends: &mut Vec<usize>| {
            if let Some(stream) = object.as_stream() {
                let decoded = pdfrum_filters::decode_chain(stream, 0, r, &self.doc.limits, diags);
                out.extend_from_slice(&decoded.data);
                // The separating space belongs to the element before it: it is
                // what terminates a stream ending mid-token.
                out.push(b' ');
            }
            ends.push(out.len());
        };
        let Some(direct) = contents.as_direct() else {
            return (out, ends);
        };
        match direct {
            pdfrum_object::Object::Stream(_) => push(direct, &mut out, &mut ends),
            pdfrum_object::Object::Array(array) => {
                for element in array.iter() {
                    if let Ok(resolved) = element.resolve(r) {
                        push(resolved.get(), &mut out, &mut ends);
                    } else {
                        // A dangling element still occupies an index, so the
                        // ones after it keep their numbers.
                        ends.push(out.len());
                    }
                }
            }
            _ => {}
        }
        (out, ends)
    }

    /// Interpret the content stream, recording which `/Contents` element each
    /// object came from.
    ///
    /// The editor's build: it costs one extra pass over the operators to find
    /// the element boundaries, which the render and text paths have no use
    /// for.
    pub(crate) fn build_for_edit(&self, ctx: &mut BuildContext) -> pdfrum_page::Page {
        let mut diags = Diagnostics::default();
        let (bytes, ends) = self.content_segments(&mut diags);
        let ops = pdfrum_page::parse_content(&bytes, &self.doc.limits, &mut diags);
        let bounds =
            pdfrum_page::StreamBounds::from_joined(&bytes, ops.len(), &ends, &self.doc.limits);
        let resources = pdfrum_page::Resources::for_page(
            self.dict
                .inherited(&Name::from("Resources"), &self.doc.inner)
                .and_then(|object| object.resolve(&self.doc.inner).ok()?.as_dict().cloned()),
        );
        let page = pdfrum_page::build_page_streams(
            &ops,
            &bounds,
            &self.dict.dict,
            |key| self.dict.inherited(key, &self.doc.inner),
            &resources,
            &self.doc.inner,
            ctx,
            &self.doc.limits,
            &mut diags,
        );
        self.doc.note(&diags);
        page
    }
}

/// A page's `/Rotate`, normalized to one of four quarter turns
/// (ISO 32000-1 §7.7.3.3).
///
/// The value is always clockwise and always a multiple of 90 degrees: a
/// document writing `/Rotate 450` means [`Rotation::Quarter`], and one
/// writing `-90` means [`Rotation::ThreeQuarter`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Rotation {
    /// Not rotated.
    #[default]
    None,
    /// 90 degrees clockwise.
    Quarter,
    /// 180 degrees.
    Half,
    /// 270 degrees clockwise.
    ThreeQuarter,
}

impl Rotation {
    /// The rotation in degrees clockwise: 0, 90, 180 or 270.
    ///
    /// ```
    /// assert_eq!(pdfrum::Rotation::ThreeQuarter.degrees(), 270);
    /// ```
    #[must_use]
    pub fn degrees(self) -> u32 {
        match self {
            Rotation::None => 0,
            Rotation::Quarter => 90,
            Rotation::Half => 180,
            Rotation::ThreeQuarter => 270,
        }
    }
}

impl From<pdfrum_page::Rotation> for Rotation {
    fn from(inner: pdfrum_page::Rotation) -> Rotation {
        match inner {
            pdfrum_page::Rotation::None => Rotation::None,
            pdfrum_page::Rotation::Quarter => Rotation::Quarter,
            pdfrum_page::Rotation::Half => Rotation::Half,
            pdfrum_page::Rotation::ThreeQuarter => Rotation::ThreeQuarter,
        }
    }
}

impl Document {
    /// Whether the catalog asks for right-to-left reading order
    /// (`/Root /ViewerPreferences /Direction /R2L`).
    ///
    /// Text extraction's whole line ordering turns on this.
    pub(crate) fn reads_right_to_left(&self) -> bool {
        let Some(prefs) = self
            .catalog()
            .dict(&Name::from("ViewerPreferences"), &self.inner)
        else {
            return false;
        };
        prefs
            .name(&Name::from("Direction"))
            .map(pdfrum_object::Name::as_bytes)
            == Some(b"R2L".as_slice())
    }
}

/// The identity transform, spelled out for a reader who has not met `kurbo`.
#[allow(dead_code)]
const _: Affine = Affine::IDENTITY;
