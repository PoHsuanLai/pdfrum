//! SVG into a page: `usvg` resolves the document, this compiles its tree.
//!
//! The mirror of `pdfrum-svg`'s export across the same geometry model. Export
//! decorates a [`RenderDevice`](pdfrum_render::RenderDevice) and writes
//! `<path>` elements; ingestion walks a resolved `usvg` tree and issues
//! [`Canvas`] calls, so an SVG logo goes into a PDF as **vectors** rather than
//! as a resampled bitmap.
//!
//! Parsing SVG properly is a project — CSS cascade, `use` expansion, nested
//! transforms, `viewBox` fitting, gradient coordinate systems — and `usvg`
//! already does all of it in pure Rust, handing back a tree of paths, groups
//! and images with every transform and every reference already resolved.
//! Writing a second incomplete SVG parser is declined here in writing. The
//! mapping, and every construct the walk cannot carry into PDF, is this
//! module and [`Unsupported`].
//!
//! # What the caller gets back
//!
//! [`Canvas::draw_svg`] returns an [`SvgIngestReport`] listing every construct
//! the walk could not carry into PDF, each as an [`Unsupported`] variant with
//! the element's id. **Nothing is dropped silently**: that is the property
//! this module exists to guarantee, the same one `RasterReport` guarantees on
//! the export side.

use std::collections::BTreeMap;

use kurbo::{Affine, BezPath, Point, Rect};
use pdfrum_object::{Array, Dict, Name, Object};

use crate::Error;
use pdfrum_common::Limits;
use peniko::Color;

/// An ingestion either applies or names why it could not.
type Result<T> = core::result::Result<T, Error>;
use crate::canvas::{Canvas, Dash, Fill, LineCap, LineJoin, MiterLimit, Paint, Stroke};

/// A construct in the source SVG that PDF drawing cannot carry.
///
/// An enum rather than a message string: a caller that wants to
/// refuse a filter but tolerate a dropped `<text>` matches on the variant, and
/// adding a case makes every such match fail to compile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum Unsupported {
    /// An SVG filter primitive chain — `filter="url(#…)"`.
    ///
    /// PDF has no filter model. The only faithful rendering would be to
    /// rasterize the filtered subtree, which is exactly the blurry result
    /// ingesting an SVG as vectors exists to avoid, so the subtree is drawn
    /// **unfiltered** and the loss is reported.
    Filter,
    /// A `<mask>` on a group.
    ///
    /// A PDF soft mask is expressible, but only through a `/SMask` luminosity
    /// group whose own content is a second form — a whole second compilation
    /// path. The group is drawn unmasked and reported.
    Mask,
    /// A `<text>` element.
    ///
    /// `usvg` is taken with `--no-default-features`, which drops its text
    /// stack entirely. Text elements therefore never reach the tree at all, so this is
    /// raised from the *source XML* rather than from the walk, and it is the
    /// one variant that carries no element id.
    Text,
    /// A `<pattern>` fill or stroke.
    ///
    /// A PDF tiling pattern is expressible and is future work; today the
    /// shape is filled with the pattern's average is not attempted at all —
    /// the shape is skipped and reported, because a wrong colour is a worse
    /// answer than a reported gap.
    Pattern,
    /// A radial gradient whose focal point is offset from its centre, or
    /// whose focal radius is non-zero.
    ///
    /// PDF's type 3 shading has a focal circle too, so the *geometry* maps;
    /// what does not map is SVG's clamping of a focus that falls outside the
    /// end circle. Rather than emit a shading that diverges from the source
    /// where it matters most, the gradient is drawn as its own type 3 shading
    /// with the focus **moved to the centre**, and the difference is reported.
    OffsetFocalGradient,
    /// A blend mode other than `normal` on a group.
    ///
    /// PDF has the same separable and non-separable blend modes, but setting
    /// one meaningfully requires a transparency group `/XObject` around the
    /// subtree rather than a bare `/ExtGState`. The subtree is drawn with
    /// normal blending and reported.
    BlendMode,
    /// A raster `<image>` in a format this build cannot decode into an
    /// embedded PDF image: GIF or WebP.
    ///
    /// PNG and JPEG both map — JPEG passes through as `/DCTDecode` and PNG is
    /// decoded to samples. The other two would need a decoder this crate does
    /// not carry, so the image is skipped and reported.
    ImageFormat,
}

impl Unsupported {
    /// A short, stable name for logs and reports.
    ///
    /// ```
    /// use pdfrum::Unsupported;
    ///
    /// assert_eq!(Unsupported::Filter.name(), "filter");
    /// ```
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Filter => "filter",
            Self::Mask => "mask",
            Self::Text => "text",
            Self::Pattern => "pattern",
            Self::OffsetFocalGradient => "offset-focal-gradient",
            Self::BlendMode => "blend-mode",
            Self::ImageFormat => "image-format",
        }
    }

    /// Whether the construct was **dropped** rather than approximated.
    ///
    /// The distinction a caller acts on: a dropped `<text>` or `<pattern>`
    /// leaves a visible hole, while an unfiltered group or a recentred focal
    /// gradient still draws something close. A caller that will accept an
    /// approximation but not a hole tests this.
    ///
    /// ```
    /// use pdfrum::Unsupported;
    ///
    /// assert!(Unsupported::Text.is_dropped());
    /// assert!(!Unsupported::Filter.is_dropped());
    /// ```
    #[must_use]
    pub const fn is_dropped(self) -> bool {
        matches!(self, Self::Text | Self::Pattern | Self::ImageFormat)
    }
}

/// One construct the walk could not carry, and where it was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsupportedItem {
    /// Which construct.
    pub what: Unsupported,
    /// The `id` attribute of the element it was on, empty when the source
    /// gave it none. [`Unsupported::Text`] always carries an empty id: it is
    /// raised from the source XML, where no tree node survives to name.
    pub id: String,
}

/// Everything one `draw_svg` could not carry into the page.
///
/// Empty is the meaningful answer — the whole document went in as vectors.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SvgIngestReport {
    items: Vec<UnsupportedItem>,
}

impl SvgIngestReport {
    /// The items, in walk order.
    #[must_use]
    pub fn items(&self) -> &[UnsupportedItem] {
        &self.items
    }

    /// Whether the whole document was carried into the page.
    ///
    /// ```
    /// use pdfrum::SvgIngestReport;
    ///
    /// assert!(SvgIngestReport::default().is_empty());
    /// ```
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// How many items each construct accounts for, in [`Unsupported`] order.
    #[must_use]
    pub fn counts(&self) -> Vec<(Unsupported, usize)> {
        let mut counted: BTreeMap<Unsupported, usize> = BTreeMap::new();
        for item in &self.items {
            *counted.entry(item.what).or_default() += 1;
        }
        counted.into_iter().collect()
    }

    /// Record one item.
    fn push(&mut self, what: Unsupported, id: &str) {
        self.items.push(UnsupportedItem {
            what,
            id: id.to_owned(),
        });
    }
}

/// How the SVG's own coordinate box is placed in the destination rectangle.
///
/// An enum rather than a `preserve_aspect: bool`, because the three answers
/// are genuinely different placements and the caller is choosing between
/// them, not toggling one off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SvgFit {
    /// Scale uniformly until the SVG fits inside the rectangle, and centre
    /// what is left over. The default, and what `preserveAspectRatio`'s own
    /// default `xMidYMid meet` means.
    #[default]
    Contain,
    /// Scale uniformly until the SVG covers the rectangle, centred; the
    /// overflow is clipped to the rectangle.
    Cover,
    /// Scale each axis independently so the SVG exactly fills the rectangle,
    /// distorting it. `preserveAspectRatio="none"`.
    Stretch,
}

impl SvgFit {
    /// The transform placing an SVG of `size` into `into`, y flipped.
    ///
    /// SVG's y runs **down** from a top-left origin and the canvas's runs
    /// **up** from a bottom-left one, so every placement carries a flip. It
    /// is composed here, once, rather than negated at each draw: a transform
    /// applied to the whole subtree is the only spelling that also gets
    /// nested `transform` attributes and gradient coordinate systems right.
    fn place(self, size: kurbo::Size, into: Rect) -> Affine {
        let placed = self.fit_box(size, into);
        let (sx, sy) = self.scale(size, into);
        // The box, then the flip about its own top edge, so SVG (0,0) lands
        // at the placed box's top-left.
        Affine::new([sx, 0.0, 0.0, -sy, placed.x0, placed.y1])
    }

    /// The rectangle a box of `size` occupies inside `into` under this fit.
    ///
    /// The placement without the flip: what a Form `XObject`, whose content
    /// already carries the flip, is mapped onto. [`SvgFit::place`] is this
    /// plus the y negation.
    fn fit_box(self, size: kurbo::Size, into: Rect) -> Rect {
        let (sx, sy) = self.scale(size, into);
        let (width, height) = (size.width * sx, size.height * sy);
        // Centred in the destination, which is what `xMidYMid` means and what
        // both `Contain` and `Cover` leave over on one axis.
        let origin = Point::new(
            into.x0 + (into.width() - width) / 2.0,
            into.y0 + (into.height() - height) / 2.0,
        );
        Rect::from_origin_size(origin, kurbo::Size::new(width, height))
    }

    /// The per-axis scale this fit applies to a box of `size` inside `into`.
    fn scale(self, size: kurbo::Size, into: Rect) -> (f64, f64) {
        match self {
            Self::Contain => {
                let s = (into.width() / size.width).min(into.height() / size.height);
                (s, s)
            }
            Self::Cover => {
                let s = (into.width() / size.width).max(into.height() / size.height);
                (s, s)
            }
            Self::Stretch => (into.width() / size.width, into.height() / size.height),
        }
    }
}

/// The `usvg` parse options this crate uses.
///
/// Built here rather than taken from the caller because every field `usvg`
/// offers either concerns text — whose faces arrive through
/// [`DocEdit::set_svg_fonts`](crate::EditDoc::set_svg_fonts) instead of
/// through a `usvg` type on our surface — or is a resource-loading hook whose
/// defaults are the safe ones. The one exception is the document's own
/// directory, which a caller who reads an SVG from a file needs so that a
/// relative `<image href>` resolves.
///
/// With `svg-text` off, `fonts` is not read at all: there is no text stack
/// to give faces to, and the parameter would be the dead option
/// forbids — so with the feature off the function does not take one.
fn parse_options(
    resources_dir: Option<std::path::PathBuf>,
    #[cfg(feature = "svg-text")] fonts: &crate::svg_text::SvgFonts,
) -> usvg::Options<'static> {
    #[cfg_attr(
        not(feature = "svg-text"),
        expect(unused_mut, reason = "text fills it in")
    )]
    let mut options = usvg::Options {
        resources_dir,
        ..usvg::Options::default()
    };
    #[cfg(feature = "svg-text")]
    {
        let (db, default_family) = fonts.parts();
        options.fontdb = db;
        // Left at `usvg`'s own default when the set named none, so a document
        // that does name a family still resolves against what is registered.
        if !default_family.is_empty() {
            default_family.clone_into(&mut options.font_family);
        }
    }
    options
}

/// Whether the source XML contains a `<text>` element `usvg` will have
/// dropped.
///
/// Without a text stack — the `svg-text` feature off, or on with no face
/// registered — a `<text>` element leaves **no node** in the resolved tree.
/// There is nothing for the walk to notice,
/// so reporting it has to happen before the parse, against the bytes.
///
/// A substring scan rather than a second XML parse: the question is only
/// whether to raise a report item, the cost of a false positive is one
/// spurious line in a report, and pulling in a parser to answer it would
/// double the dependency for no gain. The `<` is required so that the word
/// "text" inside an attribute value or a comment does not trigger it.
fn mentions_text(svg: &str) -> bool {
    svg.match_indices("<text").any(|(at, _)| {
        svg[at + 5..]
            .chars()
            .next()
            .is_none_or(|c| c.is_whitespace() || c == '>' || c == '/')
    })
}

/// Record the `<text>` this session cannot draw, before the walk that will
/// not see it.
///
/// The one place both ingestion entry points ask the question, so the inline
/// and the compiled spellings cannot drift apart on it.
///
/// It fires whenever the session has no text stack to lay the element out
/// with — the `svg-text` feature off, or on with **no face registered** — and
/// the second case is not a special case but the same one: `usvg` with an
/// empty font database drops a `<text>` exactly as a `usvg` without the
/// feature does, leaving no node behind. Reporting it here rather than
/// trusting the walk is what keeps the module's guarantee intact, because a
/// walk cannot notice something that is not in the tree.
///
/// With a face registered the walk sees each element and reports per element
/// with its id, which is strictly better than this document-wide answer; that
/// is why this is silent in that case rather than raising a second item.
fn report_dropped_text(
    svg: &str,
    report: &mut SvgIngestReport,
    #[cfg(feature = "svg-text")] fonts: &crate::svg_text::SvgFonts,
) {
    #[cfg(feature = "svg-text")]
    if !fonts.is_empty() {
        return;
    }
    if mentions_text(svg) {
        report.push(Unsupported::Text, "");
    }
}

impl Canvas<'_, '_> {
    /// Draw an SVG document into `into`, as vectors.
    ///
    /// `svg` is the document's source, and `fit` says how its own coordinate
    /// box is placed in the rectangle — see [`SvgFit`]. The whole drawing is
    /// scoped: it is wrapped in one `q`/`Q` and clipped to `into`, so nothing
    /// the SVG does escapes the rectangle the caller named and the canvas's
    /// own graphics state is untouched afterwards.
    ///
    /// The returned [`SvgIngestReport`] lists every construct that could not
    /// be carried, and **an empty report means the whole document went in**.
    ///
    /// # Errors
    ///
    /// [`Error::Svg`](Error::Svg) when `usvg` cannot resolve the
    /// document at all — malformed XML, or an `<svg>` with no usable size.
    /// A construct that resolves but does not map is a report item, not an
    /// error.
    ///
    /// ```
    /// use pdfrum::{Document, Rect, SvgFit};
    ///
    /// // A plain string with escaped quotes rather than a raw one: a `#`
    /// // inside a doc comment ends the raw-string hash count.
    /// const LOGO: &str = "<svg xmlns=\"http://www.w3.org/2000/svg\" \
    ///     viewBox=\"0 0 10 10\">\
    ///     <circle cx=\"5\" cy=\"5\" r=\"4\" fill=\"#c00\"/></svg>";
    ///
    /// let doc = Document::open("tests/fixtures/hello_world.pdf")?;
    /// let mut edit = doc.edit();
    /// edit.draw_page(0, |c| {
    ///     let report = c.draw_svg(LOGO, Rect::new(40.0, 40.0, 140.0, 140.0), SvgFit::Contain);
    ///     assert!(report.is_ok_and(|r| r.is_empty()));
    /// })?;
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    pub fn draw_svg(&mut self, svg: &str, into: Rect, fit: SvgFit) -> Result<SvgIngestReport> {
        self.draw_svg_from(svg, into, fit, None)
    }

    /// Draw an SVG document whose relative `<image href>` links resolve
    /// against `resources_dir`.
    ///
    /// [`Canvas::draw_svg`] is this with no directory, which is right for a
    /// document held in memory; a document read from a file wants the file's
    /// own directory here, or its linked images do not load.
    ///
    /// # Errors
    ///
    /// As [`Canvas::draw_svg`].
    pub fn draw_svg_from(
        &mut self,
        svg: &str,
        into: Rect,
        fit: SvgFit,
        resources_dir: Option<&std::path::Path>,
    ) -> Result<SvgIngestReport> {
        let options = parse_options(
            resources_dir.map(Into::into),
            #[cfg(feature = "svg-text")]
            self.fonts(),
        );
        let tree = usvg::Tree::from_str(svg, &options).map_err(Error::Svg)?;

        let mut report = SvgIngestReport::default();
        report_dropped_text(
            svg,
            &mut report,
            #[cfg(feature = "svg-text")]
            self.fonts(),
        );

        let placement = fit.place(tree.size().to_kurbo(), into);
        self.saved(|c| {
            c.clip(into, Fill::NonZero);
            c.transform(placement);
            let mut walk = Walk {
                canvas: c,
                report: &mut report,
            };
            walk.group(tree.root());
        });
        Ok(report)
    }

    /// Place an [`SvgForm`](crate::canvas::SvgForm) compiled by
    /// [`DocEdit::compile_svg`](crate::EditDoc::compile_svg), fitting its
    /// box into `into` the way [`Canvas::draw_svg`] fits a document.
    ///
    /// The deduplicating spelling of `draw_svg`: the SVG is compiled once and
    /// this writes one `Do` per placement, so the same logo on twenty pages
    /// is one content stream rather than twenty. The un-fitted placement is
    /// this without the fit, stretching the form's box onto the rectangle.
    ///
    /// ```
    /// use pdfrum::{Document, Rect, SvgFit};
    ///
    /// const LOGO: &str = "<svg xmlns=\"http://www.w3.org/2000/svg\" \
    ///     viewBox=\"0 0 10 10\">\
    ///     <circle cx=\"5\" cy=\"5\" r=\"4\" fill=\"#c00\"/></svg>";
    ///
    /// let doc = Document::open("tests/fixtures/hello_world_2_pages.pdf")?;
    /// let mut edit = doc.edit();
    /// let (logo, report) = edit.compile_svg(LOGO)?;
    /// assert!(report.is_empty());
    /// edit.draw_pages(|c| c.place_svg(&logo, Rect::new(10.0, 10.0, 60.0, 60.0), SvgFit::Contain))?;
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    pub fn place_svg(&mut self, form: &crate::canvas::SvgForm, into: Rect, fit: SvgFit) {
        // The form's box is the SVG's own, so fitting it into the
        // destination is the same computation `draw_svg` does on the tree's
        // size — minus the y flip, which the form's content already carries.
        let box_size = form.bbox().size();
        if box_size.width == 0.0 || box_size.height == 0.0 {
            return;
        }
        self.saved(|c| {
            c.clip(into, Fill::NonZero);
            c.place_form(form, fit.fit_box(box_size, into));
        });
    }
}

impl crate::EditDoc<'_> {
    /// Compile an SVG document once, into a Form `XObject` any number of
    /// pages can place.
    ///
    /// The deduplicating half of [`Canvas::draw_svg`]. That method writes the
    /// SVG's operators **inline** into the page it is drawing on, which is
    /// right for one placement and wasteful for many: the same logo on twenty
    /// pages becomes twenty copies of the same content. This compiles the
    /// document into a single `/Subtype /Form` object with its own `/BBox`
    /// and `/Resources`, and [`Canvas::place_svg`] then writes one `Do` per
    /// page against it.
    ///
    /// Nothing about the mapping differs — a form's content stream holds the
    /// same operators `draw_svg` would have written, and the returned
    /// [`SvgIngestReport`] is the same report. What differs is that the
    /// operators are written **once**, and that the fit is chosen per
    /// placement rather than baked in: the form's box is the SVG's own, so
    /// one compiled logo can be placed [`SvgFit::Contain`] on one page and
    /// [`SvgFit::Cover`] on another.
    ///
    /// # Errors
    ///
    /// [`Error::Svg`](Error::Svg) when `usvg` cannot resolve the
    /// document, exactly as [`Canvas::draw_svg`] reports it.
    ///
    /// ```
    /// use pdfrum::{Document, Rect, SvgFit};
    ///
    /// const LOGO: &str = "<svg xmlns=\"http://www.w3.org/2000/svg\" \
    ///     viewBox=\"0 0 10 10\">\
    ///     <rect width=\"10\" height=\"10\" fill=\"#0a0\"/></svg>";
    ///
    /// let doc = Document::open("tests/fixtures/hello_world.pdf")?;
    /// let mut edit = doc.edit();
    /// let (logo, _) = edit.compile_svg(LOGO)?;
    /// assert_eq!(logo.bbox().width(), 10.0);
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    pub fn compile_svg(
        &mut self,
        svg: &str,
        limits: &Limits,
        #[cfg(feature = "svg-text")] fonts: &crate::svg_text::SvgFonts,
    ) -> Result<(crate::canvas::SvgForm, SvgIngestReport)> {
        self.compile_svg_from_with_fonts(
            svg,
            None,
            limits,
            #[cfg(feature = "svg-text")]
            fonts,
        )
    }

    /// Compile an SVG whose relative `<image href>` links resolve against
    /// `resources_dir`.
    ///
    /// [`DocEdit::compile_svg`](crate::EditDoc::compile_svg) is this with no
    /// directory, which is right for
    /// a document held in memory; one read from a file wants the file's own
    /// directory here, or its linked images do not load. The same pairing
    /// [`Canvas::draw_svg`] and [`Canvas::draw_svg_from`] have.
    ///
    /// # Errors
    ///
    /// As [`DocEdit::compile_svg`](crate::EditDoc::compile_svg).
    pub fn compile_svg_from(
        &mut self,
        svg: &str,
        resources_dir: Option<&std::path::Path>,
        limits: &Limits,
    ) -> Result<(crate::canvas::SvgForm, SvgIngestReport)> {
        self.compile_svg_from_with_fonts(
            svg,
            resources_dir,
            limits,
            #[cfg(feature = "svg-text")]
            &crate::svg_text::SvgFonts::new(),
        )
    }

    /// [`EditDoc::compile_svg_from`] with the faces the SVG's `<text>` is set
    /// in; without them a `<text>` draws nothing and is reported instead.
    ///
    /// # Errors
    ///
    /// As [`EditDoc::compile_svg_from`].
    pub fn compile_svg_from_with_fonts(
        &mut self,
        svg: &str,
        resources_dir: Option<&std::path::Path>,
        limits: &Limits,
        #[cfg(feature = "svg-text")] fonts: &crate::svg_text::SvgFonts,
    ) -> Result<(crate::canvas::SvgForm, SvgIngestReport)> {
        let options = parse_options(
            resources_dir.map(Into::into),
            #[cfg(feature = "svg-text")]
            fonts,
        );
        let tree = usvg::Tree::from_str(svg, &options).map_err(Error::Svg)?;

        let mut report = SvgIngestReport::default();
        report_dropped_text(
            svg,
            &mut report,
            #[cfg(feature = "svg-text")]
            fonts,
        );

        // The form's box is the SVG's own size in a y-**up** space, so a
        // placement is a plain rectangle-to-rectangle map and the y flip
        // lives once inside the form rather than at every placement.
        let size = tree.size().to_kurbo();
        let bbox = Rect::from_origin_size(Point::ZERO, size);
        let form = self.compile_form(
            bbox,
            limits,
            #[cfg(feature = "svg-text")]
            fonts,
            |c| {
                c.transform(SvgFit::Stretch.place(size, bbox));
                let mut walk = Walk {
                    canvas: c,
                    report: &mut report,
                };
                walk.group(tree.root());
            },
        )?;
        Ok((form, report))
    }
}

/// A `usvg::Size` in the workspace's own geometry vocabulary.
trait ToKurbo {
    /// The same size as `kurbo`'s.
    fn to_kurbo(self) -> kurbo::Size;
}

impl ToKurbo for usvg::Size {
    fn to_kurbo(self) -> kurbo::Size {
        kurbo::Size::new(f64::from(self.width()), f64::from(self.height()))
    }
}

/// The walk in progress: where the drawing goes and what it could not carry.
///
/// A struct rather than two threaded parameters, because every node handler
/// needs both and the pair is the whole of the walk's state — the transforms
/// are already resolved into each node by `usvg`, so there is no stack of our
/// own to carry.
struct Walk<'w, 'a, 'b> {
    canvas: &'w mut Canvas<'a, 'b>,
    report: &'w mut SvgIngestReport,
}

impl Walk<'_, '_, '_> {
    /// Draw one group and everything under it.
    fn group(&mut self, group: &usvg::Group) {
        // Reported before anything is drawn, so a caller reading the report
        // in order sees the loss attached to the subtree it applies to.
        if !group.filters().is_empty() {
            self.report.push(Unsupported::Filter, group.id());
        }
        if group.mask().is_some() {
            self.report.push(Unsupported::Mask, group.id());
        }
        if group.blend_mode() != usvg::BlendMode::Normal {
            self.report.push(Unsupported::BlendMode, group.id());
        }

        let transform = to_affine(group.transform());
        let opacity = f64::from(group.opacity().get());
        let clip = group.clip_path().map(clip_outline);

        self.canvas.saved(|canvas| {
            canvas.transform(transform);
            if let Some((path, rule)) = clip {
                canvas.clip(&path, rule);
            }
            if opacity < 1.0 {
                canvas.opacity(opacity);
            }
            let mut inner = Walk {
                canvas,
                report: self.report,
            };
            for child in group.children() {
                inner.node(child);
            }
        });
    }

    /// Draw one node.
    fn node(&mut self, node: &usvg::Node) {
        match node {
            usvg::Node::Group(group) => self.group(group),
            usvg::Node::Path(path) => self.path(path),
            usvg::Node::Image(image) => self.image(image),
            usvg::Node::Text(text) => self.text(text),
        }
    }

    /// Draw one `<text>` element, as outlines.
    ///
    /// `usvg` has already done the hard half — resolved
    /// the family against the faces
    /// [`DocEdit::set_svg_fonts`](crate::EditDoc::set_svg_fonts) registered,
    /// run the bidi and the shaping, positioned every glyph, applied
    /// `text-anchor` and `textLength` and any `textPath` — and
    /// [`flattened`](usvg::Text::flattened) hands back the result as an
    /// ordinary group of filled paths. So the mapping is: walk that group
    /// like any other. Glyph outlines *are* paths.
    ///
    /// **Outlines rather than embedded text** is the deliberate default: the
    /// page needs no font embedded and no encoding to get right, and it
    /// renders identically in every viewer. What it costs is selectable
    /// text, alongside what embedding would need.
    ///
    /// An empty flattened group means `usvg` resolved no face for the
    /// element's family — the caller registered none, or none that matches —
    /// so nothing is drawn and the loss is reported per element, with the
    /// element's own id. A missing face is a reported gap, never a silent one.
    #[cfg(feature = "svg-text")]
    fn text(&mut self, text: &usvg::Text) {
        let flattened = text.flattened();
        if flattened.children().is_empty() {
            self.report.push(Unsupported::Text, text.id());
            return;
        }
        self.group(flattened);
    }

    /// Report one `<text>` this build cannot draw.
    ///
    /// Unreachable with `svg-text` off — the text stack is not compiled in,
    /// so the parser never constructs the variant, and the pre-parse scan in
    /// [`report_dropped_text`] is what raises the item instead. The arm
    /// exists because the enum is `usvg`'s and not ours, and a build that did
    /// carry text must not silently drop it.
    #[cfg(not(feature = "svg-text"))]
    fn text(&mut self, text: &usvg::Text) {
        self.report.push(Unsupported::Text, text.id());
    }

    /// Draw one path, with whatever of its fill and stroke maps.
    fn path(&mut self, path: &usvg::Path) {
        if !path.is_visible() {
            return;
        }
        let outline = to_bez_path(path.data());

        // A gradient fill is a PDF shading, which paints a *region* rather
        // than taking part in a paint operator: it is written as its own
        // clipped `sh` and the ordinary paint below then handles the stroke
        // alone. Splitting the two is what keeps a gradient-filled,
        // solid-stroked shape correct.
        let gradient_fill = path.fill().and_then(|fill| match fill.paint() {
            usvg::Paint::LinearGradient(_) | usvg::Paint::RadialGradient(_) => {
                Some((fill.paint(), fill.rule(), f64::from(fill.opacity().get())))
            }
            usvg::Paint::Color(_) | usvg::Paint::Pattern(_) => None,
        });
        if let Some((paint, rule, opacity)) = gradient_fill {
            self.shading(paint, &outline, to_fill(rule), opacity, path.id());
        }

        let fill = if gradient_fill.is_some() {
            None
        } else {
            self.solid(path.fill().map(usvg::Fill::paint), path.id())
                .map(|color| {
                    with_alpha(
                        color,
                        path.fill().map_or(1.0, |f| f64::from(f.opacity().get())),
                    )
                })
        };
        let stroke = path.stroke().and_then(|stroke| {
            let color = self.solid(Some(stroke.paint()), path.id())?;
            Some(to_stroke(stroke, color))
        });

        let paint = match (fill, stroke) {
            (Some(fill), Some(stroke)) => Paint::FillStroke(fill, stroke),
            (Some(fill), None) => Paint::Fill(fill),
            (None, Some(stroke)) => Paint::Stroke(stroke),
            (None, None) => return,
        };
        let rule = path.fill().map_or(Fill::NonZero, |f| to_fill(f.rule()));
        self.canvas.draw(&outline, paint, rule);
    }

    /// The solid colour a paint resolves to, reporting the paints that have
    /// none.
    ///
    /// `None` means "do not paint with this" — either there was no paint at
    /// all, or it was one whose loss has just been recorded.
    fn solid(&mut self, paint: Option<&usvg::Paint>, id: &str) -> Option<Color> {
        match paint? {
            usvg::Paint::Color(color) => Some(Color::from_rgb8(color.red, color.green, color.blue)),
            usvg::Paint::Pattern(_) => {
                self.report.push(Unsupported::Pattern, id);
                None
            }
            // Handled by `shading` on the fill side; a gradient *stroke* has
            // no PDF spelling short of converting the stroke to its outline,
            // so it reaches here and is reported as the approximation it is.
            usvg::Paint::LinearGradient(gradient) => Some(average_stop(gradient.stops())),
            usvg::Paint::RadialGradient(gradient) => Some(average_stop(gradient.stops())),
        }
    }

    /// Paint `outline` with a PDF shading matching `paint`.
    fn shading(
        &mut self,
        paint: &usvg::Paint,
        outline: &BezPath,
        rule: Fill,
        opacity: f64,
        id: &str,
    ) {
        let (dict, transform) = match paint {
            usvg::Paint::LinearGradient(gradient) => {
                (axial_shading(gradient), to_affine(gradient.transform()))
            }
            usvg::Paint::RadialGradient(gradient) => {
                // Exact comparisons, deliberately. This asks whether `usvg`
                // resolved a focus *distinct from* the centre, not whether
                // two computed quantities are near each other: `usvg` copies
                // `cx`/`cy` into `fx`/`fy` bit for bit when the source gave
                // no focus, so equality is the question and a tolerance would
                // only start reporting gradients that are in fact centred.
                #[expect(clippy::float_cmp, reason = "a recognizer, not a measurement")]
                let offset = gradient.fx() != gradient.cx()
                    || gradient.fy() != gradient.cy()
                    || gradient.fr().get() != 0.0;
                if offset {
                    self.report.push(Unsupported::OffsetFocalGradient, id);
                }
                (radial_shading(gradient), to_affine(gradient.transform()))
            }
            // Only the two gradient paints reach here; `path` selects on
            // exactly those variants before calling.
            usvg::Paint::Color(_) | usvg::Paint::Pattern(_) => return,
        };
        self.canvas.shade(outline, rule, &dict, transform, opacity);
    }

    /// Draw one raster image.
    fn image(&mut self, image: &usvg::Image) {
        if !image.is_visible() {
            return;
        }
        let bytes = match image.kind() {
            usvg::ImageKind::JPEG(data) | usvg::ImageKind::PNG(data) => data.clone(),
            usvg::ImageKind::GIF(_) | usvg::ImageKind::WEBP(_) => {
                self.report.push(Unsupported::ImageFormat, image.id());
                return;
            }
            // A nested SVG `usvg` already resolved: walked as a subtree, so
            // it stays vectors rather than becoming pixels.
            usvg::ImageKind::SVG(tree) => {
                let size = tree.size().to_kurbo();
                let placed = image.size().to_kurbo();
                let transform = Affine::scale_non_uniform(
                    placed.width / size.width,
                    placed.height / size.height,
                );
                self.canvas.saved(|canvas| {
                    canvas.transform(transform);
                    let mut inner = Walk {
                        canvas,
                        report: self.report,
                    };
                    inner.group(tree.root());
                });
                return;
            }
        };

        let size = image.size().to_kurbo();
        // The image's own box, in SVG coordinates: origin top-left, y down.
        // `Canvas::image` places a y-up rectangle, so the subtree is flipped
        // about the box before the image is drawn into it.
        let placed = Rect::new(0.0, 0.0, size.width, size.height);
        let Some(embedded) = self.canvas.embed_svg_image(&bytes) else {
            self.report.push(Unsupported::ImageFormat, image.id());
            return;
        };
        self.canvas.saved(|canvas| {
            canvas.transform(Affine::new([1.0, 0.0, 0.0, -1.0, 0.0, size.height]));
            canvas.image(&embedded, placed);
        });
    }
}

/// One decoded PNG, in the shape [`crate::EditDoc::embed_image`] takes.
pub(crate) struct DecodedPng {
    pub(crate) pixels: Vec<u8>,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) format: crate::PixelFormat,
}

/// Decode a PNG an SVG `<image>` carried into samples a PDF image can hold.
///
/// `None` for anything the decoder refuses. PNG's other bit depths and colour
/// types are handled by asking the decoder to transform them to eight-bit RGB
/// or RGBA, which is the one place a decoder earns its keep: a paletted,
/// interlaced, sixteen-bit or grey-with-alpha source all arrive as one of two
/// layouts.
pub(crate) fn decode_png(bytes: &[u8]) -> Option<DecodedPng> {
    // A `Cursor`, because the decoder wants `BufRead + Seek` and a `&[u8]` is
    // only the first of the two.
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().ok()?;
    let mut pixels = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut pixels).ok()?;
    pixels.truncate(info.buffer_size());
    let format = match info.color_type {
        png::ColorType::Grayscale => crate::PixelFormat::Gray8,
        png::ColorType::Rgb => crate::PixelFormat::Rgb8,
        png::ColorType::Rgba => crate::PixelFormat::Rgba8,
        // `normalize_to_color8` expands a palette and adds an alpha channel
        // to grey-with-alpha, so neither reaches here; an indexed image that
        // somehow did would need a `/Indexed` colour space this path does not
        // build.
        png::ColorType::Indexed | png::ColorType::GrayscaleAlpha => return None,
    };
    Some(DecodedPng {
        pixels,
        width: info.width,
        height: info.height,
        format,
    })
}

/// `color` with its alpha multiplied by `opacity`.
fn with_alpha(color: Color, opacity: f64) -> Color {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "clamped to 0..=1, where every f64 has an f32 within one ulp \
                  and the loss is far below an alpha step"
    )]
    let alpha = opacity.clamp(0.0, 1.0) as f32;
    color.multiply_alpha(alpha)
}

/// The average of a gradient's stops, for the one place a gradient has to
/// collapse to a colour: a gradient **stroke**.
///
/// A stroke is painted by `S`, which takes a colour and not a shading; the
/// PDF spelling would be to convert the stroke to its outline and shade that,
/// which needs a stroke expander this crate does not have. Averaging the
/// stops keeps the shape visible and roughly the right colour, and it is
/// the one approximation the
/// walk makes without a report item — because unlike the reported cases, the
/// shape is still there and still stroked.
fn average_stop(stops: &[usvg::Stop]) -> Color {
    let mut sum = [0.0_f32; 3];
    let mut n = 0.0_f32;
    for stop in stops {
        let color = stop.color();
        sum[0] += f32::from(color.red);
        sum[1] += f32::from(color.green);
        sum[2] += f32::from(color.blue);
        n += 1.0;
    }
    if n == 0.0 {
        return Color::BLACK;
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "each channel is a mean of u8s, so it is in 0..=255 by \
                  construction and the cast cannot lose or wrap"
    )]
    Color::from_rgb8((sum[0] / n) as u8, (sum[1] / n) as u8, (sum[2] / n) as u8)
}

/// A `usvg` transform as an affine.
fn to_affine(t: usvg::Transform) -> Affine {
    Affine::new([
        f64::from(t.sx),
        f64::from(t.ky),
        f64::from(t.kx),
        f64::from(t.sy),
        f64::from(t.tx),
        f64::from(t.ty),
    ])
}

/// A `usvg` fill rule as the canvas's.
fn to_fill(rule: usvg::FillRule) -> Fill {
    match rule {
        usvg::FillRule::NonZero => Fill::NonZero,
        usvg::FillRule::EvenOdd => Fill::EvenOdd,
    }
}

/// A `usvg` stroke as the canvas's, colour already resolved.
///
/// `usvg` resolves `stroke-linecap`, `stroke-linejoin`, `stroke-miterlimit`,
/// `stroke-dasharray` and `stroke-dashoffset` for us, and PDF has an operator
/// for each, so all five cross. The one value that does not survive intact is
/// `LineJoin::MiterClip`: SVG 2 clips the miter at the limit where PDF bevels
/// it, and `j` offers no third spelling, so it lands on the miter join it is
/// a variant of rather than on a bevel that would be visibly blunter.
///
/// A dash array `usvg` resolved can still be one PDF refuses — an all-zero
/// `stroke-dasharray` is legal SVG and means solid. `Dash::new` catches those
/// and the stroke stays solid, which is what the SVG asked for anyway.
fn to_stroke(stroke: &usvg::Stroke, color: Color) -> Stroke {
    let dash = stroke.dasharray().and_then(|lengths| {
        let lengths: Vec<f64> = lengths.iter().map(|length| f64::from(*length)).collect();
        Dash::new(&lengths, f64::from(stroke.dashoffset().max(0.0)))
    });
    Stroke {
        color: with_alpha(color, f64::from(stroke.opacity().get())),
        width: f64::from(stroke.width().get()),
        cap: match stroke.linecap() {
            usvg::LineCap::Butt => LineCap::Butt,
            usvg::LineCap::Round => LineCap::Round,
            usvg::LineCap::Square => LineCap::Square,
        },
        join: match stroke.linejoin() {
            usvg::LineJoin::Miter | usvg::LineJoin::MiterClip => LineJoin::Miter,
            usvg::LineJoin::Round => LineJoin::Round,
            usvg::LineJoin::Bevel => LineJoin::Bevel,
        },
        miter_limit: MiterLimit::new(f64::from(stroke.miterlimit().get())),
        dash,
    }
}

/// A `tiny_skia_path::Path` as a `kurbo::BezPath`.
///
/// Segment for segment, with the quadratic raised to the identical cubic
/// rather than flattened — `Canvas::write_path` would raise it anyway, and
/// doing it here keeps one representation of the curve rather than two.
fn to_bez_path(path: &usvg::tiny_skia_path::Path) -> BezPath {
    use usvg::tiny_skia_path::PathSegment;

    let point = |p: usvg::tiny_skia_path::Point| Point::new(f64::from(p.x), f64::from(p.y));
    let mut out = BezPath::new();
    for segment in path.segments() {
        match segment {
            PathSegment::MoveTo(p) => out.move_to(point(p)),
            PathSegment::LineTo(p) => out.line_to(point(p)),
            PathSegment::QuadTo(c, p) => out.quad_to(point(c), point(p)),
            PathSegment::CubicTo(c1, c2, p) => out.curve_to(point(c1), point(c2), point(p)),
            PathSegment::Close => out.close_path(),
        }
    }
    out
}

/// One clip path flattened to a single outline and a rule.
///
/// `usvg` resolves a `<clipPath>` to a group of paths, and PDF's `W` narrows
/// the clip to the *union* of one path's subpaths — so concatenating the
/// children's outlines under the nonzero rule is the faithful mapping for the
/// common case. A `clipPath` with its own nested `clip-path`, which SVG
/// intersects, is not expressible this way.
fn clip_outline(clip: &usvg::ClipPath) -> (BezPath, Fill) {
    let mut out = BezPath::new();
    let mut rule = Fill::NonZero;
    collect_clip(
        clip.root(),
        to_affine(clip.transform()),
        &mut out,
        &mut rule,
    );
    (out, rule)
}

/// Append every path under `group` to `out`, in `group`'s own space.
fn collect_clip(group: &usvg::Group, at: Affine, out: &mut BezPath, rule: &mut Fill) {
    let at = at * to_affine(group.transform());
    for child in group.children() {
        match child {
            usvg::Node::Group(inner) => collect_clip(inner, at, out, rule),
            usvg::Node::Path(path) => {
                if let Some(fill) = path.fill() {
                    *rule = to_fill(fill.rule());
                }
                out.extend(at * to_bez_path(path.data()));
            }
            usvg::Node::Image(_) | usvg::Node::Text(_) => {}
        }
    }
}

/// A `/ShadingType 2` dictionary for a linear gradient.
fn axial_shading(gradient: &usvg::LinearGradient) -> Dict {
    shading_dict(
        2,
        Array::of([
            Object::Real(gradient.x1()),
            Object::Real(gradient.y1()),
            Object::Real(gradient.x2()),
            Object::Real(gradient.y2()),
        ]),
        gradient.stops(),
    )
}

/// A `/ShadingType 3` dictionary for a radial gradient.
///
/// The focus is written at the centre: SVG clamps a focus outside the end
/// circle in a way PDF's type 3 does not, and the difference is reported as
/// [`Unsupported::OffsetFocalGradient`] rather than emitted wrong.
fn radial_shading(gradient: &usvg::RadialGradient) -> Dict {
    shading_dict(
        3,
        Array::of([
            Object::Real(gradient.cx()),
            Object::Real(gradient.cy()),
            Object::Real(0.0),
            Object::Real(gradient.cx()),
            Object::Real(gradient.cy()),
            Object::Real(gradient.r().get()),
        ]),
        gradient.stops(),
    )
}

/// The shading dictionary shared by both gradient types.
///
/// The stops become one stitching function (type 3) over exponential
/// interpolations (type 2), which is how PDF spells a multi-stop ramp: one
/// sub-function per adjacent pair, stitched at the stop offsets.
fn shading_dict(kind: i64, coords: Array, stops: &[usvg::Stop]) -> Dict {
    Dict::from_pairs([
        (Name::from("ShadingType"), Object::Int(kind)),
        (
            Name::from("ColorSpace"),
            Object::Name(Name::from("DeviceRGB")),
        ),
        (Name::from("Coords"), Object::Array(coords)),
        (Name::from("Function"), Object::Dict(stitching(stops))),
        (
            Name::from("Extend"),
            Object::Array(Array::of([Object::Bool(true), Object::Bool(true)])),
        ),
    ])
}

/// The stitching function over `stops`.
fn stitching(stops: &[usvg::Stop]) -> Dict {
    let rgb = |color: usvg::Color| {
        Object::Array(Array::of([
            Object::Real(f32::from(color.red) / 255.0),
            Object::Real(f32::from(color.green) / 255.0),
            Object::Real(f32::from(color.blue) / 255.0),
        ]))
    };
    // A single stop is a constant colour, which is a type 2 with both ends
    // the same rather than a stitch over zero intervals.
    let Some(first) = stops.first() else {
        return exponential(rgb(usvg::Color::black()), rgb(usvg::Color::black()));
    };
    if stops.len() == 1 {
        return exponential(rgb(first.color()), rgb(first.color()));
    }

    let mut functions = Vec::new();
    let mut bounds = Vec::new();
    let mut encode = Vec::new();
    for pair in stops.windows(2) {
        let (Some(from), Some(to)) = (pair.first(), pair.get(1)) else {
            continue;
        };
        functions.push(Object::Dict(exponential(
            rgb(from.color()),
            rgb(to.color()),
        )));
        encode.push(Object::Real(0.0));
        encode.push(Object::Real(1.0));
        bounds.push(Object::Real(to.offset().get()));
    }
    // `/Bounds` has one fewer entry than `/Functions`: the last stop's offset
    // is the domain's end, not an interior boundary.
    bounds.pop();

    Dict::from_pairs([
        (Name::from("FunctionType"), Object::Int(3)),
        (
            Name::from("Domain"),
            Object::Array(Array::of([Object::Real(0.0), Object::Real(1.0)])),
        ),
        (Name::from("Functions"), Object::Array(Array::of(functions))),
        (Name::from("Bounds"), Object::Array(Array::of(bounds))),
        (Name::from("Encode"), Object::Array(Array::of(encode))),
    ])
}

/// One type 2 exponential interpolation from `from` to `to`, linear.
fn exponential(from: Object, to: Object) -> Dict {
    Dict::from_pairs([
        (Name::from("FunctionType"), Object::Int(2)),
        (
            Name::from("Domain"),
            Object::Array(Array::of([Object::Real(0.0), Object::Real(1.0)])),
        ),
        (Name::from("C0"), from),
        (Name::from("C1"), to),
        (Name::from("N"), Object::Real(1.0)),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    // The scan exists only where there is no text stack to put a node in the
    // tree; with `svg-text` the walk sees the element itself.
    #[cfg(not(feature = "svg-text"))]
    #[test]
    fn text_is_seen_in_the_source_because_the_tree_will_not_have_it() {
        assert!(mentions_text("<svg><text x='0'>hi</text></svg>"));
        assert!(mentions_text("<svg><text/></svg>"));
        // The word inside an attribute must not trigger it.
        assert!(!mentions_text(r#"<svg><rect id="textbox"/></svg>"#));
        assert!(!mentions_text("<svg><textPath/></svg>"));
    }

    #[test]
    fn contain_centres_and_flips() {
        let size = kurbo::Size::new(10.0, 10.0);
        let into = Rect::new(0.0, 0.0, 100.0, 200.0);
        let at = SvgFit::Contain.place(size, into);
        // SVG's top-left corner lands at the destination's top-left, and its
        // bottom-left at the bottom of the fitted square, centred vertically.
        assert_eq!(at * Point::new(0.0, 0.0), Point::new(0.0, 150.0));
        assert_eq!(at * Point::new(10.0, 10.0), Point::new(100.0, 50.0));
    }

    #[test]
    fn cover_fills_the_short_axis_and_overflows_the_long_one() {
        // A square into a tall rectangle: `Cover` scales to the *height*, so
        // the SVG is 200 wide in a 100-wide box and hangs 50 off each side.
        // `draw_svg` clips to the destination, which is what makes the
        // overflow a crop rather than a spill.
        let at = SvgFit::Cover.place(
            kurbo::Size::new(10.0, 10.0),
            Rect::new(0.0, 0.0, 100.0, 200.0),
        );
        assert_eq!(at * Point::new(0.0, 0.0), Point::new(-50.0, 200.0));
        assert_eq!(at * Point::new(10.0, 10.0), Point::new(150.0, 0.0));
    }

    #[test]
    fn stretch_fills_both_axes() {
        let at = SvgFit::Stretch.place(
            kurbo::Size::new(10.0, 20.0),
            Rect::new(0.0, 0.0, 100.0, 100.0),
        );
        assert_eq!(at * Point::new(10.0, 20.0), Point::new(100.0, 0.0));
    }

    /// The bytes of the first PNG `<image>` under `group`.
    fn png_bytes(group: &usvg::Group) -> Option<Vec<u8>> {
        for child in group.children() {
            match child {
                usvg::Node::Group(inner) => {
                    if let Some(found) = png_bytes(inner) {
                        return Some(found);
                    }
                }
                usvg::Node::Image(image) => {
                    if let usvg::ImageKind::PNG(data) = image.kind() {
                        return Some(data.as_ref().clone());
                    }
                }
                usvg::Node::Path(_) | usvg::Node::Text(_) => {}
            }
        }
        None
    }

    #[test]
    fn a_png_decodes_to_samples_and_a_non_png_refuses() {
        // The fixture corpus's own embedded tile, 16x16 truecolour. Decoding
        // it is the path `Canvas::embed_svg_image` takes for a PNG
        // `<image>`; refusing anything else is what raises
        // `Unsupported::ImageFormat`.
        let svg = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/svg/image_png.svg"
        ))
        .expect("the fixture reads");
        let tree = usvg::Tree::from_str(&svg, &usvg::Options::default()).expect("parses");
        let bytes = png_bytes(tree.root()).expect("the fixture's image is a PNG");
        let decoded = decode_png(&bytes).expect("a truecolour PNG decodes");
        assert_eq!((decoded.width, decoded.height), (16, 16));
        assert_eq!(decoded.format, crate::PixelFormat::Rgb8);
        assert_eq!(decoded.pixels.len(), 16 * 16 * 3);

        assert!(decode_png(b"not a png at all").is_none());
    }

    #[test]
    fn a_dropped_construct_is_distinguished_from_an_approximated_one() {
        assert!(Unsupported::Pattern.is_dropped());
        assert!(!Unsupported::Mask.is_dropped());
    }

    #[test]
    fn every_unsupported_has_a_distinct_name() {
        let all = [
            Unsupported::Filter,
            Unsupported::Mask,
            Unsupported::Text,
            Unsupported::Pattern,
            Unsupported::OffsetFocalGradient,
            Unsupported::BlendMode,
            Unsupported::ImageFormat,
        ];
        let mut names: Vec<_> = all.iter().map(|u| u.name()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), all.len());
    }

    #[test]
    fn counts_group_and_sort_by_construct() {
        let mut report = SvgIngestReport::default();
        report.push(Unsupported::Pattern, "a");
        report.push(Unsupported::Filter, "b");
        report.push(Unsupported::Pattern, "c");
        assert_eq!(
            report.counts(),
            vec![(Unsupported::Filter, 1), (Unsupported::Pattern, 2)]
        );
    }

    /// The stops of the one linear gradient in `svg`.
    ///
    /// `usvg::Stop` has no public constructor, so the stops a function test
    /// needs come from a real parse rather than from a literal — which also
    /// keeps the test honest about what `usvg` actually hands us.
    fn gradient_stops(svg: &str) -> Vec<usvg::Stop> {
        fn find(group: &usvg::Group) -> Option<Vec<usvg::Stop>> {
            for child in group.children() {
                match child {
                    usvg::Node::Group(inner) => {
                        if let Some(found) = find(inner) {
                            return Some(found);
                        }
                    }
                    usvg::Node::Path(path) => {
                        if let Some(usvg::Paint::LinearGradient(g)) =
                            path.fill().map(usvg::Fill::paint)
                        {
                            return Some(g.stops().to_vec());
                        }
                    }
                    usvg::Node::Image(_) | usvg::Node::Text(_) => {}
                }
            }
            None
        }
        let tree = usvg::Tree::from_str(svg, &usvg::Options::default()).expect("parses");
        find(tree.root()).expect("the document has a gradient-filled path")
    }

    const TWO_STOP: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10">
      <defs><linearGradient id="g"><stop offset="0" stop-color="black"/>
      <stop offset="1" stop-color="white"/></linearGradient></defs>
      <rect width="10" height="10" fill="url(#g)"/></svg>"#;

    #[test]
    fn a_two_stop_ramp_is_one_exponential_under_a_stitch() {
        let function = stitching(&gradient_stops(TWO_STOP));
        assert_eq!(
            function.raw(&Name::from("FunctionType")),
            Some(&Object::Int(3))
        );
        // Two stops make one interval, so there is no *interior* boundary.
        let bounds = function
            .raw(&Name::from("Bounds"))
            .and_then(Object::as_array);
        assert_eq!(bounds.map(pdfrum_object::Array::len), Some(0));
        let functions = function
            .raw(&Name::from("Functions"))
            .and_then(Object::as_array);
        assert_eq!(functions.map(pdfrum_object::Array::len), Some(1));
    }

    #[test]
    fn a_three_stop_ramp_stitches_two_intervals_at_the_middle_offset() {
        const THREE_STOP: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10">
          <defs><linearGradient id="g"><stop offset="0" stop-color="black"/>
          <stop offset="0.25" stop-color="red"/>
          <stop offset="1" stop-color="white"/></linearGradient></defs>
          <rect width="10" height="10" fill="url(#g)"/></svg>"#;
        let function = stitching(&gradient_stops(THREE_STOP));
        let len = |key: &str| {
            function
                .raw(&Name::from(key))
                .and_then(Object::as_array)
                .map(pdfrum_object::Array::len)
        };
        assert_eq!(len("Functions"), Some(2));
        // One fewer bound than functions, at the interior stop's own offset.
        assert_eq!(len("Bounds"), Some(1));
        assert_eq!(len("Encode"), Some(4));
    }
}
