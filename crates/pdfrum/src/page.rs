//! One page: its geometry, its pixels, its text and its annotations.

use pdfrum_common::{Diagnostics, PageIndex};
use pdfrum_object::{Name, Resolve, names};
use pdfrum_page::BuildContext;
use pdfrum_parser::PageDict;

use crate::{
    Annotation, Document, Pixmap, RasterBackend, RenderOptions, RenderSession, Result, TextPage,
    Word,
};

/// One page of a [`Document`].
///
/// Cheap to obtain and cheap to hold: constructing a `Page` reads its
/// dictionary and its inherited attributes, which is a handful of dictionary
/// lookups. The expensive work — interpreting the content stream into a
/// drawable object graph — happens in [`Page::render`] and [`Page::text`],
/// each of which does it once for the call, and in [`Page::prepare`], which
/// does it once for any number of draws.
#[derive(Debug, Clone)]
pub struct Page<'a> {
    pub(crate) doc: &'a Document,
    pub(crate) dict: PageDict,
    pub(crate) index: PageIndex,
    media_box: kurbo::Rect,
    crop_box: kurbo::Rect,
    rotation: Rotation,
}

impl<'a> Page<'a> {
    pub(crate) fn load(doc: &'a Document, index: PageIndex) -> Result<Page<'a>> {
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
    pub fn index(&self) -> PageIndex {
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

    /// The page's `/BleedBox` (ISO 32000-1 §14.11.2), only when the page sets
    /// one: unlike the media and crop boxes it is neither inherited nor
    /// defaulted.
    #[must_use]
    pub fn bleed_box(&self) -> Option<kurbo::Rect> {
        self.named_box(names::BLEED_BOX)
    }

    /// The page's `/TrimBox`, only when the page sets one.
    #[must_use]
    pub fn trim_box(&self) -> Option<kurbo::Rect> {
        self.named_box(names::TRIM_BOX)
    }

    /// The page's `/ArtBox`, only when the page sets one.
    #[must_use]
    pub fn art_box(&self) -> Option<kurbo::Rect> {
        self.named_box(names::ART_BOX)
    }

    fn named_box(&self, key: &Name) -> Option<kurbo::Rect> {
        self.dict
            .dict
            .array(key, &self.doc.inner)
            .map(|array| array.as_rect())
    }

    /// The page's `/Rotate`, normalized to a quarter turn.
    #[must_use]
    pub fn rotation(&self) -> Rotation {
        self.rotation
    }

    /// Renders the page to a pixel buffer on the rasterizer you name, with
    /// caches of its own that it throws away afterwards.
    ///
    /// The backend is an argument, never a default: `VelloCpuBackend` is
    /// the one the `vello-cpu` feature (on by default) provides, and the
    /// `tinyskia`, `agg` and `vello-gpu` features provide the others. For a
    /// run over many pages, use [`Page::render_on`] with one
    /// [`RenderSession`].
    ///
    /// The image is sized by [`RenderOptions::transform`]: the default
    /// identity transform gives one pixel per PDF point, and
    /// `Affine::scale(2.0)` gives a 2x image. The page's own crop box and
    /// `/Rotate` are applied for you.
    ///
    /// # Errors
    ///
    /// [`Error::Render`](crate::Error::Render) when the resulting target would
    /// be empty or larger than the rasterizer's 65535-pixel limit. Content
    /// that will not draw is *not* an error: it is recorded as a diagnostic
    /// and the rest of the page still renders.
    ///
    /// ```
    /// use pdfrum::{Affine, Document, RenderOptions, VelloCpuBackend};
    ///
    /// let doc = Document::open("tests/fixtures/hello_world.pdf")?;
    /// let page = doc.page(0)?;
    /// let backend = VelloCpuBackend::new();
    ///
    /// let pixmap = page.render(&backend, &RenderOptions::default())?;
    /// assert_eq!((pixmap.width(), pixmap.height()), (200, 200));
    ///
    /// // Twice the size, same page.
    /// let big = page.render(&backend, &RenderOptions {
    ///     transform: Affine::scale(2.0),
    ///     ..RenderOptions::default()
    /// })?;
    /// assert_eq!((big.width(), big.height()), (400, 400));
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    pub fn render<B: RasterBackend>(&self, backend: &B, options: &RenderOptions) -> Result<Pixmap> {
        self.render_on(backend, options, &mut RenderSession::default())
    }

    /// [`Page::render`] reusing a caller-owned [`RenderSession`].
    ///
    /// A backend rasterizes paths and images, it does not interpret PDF. The
    /// session carries the caches a run over many pages of one document
    /// should thread through all of them — see [`RenderSession`].
    ///
    /// # Errors
    ///
    /// As [`Page::render`].
    pub fn render_on<B: RasterBackend>(
        &self,
        backend: &B,
        options: &RenderOptions,
        session: &mut RenderSession,
    ) -> Result<Pixmap> {
        self.prepare(options, session).render_on(backend, session)
    }

    /// Interprets the page once, for drawing any number of times.
    ///
    /// The expensive half of [`Page::render_on`], separated from the drawing
    /// so a repaint does not repeat it. The session's build caches are used
    /// and warmed exactly as `render_on` uses them. See [`PreparedPage`] for
    /// what the value holds and when to prepare again.
    #[must_use]
    pub fn prepare(
        &self,
        options: &RenderOptions,
        session: &mut RenderSession,
    ) -> PreparedPage<'a> {
        let ctx = &mut session.build;
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
        PreparedPage {
            doc: self.doc,
            graph: page,
            options: options.clone(),
        }
    }

    /// The device box this page's images should be decoded against.
    ///
    /// The bound is the **render device's own dimensions**, not the rectangle
    /// an individual image lands in — one value for the whole page, and one
    /// that can be computed before a single object is interpreted, since the
    /// display size is fixed by the crop box and `/Rotate` alone.
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

    /// Extracts the page's text.
    ///
    /// The [`TextPage`] carries the characters in reading order and answers
    /// search, selection and link queries over them. Built with caches of its
    /// own that it throws away afterwards; for a run over many pages, or when
    /// the text must be measured with the same fonts a render uses, use
    /// [`Page::text_on`].
    ///
    /// ```
    /// let doc = pdfrum::Document::open("tests/fixtures/hello_world.pdf")?;
    /// let text = doc.page(0)?.text();
    /// assert!(text.to_string().contains("Hello, world!"));
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    #[must_use]
    pub fn text(&self) -> TextPage {
        self.text_on(&mut RenderSession::default())
    }

    /// The page's words in reading order, each with its box, its font and its
    /// size — the shape an extraction or citation pipeline wants.
    ///
    /// A view over [`Page::text`]: the text page is extracted once and split
    /// on whitespace, so a word's [`range`](Word::range) indexes that page's
    /// characters and its [`rect`](Word::rect) is in the same page space
    /// [`TextPage::rects`] and [`PageLink::rect`] use. See
    /// [`TextPage::words`] for the rules.
    ///
    /// ```
    /// let doc = pdfrum::Document::open("tests/fixtures/hello_world.pdf")?;
    /// let words = doc.page(0)?.words();
    /// let texts: Vec<&str> = words.iter().map(|w| w.text.as_str()).collect();
    /// assert_eq!(texts, ["Hello,", "world!", "Goodbye,", "world!"]);
    /// assert!(words[0].rect.x1 <= words[1].rect.x0);
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    #[must_use]
    pub fn words(&self) -> Vec<Word> {
        self.text().words()
    }

    /// [`Page::text`] reusing a caller-owned [`RenderSession`], so a run over
    /// many pages parses each font once.
    ///
    /// One session serves a run that both renders and extracts, which is what
    /// a caller whose [`BuildContext`] carries substitution options needs:
    /// text measured against a different face than the page is drawn with is
    /// text measured twice. Extraction never rasterizes, so only
    /// `session.build` moves.
    #[must_use]
    pub fn text_on(&self, session: &mut RenderSession) -> TextPage {
        // Extraction never reads an image, so the build decodes none: on the
        // corpus's image documents the decode was 87% of a text run.
        let previous = std::mem::replace(
            &mut session.build.decode_target,
            pdfrum_page::RequestedSize::NoSamples,
        );
        let page = self.build(&mut session.build);
        session.build.decode_target = previous;
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

    /// The page's annotations, in `/Annots` order, with pop-ups excluded.
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
        pdfrum_doc::nav::page_links(&self.dict.dict, &self.doc.inner)
            .into_iter()
            .flatten()
            .collect()
    }

    /// The page's links with where each one leads — a page of this document,
    /// a URI, or something else — in the order of [`Page::links`].
    #[must_use]
    pub fn page_links(&self) -> Vec<PageLink> {
        let mut diags = Diagnostics::default();
        let catalog = self.doc.catalog();
        let resolver = &self.doc.inner;
        let links = self
            .links()
            .into_iter()
            .map(|link| {
                let rect = link.rect(resolver);
                let uri = link
                    .action(resolver)
                    .filter(|action| action.kind() == pdfrum_doc::ActionKind::Uri)
                    .map(|action| action.uri(&catalog, resolver));
                let target = match uri {
                    Some(uri) => LinkTarget::Uri(String::from_utf8_lossy(&uri).into_owned()),
                    None => link
                        .dest(&catalog, resolver, &self.doc.limits, &mut diags)
                        .page_index(resolver, |num| self.doc.page_index_of(num), &mut diags)
                        .map_or(LinkTarget::Other, LinkTarget::Page),
                };
                PageLink { rect, target }
            })
            .collect();
        self.doc.note(&diags);
        links
    }

    /// The page as GitHub-flavoured Markdown: [`Page::markdown_blocks`]
    /// rendered, every image `![alt](image)`.
    ///
    /// A tagged document's structure tree names the headings, paragraphs,
    /// lists, tables and figures and is read as it is; an untagged one is
    /// read by its typography — sizes, weights, bullets, gaps, margins. See
    /// `pdfrum-markdown` for the rules. One page cannot see a running header
    /// repeated on the next; [`Document::markdown`] can.
    #[cfg(feature = "markdown")]
    #[must_use]
    pub fn markdown(&self) -> String {
        pdfrum_markdown::render(&self.markdown_blocks())
    }

    /// The page's content as Markdown blocks, in reading order — what
    /// [`Page::markdown`] renders, for a caller who wants to walk them or to
    /// link each [`Block::Image`] to a file through
    /// [`markdown::render_with_images`](crate::markdown::render_with_images).
    /// An image block's index is into [`Page::images`].
    #[cfg(feature = "markdown")]
    #[must_use]
    pub fn markdown_blocks(&self) -> Vec<crate::Block> {
        let (graph, tree, options, diags) = self.markdown_inputs(WITH_IMAGE_PLACES);
        let blocks = pdfrum_markdown::page_blocks(
            &graph,
            tree.as_ref(),
            &self.doc.inner,
            options,
            &self.doc.limits,
        );
        self.doc.note(&diags);
        blocks
    }

    /// The page's text with its layout kept: columns stay columns, gaps
    /// stay gaps, one character cell per half-em.
    #[cfg(feature = "markdown")]
    #[must_use]
    pub fn layout_text(&self) -> String {
        let (graph, _, options, diags) =
            self.markdown_inputs(pdfrum_page::RequestedSize::NoSamples);
        let text = pdfrum_markdown::page_layout(&graph, &self.doc.inner, options, &self.doc.limits);
        self.doc.note(&diags);
        text
    }

    /// The graph with its images decoded to `images`, the structure tree
    /// when the document is tagged, and the reading options
    /// `pdfrum-markdown` takes.
    #[cfg(feature = "markdown")]
    fn markdown_inputs(
        &self,
        images: pdfrum_page::RequestedSize,
    ) -> (
        pdfrum_page::Page,
        Option<pdfrum_doc::structure::StructTree>,
        pdfrum_markdown::Options,
        Diagnostics,
    ) {
        let mut ctx = BuildContext::new();
        ctx.decode_target = images;
        let graph = self.build(&mut ctx);
        let mut diags = Diagnostics::default();
        let catalog = self.doc.catalog();
        let tree = pdfrum_doc::structure::StructTree::load_page(
            &catalog,
            &self.dict.dict,
            self.dict.reference.map_or(0, |r| r.num),
            &self.doc.inner,
            &self.doc.limits,
            &mut diags,
        );
        let options = pdfrum_markdown::Options {
            rtl: self.doc.reads_right_to_left(),
        };
        (graph, tree, options, diags)
    }

    /// The images the page draws, in drawing order, forms included: each
    /// with its decoded pixels, and the raw stream when the file stores it
    /// in a format worth keeping as it is (JPEG, JPEG 2000, JBIG2, CCITT).
    #[must_use]
    pub fn images(&self) -> Vec<PageImage> {
        let mut ctx = BuildContext::new();
        let graph = self.build(&mut ctx);
        let mut out = Vec::new();
        collect_images(&graph.objects, &self.doc.inner, &mut out);
        out
    }

    /// The page's view of the document's structure tree, or `None` when the
    /// document is not tagged.
    #[must_use]
    pub fn structure(&self) -> Option<pdfrum_doc::structure::StructTree> {
        let mut diags = Diagnostics::default();
        let catalog = self.doc.catalog();
        let tree = pdfrum_doc::structure::StructTree::load_page(
            &catalog,
            &self.dict.dict,
            self.dict.reference.map_or(0, |r| r.num),
            &self.doc.inner,
            &self.doc.limits,
            &mut diags,
        );
        self.doc.note(&diags);
        tree
    }

    /// **Escape hatch — requires `pdfrum-page`.** The interpreted page-object
    /// graph, for a caller who wants the drawing operations themselves rather
    /// than pixels or text.
    ///
    /// Matches [`Document::parser`](crate::Document::parser), and like it the
    /// return type stays namespaced so the collision with this crate's own
    /// [`Page`] is visible at the call site.
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
        use crate::profile::{Stage, stage};

        let mut diags = Diagnostics::default();
        // The decode and the operator scan, split from the fold that reads
        // them: a document whose `/Contents` is a megabyte of Flate and a
        // document with three hundred `Do`s cost their time in different
        // halves, and one bucket cannot tell them apart.
        let ops = stage(Stage::ContentParse, || {
            let bytes = self.content_bytes(&mut diags);
            pdfrum_page::parse_content(&bytes, &self.doc.limits, &mut diags)
        });
        let resources = pdfrum_page::Resources::for_page(
            self.dict
                .inherited(&Name::from("Resources"), &self.doc.inner)
                .and_then(|object| object.resolve(&self.doc.inner).ok()?.as_dict().cloned()),
        );
        let page = stage(Stage::Interpretation, || {
            pdfrum_page::build_page_from_dict(
                &ops,
                &self.dict.dict,
                |key| self.dict.inherited(key, &self.doc.inner),
                &resources,
                &self.doc.inner,
                ctx,
                &self.doc.limits,
                &mut diags,
            )
        });
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
        content_segments(self.doc, &self.dict, &self.doc.inner, diags)
    }

    #[cfg(feature = "edit")]
    /// Interpret the content stream, recording which `/Contents` element each
    /// object came from.
    ///
    /// The editor's build: it costs one extra pass over the operators to find
    /// the element boundaries, which the render and text paths have no use
    /// for.
    pub(crate) fn build_for_edit(&self, ctx: &mut BuildContext) -> pdfrum_page::Page {
        build_graph(self.doc, &self.dict, &self.doc.inner, ctx)
    }
}

/// The content of `dict`'s page, decoded and joined, with the end offset of
/// each `/Contents` element — read through `r`.
fn content_segments(
    doc: &Document,
    dict: &PageDict,
    r: &impl Resolve,
    diags: &mut Diagnostics,
) -> (Vec<u8>, Vec<usize>) {
    let Some(contents) = dict.dict.get(&Name::from("Contents"), r) else {
        return (Vec::new(), Vec::new());
    };
    let mut out = Vec::new();
    let mut ends = Vec::new();
    let mut push = |object: &pdfrum_object::Object, out: &mut Vec<u8>, ends: &mut Vec<usize>| {
        if let Some(stream) = object.as_stream() {
            let decoded = pdfrum_filters::decode_chain(stream, 0, r, &doc.limits, diags);
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

/// The object graph of `dict`'s page for editing, read through `r`: the
/// base document for a page as it was opened, or an editing session's
/// overlay for the page as that session's edits leave it — so a stream an
/// earlier edit appended is a clean stream of the graph, and the names it
/// uses are kept.
#[cfg(feature = "edit")]
pub(crate) fn build_graph(
    doc: &Document,
    dict: &PageDict,
    r: &impl Resolve,
    ctx: &mut BuildContext,
) -> pdfrum_page::Page {
    let mut diags = Diagnostics::default();
    let (bytes, ends) = content_segments(doc, dict, r, &mut diags);
    let ops = pdfrum_page::parse_content(&bytes, &doc.limits, &mut diags);
    let bounds = pdfrum_page::StreamBounds::from_joined(&bytes, ops.len(), &ends, &doc.limits);
    let resources = pdfrum_page::Resources::for_page(
        dict.inherited(&Name::from("Resources"), r)
            .and_then(|object| object.resolve(r).ok()?.as_dict().cloned()),
    );
    let page = pdfrum_page::build_page_streams(
        &ops,
        &bounds,
        &dict.dict,
        |key| dict.inherited(key, r),
        &resources,
        r,
        ctx,
        &doc.limits,
        &mut diags,
    );
    doc.note(&diags);
    page
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

/// A page whose content has been interpreted, ready to be drawn any number
/// of times.
///
/// [`Page::render`] interprets the content stream on every call; a viewer
/// that repaints the same page on every scroll pays that again each time.
/// [`Page::prepare`] does the interpretation once and hands back the result
/// as a value you own: draw it as often as you like, and drop it when the
/// page leaves the screen. What it retains is visible in the type — the
/// interpreted object graph, including every image decoded for the device
/// size it was prepared at — and nothing is cached behind your back.
///
/// Prepared **for** one [`RenderOptions`]: the annotations flag decides what
/// is in the graph and the transform decides how much resolution its images
/// were decoded at, so a page is drawn with the options it was prepared
/// with. To draw at another size or without annotations, prepare again.
///
/// ```
/// use pdfrum::{Document, RenderOptions, RenderSession, VelloCpuBackend};
///
/// let doc = Document::open("tests/fixtures/hello_world.pdf")?;
/// let backend = VelloCpuBackend::new();
/// let mut session = RenderSession::new();
/// let prepared = doc.page(0)?.prepare(&RenderOptions::default(), &mut session);
///
/// // Interpreted once, drawn twice.
/// let first = prepared.render_on(&backend, &mut session)?;
/// let second = prepared.render_on(&backend, &mut session)?;
/// assert_eq!(first.data(), second.data());
/// # Ok::<(), pdfrum::Error>(())
/// ```
#[derive(Debug)]
pub struct PreparedPage<'a> {
    doc: &'a Document,
    graph: pdfrum_page::Page,
    options: RenderOptions,
}

impl PreparedPage<'_> {
    /// Draws the prepared page on the rasterizer you name, with glyph caches
    /// of its own that it throws away afterwards.
    ///
    /// # Errors
    ///
    /// As [`Page::render`].
    pub fn render<B: RasterBackend>(&self, backend: &B) -> Result<Pixmap> {
        self.render_on(backend, &mut RenderSession::default())
    }

    /// Draws the prepared page on a rasterizer you name, reusing a
    /// caller-owned [`RenderSession`]'s glyph caches.
    ///
    /// # Errors
    ///
    /// As [`Page::render`].
    pub fn render_on<B: RasterBackend>(
        &self,
        backend: &B,
        session: &mut RenderSession,
    ) -> Result<Pixmap> {
        let inner = self.options.to_inner();
        let mut diags = Diagnostics::default();
        let render_session = pdfrum_render::RenderSession {
            caches: Some(&mut session.caches),
            ..Default::default()
        };
        let pixmap = crate::profile::stage(crate::profile::Stage::Raster, || {
            pdfrum_render::render_page_with(
                &self.graph,
                &inner,
                backend,
                render_session,
                &mut diags,
            )
        });
        // Recorded whether or not the render succeeded: a page too large to
        // rasterize may still have reported damage on the way there.
        self.doc.note(&diags);
        Ok(pixmap?)
    }
}

/// How far a Markdown read decodes the page's images: to a pixel, enough
/// for the graph to carry each image's place and marked-content id — a
/// build that asks for no samples emits no image object at all — and no
/// more, since the text tiers never read a sample.
#[cfg(feature = "markdown")]
const WITH_IMAGE_PLACES: pdfrum_page::RequestedSize = pdfrum_page::RequestedSize::Reduced {
    width: 1,
    height: 1,
};

#[cfg(feature = "markdown")]
impl Document {
    /// The document as one Markdown string: [`Document::markdown_pages`]
    /// with a `---` rule between pages — what `pdfrum extract markdown`
    /// prints.
    #[must_use]
    pub fn markdown(&self) -> String {
        self.markdown_pages().join("\n---\n\n")
    }

    /// Each page as Markdown, read as one document: a line repeated in the
    /// top or bottom band of a majority of the pages, three at least, is a
    /// running header or footer and is dropped — `Page 3 of 11` and `Page 4
    /// of 11` are one line — which one page alone cannot tell
    /// ([`Page::markdown`] keeps such a line unless it looks like one). Each
    /// image is `![alt](image)`; [`Document::markdown_blocks`] is the way
    /// to link them. A page that will not load is an empty string, so the
    /// index is the page's.
    #[must_use]
    pub fn markdown_pages(&self) -> Vec<String> {
        let loadable: Vec<PageIndex> = (0..self.page_count())
            .map(PageIndex::from)
            .filter(|&index| self.page(index).is_ok())
            .collect();
        let mut out: Vec<String> = (0..self.page_count()).map(|_| String::new()).collect();
        if let Ok(pages) = self.markdown_blocks(loadable.iter().copied()) {
            for (index, blocks) in loadable.iter().zip(&pages) {
                let at = usize::try_from(u32::from(*index)).unwrap_or(usize::MAX);
                if let Some(slot) = out.get_mut(at) {
                    *slot = pdfrum_markdown::render(blocks);
                }
            }
        }
        out
    }

    /// The Markdown blocks of `pages`, read as one document the way
    /// [`Document::markdown_pages`] reads them, one list per page in the
    /// order given. An image block's index is into that page's
    /// [`Page::images`]; [`markdown::render_with_images`](crate::markdown::render_with_images)
    /// links them.
    ///
    /// # Errors
    ///
    /// A page that will not load, as [`Document::page`] reports it.
    pub fn markdown_blocks(
        &self,
        pages: impl IntoIterator<Item = PageIndex>,
    ) -> Result<Vec<Vec<crate::Block>>> {
        let pages: Vec<Page<'_>> = pages
            .into_iter()
            .map(|index| self.page(index))
            .collect::<Result<_>>()?;
        let inputs: Vec<_> = pages
            .iter()
            .map(|page| page.markdown_inputs(WITH_IMAGE_PLACES))
            .collect();
        let options = pdfrum_markdown::Options {
            rtl: self.reads_right_to_left(),
        };
        let page_inputs: Vec<pdfrum_markdown::PageInput<'_>> = inputs
            .iter()
            .map(|(graph, tree, _, _)| pdfrum_markdown::PageInput {
                page: graph,
                tree: tree.as_ref(),
            })
            .collect();
        let blocks =
            pdfrum_markdown::document_blocks(&page_inputs, &self.inner, options, &self.limits);
        for (_, _, _, diags) in &inputs {
            self.note(diags);
        }
        Ok(blocks)
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

/// Where a link annotation leads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkTarget {
    /// A page of this document.
    Page(PageIndex),
    /// A URI action (`/S /URI`).
    Uri(String),
    /// Anything else: a named action, a script, another file, or a
    /// destination that names no page of this document.
    Other,
}

/// A link annotation with its target resolved, from [`Page::page_links`].
#[derive(Debug, Clone, PartialEq)]
pub struct PageLink {
    /// The link's `/Rect` in page space.
    pub rect: kurbo::Rect,
    /// Where it leads.
    pub target: LinkTarget,
}

/// One image a page draws, from [`Page::images`].
#[derive(Debug, Clone)]
pub struct PageImage {
    /// The image `XObject`, or `None` for an inline image.
    pub source: Option<pdfrum_object::ObjRef>,
    /// Pixel width.
    pub width: u32,
    /// Pixel height.
    pub height: u32,
    /// A stencil mask rather than a picture.
    pub is_mask: bool,
    /// The stream as stored, when its encoding is one worth keeping.
    pub raw: Option<RawImage>,
    image: std::sync::Arc<pdfrum_page::ImageData>,
}

impl PageImage {
    /// The decoded pixels, ready to save as PNG.
    #[must_use]
    pub fn pixmap(&self) -> Pixmap {
        pdfrum_render::image_to_pixmap(&self.image)
    }
}

/// An image's stored bytes, from [`PageImage::raw`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawImage {
    /// The bytes as the file stores them, the image codec's own format.
    pub data: Vec<u8>,
    /// Which codec.
    pub encoding: ImageEncoding,
}

/// The codecs whose streams are files in their own right.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageEncoding {
    /// `/DCTDecode`.
    Jpeg,
    /// `/JPXDecode`.
    Jpeg2000,
    /// `/JBIG2Decode` — an embedded stream, not a standalone file.
    Jbig2,
    /// `/CCITTFaxDecode` — raw fax data, not a TIFF.
    CcittFax,
}

impl ImageEncoding {
    /// The file extension the bytes are usually given.
    #[must_use]
    pub fn extension(self) -> &'static str {
        match self {
            Self::Jpeg => "jpg",
            Self::Jpeg2000 => "jp2",
            Self::Jbig2 => "jb2",
            Self::CcittFax => "ccitt",
        }
    }

    fn of_filter(name: &[u8]) -> Option<Self> {
        match name {
            b"DCTDecode" | b"DCT" => Some(Self::Jpeg),
            b"JPXDecode" => Some(Self::Jpeg2000),
            b"JBIG2Decode" => Some(Self::Jbig2),
            b"CCITTFaxDecode" | b"CCF" => Some(Self::CcittFax),
            _ => None,
        }
    }
}

fn collect_images(
    objects: &[pdfrum_page::PageObject],
    resolver: &pdfrum_parser::Document,
    out: &mut Vec<PageImage>,
) {
    for object in objects {
        match object {
            pdfrum_page::PageObject::Image(content) => {
                let image = &content.object;
                let raw = image.source.and_then(|r| raw_image(r, resolver));
                out.push(PageImage {
                    source: image.source,
                    width: image.image.width,
                    height: image.image.height,
                    is_mask: image.is_mask,
                    raw,
                    image: std::sync::Arc::clone(&image.image),
                });
            }
            pdfrum_page::PageObject::Form(content) => {
                collect_images(&content.object.objects, resolver, out);
            }
            _ => {}
        }
    }
}

/// The stream's bytes when its only filter is an image codec.
fn raw_image(
    reference: pdfrum_object::ObjRef,
    resolver: &pdfrum_parser::Document,
) -> Option<RawImage> {
    let object = resolver.fetch(reference).ok()?;
    let stream = object.as_stream()?;
    let filters: Vec<Name> = match stream.dict.get(names::FILTER, resolver)?.get() {
        pdfrum_object::Object::Name(n) => vec![n.clone()],
        pdfrum_object::Object::Array(a) => a.iter().filter_map(|o| o.as_name().cloned()).collect(),
        _ => return None,
    };
    let [only] = filters.as_slice() else {
        return None;
    };
    let encoding = ImageEncoding::of_filter(only.as_bytes())?;
    Some(RawImage {
        data: stream.data.as_ref().to_vec(),
        encoding,
    })
}
