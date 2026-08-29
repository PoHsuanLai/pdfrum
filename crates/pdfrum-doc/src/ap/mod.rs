//! Appearance-stream generation (`cpdf_generateap`): turning an annotation's
//! dictionary into the content stream a viewer draws.
//!
//! # The overlay
//!
//! Upstream this **mutates the document**. Generating a sticky note's
//! appearance replaces its `/Rect` with a 20×20 box; generating an ink
//! annotation's inflates its `/Rect`; every annotation touched gains an
//! `/AP /N` pointing at a new stream and a marker key saying so. Everything
//! that reads the file afterwards — including the `--annot` dump the
//! conformance harness diffs byte for byte — sees the mutated state, which is
//! why the dump reports 20×20 rectangles for sticky notes whose files say
//! otherwise.
//!
//! Parsed objects here are values and the parser's store is immutable, so
//! [`generate_appearances`] returns an [`AnnotOverlay`] instead: one entry per
//! `/Annots` index recording the stream it produced and the dictionary edits
//! it implies. Readers consult the overlay before the dictionary.
//!
//! # Which annotations get one
//!
//! Ten subtypes have a generator, and a widget annotation with no `/AP`
//! dictionary gets its chrome from [`widget`] besides. Generation is refused
//! outright when the
//! annotation is hidden, or when `/AP /N` already reads as a dictionary —
//! and a **stream** answers as its own dictionary, so the ordinary "it
//! already has an appearance" case is covered by the same test. Only a
//! missing `/AP`, a missing `/N`, or a scalar `/N` leaves the door open.

pub mod border;
pub mod da;
pub mod emit;
pub mod fmt;
pub mod markup;
pub mod shapes;
pub mod widget;

use kurbo::{Affine, Rect};
use pdfrum_common::{DiagKind, Diagnostics, Severity};
use pdfrum_object::{Array, Dict, Name, Object, Resolve, names as obj_names};

use crate::annot::{Subtype, appearance, quad};
use crate::names;

/// One generated appearance and the dictionary edits it implies.
#[derive(Debug, Clone, PartialEq)]
pub struct GeneratedAp {
    /// The content-stream bytes.
    pub stream: Vec<u8>,
    /// The form `XObject`'s bounding box.
    pub bbox: Rect,
    /// Its matrix, which these generators always leave as the identity.
    pub matrix: Affine,
    /// Its resource dictionary.
    pub resources: Dict,
    /// A rewritten `/Rect`, when generation moved one.
    pub rect_override: Option<Rect>,
    /// A copied-down `/AS`, for the `/NeedAppearances` widget path.
    pub as_override: Option<Name>,
}

/// Per-annotation generated appearances, keyed by `/Annots` index.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AnnotOverlay {
    entries: Vec<Option<GeneratedAp>>,
}

impl AnnotOverlay {
    /// An overlay with room for `count` annotations and nothing generated.
    #[must_use]
    pub fn with_capacity(count: usize) -> AnnotOverlay {
        AnnotOverlay {
            entries: vec![None; count],
        }
    }

    /// Records a generated appearance at one `/Annots` index.
    pub fn set(&mut self, index: usize, generated: GeneratedAp) {
        if let Some(slot) = self.entries.get_mut(index) {
            *slot = Some(generated);
        }
    }

    /// What was generated at one `/Annots` index, if anything.
    #[must_use]
    pub fn get(&self, index: usize) -> Option<&GeneratedAp> {
        self.entries.get(index).and_then(Option::as_ref)
    }

    /// The rectangle an annotation should be read as having.
    #[must_use]
    pub fn rect(&self, index: usize, raw: Rect) -> Rect {
        self.get(index)
            .and_then(|generated| generated.rect_override)
            .unwrap_or(raw)
    }

    /// How many annotations the overlay covers.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the overlay covers no annotations at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Generates appearances for every annotation on a page that wants one.
///
/// The walk mirrors what a viewer does when it opens a page, because that
/// ordering is what the `--annot` contract describes: pop-ups written into
/// the file are skipped, everything else is offered to its generator, and the
/// results are keyed by position in `/Annots`.
#[must_use]
pub fn generate_appearances<R: Resolve>(
    page: &Dict,
    r: &R,
    diags: &mut Diagnostics,
) -> AnnotOverlay {
    let Some(annots) = page.array(obj_names::ANNOTS, r) else {
        return AnnotOverlay::default();
    };
    let mut overlay = AnnotOverlay::with_capacity(annots.len());
    for index in 0..annots.len() {
        let Some(dict) = annots.dict_at(index, r) else {
            continue;
        };
        if crate::annot::is_popup(&dict, r) {
            continue;
        }
        if let Some(generated) = generate_one(&dict, r, diags) {
            overlay.set(index, generated);
        } else if let Some(generated) = widget::generate(&dict, r) {
            // A widget with no appearance dictionary gets its chrome built
            // when the page opens, whatever the form says about regenerating
            // appearances. See `widget` for how far that goes.
            diags.record(Severity::Recovered, DiagKind::AppearanceGenerated, None);
            overlay.set(index, generated);
        }
    }
    overlay
}

/// Generates one annotation's appearance, if it should have one.
#[must_use]
pub fn generate_one<R: Resolve>(
    dict: &Dict,
    r: &R,
    diags: &mut Diagnostics,
) -> Option<GeneratedAp> {
    if !should_generate(dict, r) {
        return None;
    }
    let subtype = Subtype::from_bytes(&dict.byte_string(obj_names::SUBTYPE, r).unwrap_or_default());
    let generated = match subtype {
        Subtype::Circle => markup::circle(dict, r),
        Subtype::Highlight => markup::highlight(dict, r),
        Subtype::Ink => markup::ink(dict, r)?,
        Subtype::Square => markup::square(dict, r),
        Subtype::Squiggly => markup::squiggly(dict, r),
        Subtype::StrikeOut => markup::strike_out(dict, r),
        Subtype::Text => markup::text(dict, r),
        Subtype::Underline => markup::underline(dict, r),
        // Everything else has no generator. The two text-bearing subtypes —
        // free text and pop-ups — do have one upstream, but it needs the
        // layout engine and is built on top of this dispatch rather than
        // inside it, so they answer the same way here.
        _ => return None,
    };
    diags.record(Severity::Recovered, DiagKind::AppearanceGenerated, None);

    // The bounding box is the annotation's rectangle **as it stands after
    // this generator ran** — so a sticky note's is the 20×20 box it just
    // produced, not the one the file declared.
    let rect = generated
        .rect_override
        .unwrap_or_else(|| dict.rect(obj_names::RECT, r));
    let bbox = if generated.is_text_markup {
        quad::bounding_rect_from_quad_points(dict.array(names::QUAD_POINTS, r).as_ref())
    } else {
        rect
    };
    Some(GeneratedAp {
        stream: generated.stream,
        bbox,
        matrix: Affine::IDENTITY,
        resources: resources_dict(ext_gstate_dict(dict, generated.blend_multiply, r), None),
        rect_override: generated.rect_override,
        as_override: None,
    })
}

/// Whether an annotation is eligible for a generated appearance.
///
/// Two gates. A **dictionary-valued** `/AP /N` suppresses generation, and a
/// stream answers as its own dictionary — so the common "it already has an
/// appearance" case and the multi-state checkbox case are the same test. And
/// a hidden annotation never generates.
#[must_use]
pub fn should_generate<R: Resolve>(dict: &Dict, r: &R) -> bool {
    if appearance::has_appearance(dict, r) {
        return false;
    }
    let flags = crate::annot::AnnotFlags(dict.int(names::F, r).unwrap_or(0));
    !flags.is_hidden()
}

/// The graphics-state dictionary a generated appearance names.
///
/// Both alphas take the annotation's `/CA` when the key is present, whatever
/// its type reads as, and one otherwise. Only the highlight generator asks
/// for a blend mode other than normal.
#[must_use]
pub fn ext_gstate_dict<R: Resolve>(dict: &Dict, multiply: bool, r: &R) -> Dict {
    let opacity = if dict.contains_key(names::CA) {
        dict.number(names::CA, r).unwrap_or(0.0)
    } else {
        1.0
    };
    let blend = if multiply {
        names::MULTIPLY_BLEND
    } else {
        obj_names::NORMAL
    };
    let state = Dict::from_pairs([
        (
            obj_names::TYPE.clone(),
            Object::Name(names::EXT_GSTATE.clone()),
        ),
        (names::CA.clone(), Object::Real(opacity)),
        (names::CA_LOWER.clone(), Object::Real(opacity)),
        (names::AIS.clone(), Object::Bool(false)),
        (names::BM.clone(), Object::Name(blend.clone())),
    ]);
    Dict::from_pairs([(names::GS.clone(), Object::Dict(state))])
}

/// The appearance stream's `/Resources`, omitting either half when absent.
#[must_use]
pub fn resources_dict(ext_gstate: Dict, font: Option<Dict>) -> Dict {
    let mut resources = Dict::new();
    resources.push(names::EXT_GSTATE.clone(), Object::Dict(ext_gstate));
    if let Some(font) = font {
        resources.push(names::FONT.clone(), Object::Dict(font));
    }
    resources
}

/// The stream dictionary a generated appearance is stored under.
#[must_use]
pub fn stream_dict(generated: &GeneratedAp) -> Dict {
    Dict::from_pairs([
        (names::FORM_TYPE.clone(), Object::Int(1)),
        (
            obj_names::TYPE.clone(),
            Object::Name(names::XOBJECT.clone()),
        ),
        (
            obj_names::SUBTYPE.clone(),
            Object::Name(names::FORM.clone()),
        ),
        (
            names::MATRIX.clone(),
            Object::Array(matrix_array(generated.matrix)),
        ),
        (
            names::BBOX.clone(),
            Object::Array(rect_array(generated.bbox)),
        ),
        (
            names::RESOURCES.clone(),
            Object::Dict(generated.resources.clone()),
        ),
        (
            names::LENGTH.clone(),
            Object::Int(i64::try_from(generated.stream.len()).unwrap_or(0)),
        ),
    ])
}

/// A transform as its six numbers.
///
/// Narrowed to single precision because that is what a PDF real is; the
/// transforms these generators write are all exactly representable anyway.
#[allow(clippy::cast_possible_truncation)]
fn matrix_array(matrix: Affine) -> Array {
    Array::of(
        matrix
            .as_coeffs()
            .into_iter()
            .map(|value| Object::Real(value as f32)),
    )
}

/// A rectangle as its four corner numbers, in PDF's ordering.
fn rect_array(rect: Rect) -> Array {
    use crate::geom;
    Array::of(
        [
            geom::left(rect),
            geom::bottom(rect),
            geom::right(rect),
            geom::top(rect),
        ]
        .map(Object::Real),
    )
}

#[cfg(test)]
mod tests {
    use super::{ext_gstate_dict, generate_appearances, generate_one, should_generate};
    use crate::geom;
    use pdfrum_common::Diagnostics;
    use pdfrum_object::{Array, ByteSpan, Dict, Name, NoResolve, Object, Stream};

    fn dict(pairs: &[(&str, Object)]) -> Dict {
        Dict::from_pairs(
            pairs
                .iter()
                .map(|(k, v)| (Name::from(*k), v.clone()))
                .collect::<Vec<_>>(),
        )
    }

    fn numbers(values: &[f32]) -> Object {
        Object::Array(Array::of(values.iter().copied().map(Object::from)))
    }

    fn sticky_note() -> Dict {
        dict(&[
            ("Subtype", Object::Name(Name::from("Text"))),
            ("Rect", numbers(&[10.0, 20.0, 200.0, 300.0])),
        ])
    }

    #[test]
    fn an_annotation_with_an_appearance_stream_generates_nothing() {
        let with_ap = dict(&[
            ("Subtype", Object::Name(Name::from("Text"))),
            (
                "AP",
                Object::Dict(dict(&[(
                    "N",
                    Object::Stream(Stream::new(Dict::new(), ByteSpan::from(b"x".to_vec()))),
                )])),
            ),
        ]);
        assert!(!should_generate(&with_ap, &NoResolve));
        assert!(should_generate(&sticky_note(), &NoResolve));
    }

    #[test]
    fn a_hidden_annotation_never_generates() {
        let mut hidden = sticky_note();
        hidden.push(Name::from("F"), Object::Int(2));
        assert!(!should_generate(&hidden, &NoResolve));
    }

    #[test]
    fn a_sticky_notes_rectangle_override_reaches_the_overlay() {
        let page = dict(&[(
            "Annots",
            Object::Array(Array::of([Object::Dict(sticky_note())])),
        )]);
        let mut diags = Diagnostics::default();
        let overlay = generate_appearances(&page, &NoResolve, &mut diags);
        assert_eq!(overlay.len(), 1);
        let raw = geom::rect(10.0, 20.0, 200.0, 300.0);
        assert_eq!(overlay.rect(0, raw), geom::rect(10.0, 20.0, 30.0, 40.0));
        // The bounding box follows the rewritten rectangle, not the file's.
        assert_eq!(
            overlay.get(0).map(|generated| generated.bbox),
            Some(geom::rect(10.0, 20.0, 30.0, 40.0))
        );
    }

    #[test]
    fn a_pop_up_written_into_the_file_is_skipped_by_the_walk() {
        let page = dict(&[(
            "Annots",
            Object::Array(Array::of([Object::Dict(dict(&[(
                "Subtype",
                Object::Name(Name::from("Popup")),
            )]))])),
        )]);
        let mut diags = Diagnostics::default();
        let overlay = generate_appearances(&page, &NoResolve, &mut diags);
        // The slot still exists — indices stay aligned with `/Annots` — but
        // nothing was generated into it.
        assert_eq!(overlay.len(), 1);
        assert!(overlay.get(0).is_none());
    }

    #[test]
    fn a_subtype_with_no_generator_produces_nothing() {
        let stamp = dict(&[("Subtype", Object::Name(Name::from("Stamp")))]);
        let mut diags = Diagnostics::default();
        assert!(generate_one(&stamp, &NoResolve, &mut diags).is_none());
    }

    #[test]
    fn a_text_markup_bounding_box_comes_from_the_quadrilaterals() {
        let highlight = dict(&[
            ("Subtype", Object::Name(Name::from("Highlight"))),
            ("Rect", numbers(&[0.0, 0.0, 5.0, 5.0])),
            (
                "QuadPoints",
                numbers(&[10.0, 20.0, 30.0, 20.0, 10.0, 10.0, 30.0, 10.0]),
            ),
        ]);
        let mut diags = Diagnostics::default();
        let got = generate_one(&highlight, &NoResolve, &mut diags).expect("generates");
        assert_eq!(got.bbox, geom::rect(10.0, 10.0, 30.0, 20.0));
    }

    #[test]
    fn the_graphics_state_takes_its_alpha_from_the_opacity_key() {
        let opaque = ext_gstate_dict(&Dict::new(), false, &NoResolve);
        let state = opaque
            .dict(&Name::from("GS"), &NoResolve)
            .expect("one entry");
        assert_eq!(state.number(&Name::from("CA"), &NoResolve), Some(1.0));
        assert_eq!(
            state.name(&Name::from("BM")).map(Name::as_bytes),
            Some(&b"Normal"[..])
        );

        let half = dict(&[("CA", Object::from(0.5_f32))]);
        let state = ext_gstate_dict(&half, true, &NoResolve)
            .dict(&Name::from("GS"), &NoResolve)
            .expect("one entry");
        assert_eq!(state.number(&Name::from("ca"), &NoResolve), Some(0.5));
        assert_eq!(
            state.name(&Name::from("BM")).map(Name::as_bytes),
            Some(&b"Multiply"[..])
        );
    }
}
