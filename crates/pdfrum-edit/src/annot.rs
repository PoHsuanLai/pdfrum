//! Creating page annotations and attaching them to a page's `/Annots`.
//!
//! When a generator exists for the subtype (Highlight, Underline, Ink,
//! FreeText, Text, Square, …), an `/AP /N` appearance stream is written so
//! [`crate::flatten`] and viewers that require appearances can draw them.

use kurbo::{Point, Rect};
use pdfrum_common::{Diagnostics, PageIndex};
use pdfrum_doc::Subtype;
use pdfrum_object::{
    Array, ByteSpan, Dict, Name, ObjRef, Object, PdfString, Resolve, Stream, encode_text,
};
use peniko::Color;

use crate::doc::EditDoc;
use crate::error::Error;
use crate::names;

/// An annotation write either applies or names why it could not.
type Result<T> = core::result::Result<T, Error>;

/// The Print bit of `/F` (ISO 32000-1 §12.5.3): annotations appear when the
/// page is printed.
const FLAG_PRINT: i64 = 4;

/// Default `/DA` for [`AnnotSpec::FreeText`]: black Helvetica 12 pt.
///
/// Matches the string Rotero writes today.
pub const DEFAULT_DA: &str = "0 0 0 rg /Helvetica 12 Tf";

/// One quadrilateral for a text-markup annotation (`/QuadPoints`).
///
/// Eight numbers are written in **top-left, top-right, bottom-left,
/// bottom-right** order — the order Rotero writes and the order
/// [`pdfrum_doc::annot`] reads as left/bottom = bl, right/top = tr.
///
/// The read path returns axis-aligned [`Rect`]s via
/// [`pdfrum_doc::Annotation::quad_points`]; `Quad` is the write-side type for
/// the same geometry. Convert with [`Quad::from_rect`] or [`From<Rect>`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quad {
    /// Top-left corner in page space.
    pub top_left: Point,
    /// Top-right corner in page space.
    pub top_right: Point,
    /// Bottom-left corner in page space.
    pub bottom_left: Point,
    /// Bottom-right corner in page space.
    pub bottom_right: Point,
}

impl Quad {
    /// An axis-aligned quadrilateral from a rectangle: corners in tl, tr, bl,
    /// br order.
    ///
    /// ```
    /// use pdfrum_edit::Quad;
    /// use kurbo::Rect;
    ///
    /// let q = Quad::from_rect(Rect::new(10.0, 20.0, 110.0, 40.0));
    /// assert_eq!(q.top_left, kurbo::Point::new(10.0, 40.0));
    /// assert_eq!(q.bottom_right, kurbo::Point::new(110.0, 20.0));
    /// ```
    #[must_use]
    pub fn from_rect(rect: Rect) -> Self {
        let rect = rect.abs();
        Self {
            top_left: Point::new(rect.x0, rect.y1),
            top_right: Point::new(rect.x1, rect.y1),
            bottom_left: Point::new(rect.x0, rect.y0),
            bottom_right: Point::new(rect.x1, rect.y0),
        }
    }
}

impl From<Rect> for Quad {
    /// Delegates to [`Quad::from_rect`].
    ///
    /// ```
    /// use pdfrum_edit::Quad;
    /// use kurbo::Rect;
    ///
    /// let q: Quad = Rect::new(10.0, 20.0, 110.0, 40.0).into();
    /// assert_eq!(q, Quad::from_rect(Rect::new(10.0, 20.0, 110.0, 40.0)));
    /// ```
    fn from(rect: Rect) -> Self {
        Self::from_rect(rect)
    }
}

/// What kind of annotation to create and attach to a page.
///
/// This is the **write** payload for [`add_annotation`]. The ISO subtype
/// spelling itself is [`pdfrum_doc::Subtype`] (also re-exported from the
/// facade): reading an annotation yields `Subtype`, while building one takes
/// an `AnnotSpec` variant that carries the keys that subtype needs.
///
/// Each variant carries the keys Rotero's `write_annotations` needs today.
/// Appearance streams are generated when the subtype has a generator.
///
/// Prefer the associated constructors (`highlight`, `text`, …) over spelling
/// every field at the call site.
///
/// ```
/// use pdfrum::{AnnotSpec, Color, Document, Rect, SaveOptions};
///
/// let doc = Document::open("tests/fixtures/hello_world.pdf")?;
/// let mut edit = doc.edit();
/// let rect = Rect::new(72.0, 700.0, 200.0, 720.0);
/// edit.add_annotation(
///     0,
///     AnnotSpec::highlight(rect, Color::from_rgb8(255, 230, 0)).with_contents("note"),
/// )?;
/// let mut bytes = Vec::new();
/// edit.write_to(&mut bytes, &SaveOptions::default())?;
/// # Ok::<(), pdfrum::Error>(())
/// ```
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum AnnotSpec {
    /// A highlight over one or more text runs (`/Subtype /Highlight`).
    ///
    /// `/QuadPoints` is required and must hold at least one quadrilateral.
    Highlight {
        /// The annotation's `/Rect` in page space.
        rect: Rect,
        /// Annotation colour `/C` as `DeviceRGB` in 0..1.
        color: Color,
        /// Text runs covered; each becomes eight numbers in tl, tr, bl, br
        /// order.
        quads: Vec<Quad>,
        /// Optional `/Contents`.
        contents: Option<String>,
    },
    /// A sticky-note text annotation (`/Subtype /Text`).
    ///
    /// Writes `/Name /Comment` and `/Open false`.
    Text {
        /// The annotation's `/Rect` in page space.
        rect: Rect,
        /// Annotation colour `/C` as `DeviceRGB` in 0..1.
        color: Color,
        /// Optional `/Contents`.
        contents: Option<String>,
    },
    /// A square / area annotation (`/Subtype /Square`).
    ///
    /// Writes `/BS << /Type /Border /W 2 /S /S >>`.
    Square {
        /// The annotation's `/Rect` in page space.
        rect: Rect,
        /// Annotation colour `/C` as `DeviceRGB` in 0..1.
        color: Color,
        /// Optional `/Contents`.
        contents: Option<String>,
    },
    /// An underline over one or more text runs (`/Subtype /Underline`).
    ///
    /// `/QuadPoints` is required and must hold at least one quadrilateral.
    Underline {
        /// The annotation's `/Rect` in page space.
        rect: Rect,
        /// Annotation colour `/C` as `DeviceRGB` in 0..1.
        color: Color,
        /// Text runs covered; each becomes eight numbers in tl, tr, bl, br
        /// order.
        quads: Vec<Quad>,
        /// Optional `/Contents`.
        contents: Option<String>,
    },
    /// Freehand ink strokes (`/Subtype /Ink`).
    ///
    /// Writes `/InkList` as an array of strokes (each a flat array of x,y
    /// pairs) and `/BS << /W 2 /S /S >>`.
    Ink {
        /// The annotation's `/Rect` in page space.
        rect: Rect,
        /// Annotation colour `/C` as `DeviceRGB` in 0..1.
        color: Color,
        /// Strokes in page space; each stroke is a sequence of points.
        strokes: Vec<Vec<Point>>,
        /// Optional `/Contents`.
        contents: Option<String>,
    },
    /// A free-text annotation (`/Subtype /FreeText`).
    ///
    /// `/Contents` and `/DA` are both required. See [`DEFAULT_DA`] for a
    /// common appearance string.
    FreeText {
        /// The annotation's `/Rect` in page space.
        rect: Rect,
        /// Annotation colour `/C` as `DeviceRGB` in 0..1.
        color: Color,
        /// The visible text (`/Contents`).
        contents: String,
        /// Default appearance string (`/DA`), e.g. [`DEFAULT_DA`].
        da: String,
    },
}

impl AnnotSpec {
    /// A highlight covering `rect` as a single quadrilateral, with no
    /// contents.
    ///
    /// ```
    /// use pdfrum_edit::AnnotSpec;
    /// use kurbo::Rect;
    /// use peniko::Color;
    ///
    /// let spec = AnnotSpec::highlight(Rect::new(0.0, 0.0, 10.0, 2.0), Color::from_rgb8(255, 255, 0));
    /// assert!(matches!(spec, AnnotSpec::Highlight { contents: None, .. }));
    /// ```
    #[must_use]
    pub fn highlight(rect: Rect, color: Color) -> Self {
        Self::Highlight {
            rect,
            color,
            quads: vec![Quad::from(rect)],
            contents: None,
        }
    }

    /// An underline covering `rect` as a single quadrilateral, with no
    /// contents.
    #[must_use]
    pub fn underline(rect: Rect, color: Color) -> Self {
        Self::Underline {
            rect,
            color,
            quads: vec![Quad::from(rect)],
            contents: None,
        }
    }

    /// A sticky-note text annotation with no contents.
    #[must_use]
    pub fn text(rect: Rect, color: Color) -> Self {
        Self::Text {
            rect,
            color,
            contents: None,
        }
    }

    /// A square / area annotation with no contents.
    #[must_use]
    pub fn square(rect: Rect, color: Color) -> Self {
        Self::Square {
            rect,
            color,
            contents: None,
        }
    }

    /// Freehand ink with the given strokes and no contents.
    #[must_use]
    pub fn ink(rect: Rect, color: Color, strokes: Vec<Vec<Point>>) -> Self {
        Self::Ink {
            rect,
            color,
            strokes,
            contents: None,
        }
    }

    /// A free-text annotation with required contents and `/DA`.
    ///
    /// Pass [`DEFAULT_DA`] when the caller has no custom appearance string.
    #[must_use]
    pub fn free_text(
        rect: Rect,
        color: Color,
        contents: impl Into<String>,
        da: impl Into<String>,
    ) -> Self {
        Self::FreeText {
            rect,
            color,
            contents: contents.into(),
            da: da.into(),
        }
    }

    /// Sets `/Contents` on variants that take optional contents.
    ///
    /// [`AnnotSpec::FreeText`] already requires contents at construction; this
    /// leaves it unchanged.
    ///
    /// ```
    /// use pdfrum_edit::AnnotSpec;
    /// use kurbo::Rect;
    /// use peniko::Color;
    ///
    /// let spec = AnnotSpec::text(Rect::new(0.0, 0.0, 1.0, 1.0), Color::from_rgb8(255, 255, 0))
    ///     .with_contents("sticky");
    /// assert!(matches!(
    ///     spec,
    ///     AnnotSpec::Text {
    ///         contents: Some(ref c),
    ///         ..
    ///     } if c == "sticky"
    /// ));
    /// ```
    #[must_use]
    pub fn with_contents(self, contents: impl Into<String>) -> Self {
        let contents = Some(contents.into());
        match self {
            Self::Highlight {
                rect, color, quads, ..
            } => Self::Highlight {
                rect,
                color,
                quads,
                contents,
            },
            Self::Text { rect, color, .. } => Self::Text {
                rect,
                color,
                contents,
            },
            Self::Square { rect, color, .. } => Self::Square {
                rect,
                color,
                contents,
            },
            Self::Underline {
                rect, color, quads, ..
            } => Self::Underline {
                rect,
                color,
                quads,
                contents,
            },
            Self::Ink {
                rect,
                color,
                strokes,
                ..
            } => Self::Ink {
                rect,
                color,
                strokes,
                contents,
            },
            other @ Self::FreeText { .. } => other,
        }
    }
}

/// Adds an annotation described by `spec` to `page`, returning the new
/// annotation's object reference.
///
/// The annotation is written as a new indirect object (`/Type /Annot`,
/// `/Subtype`, `/Rect`, `/C`, `/F` with the Print bit) and appended to the
/// page's `/Annots`: a missing array is created; an indirect array is
/// extended in place; an inline array is rewritten on the page. `/P` is set
/// to the page object.
///
/// # Errors
///
/// - [`Error::PageIndexOutOfRange`] when `page` is outside the document.
/// - [`Error::InlinePage`] when the page has no object of its own.
/// - [`Error::EmptyQuadPoints`] when a highlight or underline has no quads.
pub fn add_annotation(
    edit: &mut EditDoc<'_>,
    page: impl Into<PageIndex>,
    spec: AnnotSpec,
) -> Result<ObjRef> {
    let page = page.into();
    let Some((page_ref, mut page_dict, _)) = edit.page_state(page)? else {
        return Err(Error::InlinePage(page));
    };
    let mut dict = build_dict(spec, page_ref)?;
    attach_appearance(edit, &mut dict);
    let annot_ref = edit.add(Object::Dict(dict));
    attach_to_page(edit, page_ref, &mut page_dict, annot_ref);
    Ok(annot_ref)
}

/// Generate `/AP /N` for `dict` when a subtype generator exists.
///
/// Uses the same appearance generators flatten consults, so a newly written
/// annotation survives flatten and viewers that require appearance streams.
fn attach_appearance(edit: &mut EditDoc<'_>, dict: &mut Dict) {
    let catalog = edit.base().catalog().unwrap_or_default();
    let mut build = pdfrum_page::BuildContext::new();
    let fonts = pdfrum_doc::ap::FormFonts::load(&catalog, edit, &mut build);
    let mut diags = Diagnostics::default();
    // Synthetic one-annot page so we can reuse the page-walk generators.
    let page = Dict::from_pairs([(
        Name::from("Annots"),
        Object::Array(Array::of([Object::Dict(dict.clone())])),
    )]);
    let overlay = pdfrum_doc::ap::generate_appearances_with_text(
        &page,
        &catalog,
        Some(&fonts),
        edit,
        &mut diags,
    );
    let Some(generated) = overlay.get(0) else {
        return;
    };
    if generated.stream.is_empty() {
        return;
    }
    if let Some(rect) = generated.rect_override {
        dict.insert(names::RECT.clone(), rect_object(rect));
    }
    let stream = Stream::new(
        pdfrum_doc::ap::stream_dict(generated),
        ByteSpan::from(generated.stream.clone()),
    );
    let ap_ref = edit.add(Object::Stream(Box::new(stream)));
    let mut ap = Dict::new();
    ap.insert(Name::from("N"), Object::Ref(ap_ref));
    dict.insert(Name::from("AP"), Object::Dict(ap));
}

/// Builds the annotation dictionary for `spec`, with `/P` naming `page_ref`.
fn build_dict(spec: AnnotSpec, page_ref: ObjRef) -> Result<Dict> {
    match spec {
        AnnotSpec::Highlight {
            rect,
            color,
            quads,
            contents,
        } => {
            if quads.is_empty() {
                return Err(Error::EmptyQuadPoints);
            }
            let mut dict = common(Subtype::Highlight, rect, color, page_ref);
            dict.insert(
                names::QUAD_POINTS.clone(),
                Object::Array(quad_points(&quads)),
            );
            insert_contents(&mut dict, contents.as_deref());
            Ok(dict)
        }
        AnnotSpec::Text {
            rect,
            color,
            contents,
        } => {
            let mut dict = common(Subtype::Text, rect, color, page_ref);
            dict.insert(names::NAME.clone(), Object::Name(names::COMMENT.clone()));
            dict.insert(names::OPEN.clone(), Object::Bool(false));
            insert_contents(&mut dict, contents.as_deref());
            Ok(dict)
        }
        AnnotSpec::Square {
            rect,
            color,
            contents,
        } => {
            let mut dict = common(Subtype::Square, rect, color, page_ref);
            dict.insert(names::BS.clone(), Object::Dict(border_style(true)));
            insert_contents(&mut dict, contents.as_deref());
            Ok(dict)
        }
        AnnotSpec::Underline {
            rect,
            color,
            quads,
            contents,
        } => {
            if quads.is_empty() {
                return Err(Error::EmptyQuadPoints);
            }
            let mut dict = common(Subtype::Underline, rect, color, page_ref);
            dict.insert(
                names::QUAD_POINTS.clone(),
                Object::Array(quad_points(&quads)),
            );
            insert_contents(&mut dict, contents.as_deref());
            Ok(dict)
        }
        AnnotSpec::Ink {
            rect,
            color,
            strokes,
            contents,
        } => {
            let mut dict = common(Subtype::Ink, rect, color, page_ref);
            dict.insert(names::INK_LIST.clone(), Object::Array(ink_list(&strokes)));
            dict.insert(names::BS.clone(), Object::Dict(border_style(false)));
            insert_contents(&mut dict, contents.as_deref());
            Ok(dict)
        }
        AnnotSpec::FreeText {
            rect,
            color,
            contents,
            da,
        } => {
            let mut dict = common(Subtype::FreeText, rect, color, page_ref);
            dict.insert(names::CONTENTS.clone(), Object::Str(pdf_string(&contents)));
            dict.insert(names::DA.clone(), Object::Str(pdf_string(&da)));
            Ok(dict)
        }
    }
}

/// `/Type /Annot`, `/Subtype`, `/Rect`, `/C`, `/F` Print, and `/P`.
///
/// Subtype spellings come from [`Subtype::as_bytes`] so they stay aligned with
/// the ISO enum in `pdfrum-doc`.
fn common(subtype: Subtype, rect: Rect, color: Color, page_ref: ObjRef) -> Dict {
    let mut dict = Dict::new();
    dict.insert(names::TYPE.clone(), Object::Name(names::ANNOT.clone()));
    dict.insert(
        names::SUBTYPE.clone(),
        Object::Name(Name::from(subtype.as_bytes())),
    );
    dict.insert(names::RECT.clone(), rect_object(rect));
    dict.insert(names::C.clone(), color_object(color));
    dict.insert(names::F.clone(), Object::Int(FLAG_PRINT));
    dict.insert(names::P.clone(), Object::Ref(page_ref));
    dict
}

/// Appends `annot_ref` to the page's `/Annots`, creating or extending as
/// needed. When `/Annots` is already an indirect array, that array is
/// replaced and the page dictionary is left alone.
fn attach_to_page(
    edit: &mut EditDoc<'_>,
    page_ref: ObjRef,
    page_dict: &mut Dict,
    annot_ref: ObjRef,
) {
    let annots_key = names::ANNOTS.clone();
    match page_dict.raw(&annots_key).cloned() {
        Some(Object::Ref(array_ref)) => {
            let mut array = edit
                .fetch(array_ref)
                .ok()
                .as_deref()
                .and_then(Object::as_array)
                .cloned()
                .unwrap_or_default();
            array.push(Object::Ref(annot_ref));
            edit.replace(array_ref, Object::Array(array));
        }
        Some(Object::Array(mut array)) => {
            array.push(Object::Ref(annot_ref));
            page_dict.insert(annots_key, Object::Array(array));
            edit.replace(page_ref, Object::Dict(page_dict.clone()));
        }
        _ => {
            page_dict.insert(
                annots_key,
                Object::Array(Array::of([Object::Ref(annot_ref)])),
            );
            edit.replace(page_ref, Object::Dict(page_dict.clone()));
        }
    }
}

fn insert_contents(dict: &mut Dict, contents: Option<&str>) {
    if let Some(text) = contents.filter(|t| !t.is_empty()) {
        dict.insert(names::CONTENTS.clone(), Object::Str(pdf_string(text)));
    }
}

/// PDF text string: `PDFDocEncoding` when it fits, otherwise UTF-16BE with a
/// byte-order mark — the same encoding [`encode_text`] produces for Info
/// strings and attachments.
fn pdf_string(text: &str) -> PdfString {
    PdfString::literal(encode_text(text))
}

fn rect_object(rect: Rect) -> Object {
    let rect = rect.abs();
    Object::Array(Array::of([
        Object::Real(as_f32(rect.x0)),
        Object::Real(as_f32(rect.y0)),
        Object::Real(as_f32(rect.x1)),
        Object::Real(as_f32(rect.y1)),
    ]))
}

fn color_object(color: Color) -> Object {
    let [r, g, b, _] = color.components;
    Object::Array(Array::of([
        Object::Real(r.clamp(0.0, 1.0)),
        Object::Real(g.clamp(0.0, 1.0)),
        Object::Real(b.clamp(0.0, 1.0)),
    ]))
}

/// Flat `/QuadPoints` array: eight numbers per quad in tl, tr, bl, br order.
fn quad_points(quads: &[Quad]) -> Array {
    let mut out = Array::new();
    for quad in quads {
        for point in [
            quad.top_left,
            quad.top_right,
            quad.bottom_left,
            quad.bottom_right,
        ] {
            out.push(Object::Real(as_f32(point.x)));
            out.push(Object::Real(as_f32(point.y)));
        }
    }
    out
}

/// `/InkList`: array of strokes, each a flat array of x,y pairs.
fn ink_list(strokes: &[Vec<Point>]) -> Array {
    let mut out = Array::new();
    for stroke in strokes {
        let mut points = Array::new();
        for point in stroke {
            points.push(Object::Real(as_f32(point.x)));
            points.push(Object::Real(as_f32(point.y)));
        }
        out.push(Object::Array(points));
    }
    out
}

/// Border style: Square gets `/Type /Border`; Ink does not (matching Rotero).
fn border_style(with_type: bool) -> Dict {
    let mut bs = Dict::new();
    if with_type {
        bs.insert(names::TYPE.clone(), Object::Name(names::BORDER.clone()));
    }
    bs.insert(names::W.clone(), Object::Real(2.0));
    bs.insert(names::S.clone(), Object::Name(names::S.clone()));
    bs
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "PDF reals are f32; page-space points fit"
)]
fn as_f32(value: f64) -> f32 {
    value as f32
}
