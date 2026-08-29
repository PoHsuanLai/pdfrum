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
//! dictionary gets its chrome from [`widget`] besides — plus, when the caller
//! has fonts to set text with, the field body [`field_body`] lays out.
//! Generation is refused
//! outright when the
//! annotation is hidden, or when `/AP /N` already reads as a dictionary —
//! and a **stream** answers as its own dictionary, so the ordinary "it
//! already has an appearance" case is covered by the same test. Only a
//! missing `/AP`, a missing `/N`, or a scalar `/N` leaves the door open.

pub mod border;
pub mod da;
pub mod emit;
pub mod field_body;
pub mod fmt;
pub mod freetext;
pub mod markup;
pub mod shapes;
pub mod widget;

use kurbo::{Affine, Rect};
use pdfrum_common::{DiagKind, Diagnostics, Severity};
use pdfrum_object::{Array, Dict, Name, Object, Resolve, names as obj_names};

use crate::annot::{Subtype, appearance, quad};
use crate::names;
use crate::vt;

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

/// The font a text-bearing generator sets its text with.
///
/// Threaded in rather than loaded here, because loading one needs a font
/// cache the caller already owns, and because the layout engine is a pure
/// function of these numbers — which is what lets it be tested against a stub.
pub struct TextFont<'a> {
    /// The loaded font.
    pub font: &'a pdfrum_font::Font,
    /// Metrics derived from it, for the layout engine.
    pub metrics: vt::Metrics<'a>,
}

impl std::fmt::Debug for TextFont<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextFont")
            .field("metrics", &self.metrics)
            .finish_non_exhaustive()
    }
}

/// The character code a code point the face cannot map is written as.
///
/// A simple font's codes are one byte and `Font::append_char` truncates to
/// one, so the code the stream ends up carrying is the code point's **low
/// byte** — and the width has to be looked up under that same byte or the
/// layout advances by a glyph the stream does not name. A composite font
/// keeps the whole value, because its CMap decides the width itself.
fn unmapped_code(font: &pdfrum_font::Font, code: u32) -> pdfrum_font::CharCode {
    match font {
        pdfrum_font::Font::Type0(_) => pdfrum_font::CharCode(code),
        pdfrum_font::Font::Simple(_) | pdfrum_font::Font::Type3(_) => {
            pdfrum_font::CharCode(code & 0xff)
        }
    }
}

impl TextFont<'_> {
    /// How one code point is written into a content stream.
    ///
    /// A `Symbol` or `ZapfDingbats` font takes the code point's **low byte**
    /// verbatim, relying on the font's built-in encoding: there is no
    /// named-glyph table and no `/Encoding` consultation anywhere in this
    /// path. Anything else goes through the reverse `ToUnicode` mapping.
    ///
    /// **A code point the font cannot represent is still written**, as its own
    /// value taken for a character code. The glyph that draws is whatever that
    /// code happens to name in the chosen face and is usually wrong — but the
    /// text object exists, occupies the layout, and is what a reader sees.
    /// Dropping the character instead loses the object entirely, which on
    /// `bug_725389` — three Hebrew characters in a `/DA` naming Times-Roman —
    /// is the difference between six text objects and three.
    ///
    /// Upstream reaches the same place by a longer road: `CPDF_BAFontMap`
    /// would first look for a second face that knows the character, and only
    /// `CPWL_EditImpl::GetPDFWordString`'s fallthrough appends the raw value
    /// when none does. On a hermetic font set no second face is found, so the
    /// fallthrough is the whole of the observable behaviour, and the N-slot map
    /// is not built here for a result it does not change.
    #[must_use]
    pub fn encode(&self, code: u32) -> Vec<u8> {
        let name = self.font.base_font_name();
        if name == b"Symbol" || name == b"ZapfDingbats" {
            #[allow(clippy::cast_possible_truncation)]
            return vec![code as u8];
        }
        let mut out = Vec::new();
        let mapped = char::from_u32(code)
            .and_then(|ch| self.font.char_code_from_unicode(ch))
            .unwrap_or_else(|| unmapped_code(self.font, code));
        self.font.append_char(&mut out, mapped);
        out
    }

    /// One code point's width, in thousandths of an em.
    ///
    /// The width is the one the face gives whatever [`Self::encode`] wrote, so
    /// an unrepresentable code point measures the glyph its raw value names
    /// rather than nothing — the two have to agree or the layout advances past
    /// characters the stream still contains, and the line comes out the wrong
    /// length.
    ///
    /// A free function rather than a method because [`Self::metrics_of`] wants
    /// it as a `&dyn Fn` borrowed for the same lifetime as the font, which a
    /// closure over `self` cannot supply before `self` exists.
    #[must_use]
    pub fn char_width(font: &pdfrum_font::Font, code: u32) -> i32 {
        let charcode = char::from_u32(code)
            .and_then(|ch| font.char_code_from_unicode(ch))
            .unwrap_or_else(|| unmapped_code(font, code));
        #[allow(clippy::cast_possible_truncation)]
        {
            font.char_width(charcode) as i32
        }
    }

    /// The layout metrics a loaded font supplies.
    #[must_use]
    pub fn metrics_of<'a>(
        font: &'a pdfrum_font::Font,
        width: &'a dyn Fn(u32) -> i32,
    ) -> vt::Metrics<'a> {
        vt::Metrics {
            width,
            ascent: font.type_ascent(),
            descent: font.type_descent(),
        }
    }
}

/// The faces a form's default resources name, loaded once for a page.
///
/// # Why the fonts are loaded rather than substituted for
///
/// A generator wants *metrics*, and it was tempting to hand every generator
/// one stock Helvetica on the reasoning that a non-embedded `/DA` font
/// substitutes to that face anyway. The metrics do not agree with that
/// reasoning, and the disagreement is visible: an ascent and descent taken
/// from the base-14 metric tables are 718 and −219, while the ones taken from
/// the **substituted face** — the size the layout engine actually stacks lines
/// by — are the face's own, and for the hermetic corpus's metric-compatible
/// Helvetica that is 905 and −211. On a list box the difference is the row
/// pitch: 11.24 units per row against 13.39, which is two extra rows in a
/// thirty-unit box.
///
/// So the font a widget's `/DA` names is loaded from the form's `/DR /Font`,
/// through the same loader and the same substitution options every other font
/// on the page goes through. A name the resources do not carry gets a stock
/// Helvetica, which is what the fallback is actually for.
pub struct FormFonts {
    /// Resource name and the face loaded under it, in `/DR /Font` order with
    /// the fallback last.
    entries: Vec<(Name, pdfrum_font::Font)>,
}

impl std::fmt::Debug for FormFonts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FormFonts")
            .field(
                "names",
                &self.entries.iter().map(|(n, _)| n).collect::<Vec<_>>(),
            )
            .finish_non_exhaustive()
    }
}

impl FormFonts {
    /// Loads every font a document's interactive form declares, plus the
    /// fallback a name outside them resolves to.
    ///
    /// The fallback is loaded unconditionally and stored under an empty name,
    /// so a `/DA` naming nothing — or naming a font the resources lack — still
    /// has a face to measure with. That is the same substitution a viewer
    /// performs; it is only the *metric source* that this fixes.
    #[must_use]
    pub fn load<R: Resolve>(
        catalog: &Dict,
        r: &R,
        ctx: &mut pdfrum_page::BuildContext,
    ) -> FormFonts {
        let (limits, mut diags) = (
            pdfrum_common::Limits::default(),
            pdfrum_common::Diagnostics::default(),
        );
        let mut load = |dict: &Dict| {
            pdfrum_font::load_with_options(
                dict,
                r,
                &ctx.fonts,
                &ctx.substitution,
                &limits,
                &mut diags,
            )
        };

        let mut entries = Vec::new();
        let fonts = catalog
            .dict(names::ACRO_FORM, r)
            .and_then(|form| form.dict(names::DR, r))
            .and_then(|resources| resources.dict(names::FONT, r));
        if let Some(fonts) = fonts {
            for key in fonts.keys() {
                let Some(dict) = fonts.dict(key, r) else {
                    continue;
                };
                if let Some(font) = load(&dict) {
                    entries.push((key.clone(), font));
                }
            }
        }
        // The fallback goes through the **same loader**, not the stock-metrics
        // constructor: the point of this type is that the ascent and descent
        // come from the face that is actually substituted, and a font built
        // from the base-14 tables would answer 718 and −219 where the face
        // answers its own. A field with no `/DR` at all is exactly where that
        // shows, because there is nothing else for it to measure with.
        entries.extend(load(&freetext::fallback_font()).map(|font| (Name::new(Vec::new()), font)));
        FormFonts { entries }
    }

    /// The face filed under one resource name, or the fallback.
    ///
    /// Answers nothing only if the fallback itself is missing, which
    /// [`Self::load`] makes impossible — the caller then generates chrome
    /// alone rather than being told a face exists that does not.
    #[must_use]
    pub fn face(&self, name: &[u8]) -> Option<&pdfrum_font::Font> {
        self.entries
            .iter()
            .find(|(key, _)| key.as_bytes() == name)
            .or_else(|| self.entries.last())
            .map(|(_, font)| font)
    }

    /// A [`TextFont`] over one resource name, with `width` borrowed for the
    /// same lifetime.
    ///
    /// The width closure cannot live inside the returned value — it has to be
    /// borrowed for the font's lifetime, which a closure over `self` cannot
    /// supply before `self` exists — so the caller keeps it and passes it in,
    /// the same shape [`TextFont::metrics_of`] already has.
    #[must_use]
    pub fn text_font<'a>(
        &'a self,
        name: &[u8],
        width: &'a dyn Fn(u32) -> i32,
    ) -> Option<TextFont<'a>> {
        let font = self.face(name)?;
        Some(TextFont {
            metrics: TextFont::metrics_of(font, width),
            font,
        })
    }
}

/// Generates appearances for every annotation on a page that wants one.
///
/// The walk mirrors what a viewer does when it opens a page, because that
/// ordering is what the `--annot` contract describes: pop-ups written into
/// the file are skipped, everything else is offered to its generator, and the
/// results are keyed by position in `/Annots`.
///
/// The text-bearing generators are skipped here; [`generate_appearances_with_text`]
/// is the walk that enables them.
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

/// The same walk, with the text-bearing generators enabled.
///
/// The generators only produce an appearance when a font is in hand, so a
/// caller without one gets the same result as [`generate_appearances`].
///
/// Each annotation is measured with the face **its own** `/DA` names, looked
/// up in the form's default resources — not with one page-wide font. A page
/// whose fields name two different faces stacks their lines by two different
/// ascents, which is what a viewer does.
#[must_use]
pub fn generate_appearances_with_text<R: Resolve>(
    page: &Dict,
    catalog: &Dict,
    fonts: Option<&FormFonts>,
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
        // The width closure has to outlive the `TextFont` that borrows it, so
        // it is built here rather than inside the lookup.
        let named = fonts.and_then(|fonts| fonts.face(&font_name_of(&dict, catalog, r)));
        let width = named.map(|font| move |code: u32| TextFont::char_width(font, code));
        let text_font = named.zip(width.as_ref()).map(|(font, width)| TextFont {
            metrics: TextFont::metrics_of(font, width),
            font,
        });
        let generated = generate_one(&dict, r, diags)
            .or_else(|| generate_text_bearing(&dict, catalog, text_font.as_ref(), r, diags))
            .or_else(|| {
                // A widget's own body needs the same font the free-text
                // generator wanted, so a caller with one gets the field's
                // value laid out and a caller without one gets the chrome
                // alone.
                match text_font.as_ref() {
                    Some(font) => widget::generate_with_text(&dict, catalog, font, r),
                    None => widget::generate(&dict, r),
                }
                .inspect(|_| {
                    diags.record(Severity::Recovered, DiagKind::AppearanceGenerated, None);
                })
            });
        if let Some(generated) = generated {
            overlay.set(index, generated);
        }
    }
    overlay
}

/// The `/DR /Font` resource name one annotation's default appearance names.
///
/// Falls back to the form's own `/DA`, then to nothing — and nothing resolves
/// to [`FormFonts`]'s fallback face rather than declining.
fn font_name_of<R: Resolve>(dict: &Dict, catalog: &Dict, r: &R) -> Vec<u8> {
    let form = catalog.dict(names::ACRO_FORM, r).unwrap_or_default();
    freetext::default_appearance(dict, &form, r)
        .map(|appearance| appearance.font_name)
        .unwrap_or_default()
}

/// The free-text generator, when its preconditions and a font allow.
fn generate_text_bearing<R: Resolve>(
    dict: &Dict,
    catalog: &Dict,
    text_font: Option<&TextFont<'_>>,
    r: &R,
    diags: &mut Diagnostics,
) -> Option<GeneratedAp> {
    if !should_generate(dict, r) {
        return None;
    }
    let subtype = Subtype::from_bytes(&dict.byte_string(obj_names::SUBTYPE, r).unwrap_or_default());
    if subtype != Subtype::FreeText {
        return None;
    }
    let font = text_font?;
    let generated =
        freetext::free_text(dict, catalog, r, &font.metrics, &|code| font.encode(code))?;
    diags.record(Severity::Recovered, DiagKind::AppearanceGenerated, None);
    Some(GeneratedAp {
        stream: generated.stream,
        bbox: dict.rect(obj_names::RECT, r),
        matrix: Affine::IDENTITY,
        resources: resources_dict(
            ext_gstate_dict(dict, false, r),
            generated.font_resources.clone(),
        ),
        rect_override: None,
        as_override: None,
    })
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
        resources: resources_dict(
            ext_gstate_dict(dict, generated.blend_multiply, r),
            generated.font_resources.clone(),
        ),
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
    fn a_character_the_face_cannot_map_is_still_written() {
        // `bug_725389` shows three Hebrew characters through a `/DA` naming
        // Times-Roman, which has no glyph for any of them. Dropping them loses
        // the text objects entirely — six become three — where the oracle
        // writes the raw code point as a character code and draws whatever it
        // names. Wrong glyph, right object count, right layout.
        let font = pdfrum_font::Font::load_standard(
            pdfrum_font::StandardFont::Times,
            &pdfrum_font::FontCache::new(),
        );
        let width = |code: u32| super::TextFont::char_width(&font, code);
        let text = super::TextFont {
            metrics: super::TextFont::metrics_of(&font, &width),
            font: &font,
        };
        // Hebrew bet, which no standard Latin face encodes.
        assert_eq!(text.encode(0x05D1), vec![0xD1]);
        // And a character it does encode still round-trips through the
        // `ToUnicode` mapping rather than through the fallthrough.
        assert_eq!(text.encode(u32::from('A')), vec![b'A']);
        // The width follows whatever `encode` wrote, so the layout advances by
        // the same glyph the stream names.
        assert_eq!(
            super::TextFont::char_width(&font, 0x05D1),
            super::TextFont::char_width(&font, 0xD1)
        );
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
