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
//! `generate_with_text` is the entry point that has a font to build one
//! with.
//!
//! `generate` is the font-less door and produces chrome alone. Both are
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
/// The subtype must be `/Widget`, and the **field type must be one the
/// appearance builder knows**: the builder dispatches on it and a type it does
/// not recognize falls off the end, writing nothing at all — so an intermediate
/// field node that carries `/Kids` and no `/FT` of its own keeps having no
/// appearance, which is visible in the dump because the two colour lines report
/// a colour exactly when no appearance stream exists. `field_methods`'s
/// `MyField` is that node.
///
/// # The appearance test is one dictionary lookup
///
/// Past those, a widget is regenerated exactly when it has **no `/AP`
/// dictionary at all** — the presence of the key, nothing more. Not whether
/// `/N` resolves, not whether `/AS` names a state that exists. So a radio
/// button whose `/AP /N` lists only its on-state while `/AS` reads `Off`
/// **keeps having no drawable appearance** and is never regenerated.
///
/// What draws that widget instead is the grey outline
/// [`crate::annot_render`] strokes over an invalid checkbox or radio — a
/// *deeper* validity test, on a different code path. Porting either of those
/// two without the other is a measured loss, which is why they landed
/// together.
///
/// # `/NeedAppearances`, and why it so often changes nothing
///
/// A document whose `/AcroForm` sets `/NeedAppearances` rebuilds *every*
/// widget's appearance, consulting no `/AP` at all. That rebuild always runs
/// — but for a checkbox or a radio button it is very often **invisible**, and
/// the reason is a key mismatch rather than a gate:
///
/// A rebuilt checkbox or radio button writes exactly two sub-states,
/// `/AP /N /<`[`checked_ap_state`]`>` and `/AP /N /Off`, while readback
/// resolves `/AP /N /<AS>`. When `/AS` names neither of those, the new
/// streams land in keys nothing looks up and the file's own stream is what
/// draws.
///
/// [`checked_ap_state`] is where that goes wrong most often. It answers the
/// first non-`Off` key of `/AP /N` — **unless** the field carries an `/Opt`
/// array, in which case it answers the widget's *control index* as a decimal
/// string. `bug_861842` is that file: `/Opt` present, control index 0, and
/// `/AS /1`, so the rebuild writes `/0` and `/Off` while the reader keeps
/// asking for `/1`. Honouring the flag without this rule takes it from .99986
/// to .93548; with the rule it is untouched, and `bug_707673`'s radios — no
/// `/Opt`, `/AS /Off`, and `Off` is always written literally — do rebuild.
///
/// Measured with gdb on the oracle: both files reach `ResetAppearance`, and
/// only one of them shows it.
#[must_use]
#[cfg(test)]
pub(crate) fn needs_appearance<R: Resolve>(dict: &Dict, r: &R) -> bool {
    needs_appearance_in(dict, None, r)
}

/// The same, knowing the document the widget belongs to.
///
/// The catalog is what `/NeedAppearances` is read from; a caller without one
/// answers as a form that does not set it would.
#[must_use]
pub(crate) fn needs_appearance_in<R: Resolve>(dict: &Dict, catalog: Option<&Dict>, r: &R) -> bool {
    // Read coercively, matching how the annotation list classifies subtypes.
    if dict.byte_string(obj_names::SUBTYPE, r).as_deref() != Some(b"Widget") {
        return false;
    }
    if !has_known_field_type(dict, r) {
        return false;
    }
    if has_kids(dict, r) {
        return false;
    }
    if dict.dict(names::AP, r).is_none() {
        return true;
    }
    needs_construct_ap(catalog, r) && rebuild_would_be_seen(dict, r)
}

/// Whether the dictionary is a form **field** with children rather than a
/// control of its own.
///
/// A field that carries `/Kids` delegates its geometry to them, and the form
/// loader never registers it as a control: a control is made for a field dict
/// with no `/Kids`, and otherwise for each kid instead. So such a dictionary
/// has no appearance to build even when it names `/Subtype /Widget` and a
/// field type, which a field shared by several controls routinely does.
///
/// **Without this the parent generates chrome as if it were a control.** Its
/// `/Rect` is `[0 0 0 0]`, so the stream is empty over an empty box —
/// invisible on the page, and yet enough to make the annotation dump report
/// the colour keys as unreadable, because any appearance outranks them.
/// `example_014` and `example_054` are that file.
fn has_kids<R: Resolve>(dict: &Dict, r: &R) -> bool {
    dict.array(names::KIDS, r)
        .is_some_and(|kids| !kids.is_empty())
}

/// Whether the document's form asks for every appearance to be rebuilt.
///
/// `/AcroForm /NeedAppearances`, read strictly as a **boolean** — so a
/// `/NeedAppearances (true)` written as a string does not set it, and neither
/// does a document with no `/AcroForm`.
fn needs_construct_ap<R: Resolve>(catalog: Option<&Dict>, r: &R) -> bool {
    let Some(form) = catalog.and_then(|catalog| catalog.dict(names::ACRO_FORM, r)) else {
        return false;
    };
    form.get(names::NEED_APPEARANCES, r)
        .and_then(|value| value.as_direct().and_then(pdfrum_object::Object::as_bool))
        .unwrap_or(false)
}

/// Whether a rebuild would land in the sub-state `/AS` reads back.
///
/// Only a checkbox or a radio button writes sub-states at all; every other
/// field type's builder writes `/AP /N` as one stream, which `/AS` never
/// filters, so a rebuild is always seen. For the two that do, it is seen
/// exactly when `/AS` names `Off` or [`checked_ap_state`] — and a widget with
/// no `/AS` reads back the empty key, which a rebuild never writes.
fn rebuild_would_be_seen<R: Resolve>(dict: &Dict, r: &R) -> bool {
    if !is_button(dict, r) {
        return true;
    }
    let Some(state) = dict.byte_string(names::AS, r) else {
        return false;
    };
    state == names::OFF.as_bytes() || state == checked_ap_state(dict, r)
}

/// The sub-state key a rebuilt checkbox or radio button writes its on-state
/// into.
///
/// The first non-`Off` key of `/AP /N`, taken in **sorted** key order rather
/// than the document order this crate's dictionaries keep — except that a
/// field carrying an `/Opt` array answers the widget's control index as a
/// decimal string instead, and an answer that comes back empty becomes
/// `Yes`.
#[must_use]
pub(crate) fn checked_ap_state<R: Resolve>(dict: &Dict, r: &R) -> Vec<u8> {
    let (limits, mut diags) = (Limits::default(), Diagnostics::default());
    if attr::field_attr(dict, names::OPT, r, &limits, &mut diags)
        .is_some_and(|value| matches!(value, pdfrum_object::Object::Array(_)))
    {
        return control_index(dict, r).to_string().into_bytes();
    }
    let on = dict
        .dict(names::AP, r)
        .and_then(|ap| ap.dict(names::N, r))
        .map(|normal| {
            let mut keys: Vec<&[u8]> = normal
                .keys()
                .map(pdfrum_object::Name::as_bytes)
                .filter(|key| *key != names::OFF.as_bytes())
                .collect();
            keys.sort_unstable();
            keys.first().map_or_else(Vec::new, |key| key.to_vec())
        })
        .unwrap_or_default();
    if on.is_empty() { b"Yes".to_vec() } else { on }
}

/// Which of its field's widgets this one is, by position in `/Kids`.
///
/// A merged field-and-widget — the dictionary is its own only control — is
/// index zero, which is also what an unfindable widget answers, because
/// `GetControlIndex` returns zero for a control the field does not list.
fn control_index<R: Resolve>(dict: &Dict, r: &R) -> usize {
    let Some(kids) = dict
        .dict(names::PARENT, r)
        .and_then(|parent| parent.array(names::KIDS, r))
    else {
        return 0;
    };
    (0..kids.len())
        .find(|index| kids.dict_at(*index, r).as_ref() == Some(dict))
        .unwrap_or(0)
}

/// Builds a widget's appearance chrome, with no text body.
///
/// Returns nothing when the widget has an appearance already, or is not a
/// widget. The stream is the background fill followed by the border path;
/// with neither colour present — the ordinary case — it comes out empty,
/// which is a valid appearance and is what the oracle writes too.
#[must_use]
pub(crate) fn generate<R: Resolve>(dict: &Dict, r: &R) -> Option<GeneratedAp> {
    build(dict, None, None, LiveInput::default(), r)
}

/// The same, with the field's own text set into it.
///
/// A text field, a combo box or a list box gains its body; every other field
/// type produces exactly what `generate` does, because only those three set
/// text at all.
///
/// The text is the one the **file** stores. A field being edited shows
/// something else, and `generate_with_live` is the entry point for that.
#[must_use]
pub(crate) fn generate_with_text<R: Resolve>(
    dict: &Dict,
    catalog: &Dict,
    font: &crate::ap::TextFont<'_>,
    substitute: Option<crate::ap::Substitute<'_>>,
    r: &R,
) -> Option<GeneratedAp> {
    build(
        dict,
        Some(catalog),
        Some(font),
        LiveInput {
            substitute,
            ..LiveInput::default()
        },
        r,
    )
}

/// What a form session shows a widget it is editing, beyond the widget's own
/// dictionary.
///
/// Three borrows that travel together because the live path needs all three
/// to draw one field: the overlay it paints, the text it paints instead of
/// the stored `/V`, and the second face for the characters the `/DA` font
/// cannot write. A record rather than three more parameters, so a fourth
/// answer can be added without moving anyone's call.
///
/// [`Default`] is "a field with nothing live about it", which
/// [`generate_with_live_faces`] renders exactly as `generate_with_text`
/// does with no substitute.
/// Adding a field to this struct breaks every exhaustive literal outside
/// this crate. `#[non_exhaustive]` is NOT the fix: it forbids the literal form
/// entirely outside the crate, `..Default::default()` included (E0639),
/// and the two external callers (`pdfrum-form::route`,
/// `pdfrum-tool::chrome`) legitimately build the whole value. Before a
/// fourth field lands, give it a constructor — `LiveInput::new()` plus
/// `with_*` setters — and convert those two sites; that keeps additions
/// source-compatible without taking literal construction away.
#[derive(Debug, Clone, Copy, Default)]
pub struct LiveInput<'a> {
    /// The focused-field caret and selection bands.
    pub caret_and_selection: Option<&'a crate::ap::field_body::Highlight>,
    /// What the session is showing in place of the stored `/V`, `/I` and
    /// `/TI`.
    pub live: Option<&'a crate::ap::field_body::LiveState<'a>>,
    /// The second face, for characters the `/DA` font's charset does not
    /// cover. [`None`] leaves every character to the `/DA` font, which is
    /// what a Hebrew value being typed into a Latin field used to get: the
    /// low byte of each code point, drawn as Latin.
    pub substitute: Option<crate::ap::Substitute<'a>>,
    /// The appearance state a session is showing, overriding the widget
    /// dictionary's own `/AS`. [`None`] reads `/AS` as before, byte for byte.
    ///
    /// # Why a session cannot say this through `/AS`
    ///
    /// A radio group is one field with several kid controls, and each kid
    /// carries a **different** on-state name in its own `/AP /N`. Checking one
    /// sets the clicked control's `/AS` to that control's own on-state and
    /// every *other* control's to `Off` — so one click restates the state of
    /// every kid in the group, in as many different names.
    ///
    /// A session holds one state record per **field**, so all it can say is
    /// which control the click chose. Turning that into what each kid draws is
    /// per-kid, and the widget dictionary on disk still names the state before
    /// the click. This is how the session tells the generator which kid to
    /// draw off — the value it passes being `Off` for a sibling and the
    /// control's own on-state for the chosen one.
    ///
    /// Only the on/off question is overridden, not the state's *name*: what
    /// the generator does with it is `is_checked_with`'s single comparison
    /// against `Off`, so any non-`Off` bytes draw the on-state shape.
    pub appearance_state: Option<&'a [u8]>,
}

/// The same again, for a widget a form session is currently editing.
///
/// `caret_and_selection` is the focused-field overlay and `live` is what the
/// session is showing in place of the stored `/V`, `/I` and `/TI`. Passing
/// [`None`] for both is exactly `generate_with_text`, byte for byte — the
/// two differ only in what this one is allowed to be handed.
///
/// # Superseded by [`generate_with_live_faces`]
///
/// This one forwards no substitute, so a live edit whose text needs a second
/// face writes the `/DA` font's low bytes for it — Latin glyphs where the
/// value is Hebrew. `pdfrum-form`'s `route.rs` has since migrated to
/// [`generate_with_live_faces`], which takes the same two answers plus that
/// face in one [`LiveInput`], so nothing in the library calls this any more.
/// It stays under `#[cfg(test)]` because the tests beside it are what pin
/// that the no-substitute spelling still agrees with `generate_with_text`
/// byte for byte.
#[must_use]
#[cfg(test)]
pub(crate) fn generate_with_live<R: Resolve>(
    dict: &Dict,
    catalog: &Dict,
    font: &crate::ap::TextFont<'_>,
    r: &R,
    caret_and_selection: Option<&crate::ap::field_body::Highlight>,
    live: Option<&crate::ap::field_body::LiveState<'_>>,
) -> Option<GeneratedAp> {
    generate_with_live_faces(
        dict,
        catalog,
        font,
        r,
        LiveInput {
            caret_and_selection,
            live,
            // Both spelled out rather than left to `..Default::default()`:
            // this function's contract is that it produces exactly what it
            // produced before either field existed, and naming them is what
            // makes a third addition a compile error here rather than a
            // silent change of behaviour.
            substitute: None,
            appearance_state: None,
        },
    )
}

/// The live entry point that can reach a **second face**.
///
/// `generate_with_live` with the substitute carried in the same record as
/// the overlay and the live text. A field being typed into asks the same
/// charset question a stored value does — the face is chosen per character,
/// and nothing in that choice knows where the characters came from — so the
/// typed path needs the same answer the stored one gets from
/// `generate_with_text`'s `substitute`.
///
/// `LiveInput::default()` here is `generate`-with-a-font, byte for byte:
/// the three fields are each [`None`] and nothing downstream distinguishes
/// them from the stored path's arguments.
#[must_use]
pub fn generate_with_live_faces<R: Resolve>(
    dict: &Dict,
    catalog: &Dict,
    font: &crate::ap::TextFont<'_>,
    r: &R,
    input: LiveInput<'_>,
) -> Option<GeneratedAp> {
    build(dict, Some(catalog), Some(font), input, r)
}

/// The shared builder: chrome, then the body when there is a font for one.
/// The shared builder's live answers travel as one [`LiveInput`] rather than
/// as four positional `Option`s, which is the same reason the public entry
/// point takes one: a fifth answer then costs no call site a change.
fn build<R: Resolve>(
    dict: &Dict,
    catalog: Option<&Dict>,
    font: Option<&crate::ap::TextFont<'_>>,
    input: LiveInput<'_>,
    r: &R,
) -> Option<GeneratedAp> {
    if !needs_appearance_in(dict, catalog, r) {
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
    if is_checked_with(dict, r, input.appearance_state)
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
    let body = catalog.zip(font).and_then(|(catalog, font)| {
        crate::ap::field_body::generate(
            dict,
            catalog,
            font,
            input.substitute,
            r,
            input.caret_and_selection,
            input.live,
        )
    });
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
pub(crate) fn has_known_field_type<R: Resolve>(dict: &Dict, r: &R) -> bool {
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
#[cfg(test)]
pub(crate) fn is_checked<R: Resolve>(dict: &Dict, r: &R) -> bool {
    is_checked_with(dict, r, None)
}

/// [`is_checked`], with a session's appearance state allowed to override the
/// dictionary's `/AS`.
///
/// `None` is exactly [`is_checked`]. A `Some` is read by the same rule the
/// dictionary's own value is — anything but `Off` is on — so a session that
/// passes a sibling's `Off` draws chrome alone and one that passes the chosen
/// control's on-state draws the shape, whatever that state happens to be
/// named. See [`LiveInput::appearance_state`] for why the session cannot say
/// this through the dictionary instead.
#[must_use]
pub(crate) fn is_checked_with<R: Resolve>(
    dict: &Dict,
    r: &R,
    override_state: Option<&[u8]>,
) -> bool {
    match override_state {
        Some(state) => state != names::OFF.as_bytes(),
        None => match dict.byte_string(names::AS, r) {
            Some(state) => state != names::OFF.as_bytes(),
            None => false,
        },
    }
}

/// Which glyph a button widget draws, or nothing when it is not a button.
///
/// The style comes from the first character of `/MK /CA`, read as a
/// ZapfDingbats code point — a naming convention, not a font lookup. An
/// unrecognized or absent caption falls back per kind: a check mark for a
/// checkbox, a circle for a radio button.
#[must_use]
pub(crate) fn check_style<R: Resolve>(dict: &Dict, r: &R) -> Option<CheckStyle> {
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
pub(crate) fn is_button<R: Resolve>(dict: &Dict, r: &R) -> bool {
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
pub(crate) fn is_radio<R: Resolve>(dict: &Dict, r: &R) -> bool {
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
pub(crate) fn text_color<R: Resolve>(dict: &Dict, r: &R) -> Color {
    let (limits, mut diags) = (Limits::default(), Diagnostics::default());
    attr::field_attr(dict, names::DA, r, &limits, &mut diags)
        .map(|value| value.to_byte_string())
        .and_then(|da| da::color(&da))
        .unwrap_or(Color::Transparent)
}

/// The widget's bounding box in its own, rotation-corrected space.
///
/// A widget's `/MK /R` names a quarter turn, folded through
/// [`geom::WidgetRotation::from_degrees`]; at 90 or 270 degrees the box's
/// width and height swap, and any angle that names no quadrant — a
/// non-multiple of 90 — leaves the box upright. There is no value of `/R`
/// that empties it.
#[must_use]
pub(crate) fn rotated_rect<R: Resolve>(dict: &Dict, r: &R) -> Rect {
    let rect = dict.rect(obj_names::RECT, r);
    let (width, height) = (geom::width(rect), geom::height(rect));
    if widget_rotation(dict, r).swaps_axes() {
        geom::rect(0.0, 0.0, height, width)
    } else {
        geom::rect(0.0, 0.0, width, height)
    }
}

/// The widget's `/MK /R`, as the quadrant its appearance stream is set into.
///
/// The one reader of the key: `pdfrum-form`'s routing calls this too, so a
/// click lands in the box this function measured.
#[must_use]
pub fn widget_rotation<R: Resolve>(dict: &Dict, r: &R) -> geom::WidgetRotation {
    let degrees = dict
        .dict(names::MK, r)
        .and_then(|mk| mk.int(names::R, r))
        .unwrap_or(0);
    geom::WidgetRotation::from_degrees(degrees)
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
    use super::{checked_ap_state, generate, needs_appearance, needs_appearance_in, rotated_rect};
    use crate::geom;
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

    /// A catalog whose form sets `/NeedAppearances` to the given value.
    fn form_catalog(need: Object) -> Dict {
        dict(&[("AcroForm", Object::Dict(dict(&[("NeedAppearances", need)])))])
    }

    /// An `/AP` whose `/N` lists the given sub-states, each a stream.
    fn states(names: &[&str]) -> Object {
        Object::Dict(dict(&[(
            "N",
            Object::Dict(Dict::from_pairs(
                names
                    .iter()
                    .map(|state| {
                        (
                            Name::from(*state),
                            Object::Stream(Stream::new(Dict::new(), ByteSpan::from(b"x".to_vec()))),
                        )
                    })
                    .collect::<Vec<_>>(),
            )),
        )]))
    }

    #[test]
    fn need_appearances_rebuilds_a_text_field_that_already_has_one() {
        // Only a checkbox and a radio button write sub-states, so nothing
        // filters a rebuilt text field: it is always seen.
        let field = dict(&[
            ("Subtype", Object::Name(Name::from("Widget"))),
            ("FT", Object::Name(Name::from("Tx"))),
            ("Rect", numbers(&[100.0, 100.0, 200.0, 130.0])),
            (
                "AP",
                Object::Dict(dict(&[(
                    "N",
                    Object::Stream(Stream::new(Dict::new(), ByteSpan::from(b"x".to_vec()))),
                )])),
            ),
        ]);
        assert!(!needs_appearance(&field, &NoResolve));
        for (need, expected) in [
            (Object::Bool(true), true),
            (Object::Bool(false), false),
            // `GetBooleanFor` reads a boolean and nothing else.
            (Object::Name(Name::from("true")), false),
            (
                Object::Str(pdfrum_object::PdfString::literal(b"true")),
                false,
            ),
        ] {
            assert_eq!(
                needs_appearance_in(&field, Some(&form_catalog(need.clone())), &NoResolve),
                expected,
                "{need:?}"
            );
        }
    }

    #[test]
    fn a_rebuild_a_button_would_never_read_back_does_not_happen() {
        let catalog = form_catalog(Object::Bool(true));
        let button = |extra: &[(&str, Object)]| {
            let mut pairs = vec![
                ("FT", Object::Name(Name::from("Btn"))),
                ("AP", states(&["Yes", "Off"])),
            ];
            pairs.extend_from_slice(extra);
            widget(&pairs)
        };

        // `/AS` names `Off`, which a rebuild always writes literally.
        let off = button(&[("AS", Object::Name(Name::from("Off")))]);
        assert!(needs_appearance_in(&off, Some(&catalog), &NoResolve));

        // `/AS` names the on-state a rebuild would write.
        let on = button(&[("AS", Object::Name(Name::from("Yes")))]);
        assert!(needs_appearance_in(&on, Some(&catalog), &NoResolve));

        // `/AS` names a third state, which a rebuild leaves untouched — so
        // the file's own stream keeps drawing and nothing is regenerated.
        let elsewhere = button(&[("AS", Object::Name(Name::from("Maybe")))]);
        assert!(!needs_appearance_in(&elsewhere, Some(&catalog), &NoResolve));

        // No `/AS` at all reads back the empty key, which is never written.
        assert!(!needs_appearance_in(
            &button(&[]),
            Some(&catalog),
            &NoResolve
        ));
    }

    #[test]
    fn an_opt_array_makes_the_on_state_a_control_index() {
        // `bug_861842`'s shape: `/Opt` present, so the rebuilt on-state is the
        // widget's control index — `0` — while `/AS` still reads `1`. The two
        // never meet and the file's own stream survives.
        let catalog = form_catalog(Object::Bool(true));
        let with_opt = widget(&[
            ("FT", Object::Name(Name::from("Btn"))),
            ("AP", states(&["1", "Off"])),
            ("AS", Object::Name(Name::from("1"))),
            ("Opt", numbers(&[0.0, 0.0])),
        ]);
        assert_eq!(checked_ap_state(&with_opt, &NoResolve), b"0".to_vec());
        assert!(!needs_appearance_in(&with_opt, Some(&catalog), &NoResolve));

        // Without `/Opt` the on-state is the first non-`Off` key, `/AS`
        // matches it, and the rebuild is seen.
        let without = widget(&[
            ("FT", Object::Name(Name::from("Btn"))),
            ("AP", states(&["1", "Off"])),
            ("AS", Object::Name(Name::from("1"))),
        ]);
        assert_eq!(checked_ap_state(&without, &NoResolve), b"1".to_vec());
        assert!(needs_appearance_in(&without, Some(&catalog), &NoResolve));
    }

    #[test]
    fn the_on_state_is_the_first_key_in_sorted_order_and_falls_back_to_yes() {
        // The C++ walks a `std::map`, so the order is the keys' own, not the
        // document's. Written `Zed` first, `Alpha` wins.
        let sorted = widget(&[
            ("FT", Object::Name(Name::from("Btn"))),
            ("AP", states(&["Zed", "Off", "Alpha"])),
        ]);
        assert_eq!(checked_ap_state(&sorted, &NoResolve), b"Alpha".to_vec());

        // An `/AP /N` with nothing but `Off` — or none at all — answers `Yes`.
        let off_only = widget(&[
            ("FT", Object::Name(Name::from("Btn"))),
            ("AP", states(&["Off"])),
        ]);
        assert_eq!(checked_ap_state(&off_only, &NoResolve), b"Yes".to_vec());
        assert_eq!(
            checked_ap_state(
                &widget(&[("FT", Object::Name(Name::from("Btn")))]),
                &NoResolve
            ),
            b"Yes".to_vec()
        );
    }

    #[test]
    fn an_unusable_appearance_is_still_an_appearance() {
        // `/AP /N` lists only the on-state while `/AS` reads `Off`, so nothing
        // resolves — and the regeneration test does not care, because it is
        // `!!GetDictFor("AP")` and no more. Neither a checkbox nor a radio is
        // rebuilt. What draws them is `annot_render::invalid_outline`, which
        // asks the *deeper* question on a different code path.
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
        assert!(!needs_appearance(&widget(&unusable), &NoResolve));

        let mut radio_pairs = unusable.to_vec();
        // Bit 16 is the radio flag.
        radio_pairs.push(("Ff", Object::Int(1 << 15)));
        assert!(!needs_appearance(&widget(&radio_pairs), &NoResolve));
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
    fn a_rotation_that_is_not_a_quarter_turn_leaves_the_box_upright() {
        // No value of `/R` empties the box. A non-multiple of 90 names no
        // quadrant — PDFium's `default:` arm and pdf.js's `angle % 90 === 0`
        // gate both land on upright — and a negative angle names the
        // quadrant it counts counterclockwise to.
        let rotated = |degrees: i64| {
            rotated_rect(
                &widget(&[("MK", Object::Dict(dict(&[("R", Object::Int(degrees))])))]),
                &NoResolve,
            )
        };
        let upright = geom::rect(0.0, 0.0, 100.0, 30.0);
        let turned = geom::rect(0.0, 0.0, 30.0, 100.0);

        assert_eq!(rotated(45), upright);
        assert_eq!(rotated(-45), upright);
        assert_eq!(rotated(1), upright);

        // `-90` is `270`: a swap, not an empty box. PDFium's `abs()` sends it
        // to `90`, which swaps the same axes but is the wrong quadrant for
        // the matrix — see the `[oracle-bug]` note on `WidgetRotation`.
        assert_eq!(rotated(-90), turned);
        assert_eq!(rotated(-270), turned);
        assert_eq!(rotated(-180), upright);
        assert_eq!(rotated(450), turned);
    }

    /// A text widget with a `/DA` the font resource below satisfies.
    fn text_widget(value: &str) -> Dict {
        dict(&[
            ("Subtype", Object::Name(Name::from("Widget"))),
            ("FT", Object::Name(Name::from("Tx"))),
            ("Rect", numbers(&[100.0, 100.0, 200.0, 130.0])),
            (
                "DA",
                Object::Str(pdfrum_object::PdfString::literal(b"0 0 0 rg /Helv 12 Tf")),
            ),
            ("V", Object::Str(pdfrum_object::PdfString::literal(value))),
        ])
    }

    fn text_catalog() -> Dict {
        dict(&[(
            "AcroForm",
            Object::Dict(dict(&[(
                "DR",
                Object::Dict(dict(&[(
                    "Font",
                    Object::Dict(dict(&[(
                        "Helv",
                        Object::Dict(crate::ap::freetext::fallback_font()),
                    )])),
                )])),
            )])),
        )])
    }

    #[test]
    fn the_live_entry_point_carries_its_override_down_to_the_body() {
        let cache = pdfrum_font::FontCache::new();
        let face = pdfrum_font::Font::load_standard(pdfrum_font::StandardFont::Helvetica, &cache);
        let width = |code: u32| crate::ap::TextFont::char_width(&face, code);
        let font = crate::ap::TextFont {
            metrics: crate::ap::TextFont::metrics_of(&face, &width),
            font: &face,
        };
        let (widget, catalog) = (text_widget("stored"), text_catalog());
        let stream = |ap: Option<super::GeneratedAp>| {
            String::from_utf8_lossy(&ap.expect("an appearance").stream).into_owned()
        };

        let stored = stream(super::generate_with_text(
            &widget, &catalog, &font, None, &NoResolve,
        ));
        assert!(stored.contains("(stored) Tj\n"), "{stored}");

        let live = crate::ap::field_body::LiveState {
            text: "typed",
            ..crate::ap::field_body::LiveState::default()
        };
        let edited = stream(super::generate_with_live(
            &widget,
            &catalog,
            &font,
            &NoResolve,
            None,
            Some(&live),
        ));
        assert!(edited.contains("(typed) Tj\n"), "{edited}");
        assert!(!edited.contains("stored"), "{edited}");

        // And handed nothing, the live entry point is the stored one.
        assert_eq!(
            stored,
            stream(super::generate_with_live(
                &widget, &catalog, &font, &NoResolve, None, None,
            ))
        );
    }

    /// Text a session is **typing** reaches the second face, which is the one
    /// thing [`super::generate_with_live`] cannot do: it forwards no
    /// substitute, so the same string comes out as the `/DA` font's low
    /// bytes.
    ///
    /// The expectations are the stored path's, from
    /// `field_body::tests::a_value_the_da_font_cannot_write_switches_to_a_second_face`:
    /// aleph is written `\340` and bet `\341` — code page 1255 — and not
    /// `\320`/`\321`, which are the low bytes of U+05D0 and U+05D1 and the
    /// mojibake this replaces.
    #[test]
    fn the_live_path_with_a_second_face_writes_hebrew_through_it() {
        let cache = pdfrum_font::FontCache::new();
        let options = pdfrum_font::SubstitutionOptions::default();
        let mut ctx = pdfrum_page::BuildContext::with_substitution(options);
        let catalog = text_catalog();
        let fonts = crate::ap::FormFonts::load(&catalog, &NoResolve, &mut ctx);
        let substitute = fonts
            .substitute(pdfrum_font::Charset::Hebrew)
            .expect("a Hebrew substitute");

        let face = pdfrum_font::Font::load_standard(pdfrum_font::StandardFont::Helvetica, &cache);
        let charset = crate::ap::font_map::font_charset(&face);
        // The run is measured by the face that writes each character, or it
        // is set in two faces and laid out by one.
        let width = |code: u32| {
            if crate::ap::font_map::da_font_writes(&face, charset, code) {
                crate::ap::TextFont::char_width(&face, code)
            } else {
                crate::ap::font_map::substitute_width(substitute.font, code)
            }
        };
        let font = crate::ap::TextFont {
            metrics: crate::ap::TextFont::metrics_of(&face, &width),
            font: &face,
        };

        // Typed, not stored: the widget's `/V` is Latin and stays unread.
        let widget = text_widget("stored");
        let state = crate::ap::field_body::LiveState {
            text: "ab\u{5D0}\u{5D1}",
            ..crate::ap::field_body::LiveState::default()
        };
        let live = |substitute| {
            super::generate_with_live_faces(
                &widget,
                &catalog,
                &font,
                &NoResolve,
                super::LiveInput {
                    live: Some(&state),
                    substitute,
                    ..super::LiveInput::default()
                },
            )
            .expect("an appearance")
        };

        // Compared as bytes: a code-page byte is not valid UTF-8, so reading
        // the stream as text could not tell 0xE0 from 0xD0.
        let got = live(Some(substitute));
        let stream = got.stream.clone();
        let has = |needle: &[u8]| stream.windows(needle.len()).any(|w| w == needle);

        assert!(has(b"/Helv 12 Tf\n"), "{stream:02X?}");
        let mut tf = b"/".to_vec();
        tf.extend_from_slice(substitute.alias.as_bytes());
        tf.extend_from_slice(b" 12 Tf\n");
        assert!(has(&tf), "the second face names itself: {stream:02X?}");
        assert!(has(b"\\340"), "aleph as 0xE0: {stream:02X?}");
        assert!(has(b"\\341"), "bet as 0xE1: {stream:02X?}");
        assert!(!has(b"\\320"), "no low-byte aleph: {stream:02X?}");
        assert!(!has(b"\\321"), "no low-byte bet: {stream:02X?}");
        assert!(
            got.resources
                .dict(crate::names::FONT, &NoResolve)
                .is_some_and(|fonts| fonts.contains_key(substitute.alias)),
            "the second face is in the appearance's own resources: {:?}",
            got.resources
        );

        // Without it, the same string is the mojibake this closes — which is
        // exactly what `generate_with_live` still produces.
        let plain = live(None);
        assert_ne!(plain.stream, stream);
        let plain_has = |needle: &[u8]| plain.stream.windows(needle.len()).any(|w| w == needle);
        assert!(plain_has(b"\\320"), "{:02X?}", plain.stream);
        assert_eq!(
            plain.stream,
            super::generate_with_live(&widget, &catalog, &font, &NoResolve, None, Some(&state),)
                .expect("an appearance")
                .stream,
            "the old entry point is the new one with no substitute"
        );
    }

    /// A session's appearance state overrides the widget's own `/AS`, in both
    /// directions.
    ///
    /// This is what lets a radio group's click be drawn. Checking one kid
    /// sets that control's `/AS` to its own on-state and every other
    /// control's to `Off` — one click, one state per kid, in as many
    /// different names. A session holds one record per **field**, so it can
    /// only say which control was chosen; this is how it says what each kid
    /// draws.
    ///
    /// The dictionary is untouched either way: the same widget answers both
    /// ways depending only on what is passed.
    #[test]
    fn a_sessions_appearance_state_overrides_the_dictionarys_own() {
        let catalog = Dict::new();
        let cache = pdfrum_font::FontCache::new();
        let font = pdfrum_font::Font::load_standard(pdfrum_font::StandardFont::Helvetica, &cache);
        let width = |code: u32| crate::ap::TextFont::char_width(&font, code);
        let text = crate::ap::TextFont {
            metrics: crate::ap::TextFont::metrics_of(&font, &width),
            font: &font,
        };
        let radio = |state: &str| {
            widget(&[
                ("FT", Object::Name(Name::from("Btn"))),
                ("Ff", Object::Int(1 << 15)),
                ("AS", Object::Name(Name::from(state))),
            ])
        };
        let draw = |dict: &Dict, override_state: Option<&[u8]>| {
            super::generate_with_live_faces(
                dict,
                &catalog,
                &text,
                &NoResolve,
                super::LiveInput {
                    appearance_state: override_state,
                    ..super::LiveInput::default()
                },
            )
            .expect("is a widget")
            .stream
        };
        // A circle is what a radio with no caption draws, so its presence is
        // the on-state and its absence the off.
        let drawn = |stream: &[u8]| String::from_utf8_lossy(stream).contains(" c\n");

        // On by its dictionary, forced off by the session — the sibling of a
        // control that was just clicked.
        let on = radio("Yes");
        assert!(drawn(&draw(&on, None)), "its own /AS says Yes");
        assert!(!drawn(&draw(&on, Some(b"Off"))), "the session says Off");

        // Off by its dictionary, forced on — the control that was clicked,
        // whose on-state is its own name and not the sibling's.
        let off = radio("Off");
        assert!(!drawn(&draw(&off, None)), "its own /AS says Off");
        assert!(drawn(&draw(&off, Some(b"Yes"))), "the session says Yes");
        assert!(
            drawn(&draw(&off, Some(b"2"))),
            "any non-Off state is on, whatever it is named"
        );

        // And `None` is byte-for-byte the unoverridden stream, which is what
        // keeps every existing caller where it was.
        assert_eq!(draw(&on, None), draw(&on, None));
        assert_eq!(
            draw(&on, Some(b"Yes")),
            draw(&on, None),
            "an override naming the state already shown changes nothing"
        );
    }

    /// `is_checked` is `is_checked_with` handed no override, and the public
    /// signature `pdfrum/src/form.rs` and `form/field.rs` call is unchanged.
    #[test]
    fn is_checked_is_the_unoverridden_case_of_is_checked_with() {
        for state in ["Off", "Yes", "2", ""] {
            let dict = widget(&[("AS", Object::Name(Name::from(state)))]);
            assert_eq!(
                super::is_checked(&dict, &NoResolve),
                super::is_checked_with(&dict, &NoResolve, None),
                "{state:?}"
            );
        }
        // A widget with no `/AS` at all is off, and an override still speaks.
        let bare = widget(&[]);
        assert!(!super::is_checked(&bare, &NoResolve));
        assert!(!super::is_checked_with(&bare, &NoResolve, Some(b"Off")));
        assert!(super::is_checked_with(&bare, &NoResolve, Some(b"Yes")));
    }
}
