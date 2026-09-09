//! Page objects built from a few fields, ready to push onto a `PageEdit`.
//!
//! Each builder states the graphics state it paints under in full rather
//! than inheriting one, because a regenerated content stream carries no
//! state from whatever drew before it.

use kurbo::{Affine, BezPath, Rect};
use pdfrum_page::{
    ColorSpace, Content, FillRule, GraphicsState, PageObject, PathObject, TextObject,
    TextRenderMode, TextSegment,
};

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

/// A filled or stroked path, ready to [`PageEdit::push`](pdfrum_page::PageEdit::push).
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

/// A run of text, ready to [`PageEdit::push`](pdfrum_page::PageEdit::push).
///
/// The font is named by an indirect `/Font` dictionary: a regenerated stream
/// writes `/Name Tf` and [`ResourceTable::realize`](crate::ResourceTable::realize) allocates
/// that name for [`TextBuilder::font`]. Obtain the reference from
/// [`PageEdit::font_of`](pdfrum_page::PageEdit::font_of) (a font the page already has) or from
/// [`EditDoc::embed_font`](crate::EditDoc::embed_font) / [`EditDoc::standard_font`](crate::EditDoc::standard_font) (a font
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
    pub render_mode: TextRenderMode,
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
            render_mode: TextRenderMode::Fill,
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

/// An image placement, ready to [`PageEdit::push`](pdfrum_page::PageEdit::push).
///
/// The pixels are not supplied here: an image page object names an `/XObject`,
/// and the regenerated stream writes `/Name Do`. Obtain the reference from
/// [`PageEdit::image_of`](pdfrum_page::PageEdit::image_of) (an image the document already holds) or from
/// [`EditDoc::embed_jpeg`](crate::EditDoc::embed_jpeg) / [`EditDoc::embed_image`](crate::EditDoc::embed_image) (one this
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
    /// [`EmbeddedImage::width`](crate::EmbeddedImage::width) and [`EmbeddedImage::height`](crate::EmbeddedImage::height).
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
            samples: pdfrum_page::Samples::Whole(pdfrum_page::Pixels::Gray8(Box::new([0]))),
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
