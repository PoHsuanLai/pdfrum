//! The page-object graph: what the interpreter produces
//! (ISO 32000-1 §8.2, §9).
//!
//! Every object is a small record — geometry or payload, a graphics-state
//! snapshot, and the marks in force at its creation. No behaviour lives
//! inside them: `pdfrum-render` walks the graph and `pdfrum-text` reads it,
//! and neither re-derives semantics (STYLE.md §1).
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

use crate::color::ColorValue;
use crate::image::ImageData;
use crate::names;
use crate::shading::Shading;
use crate::state::{ContentMarks, GraphicsState};
use crate::transparency::Transparency;
use kurbo::{Affine, BezPath, Rect};
use pdfrum_common::{DiagKind, Diagnostics, Severity};
use pdfrum_font::Font;
use pdfrum_object::{Dict, Resolve};
use std::collections::BTreeMap;
use std::sync::Arc;

/// The default page size when `/MediaBox` is missing or empty: US Letter.
pub const DEFAULT_MEDIA_BOX: Rect = Rect::new(0.0, 0.0, 612.0, 792.0);

/// A page's rotation, in quarter turns clockwise.
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
}

/// One thing to paint.
///
/// Deliberately **not** `#[non_exhaustive]`: STYLE.md §1 wants a new variant
/// to fail every match site to compile, and a downstream renderer that
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
    /// Which `/Contents` element it came from, for the editor. `-1` when it
    /// preceded any stream, which cannot happen in practice.
    pub content_stream: i32,
}

impl PageObject {
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

    /// The colour the object paints with, stroking or filling.
    #[must_use]
    pub fn color(&self, stroking: bool) -> &ColorValue {
        let state = self.state();
        if stroking { &state.stroke } else { &state.fill }
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

/// Whether a dictionary is loosely valid as a page.
///
/// The loose form tolerates a **missing** `/Type`; the strict form requires
/// it. Either way a `/Type` that is present must resolve to the name `Page` —
/// a string `"Page"` does not count.
#[must_use]
pub fn is_valid_page_dict<R: Resolve>(dict: Option<&Dict>, strict: bool, r: &R) -> bool {
    let Some(dict) = dict else {
        return false;
    };
    let Some(kind) = dict.raw(names::TYPE) else {
        // A missing `/Type` is tolerated only by the loose form.
        return !strict;
    };
    // A reference to the name `Page` counts; a string does not.
    kind.resolve(r)
        .ok()
        .and_then(|o| o.as_name().cloned())
        .as_ref()
        .map(pdfrum_object::Name::as_bytes)
        == Some(b"Page".as_slice())
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

    use super::{DEFAULT_MEDIA_BOX, Page, Rotation, derive_boxes, is_valid_page_dict};
    use pdfrum_common::{DiagKind, Diagnostics};
    use pdfrum_object::{Array, Dict, Name, NoResolve, Object, PdfString};

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
    fn page_dict_validation_is_loose_about_a_missing_type() {
        // Null fails both.
        assert!(!is_valid_page_dict(None, false, &NoResolve));
        assert!(!is_valid_page_dict(None, true, &NoResolve));

        // A missing `/Type` is loose-valid and strict-invalid.
        let no_type = Dict::new();
        assert!(is_valid_page_dict(Some(&no_type), false, &NoResolve));
        assert!(!is_valid_page_dict(Some(&no_type), true, &NoResolve));

        // A wrong `/Type` fails both.
        let font = Dict::from_pairs([(Name::from("Type"), Object::Name(Name::from("Font")))]);
        assert!(!is_valid_page_dict(Some(&font), false, &NoResolve));

        // `/Type` as a *string* is not a name.
        let string =
            Dict::from_pairs([(Name::from("Type"), Object::Str(PdfString::literal(b"Page")))]);
        assert!(!is_valid_page_dict(Some(&string), false, &NoResolve));

        // A null `/Type` fails.
        let null = Dict::from_pairs([(Name::from("Type"), Object::Null)]);
        assert!(!is_valid_page_dict(Some(&null), false, &NoResolve));

        // The right one passes.
        let page = Dict::from_pairs([(Name::from("Type"), Object::Name(Name::from("Page")))]);
        assert!(is_valid_page_dict(Some(&page), false, &NoResolve));
        assert!(is_valid_page_dict(Some(&page), true, &NoResolve));
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
