//! The page-object graph: what the interpreter produces
//! (ISO 32000-1 §8.2, §9).
//!
//! Every object is a small record — geometry or payload, a graphics-state
//! snapshot, and the marks in force at its creation. No behaviour lives
//! inside them: `pdfrum-render` walks the graph and `pdfrum-text` reads it,
//! and neither re-derives semantics.
//!
//! # Box derivation is this crate's job
//!
//! The parser hands over inherited attributes; turning them into a page's
//! geometry is here, and three of the rules bite:
//!
//! - **An empty `/MediaBox` becomes US Letter**, `(0, 0, 612, 792)`. Empty
//!   means non-positive width or height after normalization.
//! - **A `/CropBox` is intersected with the media box**, and an intersection
//!   that comes out empty is *kept* empty — a zero-by-zero page.
//! - **`/Rotate` is `((n / 90) % 4 + 4) % 4`**, so `45` is no rotation at
//!   all, `-90` is three quarter-turns, and `450` is one.

use crate::image::ImageData;
use crate::names;
use crate::shading::Shading;
use crate::state::{ContentMarks, GraphicsState};
use crate::transparency::Transparency;
use kurbo::{Affine, BezPath, Rect};
use pdfrum_common::{DiagKind, Diagnostics, Severity};
use pdfrum_font::Font;
use pdfrum_object::{Dict, Resolve};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// The default page size when `/MediaBox` is missing or empty: US Letter.
pub const DEFAULT_MEDIA_BOX: Rect = Rect::new(0.0, 0.0, 612.0, 792.0);

/// A page's `/Rotate`, normalized to one of four quarter turns
/// (ISO 32000-1 §7.7.3.3).
///
/// The value is always clockwise and always a multiple of 90 degrees: a
/// document writing `/Rotate 450` means [`Rotation::Quarter`], and one
/// writing `-90` means [`Rotation::ThreeQuarter`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Rotation {
    /// Upright.
    #[default]
    None,
    /// Ninety degrees clockwise.
    Quarter,
    /// Half a turn.
    Half,
    /// Two hundred and seventy degrees clockwise.
    ThreeQuarter,
}

impl Rotation {
    /// The rotation `/Rotate n` names.
    ///
    /// The division comes **first**, so `45` truncates to zero quarter turns
    /// rather than rounding to one.
    #[must_use]
    pub fn from_degrees(n: i64) -> Self {
        let quarters = ((n / 90) % 4 + 4) % 4;
        match quarters {
            1 => Self::Quarter,
            2 => Self::Half,
            3 => Self::ThreeQuarter,
            _ => Self::None,
        }
    }

    /// The rotation in degrees clockwise: 0, 90, 180 or 270.
    #[must_use]
    pub fn degrees(self) -> u32 {
        match self {
            Self::None => 0,
            Self::Quarter => 90,
            Self::Half => 180,
            Self::ThreeQuarter => 270,
        }
    }

    /// Quarter turns clockwise.
    #[must_use]
    pub fn quarters(self) -> u8 {
        match self {
            Self::None => 0,
            Self::Quarter => 1,
            Self::Half => 2,
            Self::ThreeQuarter => 3,
        }
    }

    /// The matrix mapping the crop box onto a `width` by `height` device
    /// rectangle.
    ///
    /// Width and height are **swapped** for the quarter and three-quarter
    /// turns, which is what makes a rotated page's device box the right way
    /// round.
    #[must_use]
    pub fn display_matrix(self, box_rect: Rect) -> Affine {
        let (left, bottom, right, top) = (box_rect.x0, box_rect.y0, box_rect.x1, box_rect.y1);
        match self {
            Self::None => Affine::new([1.0, 0.0, 0.0, 1.0, -left, -bottom]),
            Self::Quarter => Affine::new([0.0, -1.0, 1.0, 0.0, -bottom, right]),
            Self::Half => Affine::new([-1.0, 0.0, 0.0, -1.0, right, top]),
            Self::ThreeQuarter => Affine::new([0.0, 1.0, -1.0, 0.0, top, -left]),
        }
    }
}

/// The error [`Rotation`]'s [`FromStr`](std::str::FromStr) returns: the
/// string named no quarter turn.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not a quarter turn: {0}")]
pub struct NotAQuarterTurn(String);

impl std::fmt::Display for Rotation {
    /// The degrees clockwise as a bare number: `0`, `90`, `180` or `270`.
    ///
    /// Round-trips through [`FromStr`](std::str::FromStr).
    ///
    /// ```
    /// assert_eq!(pdfrum_page::Rotation::Quarter.to_string(), "90");
    /// ```
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.degrees(), f)
    }
}

impl std::str::FromStr for Rotation {
    type Err = NotAQuarterTurn;

    /// The inverse of [`Display`](std::fmt::Display): `"0"`, `"90"`, `"180"`
    /// and `"270"`, and nothing else.
    ///
    /// Not `"450"` and not `"-90"`. Normalizing a document's out-of-range
    /// `/Rotate` is [`Rotation::from_degrees`]'s job; doing it here would
    /// make a caller's round trip lossy.
    ///
    /// # Errors
    ///
    /// [`NotAQuarterTurn`] when the string is not one of those four.
    ///
    /// ```
    /// assert_eq!("270".parse(), Ok(pdfrum_page::Rotation::ThreeQuarter));
    /// assert!("45".parse::<pdfrum_page::Rotation>().is_err());
    /// ```
    fn from_str(s: &str) -> core::result::Result<Rotation, NotAQuarterTurn> {
        match s {
            "0" => Ok(Rotation::None),
            "90" => Ok(Rotation::Quarter),
            "180" => Ok(Rotation::Half),
            "270" => Ok(Rotation::ThreeQuarter),
            other => Err(NotAQuarterTurn(other.to_owned())),
        }
    }
}

/// A path, as painted.
#[derive(Debug, Clone, PartialEq)]
pub struct PathObject {
    /// The path in its own coordinate space.
    pub path: BezPath,
    /// The matrix taking it to the page.
    pub matrix: Affine,
    /// How the interior is filled.
    pub fill_rule: crate::ops::FillRule,
    /// Whether the outline is stroked.
    pub stroke: bool,
}

/// One run of glyphs sharing a position and a font.
///
/// Equality compares the font by identity, as [`TextState`] does.
///
/// [`TextState`]: crate::state::TextState
#[derive(Debug, Clone)]
pub struct TextObject {
    /// The character codes, split into runs by the adjustments between them.
    pub segments: Box<[TextSegment]>,
    /// Where the run starts, in page space.
    pub position: kurbo::Point,
    /// The matrix glyphs are drawn with, translation excluded.
    pub matrix: Affine,
    /// The font and size.
    pub font: Option<(Arc<Font>, f32)>,
    /// The `/Font` resource the font came from, for a regenerated stream to
    /// name. `None` for a font loaded from an inline dictionary.
    pub font_source: Option<pdfrum_object::ObjRef>,
    /// How the glyphs are painted.
    pub render_mode: crate::ops::TextRenderMode,
    /// For a Type 3 font only: what each shown character's glyph procedure
    /// declares about itself, keyed by character code.
    ///
    /// A Type 3 glyph has no program to measure — its advance and box come
    /// from the `d0`/`d1` operator inside its content stream — so a consumer
    /// that needs either has no way to get them from the font alone. The
    /// interpreter has already opened those streams, so it records the answer
    /// here rather than making every consumer re-interpret them.
    pub type3_metrics: BTreeMap<u32, crate::type3::Type3Metrics>,
}

impl PartialEq for TextObject {
    fn eq(&self, other: &Self) -> bool {
        let same_font = match (&self.font, &other.font) {
            (Some((a, sa)), Some((b, sb))) => a.id() == b.id() && sa == sb,
            (None, None) => true,
            _ => false,
        };
        same_font
            && self.segments == other.segments
            && self.position == other.position
            && self.matrix == other.matrix
            && self.render_mode == other.render_mode
            && self.type3_metrics == other.type3_metrics
    }
}

/// One string within a text object, and the adjustment that followed it.
#[derive(Debug, Clone, PartialEq)]
pub struct TextSegment {
    /// The character codes.
    pub codes: Box<[u8]>,
    /// The adjustment following this string, in thousandths of a text-space
    /// unit. Adjacent adjustments **accumulate**, so `[(A) 5 5 (B)]` records
    /// ten.
    pub kerning: f32,
}

/// An image, decoded.
#[derive(Debug, Clone, PartialEq)]
pub struct ImageObject {
    /// The pixels, shared with the session cache.
    pub image: Arc<ImageData>,
    /// The matrix taking the unit square onto the image's place on the page.
    pub matrix: Affine,
    /// Whether the image is a stencil painted with the fill colour.
    pub is_mask: bool,
    /// The `/OC` entry from the image `XObject`'s own dictionary.
    ///
    /// An image carries optional-content membership on the `XObject` rather
    /// than through a marked-content sequence, so this is a second, separate
    /// place visibility is declared and not a cache of the first. An inline
    /// image has no dictionary of its own to declare it in.
    pub oc: Option<Arc<Dict>>,
    /// The `XObject` this image was drawn from, when it was a named resource.
    ///
    /// The pixels here are decoded, and a regenerated stream must name the
    /// *undecoded* stream a `Do` can reach — so the reference travels with the
    /// object. `None` for an inline image, which has no indirect object to
    /// name and is therefore dropped when its stream is rewritten.
    pub source: Option<pdfrum_object::ObjRef>,
}

/// A shading painted directly by `sh`.
#[derive(Debug, Clone, PartialEq)]
pub struct ShadingObject {
    /// The shading.
    pub shading: Arc<Shading>,
    /// The matrix taking it to the page.
    pub matrix: Affine,
    /// The area it paints, which the clip and, for meshes, the mesh's own
    /// extent bound.
    pub bounds: Rect,
}

/// A form `XObject`'s contents, already interpreted.
#[derive(Debug, Clone, PartialEq)]
pub struct FormObject {
    /// The objects the form produced.
    pub objects: Vec<PageObject>,
    /// The matrix taking the form's space to the page's.
    pub matrix: Affine,
    /// The form's own bounding box, when it declared one. **A missing
    /// `/BBox` means no clip at all** — the form is unbounded.
    pub bbox: Option<Rect>,
    /// The form's transparency group, when it declared one.
    pub transparency: Transparency,
    /// The `/OC` entry from the form `XObject`'s own dictionary.
    ///
    /// Like an image's, this is where a form declares optional-content
    /// membership, alongside and independently of any marked-content
    /// sequence enclosing the `Do` that drew it.
    pub oc: Option<Arc<Dict>>,
    /// The `XObject` this form was drawn from, when it was a named resource.
    ///
    /// As an image's: the objects here are already interpreted, so a
    /// regenerated stream names the stream rather than re-emitting them.
    pub source: Option<pdfrum_object::ObjRef>,
    /// Whether this form is a **live edit's** appearance — a field the user is
    /// currently typing in, rather than anything the file itself carries.
    ///
    /// It is a fact about *where the object came from*, not an instruction to
    /// a renderer, which is why a page crate can hold it: the file's own
    /// appearance streams and a form session's regenerated ones are both
    /// `false`, and only the appearance a session produces for the field it is
    /// editing is `true`.
    ///
    /// A renderer needs it because the oracle draws that one form differently
    /// from every other object on the page: the widget's editor builds its own
    /// local render options with subpixel antialiasing forced on, which no
    /// page-level flag ever clears, so the text of a live edit — and no other
    /// text — is drawn that way. The distinction has to travel with the
    /// object because by the time anything rasterizes, this form is one entry
    /// in the page's object list among all the others, and the whole page is
    /// rendered under a single set of options.
    ///
    /// Defaults to `false`, so every existing producer keeps its meaning.
    pub live_edit: bool,
}

/// One thing to paint.
///
/// Deliberately **not** `#[non_exhaustive]`: a new variant must fail every
/// match site to compile, and a downstream renderer that
/// silently ignored a new kind of page object would silently stop drawing it.
#[derive(Debug, Clone, PartialEq)]
pub enum PageObject {
    /// A filled or stroked path.
    Path(Box<Content<PathObject>>),
    /// A run of glyphs.
    Text(Box<Content<TextObject>>),
    /// An image.
    Image(Box<Content<ImageObject>>),
    /// A shading painted by `sh`.
    Shading(Box<Content<ShadingObject>>),
    /// A form `XObject`'s contents.
    Form(Box<Content<FormObject>>),
}

/// A page object with the state and marks it was created under.
#[derive(Debug, Clone, PartialEq)]
pub struct Content<T> {
    /// The object itself.
    pub object: T,
    /// The graphics state in force at its creation.
    pub state: GraphicsState,
    /// The marked-content sequence enclosing it.
    pub marks: ContentMarks,
    /// Which `/Contents` element it came from, for the editor, or `None` for
    /// an object that was created rather than parsed.
    ///
    /// `None` sorts before `Some(0)`, which is what gives a brand-new object
    /// the lowest free `/Contents` index in the regenerator's ordered walk
    /// rather than one past the end. That ordering used to be bought with a
    /// `-1` sentinel; `Option`'s own `Ord` gives it for free and makes the
    /// check something the compiler enforces.
    pub content_stream: Option<usize>,
    /// Whether the object has been changed since it was parsed, so its
    /// content stream must be written again on save
    /// (see [`PageObject::set_active`]).
    pub dirty: bool,
    /// Whether the object is painted. An inactive object keeps its place in
    /// the list but contributes nothing to a regenerated stream.
    pub active: bool,
}

/// The fields every page object carries whatever it paints.
///
/// A borrowed view rather than a shared base struct: the five variants keep
/// their own records, and this is how a function that only cares about the
/// bookkeeping reaches it without matching five times.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Common {
    pub(crate) content_stream: Option<usize>,
    pub(crate) dirty: bool,
    pub(crate) active: bool,
}

/// A mutable view of the same, so a mutation writes through one match.
pub(crate) struct CommonMut<'a> {
    pub(crate) content_stream: &'a mut Option<usize>,
    pub(crate) dirty: &'a mut bool,
    pub(crate) active: &'a mut bool,
}

impl<T> Content<T> {
    /// A newly created object, under `state` and enclosed by no marks.
    ///
    /// It arrives dirty and streamless, which is what a *created* object is:
    /// it describes no bytes yet, so the stream it lands in has to be written
    /// for it to exist at all. An object the interpreter produced from bytes
    /// is built field-by-field instead, because it is neither.
    #[must_use]
    pub fn new(object: T, state: GraphicsState) -> Self {
        Self {
            object,
            state,
            marks: ContentMarks::new(),
            content_stream: None,
            dirty: true,
            active: true,
        }
    }

    fn common(&self) -> Common {
        Common {
            content_stream: self.content_stream,
            dirty: self.dirty,
            active: self.active,
        }
    }

    fn common_mut(&mut self) -> CommonMut<'_> {
        CommonMut {
            content_stream: &mut self.content_stream,
            dirty: &mut self.dirty,
            active: &mut self.active,
        }
    }
}

impl PageObject {
    pub(crate) fn common(&self) -> Common {
        match self {
            Self::Path(c) => c.common(),
            Self::Text(c) => c.common(),
            Self::Image(c) => c.common(),
            Self::Shading(c) => c.common(),
            Self::Form(c) => c.common(),
        }
    }

    pub(crate) fn common_mut(&mut self) -> CommonMut<'_> {
        match self {
            Self::Path(c) => c.common_mut(),
            Self::Text(c) => c.common_mut(),
            Self::Image(c) => c.common_mut(),
            Self::Shading(c) => c.common_mut(),
            Self::Form(c) => c.common_mut(),
        }
    }

    /// The graphics state the object was created under.
    #[must_use]
    pub fn state(&self) -> &GraphicsState {
        match self {
            Self::Path(c) => &c.state,
            Self::Text(c) => &c.state,
            Self::Image(c) => &c.state,
            Self::Shading(c) => &c.state,
            Self::Form(c) => &c.state,
        }
    }

    /// The marks enclosing the object.
    #[must_use]
    pub fn marks(&self) -> &ContentMarks {
        match self {
            Self::Path(c) => &c.marks,
            Self::Text(c) => &c.marks,
            Self::Image(c) => &c.marks,
            Self::Shading(c) => &c.marks,
            Self::Form(c) => &c.marks,
        }
    }
}

/// An interpreted page.
#[derive(Debug, Clone, PartialEq)]
pub struct Page {
    /// The objects, in painting order.
    pub objects: Vec<PageObject>,
    /// The page's own extent.
    pub media_box: Rect,
    /// The visible region, already intersected with the media box.
    pub crop_box: Rect,
    /// How the page is displayed.
    pub rotate: Rotation,
    /// The page's transparency group. A page is **always isolated**,
    /// whatever its `/Group` says.
    pub transparency: Transparency,
    /// The page's resource dictionary, for a caller that needs to re-resolve
    /// a name.
    pub resources: Option<Dict>,
    /// `/Contents` elements that must be written again because objects were
    /// removed from them (see [`PageObject::set_active`]).
    ///
    /// Only removals need recording here: a modified or hidden object still
    /// carries its own [`Content::dirty`], but a removed one leaves nothing
    /// behind to say its stream lost something.
    pub dirty_streams: BTreeSet<Option<usize>>,
    /// The transform each `/Contents` element leaves in force at its end,
    /// keyed by element index.
    ///
    /// A content stream can leave the transform changed for the ones after it
    /// — an unbalanced `q`/`cm` is legal and common — so a stream rewritten on
    /// its own must first undo what it inherited and then restate what it
    /// passes on. Only streams that actually changed the transform have an
    /// entry.
    pub stream_ctms: BTreeMap<usize, Affine>,
}

impl Page {
    /// An empty page of the default size.
    ///
    /// A page with no `/Contents` parses **successfully with zero objects**;
    /// it is never an error.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            objects: Vec::new(),
            media_box: DEFAULT_MEDIA_BOX,
            crop_box: DEFAULT_MEDIA_BOX,
            rotate: Rotation::None,
            transparency: Transparency {
                isolated: true,
                ..Transparency::default()
            },
            resources: None,
            dirty_streams: BTreeSet::new(),
            stream_ctms: BTreeMap::new(),
        }
    }

    /// The size the page displays at, with width and height swapped for a
    /// quarter or three-quarter turn.
    #[must_use]
    pub fn display_size(&self) -> (f64, f64) {
        let (w, h) = (self.crop_box.width(), self.crop_box.height());
        match self.rotate {
            Rotation::Quarter | Rotation::ThreeQuarter => (h, w),
            _ => (w, h),
        }
    }
}

/// A page's displayed size, read from its dictionary rather than from a built
/// [`Page`].
///
/// The same crop box and the same `/Rotate` swap [`Page::display_size`]
/// applies, for a caller that has to know how large a page will draw *before*
/// building it — which is what choosing a decode target needs, since the
/// build is the thing that decodes the images.
#[must_use]
pub fn display_size_from_dict<R: Resolve>(
    dict: &Dict,
    inherited: impl Fn(&pdfrum_object::Name) -> Option<pdfrum_object::Object>,
    r: &R,
    diags: &mut Diagnostics,
) -> (f64, f64) {
    let (_, crop_box) = derive_boxes(dict, &inherited, r, diags);
    let rotate = Rotation::from_degrees(
        dict.int(crate::names::ROTATE, r)
            .or_else(|| inherited(crate::names::ROTATE).and_then(|o| o.as_int()))
            .unwrap_or(0),
    );
    let (w, h) = (crop_box.width(), crop_box.height());
    match rotate {
        Rotation::Quarter | Rotation::ThreeQuarter => (h, w),
        Rotation::None | Rotation::Half => (w, h),
    }
}

/// Derive a page's boxes from its inherited attributes.
///
/// See the module docs for the three rules. Returns `(media, crop)`.
#[must_use]
pub fn derive_boxes<R: Resolve>(
    dict: &Dict,
    inherited: impl Fn(&pdfrum_object::Name) -> Option<pdfrum_object::Object>,
    r: &R,
    diags: &mut Diagnostics,
) -> (Rect, Rect) {
    let read = |key: &pdfrum_object::Name| -> Option<Rect> {
        let obj = dict.raw(key).cloned().or_else(|| inherited(key))?;
        let resolved = obj.resolve(r).ok()?;
        let array = resolved.as_array()?;
        (array.len() == 4).then(|| normalize(array.as_rect()))
    };

    let media = match read(names::MEDIA_BOX) {
        // "Empty" is non-positive width or height, not merely absent.
        Some(rect) if rect.width() > 0.0 && rect.height() > 0.0 => rect,
        _ => {
            diags.record(Severity::Recovered, DiagKind::MediaBoxDefaulted, None);
            DEFAULT_MEDIA_BOX
        }
    };
    let crop = match read(names::CROP_BOX) {
        Some(rect) if rect.width() > 0.0 && rect.height() > 0.0 => {
            // An intersection that comes out empty is *kept* empty, giving a
            // zero-by-zero page.
            rect.intersect(media)
        }
        _ => media,
    };
    (media, crop)
}

/// Sort a rectangle's corners so its width and height are non-negative.
fn normalize(rect: Rect) -> Rect {
    Rect::new(
        rect.x0.min(rect.x1),
        rect.y0.min(rect.y1),
        rect.x0.max(rect.x1),
        rect.y0.max(rect.y1),
    )
}

#[cfg(test)]
mod tests {
    // Test fixtures quote the oracle's own vectors, compare floats exactly
    // where the behaviour being pinned is exact, and index arrays whose
    // length the fixture itself fixes.
    #![allow(
        clippy::unreadable_literal,
        clippy::float_cmp,
        clippy::indexing_slicing,
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        reason = "test fixtures quote oracle vectors verbatim and compare exactly"
    )]

    use super::{DEFAULT_MEDIA_BOX, Page, Rotation, derive_boxes};
    use pdfrum_common::{DiagKind, Diagnostics};
    use pdfrum_object::{Array, Dict, Name, NoResolve, Object};

    fn boxes(pairs: Vec<(Name, Object)>) -> (kurbo::Rect, kurbo::Rect, Diagnostics) {
        let mut diags = Diagnostics::default();
        let (m, c) = derive_boxes(&Dict::from_pairs(pairs), |_| None, &NoResolve, &mut diags);
        (m, c, diags)
    }

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Object {
        Object::Array(Array::of([x0, y0, x1, y1].map(|v| {
            #[expect(clippy::cast_possible_truncation, reason = "test fixtures are small")]
            Object::Real(v as f32)
        })))
    }

    #[test]
    fn rotation_divides_before_taking_the_remainder() {
        assert_eq!(Rotation::from_degrees(0), Rotation::None);
        assert_eq!(Rotation::from_degrees(90), Rotation::Quarter);
        assert_eq!(Rotation::from_degrees(180), Rotation::Half);
        assert_eq!(Rotation::from_degrees(270), Rotation::ThreeQuarter);
        // 45 / 90 truncates to zero, so it is no rotation at all.
        assert_eq!(Rotation::from_degrees(45), Rotation::None);
        // Negatives wrap forward.
        assert_eq!(Rotation::from_degrees(-90), Rotation::ThreeQuarter);
        // And so do multiples past a full turn.
        assert_eq!(Rotation::from_degrees(450), Rotation::Quarter);
        assert_eq!(Rotation::from_degrees(720), Rotation::None);
    }

    #[test]
    fn an_empty_media_box_becomes_us_letter() {
        // Absent.
        let (media, crop, diags) = boxes(vec![]);
        assert_eq!(media, DEFAULT_MEDIA_BOX);
        assert_eq!(crop, DEFAULT_MEDIA_BOX);
        assert!(diags.contains(&DiagKind::MediaBoxDefaulted));

        // Zero-area.
        let (media, _, _) = boxes(vec![(Name::from("MediaBox"), rect(0.0, 0.0, 0.0, 0.0))]);
        assert_eq!(media, DEFAULT_MEDIA_BOX);

        // The wrong number of elements.
        let (media, _, _) = boxes(vec![(
            Name::from("MediaBox"),
            Object::Array(Array::of([Object::Int(0), Object::Int(0)])),
        )]);
        assert_eq!(media, DEFAULT_MEDIA_BOX);
    }

    #[test]
    fn a_reversed_media_box_is_normalized() {
        let (media, _, _) = boxes(vec![(Name::from("MediaBox"), rect(100.0, 200.0, 0.0, 0.0))]);
        assert!((media.width() - 100.0).abs() < 1e-6);
        assert!((media.height() - 200.0).abs() < 1e-6);
    }

    #[test]
    fn the_crop_box_is_intersected_with_the_media_box() {
        let (_, crop, _) = boxes(vec![
            (Name::from("MediaBox"), rect(0.0, 0.0, 100.0, 100.0)),
            // Sticking out past the media box on two sides.
            (Name::from("CropBox"), rect(50.0, 50.0, 200.0, 200.0)),
        ]);
        assert!((crop.x1 - 100.0).abs() < 1e-6);
        assert!((crop.x0 - 50.0).abs() < 1e-6);
    }

    #[test]
    fn a_disjoint_crop_box_leaves_a_zero_area_page() {
        let (_, crop, _) = boxes(vec![
            (Name::from("MediaBox"), rect(0.0, 0.0, 100.0, 100.0)),
            (Name::from("CropBox"), rect(500.0, 500.0, 600.0, 600.0)),
        ]);
        assert!(crop.area() <= 0.0, "got {crop:?}");
    }

    #[test]
    fn a_missing_crop_box_is_the_media_box() {
        let (media, crop, _) = boxes(vec![(Name::from("MediaBox"), rect(0.0, 0.0, 200.0, 300.0))]);
        assert_eq!(media, crop);
    }

    #[test]
    fn a_rotated_page_swaps_its_display_size() {
        let page = Page {
            crop_box: kurbo::Rect::new(0.0, 0.0, 200.0, 100.0),
            rotate: Rotation::Quarter,
            ..Page::empty()
        };
        assert_eq!(page.display_size(), (100.0, 200.0));
        let page = Page {
            rotate: Rotation::Half,
            ..page
        };
        assert_eq!(page.display_size(), (200.0, 100.0));
    }

    #[test]
    fn an_empty_page_is_letter_sized_and_isolated() {
        let page = Page::empty();
        assert!(page.objects.is_empty());
        assert_eq!(page.media_box, DEFAULT_MEDIA_BOX);
        assert!(page.transparency.isolated);
    }
}
