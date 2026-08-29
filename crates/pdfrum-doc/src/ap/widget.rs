//! Widget appearances: the chrome a form control gets when its dictionary
//! carries none.
//!
//! # Why this exists, and how far it goes
//!
//! A widget annotation whose dictionary has **no `/AP` dictionary at all**
//! gets an appearance built for it the moment its page is opened — not only
//! under `/NeedAppearances`, which is what the wider form-regeneration path
//! is gated on, but unconditionally. So an unadorned text field in a file
//! that never mentions `/NeedAppearances` still ends up with an appearance
//! stream, and everything reading the file afterwards sees one.
//!
//! What that appearance contains is a background rectangle, a border, and
//! then — for the three field types that show text — a **body**: the value,
//! the selected option, or the option rows. This module builds the chrome;
//! [`field_body`](crate::ap::field_body) builds the body over the same
//! variable-text engine the free-text generator already uses, and
//! [`generate_with_text`] is the entry point that has a font to build one
//! with.
//!
//! [`generate`] is the font-less door and produces chrome alone. Both are
//! kept because they answer different questions: a caller with no font in
//! hand still needs a widget's background and border, and a widget with
//! neither `/MK` colour nor a value produces an empty stream either way —
//! which is what the oracle produces too.

use kurbo::Rect;
use pdfrum_common::{Diagnostics, Limits};
use pdfrum_object::{Dict, Resolve, names as obj_names};

use crate::ap::border::{BorderStyle, BorderStyleInfo, Dash};
use crate::ap::emit::{Content, Float, PaintOp, color_op};
use crate::ap::shapes::{self, CheckStyle};
use crate::ap::{GeneratedAp, da, resources_dict};
use crate::color::Color;
use crate::form::attr;
use crate::geom;
use crate::names;

/// Whether this annotation is a widget that will be given an appearance.
///
/// Three tests, and the middle one is the least obvious. The subtype must be
/// `/Widget`. The **field type must be one the appearance builder knows**: the
/// builder dispatches on it and a type it does not recognize falls off the
/// end, writing nothing at all — so an intermediate field node that carries
/// `/Kids` and no `/FT` of its own keeps having no appearance, which is
/// visible in the dump because the two colour lines report a colour exactly
/// when no appearance stream exists. `field_methods`'s `MyField` is that node.
/// And the appearance test is whether a **usable normal appearance stream
/// resolves**, not merely whether an `/AP` key is there: a radio button whose
/// `/AP /N` lists only its on-state, with `/AS` reading `Off`, has an `/AP`
/// and yet resolves to nothing — and so gets a fresh appearance built for it,
/// `Off` state included. Every radio button in `bug_707673.pdf` is that shape.
#[must_use]
pub fn needs_appearance<R: Resolve>(dict: &Dict, r: &R) -> bool {
    // Read coercively, matching how the annotation list classifies subtypes.
    if dict.byte_string(obj_names::SUBTYPE, r).as_deref() != Some(b"Widget") {
        return false;
    }
    if !has_known_field_type(dict, r) {
        return false;
    }
    if dict.dict(names::AP, r).is_none() {
        return true;
    }
    // An `/AP` that is there but resolves to nothing — a radio button whose
    // `/AP /N` lists only its on-state while `/AS` reads `Off` — is treated
    // as absent, but **only for a radio button**. A checkbox in the same
    // shape is left alone, because its widget never reaches the generator:
    // an ungrouped checkbox is not registered as a form control, and the
    // appearance path runs per control rather than per annotation.
    //
    // Measured 2026-08-29 and deliberately kept. The loader's own validity
    // test is one dictionary lookup — the mere presence of `/AP` — and
    // matching it exactly clears four more `--annot` artifacts (a checkbox in
    // this shape then keeps having no appearance, which is what
    // `checkbox_radiobutton`'s golden reports) while taking `bug_707673`'s
    // pixels **down**, from .9944 to .9910. That file's residual is a
    // push-button caption this crate does not draw, and the generated chrome
    // stands in for it by accident; removing the accident before drawing the
    // caption is a net loss. Do not tighten this to `/AP`-presence without
    // porting `SetAsPushButton` first.
    is_radio(dict, r)
        && crate::annot::appearance::annot_ap(dict, crate::annot::ApMode::Normal, true, r).is_none()
}

/// Builds a widget's appearance chrome, with no text body.
///
/// Returns nothing when the widget has an appearance already, or is not a
/// widget. The stream is the background fill followed by the border path;
/// with neither colour present — the ordinary case — it comes out empty,
/// which is a valid appearance and is what the oracle writes too.
#[must_use]
pub fn generate<R: Resolve>(dict: &Dict, r: &R) -> Option<GeneratedAp> {
    build(dict, None, None, r)
}

/// The same, with the field's own text set into it.
///
/// A text field, a combo box or a list box gains its body; every other field
/// type produces exactly what [`generate`] does, because only those three set
/// text at all.
#[must_use]
pub fn generate_with_text<R: Resolve>(
    dict: &Dict,
    catalog: &Dict,
    font: &crate::ap::TextFont<'_>,
    r: &R,
) -> Option<GeneratedAp> {
    build(dict, Some(catalog), Some(font), r)
}

/// The shared builder: chrome, then the body when there is a font for one.
fn build<R: Resolve>(
    dict: &Dict,
    catalog: Option<&Dict>,
    font: Option<&crate::ap::TextFont<'_>>,
    r: &R,
) -> Option<GeneratedAp> {
    if !needs_appearance(dict, r) {
        return None;
    }
    let rect = rotated_rect(dict, r);
    let mk = dict.dict(names::MK, r);
    let background = mk
        .as_ref()
        .and_then(|mk| mk.array(names::BG, r))
        .map_or(Color::Transparent, |array| Color::from_array(&array));
    let border_color = mk
        .as_ref()
        .and_then(|mk| mk.array(names::BC, r))
        .map_or(Color::Transparent, |array| Color::from_array(&array));

    let mut out = Content::new();
    let fill = color_op(background, PaintOp::Fill);
    if !fill.is_empty() {
        out.raw("q\n");
        out.raw(&fill);
        out.rect(rect, Float::Shortest);
        out.raw("re f\nQ\n");
    }

    let info = widget_border(dict, r);
    let border = crate::ap::border::border_path(rect, info, border_color);
    if !border.is_empty() {
        out.raw("q\n");
        out.raw(&border);
        out.raw("Q\n");
    }

    // A checkbox or radio button's glyph belongs to its **on** state alone:
    // the generator writes four streams — on and off, normal and down — and
    // only the two on-states carry the shape. Which one a reader sees is
    // decided by `/AS`, so a button sitting at `Off` shows chrome and nothing
    // more. The shape is drawn whatever the text colour is: a transparent one
    // writes no colour operator but leaves the path behind, which is why an
    // unadorned radio button still reports one path object.
    if is_checked(dict, r)
        && let Some(style) = check_style(dict, r)
    {
        let client = geom::deflate(rect, info.width, info.width);
        let color = text_color(dict, r);
        out.raw(&if is_radio(dict, r) {
            crate::ap::shapes::radio_button(client, style, color)
        } else {
            crate::ap::shapes::check_box(client, style, color)
        });
    }

    // The body follows the chrome, and only a caller with a font can ask for
    // one. A button reaches here with `None` from the dispatch below, which is
    // how a checkbox keeps producing exactly the stream it did before.
    let body = catalog
        .zip(font)
        .and_then(|(catalog, font)| crate::ap::field_body::generate(dict, catalog, font, r));
    let fonts = body.as_ref().and_then(|body| body.font_resources.clone());
    if let Some(body) = &body {
        out.raw(&String::from_utf8_lossy(&body.stream));
    }

    Some(GeneratedAp {
        stream: out.into_bytes(),
        bbox: rect,
        matrix: kurbo::Affine::IDENTITY,
        resources: resources_dict(crate::ap::ext_gstate_dict(dict, false, r), fonts),
        rect_override: None,
        as_override: None,
    })
}

/// Whether the widget's inherited `/FT` names a type the builder dispatches
/// on.
///
/// Three of the eight field types reach no builder: a signature, and the two
/// ways a type can be unknown — no `/FT` anywhere up the `/Parent` chain, and
/// an `/FT` naming something outside the three the spec defines. Each falls
/// off the end of the dispatch, and nothing is written.
#[must_use]
pub fn has_known_field_type<R: Resolve>(dict: &Dict, r: &R) -> bool {
    let (limits, mut diags) = (Limits::default(), Diagnostics::default());
    let kind = attr::field_attr(dict, names::FT, r, &limits, &mut diags)
        .map(|value| value.to_byte_string())
        .unwrap_or_default();
    matches!(kind.as_slice(), b"Btn" | b"Tx" | b"Ch")
}

/// Whether a button widget is showing its on-state.
///
/// `/AS` names the state the reader sees; anything but `Off` is on. A button
/// with no `/AS` at all is off, because that is what the generator writes
/// when it finds none.
#[must_use]
pub fn is_checked<R: Resolve>(dict: &Dict, r: &R) -> bool {
    match dict.byte_string(names::AS, r) {
        Some(state) => state != names::OFF.as_bytes(),
        None => false,
    }
}

/// Which glyph a button widget draws, or nothing when it is not a button.
///
/// The style comes from the first character of `/MK /CA`, read as a
/// ZapfDingbats code point — a naming convention, not a font lookup. An
/// unrecognized or absent caption falls back per kind: a check mark for a
/// checkbox, a circle for a radio button.
#[must_use]
pub fn check_style<R: Resolve>(dict: &Dict, r: &R) -> Option<CheckStyle> {
    if !is_button(dict, r) {
        return None;
    }
    let caption = dict
        .dict(names::MK, r)
        .and_then(|mk| mk.text(names::CA, r))
        .unwrap_or_default();
    Some(
        shapes::style_from_caption(&caption).unwrap_or(if is_radio(dict, r) {
            CheckStyle::Circle
        } else {
            CheckStyle::Check
        }),
    )
}

/// Whether the widget presents a checkbox or radio button.
///
/// Push buttons are excluded: they carry a caption and an icon rather than a
/// glyph.
#[must_use]
pub fn is_button<R: Resolve>(dict: &Dict, r: &R) -> bool {
    let (limits, mut diags) = (Limits::default(), Diagnostics::default());
    let kind = attr::field_attr(dict, names::FT, r, &limits, &mut diags)
        .map(|value| value.to_byte_string())
        .unwrap_or_default();
    if kind != b"Btn" {
        return false;
    }
    // Bit 17 is the push-button flag.
    let flags = attr::field_attr(dict, names::FF, r, &limits, &mut diags)
        .and_then(|value| value.as_int())
        .unwrap_or(0);
    flags & (1 << 16) == 0
}

/// Whether the widget is specifically a radio button — bit 16 of `/Ff`.
#[must_use]
pub fn is_radio<R: Resolve>(dict: &Dict, r: &R) -> bool {
    let (limits, mut diags) = (Limits::default(), Diagnostics::default());
    let flags = attr::field_attr(dict, names::FF, r, &limits, &mut diags)
        .and_then(|value| value.as_int())
        .unwrap_or(0);
    flags & (1 << 15) != 0
}

/// The colour a widget's glyph and text take, from its inherited `/DA`.
///
/// A `/DA` with no colour operator leaves the colour **transparent**, which
/// writes no colour operator into the stream — the glyph then takes whatever
/// the enclosing stream had set.
#[must_use]
pub fn text_color<R: Resolve>(dict: &Dict, r: &R) -> Color {
    let (limits, mut diags) = (Limits::default(), Diagnostics::default());
    attr::field_attr(dict, names::DA, r, &limits, &mut diags)
        .map(|value| value.to_byte_string())
        .and_then(|da| da::color(&da))
        .unwrap_or(Color::Transparent)
}

/// The widget's bounding box in its own, rotation-corrected space.
///
/// A widget's `/MK /R` names a quarter turn; at 90 or 270 degrees the box's
/// width and height swap, and any other value — including a negative one,
/// since the remainder keeps its sign — leaves the box **empty**.
#[must_use]
pub fn rotated_rect<R: Resolve>(dict: &Dict, r: &R) -> Rect {
    let rect = dict.rect(obj_names::RECT, r);
    let (width, height) = (geom::width(rect), geom::height(rect));
    let rotation = dict
        .dict(names::MK, r)
        .and_then(|mk| mk.int(names::R, r))
        .unwrap_or(0);
    match rotation % 360 {
        0 | 180 => geom::rect(0.0, 0.0, width, height),
        90 | 270 => geom::rect(0.0, 0.0, height, width),
        _ => Rect::ZERO,
    }
}

/// The widget's border style, which is read from the same `/BS` a markup
/// annotation uses but defaults differently: a widget with no `/BS` gets a
/// **one-unit solid** border, and only draws it when `/MK /BC` names a
/// colour.
#[must_use]
pub fn widget_border<R: Resolve>(dict: &Dict, r: &R) -> BorderStyleInfo {
    let bs = dict.dict(names::BS, r);
    let mut info = crate::ap::border::border_style_info(bs.as_ref(), r);
    if bs.is_none() {
        info = BorderStyleInfo {
            width: crate::ap::border::border_width(dict, r),
            style: BorderStyle::Solid,
            dash: Dash::default(),
        };
    }
    info
}

#[cfg(test)]
mod tests {
    use super::{generate, needs_appearance, rotated_rect};
    use crate::geom;
    use kurbo::Rect;
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

    /// A widget of a field type the builder knows, since one it does not is
    /// refused outright and would test nothing below.
    fn widget(extra: &[(&str, Object)]) -> Dict {
        let mut pairs = vec![
            ("Subtype", Object::Name(Name::from("Widget"))),
            ("FT", Object::Name(Name::from("Btn"))),
            ("Rect", numbers(&[100.0, 100.0, 200.0, 130.0])),
        ];
        pairs.extend_from_slice(extra);
        dict(&pairs)
    }

    #[test]
    fn a_widget_with_no_appearance_dictionary_gets_one() {
        assert!(needs_appearance(&widget(&[]), &NoResolve));
    }

    #[test]
    fn a_field_type_the_builder_does_not_dispatch_on_gets_nothing() {
        // An intermediate node with `/Kids` and no `/FT` of its own, and a
        // signature — the two shapes that fall off the end of the dispatch.
        let no_type = dict(&[
            ("Subtype", Object::Name(Name::from("Widget"))),
            ("Rect", numbers(&[100.0, 100.0, 200.0, 130.0])),
        ]);
        assert!(!needs_appearance(&no_type, &NoResolve));
        assert!(generate(&no_type, &NoResolve).is_none());

        let signature = dict(&[
            ("Subtype", Object::Name(Name::from("Widget"))),
            ("FT", Object::Name(Name::from("Sig"))),
            ("Rect", numbers(&[100.0, 100.0, 200.0, 130.0])),
        ]);
        assert!(!needs_appearance(&signature, &NoResolve));

        // And the type is inherited, so a kid whose parent names it qualifies.
        let parent = dict(&[("FT", Object::Name(Name::from("Tx")))]);
        let kid = dict(&[
            ("Subtype", Object::Name(Name::from("Widget"))),
            ("Rect", numbers(&[100.0, 100.0, 200.0, 130.0])),
            ("Parent", Object::Dict(parent)),
        ]);
        assert!(needs_appearance(&kid, &NoResolve));
    }

    #[test]
    fn an_appearance_that_resolves_leaves_the_widget_alone() {
        let with_stream = widget(&[(
            "AP",
            Object::Dict(dict(&[(
                "N",
                Object::Stream(Stream::new(Dict::new(), ByteSpan::from(b"x".to_vec()))),
            )])),
        )]);
        assert!(!needs_appearance(&with_stream, &NoResolve));
    }

    #[test]
    fn a_radio_whose_state_names_nothing_is_regenerated_where_a_checkbox_is_not() {
        // `/AP /N` lists only the on-state while `/AS` reads `Off`, so
        // nothing resolves. A radio button in that shape gets a fresh
        // appearance; an ungrouped checkbox never reaches the generator and
        // keeps its unusable one.
        let unusable = [
            (
                "AP",
                Object::Dict(dict(&[(
                    "N",
                    Object::Dict(dict(&[(
                        "Yes",
                        Object::Stream(Stream::new(Dict::new(), ByteSpan::from(b"x".to_vec()))),
                    )])),
                )])),
            ),
            ("AS", Object::Name(Name::from("Off"))),
            ("FT", Object::Name(Name::from("Btn"))),
        ];
        let checkbox = widget(&unusable);
        assert!(!needs_appearance(&checkbox, &NoResolve));

        let mut radio_pairs = unusable.to_vec();
        // Bit 16 is the radio flag.
        radio_pairs.push(("Ff", Object::Int(1 << 15)));
        let radio = widget(&radio_pairs);
        assert!(needs_appearance(&radio, &NoResolve));
    }

    #[test]
    fn a_buttons_glyph_belongs_to_its_on_state_alone() {
        let base = [
            ("FT", Object::Name(Name::from("Btn"))),
            ("Ff", Object::Int(1 << 15)),
        ];
        let mut off = base.to_vec();
        off.push(("AS", Object::Name(Name::from("Off"))));
        let off = generate(&widget(&off), &NoResolve).expect("is a widget");
        assert!(off.stream.is_empty());

        let mut on = base.to_vec();
        on.push(("AS", Object::Name(Name::from("Yes"))));
        let on = generate(&widget(&on), &NoResolve).expect("is a widget");
        let stream = String::from_utf8_lossy(&on.stream).into_owned();
        // A circle, since a radio with no caption defaults to one.
        assert!(stream.contains(" c\n"), "{stream}");
        assert!(stream.ends_with("f\nQ\n"), "{stream}");
    }

    #[test]
    fn a_non_widget_is_left_alone() {
        let square = dict(&[("Subtype", Object::Name(Name::from("Square")))]);
        assert!(!needs_appearance(&square, &NoResolve));
        assert!(generate(&square, &NoResolve).is_none());
    }

    #[test]
    fn a_plain_widget_produces_an_empty_stream() {
        // No `/MK`, so neither the background nor the border has a colour and
        // nothing is drawn — a valid appearance with no page objects in it.
        let got = generate(&widget(&[]), &NoResolve).expect("is a widget");
        assert!(got.stream.is_empty());
        assert_eq!(got.bbox, geom::rect(0.0, 0.0, 100.0, 30.0));
        // The rectangle is untouched: only the sticky-note and ink
        // generators move one.
        assert_eq!(got.rect_override, None);
    }

    #[test]
    fn a_background_colour_fills_the_box() {
        let coloured = widget(&[(
            "MK",
            Object::Dict(dict(&[("BG", numbers(&[1.0, 0.0, 0.0]))])),
        )]);
        let got = generate(&coloured, &NoResolve).expect("is a widget");
        assert_eq!(
            String::from_utf8_lossy(&got.stream),
            "q\n1 0 0 rg\n0 0 100 30 re f\nQ\n"
        );
    }

    #[test]
    fn a_border_colour_draws_the_border() {
        let bordered = widget(&[(
            "MK",
            Object::Dict(dict(&[("BC", numbers(&[0.0, 0.0, 0.0]))])),
        )]);
        let got = generate(&bordered, &NoResolve).expect("is a widget");
        let stream = String::from_utf8_lossy(&got.stream).into_owned();
        assert!(stream.starts_with("q\n0 0 0 rg\n"), "{stream}");
        assert!(stream.ends_with("Q\n"), "{stream}");
    }

    #[test]
    fn a_quarter_turn_swaps_the_boxs_extents() {
        let rotated = |degrees: i64| {
            rotated_rect(
                &widget(&[("MK", Object::Dict(dict(&[("R", Object::Int(degrees))])))]),
                &NoResolve,
            )
        };
        assert_eq!(rotated(0), geom::rect(0.0, 0.0, 100.0, 30.0));
        assert_eq!(rotated(180), geom::rect(0.0, 0.0, 100.0, 30.0));
        assert_eq!(rotated(90), geom::rect(0.0, 0.0, 30.0, 100.0));
        assert_eq!(rotated(270), geom::rect(0.0, 0.0, 30.0, 100.0));
    }

    #[test]
    fn a_rotation_that_is_not_a_quarter_turn_empties_the_box() {
        // The remainder keeps its sign, so a negative quarter turn matches
        // no case and the box collapses.
        assert_eq!(
            rotated_rect(
                &widget(&[("MK", Object::Dict(dict(&[("R", Object::Int(-90))])))]),
                &NoResolve
            ),
            Rect::ZERO
        );
        assert_eq!(
            rotated_rect(
                &widget(&[("MK", Object::Dict(dict(&[("R", Object::Int(45))])))]),
                &NoResolve
            ),
            Rect::ZERO
        );
    }
}
