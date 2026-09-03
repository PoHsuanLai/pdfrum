//! The free-text annotation generator: the one per-subtype generator that
//! sets actual text.
//!
//! It is also the only one that writes through the **shortest** float writer
//! rather than the six-digit one every other per-subtype generator uses, and
//! the only one with preconditions that can decline outright: it needs a
//! default-appearance string, a `/DR` on the form, and a `/DR /Font` that is
//! a dictionary of font dictionaries. Any of those missing and no appearance
//! is produced at all — which is visible in the dump, because the two colour
//! lines then report a colour instead of failing.

use pdfrum_common::{DiagKind, Diagnostics, Severity};
use pdfrum_object::{Dict, Object, PdfString, Resolve, names as obj_names};

use crate::ap::border;
use crate::ap::da;
use crate::ap::emit::{Content, Float, PaintOp, color_op};
#[cfg(test)]
use crate::ap::fmt;
use crate::ap::markup::Generated;
use crate::color::Color;
use crate::geom;
use crate::names;
use crate::vt;

/// A `FreeText` annotation's appearance.
///
/// Returns nothing when a precondition fails, which is not the same as
/// returning an empty appearance: nothing means the annotation keeps having
/// none at all.
#[must_use]
pub(crate) fn free_text<R: Resolve>(
    dict: &Dict,
    catalog: &Dict,
    r: &R,
    metrics: &vt::Metrics<'_>,
    encode: &dyn Fn(u32) -> Vec<u8>,
    diags: &mut Diagnostics,
) -> Option<Generated> {
    // A document with no interactive form gets one **created** for it here —
    // a real mutation upstream, and the reason a free-text annotation in a
    // file that has never heard of forms still gets an appearance. We build
    // the same dictionary as a value instead.
    let form = match catalog.dict(names::ACRO_FORM, r) {
        Some(form) => form,
        None => synthesized_form(),
    };
    let appearance = default_appearance(dict, &form, r)?;
    // A `/DA` that parsed but named no font: upstream returns a
    // default-constructed `FontNameAndSize` from `GetFont`
    // (`cpdf_defaultappearance.cpp:95-97`) rather than failing, so the size is
    // zero and the name is empty and the text cannot be placed.
    if appearance.font_name.is_empty() {
        diags.record(
            Severity::Suspicious,
            DiagKind::DefaultAppearanceMalformed,
            None,
        );
    }
    let resources = form.dict(names::DR, r)?;
    let fonts = resources.dict(names::FONT, r)?;
    if !valid_font_resources(&fonts, r) {
        // `/DR /Font` is not a dictionary of font dictionaries, so no font can
        // be resolved and no appearance is generated at all
        // (`cpdf_generateap.cpp:1071-1074`).
        diags.record(Severity::Suspicious, DiagKind::FormResourcesInvalid, None);
        return None;
    }
    // The appearance stream names this font in a `Tf`, so the stream's own
    // `/Resources /Font` has to carry it or the operator resolves to nothing
    // and the text does not draw at all. Upstream builds exactly this
    // one-entry dictionary — `GenerateResourceFontDict(doc, font_name,
    // font_dict->GetObjNum())` at `cpdf_generateap.cpp:1141-1142` — from the
    // font `GetFontFromDrFontDictOrGenerateFallback` returned, which is the
    // `/DR /Font` entry under that name or, when there is none, a fresh
    // Helvetica that it also writes back into `/DR /Font`.
    let name = pdfrum_object::Name::new(appearance.font_name.clone());
    let font = fonts.dict(&name, r).unwrap_or_else(fallback_font);
    let font_resources = Dict::from_pairs([(name, Object::Dict(font))]);

    let mut out = Content::new();
    out.raw("/GS gs ");

    let info = border::border_style_info(dict.dict(names::BS, r).as_ref(), r);
    let rect = dict.rect(obj_names::RECT, r);
    let half = info.width / 2.0;
    let background = geom::deflate(rect, half, half);
    let body = geom::deflate(background, half, half);

    // A present `/C` paints the background, whatever it names — including an
    // array that names no colour at all, which paints nothing but still
    // writes the surrounding save and restore.
    if let Some(array) = dict.array(names::C, r) {
        out.raw("q\n");
        out.raw(&color_op(Color::from_array(&array), PaintOp::Fill));
        out.rect(background, Float::Shortest);
        out.raw("re f\nQ\n");
    }

    // The border takes the **text** colour, not `/C`.
    let border_path = border::border_path(rect, info, appearance.color);
    if !border_path.is_empty() {
        out.raw("q\n");
        out.raw(&border_path);
        out.raw("Q\n");
    }

    let config = vt::Config {
        plate: body,
        alignment: vt::Alignment::from_quadding(dict.int(names::Q, r).unwrap_or(0)),
        // A size of zero asks for automatic sizing; a negative one is used
        // as written and then suppresses the font operator.
        font_size: appearance.size,
        ..vt::Config::default()
    };
    let text = dict.text(obj_names::CONTENTS, r).unwrap_or_default();
    let layout = vt::layout(&text, &config, metrics);
    let content = layout.content_rect_pdf(body);
    let offset = (0.0, (geom::height(content) - geom::height(body)) / 2.0);

    let written = vt::edit_ap::generate(
        &layout,
        &config,
        metrics,
        offset,
        vt::edit_ap::Grouping::Continuous,
        |code| vt::edit_ap::Face::single(&appearance.font_name, encode(code)),
    );
    if !written.is_empty() {
        out.raw("/Tx BMC\nq\n");
        // The clip is written **only on overflow**, which is why a short
        // free-text annotation has no `W n` in its stream at all.
        if geom::width(content) > geom::width(body) || geom::height(content) > geom::height(body) {
            out.rect(body, Float::Shortest);
            out.raw("re\nW\nn\n");
        }
        out.raw("BT\n");
        out.raw(&color_op(appearance.color, PaintOp::Fill));
        out.raw(&written);
        out.raw("ET\n");
        out.raw("Q\nEMC\n");
    }

    Some(Generated {
        stream: out.into_bytes(),
        rect_override: None,
        is_text_markup: false,
        blend_multiply: false,
        font_resources: Some(font_resources),
    })
}

/// The resource name and size a `Tf` names, and the colour a `g`/`rg`/`k`
/// does.
#[derive(Debug, Clone, PartialEq)]
pub struct Appearance {
    /// The font resource name.
    pub font_name: Vec<u8>,
    /// The size, zero meaning "choose one".
    pub size: f32,
    /// The text colour, transparent when the string names none.
    pub color: Color,
}

/// Reads the default appearance an annotation is set with.
///
/// The string comes from the annotation's own inherited `/DA`, falling back
/// to the form's. Returns nothing only when neither has one — a string that
/// parses to no font still answers, with an empty name and a size of zero.
#[must_use]
pub fn default_appearance<R: Resolve>(dict: &Dict, form: &Dict, r: &R) -> Option<Appearance> {
    let (limits, mut diags) = (
        pdfrum_common::Limits::default(),
        pdfrum_common::Diagnostics::default(),
    );
    let mut string = crate::form::attr::field_attr(dict, names::DA, r, &limits, &mut diags)
        .map(|value| value.to_byte_string())
        .unwrap_or_default();
    if string.is_empty() {
        string = form.byte_string(names::DA, r).unwrap_or_default();
    }
    let font = da::font(&string)?;
    Some(Appearance {
        font_name: font.name,
        size: font.size,
        // A string with no colour operator leaves the text transparent, which
        // writes no colour operator and lets it inherit.
        color: da::color(&string).unwrap_or(Color::Transparent),
    })
}

/// Whether a `/DR /Font` is a dictionary of font dictionaries.
///
/// **An empty one passes**, vacuously; only a present entry that is not a
/// `/Type /Font` dictionary fails. The name is checked as a name, so a string
/// `/Type (Font)` is rejected.
#[must_use]
pub(crate) fn valid_font_resources<R: Resolve>(fonts: &Dict, r: &R) -> bool {
    fonts.keys().all(|key| {
        fonts
            .dict(key, r)
            .is_some_and(|font| font.name(obj_names::TYPE) == Some(names::FONT))
    })
}

/// The interactive form a document without one is given.
///
/// One stock Helvetica under a four-character alias — the resource-name
/// generator takes the first four characters of the base font name — and a
/// default appearance naming it at size 12 in black.
#[must_use]
pub(crate) fn synthesized_form() -> Dict {
    let alias = pdfrum_object::Name::new(font_resource_alias(b"Helvetica"));
    let mut da = Vec::from(b"/".as_slice());
    da.extend_from_slice(alias.as_bytes());
    da.extend_from_slice(b" 12 Tf 0 g");
    Dict::from_pairs([
        (
            names::DR.clone(),
            Object::Dict(Dict::from_pairs([(
                names::FONT.clone(),
                Object::Dict(Dict::from_pairs([(alias, Object::Dict(fallback_font()))])),
            )])),
        ),
        (names::DA.clone(), Object::Str(PdfString::literal(da))),
    ])
}

/// The resource name a font is filed under.
///
/// Four characters: the base font name's own, padded from a digit sequence
/// when it is shorter, and spaces removed. A nameless font takes the fixed
/// placeholder.
#[must_use]
pub(crate) fn font_resource_alias(base_font_name: &[u8]) -> Vec<u8> {
    const PLACEHOLDER: &[u8] = b"ZiTi";
    const WIDTH: usize = 4;

    let prefix: Vec<u8> = base_font_name
        .iter()
        .copied()
        .filter(|byte| *byte != b' ')
        .collect();
    if prefix.is_empty() {
        return PLACEHOLDER.to_vec();
    }
    let mut alias: Vec<u8> = prefix.iter().copied().take(WIDTH).collect();
    // Short names are padded with the digits of the positions they lack.
    let mut position = alias.len();
    while alias.len() < WIDTH {
        #[allow(clippy::cast_possible_truncation)]
        alias.push(b'0' + (position % 10) as u8);
        position += 1;
    }
    alias
}

/// A fallback font dictionary, for a `/DR /Font` that names none.
#[must_use]
pub(crate) fn fallback_font() -> Dict {
    Dict::from_pairs([
        (obj_names::TYPE.clone(), Object::Name(names::FONT.clone())),
        (
            obj_names::SUBTYPE.clone(),
            Object::Name(names::TYPE1.clone()),
        ),
        (
            names::BASE_FONT.clone(),
            Object::Name(pdfrum_object::Name::from("Helvetica")),
        ),
        (
            names::ENCODING.clone(),
            Object::Name(obj_names::WIN_ANSI_ENCODING.clone()),
        ),
    ])
}

/// The size a `Tf` operand of zero really means.
///
/// Zero asks for automatic sizing. A **negative** size is not zero, so it is
/// used as written — and the font operator's own `size > 0` gate then
/// suppresses the `Tf`, leaving the text at whatever the enclosing stream set.
#[must_use]
#[cfg(test)]
pub(crate) fn resolved_size(size: f32) -> Option<f32> {
    (!crate::geom::is_float_zero(size)).then_some(size)
}

/// The `/DA` a colour change rewrites, for the facade's benefit.
#[must_use]
#[cfg(test)]
pub(crate) fn default_appearance_string(appearance: &Appearance) -> Vec<u8> {
    let mut out = Vec::new();
    if !appearance.font_name.is_empty() && appearance.size > 0.0 {
        out.push(b'/');
        out.extend_from_slice(&appearance.font_name);
        out.push(b' ');
        out.extend_from_slice(fmt::shortest(appearance.size).as_bytes());
        out.extend_from_slice(b" Tf");
    }
    let color = color_op(appearance.color, PaintOp::Fill);
    if !color.is_empty() {
        if !out.is_empty() {
            out.push(b' ');
        }
        out.extend_from_slice(color.trim_end().as_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{
        default_appearance, default_appearance_string, fallback_font, free_text, resolved_size,
        valid_font_resources,
    };
    use crate::color::Color;
    use crate::vt::stub;
    use pdfrum_common::{DiagKind, Diagnostics};
    use pdfrum_object::{Array, Dict, Name, NoResolve, Object, PdfString};

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

    /// A catalog whose form carries one usable font resource.
    fn catalog() -> Dict {
        dict(&[(
            "AcroForm",
            Object::Dict(dict(&[(
                "DR",
                Object::Dict(dict(&[(
                    "Font",
                    Object::Dict(dict(&[("Helv", Object::Dict(fallback_font()))])),
                )])),
            )])),
        )])
    }

    fn annot(extra: &[(&str, Object)]) -> Dict {
        let mut pairs = vec![
            ("Subtype", Object::Name(Name::from("FreeText"))),
            ("Rect", numbers(&[100.0, 50.0, 150.0, 75.0])),
            (
                "DA",
                Object::Str(PdfString::literal(b"0 0 0 rg /Helv 12 Tf")),
            ),
        ];
        pairs.extend_from_slice(extra);
        dict(&pairs)
    }

    fn one_byte(code: u32) -> Vec<u8> {
        vec![u8::try_from(code).unwrap_or(b'?')]
    }

    fn generate(annot: &Dict, catalog: &Dict) -> Option<String> {
        let mut diags = Diagnostics::default();
        free_text(
            annot,
            catalog,
            &NoResolve,
            &stub::metrics(),
            &one_byte,
            &mut diags,
        )
        .map(|got| String::from_utf8_lossy(&got.stream).into_owned())
    }

    #[test]
    fn a_missing_precondition_declines_rather_than_producing_an_empty_stream() {
        // A form that exists but carries no default resources, and one whose
        // resources carry no fonts. Both decline: the synthesized form only
        // stands in for a form that is *absent*, not for one that is there
        // and unusable.
        let bare = dict(&[("AcroForm", Object::Dict(Dict::new()))]);
        assert!(generate(&annot(&[]), &bare).is_none());
        let no_font = dict(&[(
            "AcroForm",
            Object::Dict(dict(&[("DR", Object::Dict(Dict::new()))])),
        )]);
        assert!(generate(&annot(&[]), &no_font).is_none());
    }

    #[test]
    fn a_document_with_no_form_at_all_gets_one_synthesized() {
        // The generator creates an interactive form where there is none,
        // which is why a free-text annotation in a file that never mentions
        // forms still gets an appearance.
        let got = generate(
            &annot(&[("Contents", Object::Str(PdfString::literal(b"Hi")))]),
            &Dict::new(),
        )
        .expect("synthesizes a form");
        assert!(got.contains("(Hi) Tj\n"), "{got}");
    }

    /// The stream names a font in a `Tf`; the stream's own `/Resources /Font`
    /// has to carry it under that name or the operator resolves to nothing and
    /// the text draws as blank paper.
    #[test]
    fn the_generated_appearance_carries_the_font_its_tf_names() {
        let got = free_text(
            &annot(&[("Contents", Object::Str(PdfString::literal(b"Hi")))]),
            &catalog(),
            &NoResolve,
            &stub::metrics(),
            &one_byte,
            &mut Diagnostics::default(),
        )
        .expect("generates");
        let fonts = got.font_resources.expect("a text generator names a font");
        assert_eq!(fonts.keys().collect::<Vec<_>>(), [&Name::from("Helv")]);
        assert_eq!(
            fonts.dict(&Name::from("Helv"), &NoResolve),
            Some(fallback_font())
        );
    }

    /// `GetFontFromDrFontDictOrGenerateFallback` returns a fresh Helvetica for
    /// a `/DA` naming a font the `/DR` does not have, rather than declining —
    /// so the appearance is still produced, and still resolvable.
    #[test]
    fn a_da_naming_an_absent_font_falls_back_rather_than_declining() {
        let empty_dr = dict(&[(
            "AcroForm",
            Object::Dict(dict(&[(
                "DR",
                Object::Dict(dict(&[("Font", Object::Dict(Dict::new()))])),
            )])),
        )]);
        let got = free_text(
            &annot(&[("Contents", Object::Str(PdfString::literal(b"Hi")))]),
            &empty_dr,
            &NoResolve,
            &stub::metrics(),
            &one_byte,
            &mut Diagnostics::default(),
        )
        .expect("falls back rather than declining");
        let fonts = got.font_resources.expect("names a font");
        assert_eq!(
            fonts.dict(&Name::from("Helv"), &NoResolve),
            Some(fallback_font())
        );
    }

    #[test]
    fn the_synthesized_forms_font_alias_is_four_characters() {
        assert_eq!(super::font_resource_alias(b"Helvetica"), b"Helv");
        // A short name is padded from the positions it lacks.
        assert_eq!(super::font_resource_alias(b"AB"), b"AB23");
        // Spaces are removed before the truncation.
        assert_eq!(super::font_resource_alias(b"H e l v e t"), b"Helv");
        // A nameless font takes the fixed placeholder.
        assert_eq!(super::font_resource_alias(b""), b"ZiTi");
        assert_eq!(super::font_resource_alias(b"   "), b"ZiTi");
    }

    #[test]
    fn an_annotation_with_no_default_appearance_declines() {
        let no_da = dict(&[
            ("Subtype", Object::Name(Name::from("FreeText"))),
            ("Rect", numbers(&[100.0, 50.0, 150.0, 75.0])),
        ]);
        assert!(generate(&no_da, &catalog()).is_none());
    }

    #[test]
    fn an_empty_font_resource_dictionary_passes_vacuously() {
        assert!(valid_font_resources(&Dict::new(), &NoResolve));
        let good = dict(&[("Helv", Object::Dict(fallback_font()))]);
        assert!(valid_font_resources(&good, &NoResolve));
        // A non-font entry fails the whole dictionary.
        let bad = dict(&[("Helv", Object::Dict(dict(&[("Type", Object::Int(4))])))]);
        assert!(!valid_font_resources(&bad, &NoResolve));
        // And the type must be a *name*.
        let stringly = dict(&[(
            "Helv",
            Object::Dict(dict(&[("Type", Object::Str(PdfString::literal(b"Font")))])),
        )]);
        assert!(!valid_font_resources(&stringly, &NoResolve));
    }

    #[test]
    fn the_text_reaches_the_stream_between_its_markers() {
        let got = generate(
            &annot(&[("Contents", Object::Str(PdfString::literal(b"Hi")))]),
            &catalog(),
        )
        .expect("generates");
        assert!(got.starts_with("/GS gs "), "{got}");
        assert!(got.contains("/Tx BMC\nq\n"), "{got}");
        assert!(got.contains("BT\n"), "{got}");
        assert!(got.contains("(Hi) Tj\n"), "{got}");
        assert!(got.ends_with("ET\nQ\nEMC\n"), "{got}");
    }

    #[test]
    fn a_short_annotation_writes_no_clip_at_all() {
        let short = generate(
            &annot(&[("Contents", Object::Str(PdfString::literal(b"a")))]),
            &catalog(),
        )
        .expect("generates");
        assert!(!short.contains("W\nn\n"), "{short}");
    }

    #[test]
    fn a_background_colour_paints_before_the_border() {
        let got = generate(
            &annot(&[
                ("C", numbers(&[1.0, 0.0, 0.0])),
                ("Contents", Object::Str(PdfString::literal(b"x"))),
            ]),
            &catalog(),
        )
        .expect("generates");
        assert!(got.contains("q\n1 0 0 rg\n"), "{got}");
        assert!(got.contains("re f\nQ\n"), "{got}");
    }

    #[test]
    fn a_colour_array_naming_no_space_still_writes_its_save_and_restore() {
        let got = generate(
            &annot(&[
                ("C", Object::Array(Array::new())),
                ("Contents", Object::Str(PdfString::literal(b"x"))),
            ]),
            &catalog(),
        )
        .expect("generates");
        assert!(got.contains("q\n"), "{got}");
        assert!(got.contains(" re f\nQ\n"), "{got}");
    }

    #[test]
    fn empty_contents_produce_a_stream_with_no_text_block() {
        let got = generate(&annot(&[]), &catalog()).expect("generates");
        assert!(!got.contains("BT\n"), "{got}");
    }

    #[test]
    fn the_default_appearance_falls_back_to_the_forms_own() {
        let annot = dict(&[("Subtype", Object::Name(Name::from("FreeText")))]);
        let form = dict(&[("DA", Object::Str(PdfString::literal(b"/Helv 8 Tf")))]);
        let got = default_appearance(&annot, &form, &NoResolve).expect("has one");
        assert_eq!(got.font_name, b"Helv");
        assert!((got.size - 8.0).abs() < f32::EPSILON);
        // No colour operator leaves the text transparent.
        assert_eq!(got.color, Color::Transparent);
    }

    #[test]
    fn a_zero_size_asks_for_automatic_sizing_while_a_negative_one_does_not() {
        assert_eq!(resolved_size(0.0), None);
        assert_eq!(resolved_size(-12.0), Some(-12.0));
        assert_eq!(resolved_size(12.0), Some(12.0));
    }

    #[test]
    fn the_rewritten_default_appearance_joins_its_two_halves() {
        let got = default_appearance_string(&super::Appearance {
            font_name: b"Helv".to_vec(),
            size: 12.0,
            color: Color::Rgb(1.0, 0.0, 0.0),
        });
        assert_eq!(got, b"/Helv 12 Tf 1 0 0 rg");

        // A size of zero suppresses the font half.
        let sizeless = default_appearance_string(&super::Appearance {
            font_name: b"Helv".to_vec(),
            size: 0.0,
            color: Color::Gray(0.0),
        });
        assert_eq!(sizeless, b"0 g");
    }

    /// A `/DR /Font` entry that is not a font dictionary stops the generation
    /// dead, and now says so.
    #[test]
    fn a_malformed_dr_font_declines_and_is_recorded() {
        let bad_dr = dict(&[(
            "AcroForm",
            Object::Dict(dict(&[(
                "DR",
                Object::Dict(dict(&[(
                    "Font",
                    // `/Helv` present but not a `/Type /Font` dictionary.
                    Object::Dict(dict(&[("Helv", Object::Int(7))])),
                )])),
            )])),
        )]);
        let mut diags = Diagnostics::default();
        let got = free_text(
            &annot(&[("Contents", Object::Str(PdfString::literal(b"Hi")))]),
            &bad_dr,
            &NoResolve,
            &stub::metrics(),
            &one_byte,
            &mut diags,
        );
        assert!(got.is_none(), "no appearance can be generated");
        assert!(diags.contains(&DiagKind::FormResourcesInvalid));
    }
}
