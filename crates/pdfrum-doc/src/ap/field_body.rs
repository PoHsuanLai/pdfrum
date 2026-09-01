//! The text a form field's appearance shows: a text field's value, a combo
//! box's current label, a list box's option rows.
//!
//! # Which producer this is, and which it is not
//!
//! Two different things in the C++ build a widget appearance out of a field's
//! value, they are near-copies of each other, and only one of them runs on the
//! corpus. `GenerateFormAP` regenerates a whole form and is gated on
//! `/NeedAppearances`, which almost no corpus file sets. The producer that
//! runs on **every** widget lacking a usable `/AP` is the other one: opening a
//! page calls `ResetAppearance` unconditionally, which dispatches on the
//! field's type to one of three builders. Those three are what this module is.
//!
//! Two observable differences decide which, and `listbox_form`'s golden dump
//! settles it in this crate's favour:
//!
//! - **Selection comes from `/V`, not `/I`.** The gated generator reads only
//!   `/I`; this one reads `/V` and falls back to `/I`. The golden highlights
//!   `Banana` on a field whose only selection is `/V (Banana)`, and highlights
//!   *nothing* on one whose only selection is `/I [1 3]` — which is the
//!   fallback's own behavior, below.
//! - **A combo box shows the selected option's label**, where the gated one
//!   shows the raw `/V`.
//!
//! # It is the same layout engine, reached through one extra transform
//!
//! The three builders drive an editor shell that is **not** a second layout
//! engine: it wraps the same variable-text engine [`vt`] already is. What the
//! shell adds, and all this module needs from it, is a vertical alignment
//! offset:
//!
//! ```text
//! to_edit(p) = (p.x - (scroll.x - plate.left),
//!               p.y - (scroll.y + padding - plate.top))
//! ```
//!
//! with `padding` zero for top alignment and `(plate.height - content.height)
//! / 2` for centred. The scroll position never moves here — scrolling defaults
//! off and none of the three builders enables it, so it stays at the
//! `(plate.left, plate.top)` its setter seeded — and those two terms then
//! cancel, leaving `-padding` on y and nothing on x. That is the whole
//! difference, and it arrives as [`vt::edit_ap::generate`]'s `offset`
//! argument, exactly as the free-text generator already passes one.
//!
//! # The `/I` fallback selects nothing, and that is not a bug here
//!
//! When `/V` is absent the selection object becomes `/I`, and the lookup then
//! compares each entry's **text** against the option values. An integer's text
//! is the empty string, which matches no option, so a list box selected only
//! by index highlights no row at all. `listbox_form`'s
//! `Listbox_MultiSelectMultipleIndices` is exactly that file and its golden
//! reports five text objects and no path.

use std::borrow::Cow;

use kurbo::Rect;
use pdfrum_common::{Diagnostics, Limits};
use pdfrum_object::{Array, Dict, Name, Object, Resolve};

use crate::ap::border::BorderStyle;
use crate::ap::emit::{Content, Float, PaintOp, color_op_via};
use crate::ap::{TextFont, da, freetext, shapes, widget};
use crate::color::Color;
use crate::form::attr;
use crate::geom;
use crate::names;
use crate::vt;

/// Which of the three builders a widget's field type asks for.
///
/// Only these three set text. A push button, a checkbox and a radio button
/// reach a different builder whose output is chrome alone, and a signature
/// never reaches one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `/FT /Tx` — one value, possibly over several lines or in comb cells.
    Text,
    /// `/FT /Ch` with the combo flag — one line beside a drop button.
    Combo,
    /// `/FT /Ch` without it — every option, stacked.
    List,
    /// `/FT /Btn` with the push-button flag — a centred caption.
    Button,
}

impl Kind {
    /// Which builder a widget's inherited `/FT` and `/Ff` select.
    ///
    /// Read through the inherited-attribute walk, because a group of controls
    /// shares one `/FT` on their common parent.
    #[must_use]
    pub fn of<R: Resolve>(dict: &Dict, r: &R) -> Option<Kind> {
        let kind = inherited(dict, names::FT, r)
            .map(|value| value.to_byte_string())
            .unwrap_or_default();
        match kind.as_slice() {
            b"Tx" => Some(Kind::Text),
            b"Ch" if flags(dict, r) & FLAG_COMBO != 0 => Some(Kind::Combo),
            b"Ch" => Some(Kind::List),
            // A check box and a radio button reach `SetAsCheckBox` /
            // `SetAsRadioButton`, which draw a glyph rather than set text;
            // `ap::widget` already builds those. Only the push button has a
            // body here.
            b"Btn" if flags(dict, r) & FLAG_PUSH_BUTTON != 0 => Some(Kind::Button),
            _ => None,
        }
    }
}

/// Bit 18 of `/Ff`: a choice field that presents as a combo box.
const FLAG_COMBO: i64 = 1 << 17;
/// Bit 17 of `/Ff`: a button that pushes rather than toggling.
const FLAG_PUSH_BUTTON: i64 = 1 << 16;
/// Bit 13 of `/Ff`: a text field that may hold several lines.
const FLAG_MULTILINE: i64 = 1 << 12;
/// Bit 14 of `/Ff`: a text field that shows its value as bullets.
const FLAG_PASSWORD: i64 = 1 << 13;
/// Bit 25 of `/Ff`: a text field laid out as `/MaxLen` equal cells.
const FLAG_COMB: i64 = 1 << 24;

/// The width a combo box's drop button always takes, whatever the box is.
const DROP_BUTTON_WIDTH: f32 = 13.0;

/// The size a list-box row falls back to when its `/DA` names none.
///
/// A list box is the one builder that does **not** auto-size: each row is
/// laid out into a plate of zero height, and automatic sizing against that
/// would answer for the plate rather than for the row, so a zero size
/// substitutes twelve outright.
const LIST_ROW_DEFAULT_SIZE: f32 = 12.0;

/// The width a **live** list box keeps clear on its right for the vertical
/// scroll bar.
///
/// # Only a live one, and that is the whole of the subtlety
///
/// Two different producers draw a list box's rows and they disagree about
/// this, so a change that looks like one rule is really two:
///
/// - The **generated appearance** — what a file without an `/AP` is given
///   when the page opens — lays every row into the body rectangle whole.
///   There is no window, no scroll bar and no reservation: it is plain
///   layout into the box the border leaves behind.
/// - The **live control** — which exists only once an event has focused the
///   field — is a window, and its window asks for a vertical scroll bar when
///   it is created without ever consulting how many options there are. The
///   client rectangle then loses this much from its right edge, so a
///   three-option box that could never scroll is laid out into the same
///   narrowed box as a thirty-option one.
///
/// Applying the reservation to both is a measurable mistake and not a subtle
/// one: it narrows the band on every list box in the corpus that is drawn
/// without events, which is most of them.
///
/// Everything the client rectangle decides moves with it — where a row wraps,
/// how wide the selection band is drawn, and how far right a click still
/// counts as landing on a row — so the live path's hit testing has to reserve
/// the same width or the band and the target disagree.
///
/// The bar itself is never drawn here either way. The oracle paints it from
/// its own widget tree rather than from any appearance stream, so the space is
/// real and its occupant is not.
const LIST_SCROLLBAR_WIDTH: f32 = 12.0;

/// The colour behind a selected list-box row, as the byte triple the source
/// writes rather than the fraction it equals.
///
/// The same triple is the focused-field selection band
/// (`ArgbEncode(255, 0, 51, 113)` in `cpwl_edit_impl.cpp`).
const SELECTION_FILL: Color = Color::Rgb(0.0, 51.0 / 255.0, 113.0 / 255.0);

/// The caret's width in PDF units (`cpwl_caret.h`).
pub const CARET_WIDTH: f32 = 0.4;

/// Overlay a focused field draws on top of its unfocused body.
///
/// Rectangles are in PDF user space (y-up), the same space the body's `Td`
/// operators already use. A caret is a filled rectangle
/// [`CARET_WIDTH`] units wide; a selection band is filled
/// `ArgbEncode(255, 0, 51, 113)` and the text over it is white. A field with
/// a live selection shows no caret.
#[derive(Debug, Clone, PartialEq)]
pub struct Highlight {
    /// The caret rectangle, or `None` when a selection is showing.
    pub caret: Option<Rect>,
    /// Selection bands, painted behind the text.
    pub selection: Vec<Rect>,
}

/// What a focused field is showing, when that is not what the file stores.
///
/// A field being edited draws from the session that owns the edit, not from
/// the dictionary: the text the user has typed has not been written to `/V`
/// yet, and it must not be until the field commits. Every field of this
/// record overrides one dictionary read, and passing [`None`] for the whole
/// record leaves all four reads exactly as they were.
///
/// The scroll offset is the one field that is not a substitution. It shifts
/// the drawn text by the distance the content has been scrolled away from its
/// resting position, which is what makes the visible window of a long value
/// move: upstream keeps a scroll *position* seeded at the plate's top-left
/// corner and subtracts `(scroll.x - plate.left, scroll.y - plate.top)` from
/// every drawn point, so the difference from the seed is the whole of what a
/// reader can observe. That difference is what this carries, which is why
/// `(0.0, 0.0)` is the unscrolled field and needs no plate to interpret.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LiveState<'a> {
    /// The text as the user has it. Overrides the field's `/V`.
    pub text: &'a str,
    /// Which options are selected, overriding `/V` / `/I`.
    pub selected: &'a [usize],
    /// The first visible row, overriding `/TI`.
    pub top_visible: usize,
    /// How far the content is scrolled, in layout units, away from the
    /// resting top-left. `(0.0, 0.0)` is an unscrolled field; each component
    /// moves the drawn text by that much in the negative direction of the
    /// PDF axis, so a positive x shows text further right and a positive y
    /// shows text further down the value.
    pub scroll: (f32, f32),
}

impl Default for LiveState<'_> {
    fn default() -> Self {
        LiveState {
            text: "",
            selected: &[],
            top_visible: 0,
            scroll: (0.0, 0.0),
        }
    }
}

impl LiveState<'_> {
    /// The shift this scroll applies to every drawn point, in PDF space.
    ///
    /// Upstream's transform subtracts the scroll's distance from its seed, so
    /// both components negate.
    #[must_use]
    fn shift(&self) -> (f32, f32) {
        (-self.scroll.0, -self.scroll.1)
    }
}

/// The shift a body applies to its text, which only a scrolled live edit has.
fn live_shift(live: Option<&LiveState<'_>>) -> (f32, f32) {
    live.map_or((0.0, 0.0), LiveState::shift)
}

/// The resource name a field with no `/DA` font sets its text under.
///
/// The font map names every face it adds by its own family with the spaces
/// removed and the charset appended as two hex digits, and the one it adds
/// when nothing else supplied a default is the stock ANSI Helvetica — charset
/// zero. The alias matters because a `Tf` naming nothing is not written at
/// all, and a run set with no font in the text state produces no page object.
const DEFAULT_FONT_ALIAS: &[u8] = b"Helvetica_00";

/// An inherited field attribute, resolved one level.
fn inherited<R: Resolve>(dict: &Dict, key: &Name, r: &R) -> Option<Object> {
    let (limits, mut diags) = (Limits::default(), Diagnostics::default());
    attr::field_attr(dict, key, r, &limits, &mut diags)
}

/// A widget's inherited field flags.
fn flags<R: Resolve>(dict: &Dict, r: &R) -> i64 {
    inherited(dict, names::FF, r)
        .and_then(|value| value.as_int())
        .unwrap_or(0)
}

/// The client rectangle: the rotated box, deflated by the border.
///
/// Two details, both of which change what gets drawn.
///
/// The width is taken already-doubled for a beveled or inset border, which is
/// where the "twice the stated width" in the source's own deflation has gone.
///
/// And the deflation **normalizes afterwards**, so a box narrower than twice
/// its border comes back turned inside out rather than empty: a one-unit
/// `/Rect` with a one-unit border deflates to `(1, 1, 0, 0)` and normalizes to
/// `(0, 0, 1, 1)` — a plate of the same size it started as. That is not a
/// rounding accident; it decides whether the field sets any text at all,
/// because a plate with no width auto-sizes to zero and a zero size writes no
/// font operator, which leaves the run with no font and no page object.
/// `bug_765384`'s second field is a one-by-one box whose golden reports one
/// text object. The normalization is only ever applied to the deflated box,
/// never used as a licence to reject an inverted one: `bug_889099`'s `/Rect`
/// reads `[100 100 200 -130]`, and refusing that outright loses its value.
#[must_use]
pub fn client_rect<R: Resolve>(dict: &Dict, r: &R) -> Rect {
    let width = widget::widget_border(dict, r).width;
    geom::normalize(geom::deflate(widget::rotated_rect(dict, r), width, width))
}

/// The colour a field's text is set in.
///
/// Unlike the chrome's, this one is never transparent: a `/DA` naming no
/// colour sets **black**, so a body that sets any text always writes a colour
/// operator before it.
#[must_use]
pub fn text_color<R: Resolve>(dict: &Dict, r: &R) -> Color {
    inherited(dict, names::DA, r)
        .map(|value| value.to_byte_string())
        .and_then(|string| da::color(&string))
        .unwrap_or(Color::Gray(0.0))
}

/// Where a widget's text sits horizontally.
///
/// `/Q` is read on the **widget's own dictionary first** — not through the
/// inherited walk — and only then as an inherited field attribute. A widget
/// that sets `/Q` for itself therefore overrides its field's, which the
/// ordinary walk could not express.
#[must_use]
pub fn alignment<R: Resolve>(dict: &Dict, r: &R) -> vt::Alignment {
    if let Some(own) = dict.int(names::Q, r) {
        return vt::Alignment::from_quadding(own);
    }
    vt::Alignment::from_quadding(
        inherited(dict, names::Q, r)
            .and_then(|value| value.as_int())
            .unwrap_or(0),
    )
}

/// One generated field body: its content stream and the font it names.
#[derive(Debug, Clone, PartialEq)]
pub struct Body {
    /// The content-stream bytes, to follow the chrome.
    pub stream: Vec<u8>,
    /// The one-entry `/Font` dictionary the stream's `Tf` resolves against.
    pub font_resources: Option<Dict>,
}

/// Builds a widget's field body, or nothing when its type sets no text.
///
/// The stream returned goes **after** the background and the border, which is
/// the order the three builders concatenate their pieces in — for a comb text
/// field the cell separators sit between them, and are part of what this
/// returns because they are the same builder's work.
///
/// `caret_and_selection` is the focused-field overlay and `live` is what a
/// focused field is showing in place of what the file stores. Passing
/// [`None`] for both is the unfocused path and is byte-identical to the
/// stream this function produced before either parameter existed.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn generate<R: Resolve>(
    dict: &Dict,
    catalog: &Dict,
    font: &TextFont<'_>,
    substitute: Option<crate::ap::Substitute<'_>>,
    r: &R,
    caret_and_selection: Option<&Highlight>,
    live: Option<&LiveState<'_>>,
) -> Option<Body> {
    let kind = Kind::of(dict, r)?;
    let form = catalog.dict(names::ACRO_FORM, r);
    let empty = Dict::new();
    // A missing default appearance does **not** decline here, unlike in the
    // free-text generator: a field with no `/DA` anywhere still shows its
    // value. The font map it drives has no default font to add, so its
    // constructor falls through to adding a stock ANSI Helvetica at index
    // zero, under the synthesized alias below — and that alias, not an absent
    // one, is what the `Tf` names. `field_methods`'s `MyText` is that file: no
    // `/DA` on the widget, its parent or the form, and its golden still
    // reports one text object.
    let appearance = freetext::default_appearance(dict, form.as_ref().unwrap_or(&empty), r)
        .filter(|appearance| !appearance.font_name.is_empty())
        .unwrap_or(freetext::Appearance {
            font_name: DEFAULT_FONT_ALIAS.to_vec(),
            // A push button reads a missing `/DA` as **twelve points**, where
            // the other three read it as the automatic size. `GetFont()`
            // answers nothing only for an *empty* `/DA` string, and
            // `SetAsPushButton` is the one caller that supplies a default for
            // that case rather than passing the zero through.
            size: if kind == Kind::Button { 12.0 } else { 0.0 },
            color: Color::Transparent,
        });
    let client = client_rect(dict, r);
    let color = text_color(dict, r);
    // The *value* comes from the field, which is not always this dictionary:
    // two `/Fields` entries sharing a `/T` are one field with two controls,
    // and the field is the first of them. Everything else here — the plate,
    // the colour, the default appearance — is read off the widget, because
    // those are per-control.
    let valued = field_dict_of(dict, form.as_ref(), r);
    let valued = valued.as_ref().unwrap_or(dict);

    let input = BodyInput {
        widget: dict,
        valued,
        client,
        appearance: &appearance,
        color,
        font,
        substitute,
        caret_and_selection,
        live,
    };
    let mut out = Content::new();
    match kind {
        Kind::Text => text_field(&mut out, &input, r),
        Kind::Combo => combo_box(&mut out, &input, r),
        Kind::List => list_box(&mut out, &input, r),
        Kind::Button => push_button(&mut out, &input, r),
    }
    if out.is_empty() {
        return None;
    }
    Some(Body {
        stream: out.into_bytes(),
        font_resources: font_resources(&appearance, substitute, form.as_ref(), r),
    })
}

/// The dictionary a widget's **field** value is read from.
///
/// Usually the widget itself — a field and its single widget are one
/// dictionary in the common case, and a widget under a `/Parent` inherits
/// through it. The exception this exists for is two `/Fields` entries that
/// share a `/T` and have no parent between them: upstream's `AddTerminalField`
/// looks each name up before building anything, so the second entry becomes a
/// second *control* of the first's field rather than a field of its own, and
/// the value both controls show is the **first** dictionary's.
///
/// `bug_733528` is that file, and it is the only shape this answers anything
/// but the widget for: the walk stops at the first `/Fields` entry whose `/T`
/// matches, which is the widget itself whenever the widget is in `/Fields` at
/// all. A widget with a `/Parent`, or one the form does not list, is its own
/// value source.
///
/// Answers `None` rather than the widget so a caller can tell "no other
/// dictionary applies" from "this one does", which keeps the common case
/// borrowing rather than cloning.
fn field_dict_of<R: Resolve>(dict: &Dict, form: Option<&Dict>, r: &R) -> Option<Dict> {
    // A widget under a parent inherits through the chain and is not a
    // top-level `/Fields` entry, so the name walk below cannot apply.
    if dict.contains_key(names::PARENT) {
        return None;
    }
    let name = dict.byte_string(names::T, r)?;
    let fields = form?.array(names::FIELDS, r)?;
    let first = (0..fields.len())
        .filter_map(|index| fields.dict_at(index, r))
        .find(|entry| entry.byte_string(names::T, r).as_deref() == Some(name.as_slice()))?;
    (first != *dict).then_some(first)
}

/// The one-entry font dictionary a body's `Tf` names.
///
/// The stream names a resource; without this the operator resolves to nothing
/// and the text draws as blank paper. The entry is the form's `/DR /Font`
/// entry under that name, or a stock Helvetica when there is none.
fn font_resources<R: Resolve>(
    appearance: &freetext::Appearance,
    substitute: Option<crate::ap::Substitute<'_>>,
    form: Option<&Dict>,
    r: &R,
) -> Option<Dict> {
    if appearance.font_name.is_empty() {
        return None;
    }
    let name = Name::new(appearance.font_name.clone());
    let entry = form
        .and_then(|form| form.dict(names::DR, r))
        .and_then(|resources| resources.dict(names::FONT, r))
        .and_then(|fonts| fonts.dict(&name, r))
        .unwrap_or_else(freetext::fallback_font);
    let mut resources = Dict::from_pairs([(name, Object::Dict(entry))]);
    // The second face is added unconditionally when one exists, the way
    // `AddFontToAnnotDict` (`core/fpdfdoc/cpdf_bafontmap.cpp:318-357`) writes
    // it into the annotation's own `/AP` resources the moment the map decides
    // to hold it — not only when a character actually reached it. A resource
    // nothing names costs a dictionary entry and changes no pixel.
    if let Some(sub) = substitute {
        resources.push(sub.alias.clone(), Object::Dict(sub.dict.clone()));
    }
    Some(resources)
}

/// The offset that carries a layout's vertical alignment.
///
/// Top alignment shifts nothing. Centred alignment shifts by half the slack,
/// which is the transform's `-padding` written out.
fn vertical_offset(centred: bool, plate: Rect, content: Rect) -> (f32, f32) {
    if !centred {
        return (0.0, 0.0);
    }
    (0.0, (geom::height(content) - geom::height(plate)) * 0.5)
}

/// Lays a string out and writes its operators.
///
/// `shift` is the scroll a live edit adds on top of the vertical alignment;
/// it is `(0.0, 0.0)` for every stored appearance, and adding zero writes the
/// same `Td` operators as not adding it at all.
#[allow(clippy::too_many_arguments)]
fn set_text(
    text: &str,
    config: &vt::Config,
    font: &TextFont<'_>,
    substitute: Option<crate::ap::Substitute<'_>>,
    offset_centred: bool,
    grouping: vt::edit_ap::Grouping,
    alias: &[u8],
    shift: (f32, f32),
) -> (String, Rect) {
    let layout = vt::layout(text, config, &font.metrics);
    let content = layout.content_rect_pdf(config.plate);
    let padding = vertical_offset(offset_centred, config.plate, content);
    let offset = (padding.0 + shift.0, padding.1 + shift.1);
    let written = vt::edit_ap::generate(&layout, config, &font.metrics, offset, grouping, |code| {
        face_for(font, substitute, alias, code)
    });
    (written, content)
}

/// Which font writes one code point, and as what.
///
/// The `/DA` font when its charset covers the character and it can map it;
/// otherwise the second face, under its own alias — which is what puts a `Tf`
/// between the two runs. See [`crate::ap::font_map`] for the rule and where it
/// is written.
fn face_for(
    font: &TextFont<'_>,
    substitute: Option<crate::ap::Substitute<'_>>,
    alias: &[u8],
    code: u32,
) -> vt::edit_ap::Face {
    let da_charset = crate::ap::font_map::font_charset(font.font);
    match substitute {
        Some(sub) if !crate::ap::font_map::da_font_writes(font.font, da_charset, code) => {
            vt::edit_ap::Face {
                index: 1,
                alias: sub.alias.as_bytes().to_vec(),
                bytes: crate::ap::font_map::substitute_encode(sub.font, code),
            }
        }
        _ => vt::edit_ap::Face::single(alias, font.encode(code)),
    }
}

/// Wraps a body in the markers and the clip a text field and a combo box
/// share.
///
/// The clip is written **only on overflow** — a body that fits its plate has
/// no `W n` in its stream at all — and the colour operator sits inside the
/// text object rather than before it.
///
/// A focused overlay, when present, is painted *inside* the same `q`/`Q`:
/// selection bands before the text (so they sit behind it), the caret after
/// `ET` (so it sits on top). Passing [`None`] writes exactly the operators
/// this function wrote before the overlay existed.
fn wrap_text(
    out: &mut Content,
    plate: Rect,
    content: Rect,
    color: Color,
    written: &str,
    caret_and_selection: Option<&Highlight>,
) {
    let overlay = caret_and_selection.filter(|h| h.caret.is_some() || !h.selection.is_empty());
    if written.is_empty() && overlay.is_none() {
        return;
    }
    out.raw("/Tx BMC\nq\n");
    if geom::width(content) > geom::width(plate) || geom::height(content) > geom::height(plate) {
        out.rect(plate, Float::Shortest);
        out.raw("re\nW\nn\n");
    }
    if let Some(highlight) = overlay {
        for band in &highlight.selection {
            out.raw("q\n");
            out.raw(&color_op_via(SELECTION_FILL, PaintOp::Fill, Float::G6));
            out.rect(*band, Float::Shortest);
            out.raw("re\nf\nQ\n");
        }
    }
    if !written.is_empty() {
        out.raw("BT\n");
        let fill = if overlay.is_some_and(|h| !h.selection.is_empty()) {
            Color::Gray(1.0)
        } else {
            color
        };
        out.raw(&color_op_via(fill, PaintOp::Fill, Float::G6));
        out.raw(written);
        out.raw("ET\n");
    }
    if let Some(caret) = overlay.and_then(|h| {
        if h.selection.is_empty() {
            h.caret
        } else {
            None
        }
    }) {
        out.raw("q\n");
        out.raw(&color_op_via(Color::Gray(0.0), PaintOp::Fill, Float::G6));
        out.rect(caret, Float::Shortest);
        out.raw("re\nf\nQ\n");
    }
    out.raw("Q\nEMC\n");
}

/// Everything the four body builders read that is not the output stream.
///
/// A record rather than six parameters repeated four times, and the split
/// inside it is the one that matters: `widget` is the annotation being drawn
/// and `valued` is the dictionary its **field** value comes from. Those differ
/// only when two `/Fields` entries share a `/T` — see [`field_dict_of`] — and
/// conflating them would give a second control the first one's geometry along
/// with its value.
struct BodyInput<'a> {
    /// The widget annotation. Geometry, flags and `/MK` come from here.
    widget: &'a Dict,
    /// The field dictionary. The value, options and selection come from here.
    valued: &'a Dict,
    /// The plate, already deflated by the border width.
    client: Rect,
    /// The font name and size the inherited `/DA` names.
    appearance: &'a freetext::Appearance,
    /// The colour that same `/DA` sets, black when it sets none.
    color: Color,
    /// The loaded face and the layout metrics taken from it.
    font: &'a TextFont<'a>,
    /// The second face, for characters the `/DA` font's charset does not
    /// cover. `None` leaves every character to the `/DA` font.
    substitute: Option<crate::ap::Substitute<'a>>,
    /// Focused-field caret and selection, if this body is a live edit.
    caret_and_selection: Option<&'a Highlight>,
    /// What a focused field is showing instead of the stored value.
    live: Option<&'a LiveState<'a>>,
}

impl BodyInput<'_> {
    /// The text a text field or an editable combo box draws.
    ///
    /// The live text when a session has one, else the stored `/V`. Borrowed
    /// in the live case and owned in the stored one, which is what the `Cow`
    /// is for — the stored read builds its string out of the dictionary.
    fn text<R: Resolve>(&self, r: &R) -> Cow<'_, str> {
        match self.live {
            Some(live) => Cow::Borrowed(live.text),
            None => Cow::Owned(field_value(self.valued, r)),
        }
    }
}

/// A text field's body, and the comb separators that precede it.
fn text_field<R: Resolve>(out: &mut Content, input: &BodyInput<'_>, r: &R) {
    let (dict, client) = (input.widget, input.client);
    let (appearance, color, font) = (input.appearance, input.color, input.font);
    let flags = flags(dict, r);
    let multi_line = flags & FLAG_MULTILINE != 0;
    let comb = flags & FLAG_COMB != 0;
    let max_len = inherited(dict, names::MAX_LEN, r)
        .and_then(|value| value.as_int())
        .unwrap_or(0);
    let value = input.text(r);

    let mut config = vt::Config {
        plate: client,
        alignment: alignment(dict, r),
        font_size: appearance.size,
        multi_line,
        auto_return: multi_line,
        sub_word: (flags & FLAG_PASSWORD != 0).then_some('*'),
        ..vt::Config::default()
    };
    if max_len > 0 {
        let cells = usize::try_from(max_len).unwrap_or(0);
        if comb {
            config.char_array = cells;
        } else {
            config.limit_char = cells;
        }
    }

    // The separators are written first even though they are decided from the
    // border rather than the value, because the builder concatenates its
    // pieces in that order rather than in the order it computes them.
    comb_separators(out, dict, client, if comb { max_len } else { 0 }, r);

    let (written, content) = set_text(
        &value,
        &config,
        font,
        input.substitute,
        !multi_line,
        if comb {
            vt::edit_ap::Grouping::PerCharacter
        } else {
            vt::edit_ap::Grouping::Continuous
        },
        &appearance.font_name,
        live_shift(input.live),
    );
    wrap_text(
        out,
        client,
        content,
        color,
        &written,
        input.caret_and_selection,
    );
}

/// A push button's caption.
///
/// # What this builds, and what it deliberately does not
///
/// `SetAsPushButton` lays out a caption and an **icon** side by side, in one of
/// seven arrangements chosen by `/MK /TP`, with the icon scaled and positioned
/// by `/MK /IF`. Six of the seven arrangements and the whole icon half are
/// unreachable on this corpus and are not built: **no corpus file carries a
/// `/MK /I`, `/RI` or `/IX` at all**, and with no icon stream every split
/// arrangement collapses to `rcLabel = rcBBox` — the same box the caption-only
/// arrangement uses. The two files that name a `/TP` (`field.fragment`'s
/// `MyPushButton` at `4` and `MyBadPushButton` at `7`, the latter out of range
/// and clamped to `0`) carry no `/MK /CA` either, so both produce nothing on
/// any path. Adding the icon half would be code no golden can distinguish.
///
/// What *is* behavior here:
///
/// - **Both alignments are centred**, horizontally and vertically, and neither
///   is read from the dictionary: `SetAlignmentH(1)`/`SetAlignmentV(1)` are
///   hard-coded, so a push button ignores the `/Q` every other text-bearing
///   field obeys.
/// - **The caption is read only when `/MK` has the key.** `HasMKEntry` gates
///   it, which is a different thing from reading an absent key as empty:
///   a caption of `()` is a caption, and it lays out to nothing anyway.
/// - **A missing `/DA` means size 12, not automatic.** `GetFont()` answers
///   nothing only when the `/DA` string is *empty*; a `/DA` that parses but
///   names no `Tf` answers size **0**, which is the automatic-size request.
///   So the fallback size is 12 and it applies to a widget with no `/DA` at
///   all — the opposite way round from the other three builders, whose
///   fallback is the automatic size.
/// - The clip is written **unconditionally**, not only on overflow, and there
///   are no `/Tx BMC` markers — both unlike [`wrap_text`].
///
/// The text colour falls back to black, which is what [`text_color`] already
/// does for every builder here — the check box's transparent fallback lives in
/// [`widget::text_color`](crate::ap::widget::text_color), a different reader.
fn push_button<R: Resolve>(out: &mut Content, input: &BodyInput<'_>, r: &R) {
    let (dict, client) = (input.widget, input.client);
    let (appearance, color, font) = (input.appearance, input.color, input.font);
    let Some(mk) = dict.dict(names::MK, r) else {
        return;
    };
    if !mk.contains_key(names::CA) {
        return;
    }
    let caption = mk.text(names::CA, r).unwrap_or_default();
    let config = vt::Config {
        plate: client,
        // Hard-coded, not `/Q`.
        alignment: vt::Alignment::Center,
        font_size: appearance.size,
        ..vt::Config::default()
    };
    let (written, content) = set_text(
        &caption,
        &config,
        font,
        input.substitute,
        true,
        vt::edit_ap::Grouping::Continuous,
        &appearance.font_name,
        // A push button draws a caption from `/MK`, never a value, so nothing
        // a session holds can override it and it never scrolls.
        (0.0, 0.0),
    );
    if written.is_empty() {
        return;
    }
    // `GetPushButtonAppStream`'s own `q … Q`, with the clip always present.
    out.raw("q\n");
    out.rect(client, Float::Shortest);
    out.raw("re\nW n\n");
    out.raw("BT\n");
    out.raw(&color_op_via(color, PaintOp::Fill, Float::G6));
    out.raw(&written);
    out.raw("ET\nQ\n");
    let _ = content;
}

/// A comb field's cell separators.
///
/// Drawn only for a solid or dashed border, and only when the border colour
/// resolves. The width comes from the widget's own border; the dash does not
/// — a dashed comb takes the fixed `[3 3] 0`, whatever the border's own
/// pattern says.
fn comb_separators<R: Resolve>(out: &mut Content, dict: &Dict, client: Rect, cells: i64, r: &R) {
    if cells <= 1 {
        return;
    }
    let info = widget::widget_border(dict, r);
    let dashed = match info.style {
        BorderStyle::Solid => false,
        BorderStyle::Dash => true,
        BorderStyle::Beveled | BorderStyle::Inset | BorderStyle::Underline => return,
    };
    let stroke = color_op_via(border_color(dict, r), PaintOp::Stroke, Float::G6);
    if stroke.is_empty() {
        return;
    }

    out.raw("q\n");
    out.num(info.width, Float::G6);
    out.raw("w\n");
    out.raw(&stroke);
    if dashed {
        out.raw("[3 3] 0 d\n");
    } else {
        out.raw(" 2 J 0 j\n");
    }
    #[allow(clippy::cast_precision_loss)]
    let total = cells as f32;
    let width = geom::width(client);
    for cell in 1..cells {
        #[allow(clippy::cast_precision_loss)]
        let left = geom::left(client) + (width / total) * cell as f32;
        out.point(left, geom::bottom(client), Float::Shortest);
        out.raw("m\n");
        out.point(left, geom::top(client), Float::Shortest);
        out.raw("l\n");
        out.raw("S\n");
    }
    out.raw("Q\n");
}

/// A combo box's body: one line, then the drop button.
fn combo_box<R: Resolve>(out: &mut Content, input: &BodyInput<'_>, r: &R) {
    let (valued, client) = (input.valued, input.client);
    let (appearance, color, font) = (input.appearance, input.color, input.font);
    let button = geom::normalize(geom::rect(
        geom::right(client) - DROP_BUTTON_WIDTH,
        geom::bottom(client),
        geom::right(client),
        geom::top(client),
    ));
    let plate = geom::normalize(geom::rect(
        geom::left(client),
        geom::bottom(client),
        geom::left(button),
        geom::top(client),
    ));

    // A focused combo box draws the edit box's own text, whichever option it
    // does or does not name: an editable one is being typed into, and a
    // read-only one has had its text set from the row the user picked. Either
    // way the session already resolved the label, so no lookup happens here.
    //
    // Unfocused, the **label** of the selected option, or the value itself
    // when nothing is selected — which is how a combo box whose `/V` names no
    // option still shows what the file says.
    let options = options(valued, r);
    let text: Cow<'_, str> = match input.live {
        Some(live) => Cow::Borrowed(live.text),
        None => match selected_indices(valued, &options, r).first().copied() {
            Some(index) => options
                .get(index)
                .map_or(Cow::Borrowed(""), |option| Cow::Owned(option.label.clone())),
            None => Cow::Owned(field_value(valued, r)),
        },
    };

    let config = vt::Config {
        plate,
        font_size: appearance.size,
        ..vt::Config::default()
    };
    let (written, content) = set_text(
        &text,
        &config,
        font,
        input.substitute,
        true,
        vt::edit_ap::Grouping::Continuous,
        &appearance.font_name,
        live_shift(input.live),
    );
    wrap_text(
        out,
        plate,
        content,
        color,
        &written,
        input.caret_and_selection,
    );
    out.raw(&shapes::drop_button(button));
}

/// A list box's body: every option from `/TI` down.
fn list_box<R: Resolve>(out: &mut Content, input: &BodyInput<'_>, r: &R) {
    let (dict, valued) = (input.widget, input.valued);
    // A **live** list box reserves the scroll bar's width; a generated
    // appearance does not — see [`LIST_SCROLLBAR_WIDTH`] for why the two
    // disagree and which upstream function each one is.
    let client = if input.live.is_some() {
        geom::rect(
            geom::left(input.client),
            geom::bottom(input.client),
            geom::right(input.client) - LIST_SCROLLBAR_WIDTH,
            geom::top(input.client),
        )
    } else {
        input.client
    };
    let (appearance, color, font) = (input.appearance, input.color, input.font);
    let options = options(valued, r);
    // A focused list box's selection lives in the session, not in `/V` — the
    // arrow keys move it long before anything is committed — and the same is
    // true of the first visible row, which scrolling changes. Both are read
    // from the dictionary only when no session is holding them.
    let selected: Cow<'_, [usize]> = input.live.map_or_else(
        || Cow::Owned(selected_indices(valued, &options, r)),
        |live| Cow::Borrowed(live.selected),
    );
    let top = input.live.map_or_else(
        || {
            usize::try_from(
                inherited(dict, names::TI, r)
                    .and_then(|value| value.as_int())
                    .unwrap_or(0),
            )
            .unwrap_or(0)
        },
        |live| live.top_visible,
    );
    let (shift_x, shift_y) = live_shift(input.live);

    // Each row is laid out into a plate of **zero height**, so the layout
    // reports the row's own extent rather than the box's.
    let plate = geom::rect(geom::left(client), 0.0, geom::right(client), 0.0);
    let config = vt::Config {
        plate,
        font_size: if geom::is_float_zero(appearance.size) {
            LIST_ROW_DEFAULT_SIZE
        } else {
            appearance.size
        },
        ..vt::Config::default()
    };

    let mut rows = Content::new();
    let mut y = geom::top(client);
    for (index, option) in options.iter().enumerate().skip(top) {
        let layout = vt::layout(&option.label, &config, &font.metrics);
        let height = geom::height(layout.content_rect_pdf(plate));
        let written = vt::edit_ap::generate(
            &layout,
            &config,
            &font.metrics,
            (shift_x, y + shift_y),
            vt::edit_ap::Grouping::Continuous,
            |code| face_for(font, input.substitute, &appearance.font_name, code),
        );
        if selected.contains(&index) {
            rows.raw("q\n");
            rows.raw(&color_op_via(SELECTION_FILL, PaintOp::Fill, Float::G6));
            // The band moves with its row, so a scrolled box keeps the fill
            // under the text it belongs to rather than where the row rested.
            rows.rect(
                geom::rect(
                    geom::left(client) + shift_x,
                    y + shift_y - height,
                    geom::right(client) + shift_x,
                    y + shift_y,
                ),
                Float::Shortest,
            );
            rows.raw("re\nf\nQ\n");
            rows.raw("BT\n");
            rows.raw(&color_op_via(Color::Gray(1.0), PaintOp::Fill, Float::G6));
        } else {
            rows.raw("BT\n");
            rows.raw(&color_op_via(color, PaintOp::Fill, Float::G6));
        }
        rows.raw(&written);
        rows.raw("ET\n");
        y -= height;
    }

    if rows.is_empty() {
        return;
    }
    out.raw("/Tx BMC\nq\n");
    out.rect(client, Float::Shortest);
    out.raw("re\nW\nn\n");
    out.raw(rows.as_str());
    out.raw("Q\nEMC\n");
}

/// One `/Opt` entry: the value a selection is matched against and the label
/// that is drawn.
///
/// A plain string option is both. A two-element array names them apart, and a
/// shorter array leaves the missing half **empty** rather than reusing the
/// other.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    /// What a selection is compared against.
    pub value: String,
    /// What is drawn.
    pub label: String,
}

/// A choice field's options, in file order.
#[must_use]
pub fn options<R: Resolve>(dict: &Dict, r: &R) -> Vec<Choice> {
    let Some(array) = inherited(dict, names::OPT, r).and_then(|value| value.as_array().cloned())
    else {
        return Vec::new();
    };
    (0..array.len())
        .map(|index| {
            let entry = array.get(index, r);
            if let Some(pair) = entry.as_ref().and_then(|object| object.as_array()) {
                return Choice {
                    value: text_at(pair, 0, r),
                    label: text_at(pair, 1, r),
                };
            }
            let text = entry.as_deref().map(Object::to_text).unwrap_or_default();
            Choice {
                value: text.clone(),
                label: text,
            }
        })
        .collect()
}

/// One element of an option pair, as text.
fn text_at<R: Resolve>(array: &Array, index: usize, r: &R) -> String {
    array
        .get(index, r)
        .as_deref()
        .map(Object::to_text)
        .unwrap_or_default()
}

/// A field's value, as the single string a text or combo field shows.
///
/// An array-valued `/V` — which a multi-select list box carries — reports its
/// **first** element.
#[must_use]
pub fn field_value<R: Resolve>(dict: &Dict, r: &R) -> String {
    let Some(value) = inherited(dict, names::V, r) else {
        return String::new();
    };
    match value.as_array() {
        Some(array) => array
            .get(0, r)
            .as_deref()
            .map(Object::to_text)
            .unwrap_or_default(),
        None => value.to_text(),
    }
}

/// Which options are selected, as indices into [`options`].
///
/// `/V` decides when it is present and `/I` only when it is not. The lookup
/// compares each entry's **text** against the option values, so an integer
/// index matches nothing — which is why the `/I` fallback selects nothing at
/// all. A number-valued selection object is the one shape read as an index
/// directly, and it is read before the text comparison rather than through it.
///
/// # This is the appearance's answer, and it is not the only one
///
/// Upstream reads `/V` and `/I` **two different ways**, and they disagree.
/// This function is the one that draws:
/// `CPDFSDK_AppStream::SetAsListBox` asks `CPDF_FormField::GetSelectedIndex`,
/// which reads `GetValueOrSelectedIndicesObject` — `/V` first, `/I` only in
/// its absence — and then matches each entry's text against the option
/// values (`core/fpdfdoc/cpdf_formfield.cpp`, `GetSelectedIndex`). An integer
/// index has no text that names an option, so `/I` alone draws no band.
///
/// The *interaction* reader is the other way round and lives in
/// [`crate::form::selected_indices_for_interaction`]: `/I` first, as indices,
/// with `/V` as the fallback. `listbox_form.pdf`'s
/// `Listbox_MultiSelectMultipleIndices` is the fixture where the two part
/// company — the oracle's `--annot` dump shows five text objects and **no**
/// path for it, while an embedder asking about its rows is told 1 and 3 are
/// selected. Making this function follow the interaction rule turns that
/// fixture's row red; the two readers have to stay apart.
#[must_use]
pub fn selected_indices<R: Resolve>(dict: &Dict, options: &[Choice], r: &R) -> Vec<usize> {
    let Some(value) = inherited(dict, names::V, r).or_else(|| inherited(dict, names::I, r)) else {
        return Vec::new();
    };
    if let Some(index) = value.as_int() {
        return usize::try_from(index).into_iter().collect();
    }
    let wanted: Vec<String> = match value.as_array() {
        Some(array) => (0..array.len())
            .map(|index| {
                array
                    .get(index, r)
                    .as_deref()
                    .map(Object::to_text)
                    .unwrap_or_default()
            })
            .collect(),
        None => vec![value.to_text()],
    };
    wanted
        .into_iter()
        .filter_map(|text| options.iter().position(|option| option.value == text))
        .collect()
}

/// The colour a widget's border draws in.
fn border_color<R: Resolve>(dict: &Dict, r: &R) -> Color {
    dict.dict(names::MK, r)
        .and_then(|mk| mk.array(names::BC, r))
        .map_or(Color::Transparent, |array| Color::from_array(&array))
}

#[cfg(test)]
mod tests {
    use super::{
        CARET_WIDTH, Choice, Highlight, Kind, LiveState, field_dict_of, field_value, options,
        selected_indices,
    };
    use crate::ap::{TextFont, freetext};
    use crate::geom;
    use pdfrum_object::{Array, Dict, Name, NoResolve, Object, PdfString};

    fn dict(pairs: &[(&str, Object)]) -> Dict {
        Dict::from_pairs(
            pairs
                .iter()
                .map(|(k, v)| (Name::from(*k), v.clone()))
                .collect::<Vec<_>>(),
        )
    }

    /// A widget in `/Fields` under the given name, holding the given value.
    fn shared(field_name: &str, value: &str) -> Dict {
        dict(&[
            ("Subtype", Object::Name(Name::from("Widget"))),
            ("FT", Object::Name(Name::from("Tx"))),
            ("T", text(field_name)),
            ("V", text(value)),
        ])
    }

    #[test]
    fn a_second_fields_entry_sharing_a_name_takes_the_firsts_value() {
        let (first, second) = (shared("Same", "Hello, world"), shared("Same", ""));
        let form = dict(&[(
            "Fields",
            Object::Array(Array::of([
                Object::Dict(first.clone()),
                Object::Dict(second.clone()),
            ])),
        )]);
        // The first entry *is* the field, so nothing else applies to it.
        assert_eq!(field_dict_of(&first, Some(&form), &NoResolve), None);
        // The second reads its value from the first.
        assert_eq!(
            field_dict_of(&second, Some(&form), &NoResolve).as_ref(),
            Some(&first)
        );
        assert_eq!(field_value(&second, &NoResolve), "");
        assert_eq!(field_value(&first, &NoResolve), "Hello, world");
    }

    #[test]
    fn a_widget_the_form_does_not_share_a_name_with_is_its_own_field() {
        let alone = shared("Alone", "mine");
        let form = dict(&[(
            "Fields",
            Object::Array(Array::of([Object::Dict(shared("Other", "theirs"))])),
        )]);
        assert_eq!(field_dict_of(&alone, Some(&form), &NoResolve), None);
        // And with no form at all.
        assert_eq!(field_dict_of(&alone, None, &NoResolve), None);
    }

    #[test]
    fn a_widget_under_a_parent_inherits_rather_than_sharing() {
        // A parented widget is not a top-level `/Fields` entry, so the
        // name walk cannot apply to it however its `/T` reads — its value
        // comes up the `/Parent` chain as it always has.
        let mut kid = shared("Same", "");
        kid.push(
            Name::from("Parent"),
            Object::Dict(dict(&[("V", text("from the parent"))])),
        );
        let form = dict(&[(
            "Fields",
            Object::Array(Array::of([Object::Dict(shared("Same", "elsewhere"))])),
        )]);
        assert_eq!(field_dict_of(&kid, Some(&form), &NoResolve), None);
    }

    fn strings(values: &[&str]) -> Object {
        Object::Array(Array::of(
            values
                .iter()
                .map(|s| Object::Str(PdfString::literal(s.as_bytes()))),
        ))
    }

    fn text(value: &str) -> Object {
        Object::Str(PdfString::literal(value.as_bytes()))
    }

    /// A text string in UTF-16BE behind the byte-order mark, which is how a
    /// `/V` spells a code point `PDFDocEncoding` has no byte for.
    fn utf16(value: &str) -> Object {
        let mut bytes = vec![0xFE, 0xFF];
        for unit in value.encode_utf16() {
            bytes.extend_from_slice(&unit.to_be_bytes());
        }
        Object::Str(PdfString::literal(bytes))
    }

    fn numbers(values: &[f32]) -> Object {
        Object::Array(Array::of(values.iter().copied().map(Object::from)))
    }

    /// A catalog whose form carries one usable font resource under `Helv`.
    fn catalog() -> Dict {
        dict(&[(
            "AcroForm",
            Object::Dict(dict(&[(
                "DR",
                Object::Dict(dict(&[(
                    "Font",
                    Object::Dict(dict(&[("Helv", Object::Dict(freetext::fallback_font()))])),
                )])),
            )])),
        )])
    }

    /// A widget of one field type, with the stock `/DA` every fixture uses.
    fn widget_of(kind: &str, extra: &[(&str, Object)]) -> Dict {
        let mut pairs = vec![
            ("Subtype", Object::Name(Name::from("Widget"))),
            ("FT", Object::Name(Name::from(kind))),
            ("Rect", numbers(&[100.0, 100.0, 200.0, 130.0])),
            (
                "DA",
                Object::Str(PdfString::literal(b"0 0 0 rg /Helv 12 Tf")),
            ),
        ];
        pairs.extend_from_slice(extra);
        dict(&pairs)
    }

    /// One field body's content stream, set with a stock Helvetica.
    ///
    /// A real face rather than the layout stub, because the builders under
    /// test are the ones that *encode* as well as place, and encoding needs a
    /// font's reverse `ToUnicode`. The numbers below are therefore Helvetica's
    /// and not the stub's ten-per-character.
    fn body(widget: &Dict) -> Option<String> {
        body_with(widget, None)
    }

    fn body_with(widget: &Dict, caret_and_selection: Option<&Highlight>) -> Option<String> {
        body_live(widget, caret_and_selection, None)
    }

    /// The same body, for a widget a session is editing.
    fn live_body(widget: &Dict, live: &LiveState<'_>) -> Option<String> {
        body_live(widget, None, Some(live))
    }

    fn body_live(
        widget: &Dict,
        caret_and_selection: Option<&Highlight>,
        live: Option<&LiveState<'_>>,
    ) -> Option<String> {
        let cache = pdfrum_font::FontCache::new();
        let face =
            pdfrum_font::Font::load_standard(pdfrum_font::subst::StandardFont::Helvetica, &cache);
        let width = |code: u32| TextFont::char_width(&face, code);
        let font = TextFont {
            metrics: TextFont::metrics_of(&face, &width),
            font: &face,
        };
        super::generate(
            widget,
            &catalog(),
            &font,
            None,
            &NoResolve,
            caret_and_selection,
            live,
        )
        .map(|body| String::from_utf8_lossy(&body.stream).into_owned())
    }

    /// The widgets the existing body tests already exercise.
    fn widget_fixtures() -> Vec<(&'static str, Dict)> {
        vec![
            ("tx_hello", widget_of("Tx", &[("V", text("Hello"))])),
            (
                "tx_comb",
                widget_of(
                    "Tx",
                    &[
                        ("V", text("ab")),
                        ("Ff", Object::Int(1 << 24)),
                        ("MaxLen", Object::Int(3)),
                    ],
                ),
            ),
            (
                "ch_combo",
                widget_of(
                    "Ch",
                    &[
                        ("Ff", Object::Int(1 << 17)),
                        (
                            "Opt",
                            Object::Array(Array::of([
                                strings(&["a", "Apple"]),
                                strings(&["b", "Banana"]),
                            ])),
                        ),
                        ("V", text("b")),
                    ],
                ),
            ),
            (
                "ch_list_v",
                widget_of(
                    "Ch",
                    &[("Opt", strings(&["Dog", "Cat"])), ("V", text("Cat"))],
                ),
            ),
            (
                "ch_list_ti",
                widget_of(
                    "Ch",
                    &[("Opt", strings(&["a", "b", "c"])), ("TI", Object::Int(2))],
                ),
            ),
            (
                "ch_list_i",
                widget_of(
                    "Ch",
                    &[
                        ("Opt", strings(&["a", "b", "c"])),
                        (
                            "I",
                            Object::Array(Array::of([Object::Int(0), Object::Int(2)])),
                        ),
                    ],
                ),
            ),
            // The two that generate nothing at all: an empty text field and a
            // caption-only push button. Their `<none>` rows are as much a part
            // of the identity as the streams above.
            ("tx_empty", widget_of("Tx", &[])),
            (
                "btn",
                widget_of(
                    "Btn",
                    &[("MK", Object::Dict(dict(&[("CA", text("Push"))])))],
                ),
            ),
        ]
    }

    #[test]
    fn a_choice_fields_kind_follows_its_combo_flag() {
        let list = dict(&[("FT", Object::Name(Name::from("Ch")))]);
        assert_eq!(Kind::of(&list, &NoResolve), Some(Kind::List));
        let combo = dict(&[
            ("FT", Object::Name(Name::from("Ch"))),
            ("Ff", Object::Int(1 << 17)),
        ]);
        assert_eq!(Kind::of(&combo, &NoResolve), Some(Kind::Combo));
        let text = dict(&[("FT", Object::Name(Name::from("Tx")))]);
        assert_eq!(Kind::of(&text, &NoResolve), Some(Kind::Text));
        // A button sets no text and reaches no builder here.
        let button = dict(&[("FT", Object::Name(Name::from("Btn")))]);
        assert_eq!(Kind::of(&button, &NoResolve), None);
    }

    #[test]
    fn an_option_pair_names_its_value_and_its_label_apart() {
        let field = dict(&[(
            "Opt",
            Object::Array(Array::of([
                strings(&["foo", "Foo"]),
                text("bar"),
                Object::Array(Array::of([text("solo")])),
            ])),
        )]);
        assert_eq!(
            options(&field, &NoResolve),
            vec![
                Choice {
                    value: "foo".to_owned(),
                    label: "Foo".to_owned()
                },
                // A plain string is both halves.
                Choice {
                    value: "bar".to_owned(),
                    label: "bar".to_owned()
                },
                // A one-element array leaves the label empty rather than
                // reusing the value.
                Choice {
                    value: "solo".to_owned(),
                    label: String::new()
                },
            ]
        );
    }

    #[test]
    fn a_selection_by_index_alone_selects_nothing() {
        // The fallback compares an entry's *text* against the option values,
        // and an integer's text is empty, so no option matches. This is the
        // behavior that tells the two producers of this appearance apart.
        let by_index = dict(&[
            ("Opt", strings(&["Albania", "Belgium", "Croatia"])),
            (
                "I",
                Object::Array(Array::of([Object::Int(1), Object::Int(2)])),
            ),
        ]);
        let choices = options(&by_index, &NoResolve);
        assert!(selected_indices(&by_index, &choices, &NoResolve).is_empty());
    }

    #[test]
    fn a_value_selects_every_option_it_names_and_a_present_value_hides_the_indices() {
        let field = dict(&[
            ("Opt", strings(&["Alpha", "Beta", "Gamma", "Delta"])),
            ("V", strings(&["Delta", "Beta"])),
            // Ignored outright: `/V` is present.
            ("I", Object::Array(Array::of([Object::Int(0)]))),
        ]);
        let choices = options(&field, &NoResolve);
        assert_eq!(selected_indices(&field, &choices, &NoResolve), vec![3, 1]);
    }

    #[test]
    fn a_multi_valued_field_shows_only_its_first_value() {
        let field = dict(&[("V", strings(&["one", "two"]))]);
        assert_eq!(field_value(&field, &NoResolve), "one");
        let single = dict(&[("V", text("only"))]);
        assert_eq!(field_value(&single, &NoResolve), "only");
        assert_eq!(field_value(&Dict::new(), &NoResolve), "");
    }

    #[test]
    fn a_value_naming_no_option_selects_nothing_but_still_reads_as_text() {
        let field = dict(&[("Opt", strings(&["a", "b"])), ("V", text("z"))]);
        let choices = options(&field, &NoResolve);
        assert!(selected_indices(&field, &choices, &NoResolve).is_empty());
        assert_eq!(field_value(&field, &NoResolve), "z");
    }

    #[test]
    fn a_field_with_nothing_to_show_produces_no_body_at_all() {
        // Not an empty stream: nothing, so the caller writes chrome alone and
        // the appearance reports no page objects.
        assert_eq!(body(&widget_of("Tx", &[])), None);
        assert_eq!(body(&widget_of("Ch", &[])), None);
        // And a field type that sets no text never reaches a builder.
        assert_eq!(body(&widget_of("Btn", &[("V", text("x"))])), None);
    }

    #[test]
    fn a_text_fields_value_is_set_between_the_markers() {
        let got = body(&widget_of("Tx", &[("V", text("Hi"))])).expect("a body");
        assert!(got.starts_with("/Tx BMC\nq\nBT\n"), "{got}");
        assert!(got.contains("0 0 0 rg\n"), "{got}");
        assert!(got.contains("(Hi) Tj\n"), "{got}");
        assert!(got.ends_with("ET\nQ\nEMC\n"), "{got}");
        // A single line inside a thirty-unit box does not overflow, so no clip
        // is written at all.
        assert!(!got.contains("W\nn\n"), "{got}");
    }

    #[test]
    fn a_password_field_sets_bullets_rather_than_its_value() {
        let got = body(&widget_of(
            "Tx",
            &[("V", text("secret")), ("Ff", Object::Int(1 << 13))],
        ))
        .expect("a body");
        assert!(got.contains("(******) Tj\n"), "{got}");
        assert!(!got.contains("secret"), "{got}");
    }

    #[test]
    fn a_comb_field_places_every_character_on_its_own() {
        let got = body(&widget_of(
            "Tx",
            &[
                ("V", text("abc")),
                ("Ff", Object::Int(1 << 24)),
                ("MaxLen", Object::Int(4)),
                // A border colour, so the separators between the four cells
                // are drawn as well as the characters.
                (
                    "MK",
                    Object::Dict(dict(&[("BC", numbers(&[0.0, 0.0, 0.0]))])),
                ),
            ],
        ))
        .expect("a body");
        // One show operator per character rather than one for the run.
        assert_eq!(got.matches(" Tj\n").count(), 3);
        // Three separators for four cells, each its own stroked segment.
        assert_eq!(got.matches("S\n").count(), 3);
        // The separators precede the text even though the border decides them.
        assert!(
            got.find("S\n") < got.find("/Tx BMC"),
            "separators must come first: {got}"
        );
    }

    #[test]
    fn a_comb_field_with_no_border_colour_draws_no_separators() {
        let got = body(&widget_of(
            "Tx",
            &[
                ("V", text("ab")),
                ("Ff", Object::Int(1 << 24)),
                ("MaxLen", Object::Int(3)),
            ],
        ))
        .expect("a body");
        assert!(!got.contains("S\n"), "{got}");
    }

    #[test]
    fn a_combo_box_shows_the_selected_options_label_and_then_its_button() {
        let got = body(&widget_of(
            "Ch",
            &[
                ("Ff", Object::Int(1 << 17)),
                (
                    "Opt",
                    Object::Array(Array::of([
                        strings(&["a", "Apple"]),
                        strings(&["b", "Banana"]),
                    ])),
                ),
                ("V", text("b")),
            ],
        ))
        .expect("a body");
        // The label, not the value.
        assert!(got.contains("(Banana) Tj\n"), "{got}");
        assert!(!got.contains("(b) Tj"), "{got}");
        // The drop button's pale grey, through the six-digit writer.
        assert!(got.contains("0.862745 g\n"), "{got}");
        assert!(got.find("0.862745 g\n") > got.find("EMC"), "{got}");
    }

    #[test]
    fn a_combo_box_whose_value_names_no_option_shows_the_value_itself() {
        let got = body(&widget_of(
            "Ch",
            &[
                ("Ff", Object::Int(1 << 17)),
                ("Opt", strings(&["Apple"])),
                ("V", text("Pear")),
            ],
        ))
        .expect("a body");
        assert!(got.contains("(Pear) Tj\n"), "{got}");
    }

    #[test]
    fn a_list_box_stacks_its_rows_and_paints_only_the_selected_one() {
        let got = body(&widget_of(
            "Ch",
            &[("Opt", strings(&["Dog", "Cat"])), ("V", text("Cat"))],
        ))
        .expect("a body");
        // A list box always clips, unlike the other two.
        assert!(got.contains("re\nW\nn\n"), "{got}");
        assert_eq!(got.matches("BT\n").count(), 2);
        // One selection rectangle, in the six-digit spelling of 0/51/113.
        assert_eq!(got.matches("0 0.2 0.443137 rg\n").count(), 1);
        // And the selected row's text is white where the other is the `/DA`'s
        // black.
        assert!(got.contains("1 g\n"), "{got}");
        assert!(
            got.contains("(Dog) Tj\n") && got.contains("(Cat) Tj\n"),
            "{got}"
        );
    }

    #[test]
    fn only_a_live_list_box_keeps_the_scroll_bars_width_clear() {
        // The widget fixtures are 100 wide with a one-unit border, so the
        // client is 98 and the narrowed box is 86.
        let list = widget_of(
            "Ch",
            &[("Opt", strings(&["Dog", "Cat"])), ("V", text("Cat"))],
        );

        // Generated: the whole body, because `GenerateListBoxAP` knows nothing
        // about windows or scroll bars.
        let stored = body(&list).expect("a body");
        assert!(stored.contains("1 1 98 28 re\n"), "{stored}");
        assert!(stored.contains(" 98 11.244 re\n"), "{stored}");

        // Live: twelve units narrower — and narrower even here, where two
        // options in a 28-unit box could never scroll. That the reservation
        // does not wait for there to be something to scroll is the rule.
        let live = live_body(
            &list,
            &LiveState {
                selected: &[1],
                ..LiveState::default()
            },
        )
        .expect("a live body");
        assert!(live.contains("1 1 86 28 re\n"), "{live}");
        assert!(live.contains(" 86 11.244 re\n"), "{live}");
    }

    #[test]
    fn a_list_box_starts_at_its_top_visible_index() {
        let got = body(&widget_of(
            "Ch",
            &[("Opt", strings(&["a", "b", "c"])), ("TI", Object::Int(2))],
        ))
        .expect("a body");
        assert_eq!(got.matches("BT\n").count(), 1);
        assert!(got.contains("(c) Tj\n"), "{got}");
    }

    #[test]
    fn a_list_box_selected_only_by_index_paints_no_row() {
        // The producer this module ports reads `/V` and falls back to `/I`,
        // and the fallback matches nothing. The *other* producer — the one
        // gated on `/NeedAppearances` — would paint two rectangles here.
        let got = body(&widget_of(
            "Ch",
            &[
                ("Opt", strings(&["a", "b", "c"])),
                (
                    "I",
                    Object::Array(Array::of([Object::Int(0), Object::Int(2)])),
                ),
            ],
        ))
        .expect("a body");
        assert!(!got.contains("0 0.2 0.443137 rg\n"), "{got}");
        assert_eq!(got.matches("BT\n").count(), 3);
    }

    #[test]
    fn none_is_byte_identical_on_every_existing_widget_fixture() {
        // `tests/data/unfocused_field_bodies.txt` was captured by running the
        // fixtures below through the generator *as it stood before the
        // overlay parameter existed*. Comparing against it — rather than
        // against a second `None` call, which would only restate itself — is
        // what makes this a byte-identity check: every operator, every
        // rounded coordinate and every absent operator is pinned.
        let want = include_str!("../../tests/data/unfocused_field_bodies.txt");
        let mut got = String::new();
        for (name, widget) in widget_fixtures() {
            got.push_str("=== ");
            got.push_str(name);
            got.push_str(" ===\n");
            match body_with(&widget, None) {
                Some(stream) => got.push_str(&stream),
                None => got.push_str("<none>\n"),
            }
        }
        assert_eq!(got, want, "the unfocused path changed shape");
    }

    #[test]
    fn a_caret_is_a_filled_rectangle_four_tenths_wide() {
        let caret = geom::rect(110.0, 104.0, 110.0 + CARET_WIDTH, 120.0);
        let highlight = Highlight {
            caret: Some(caret),
            selection: Vec::new(),
        };
        let got =
            body_with(&widget_of("Tx", &[("V", text("Hello"))]), Some(&highlight)).expect("a body");
        let none = body(&widget_of("Tx", &[("V", text("Hello"))])).expect("a body");
        assert_ne!(got, none, "a caret must change the stream");
        assert!(got.contains("0 g\n"), "{got}");
        assert!(got.contains("re\nf\n"), "{got}");
        // After the text, not before it.
        let et = got.find("ET\n").expect("text");
        let re = got.find("re\nf\n").expect("caret fill");
        assert!(re > et, "caret sits on top of the text: {got}");
        assert!((CARET_WIDTH - 0.4).abs() < f32::EPSILON);
    }

    #[test]
    fn a_selection_band_is_filled_behind_white_text() {
        let band = geom::rect(100.0, 100.0, 140.0, 130.0);
        let highlight = Highlight {
            caret: Some(geom::rect(140.0, 104.0, 140.4, 120.0)),
            selection: vec![band],
        };
        let got =
            body_with(&widget_of("Tx", &[("V", text("Hello"))]), Some(&highlight)).expect("a body");
        // The list-box selection colour, six-digit spelling of 0/51/113.
        assert!(got.contains("0 0.2 0.443137 rg\n"), "{got}");
        // Selected text is white.
        assert!(got.contains("1 g\n"), "{got}");
        // A live selection hides the caret (brief §1.20.5 rule 7).
        let sel = got.find("0 0.2 0.443137 rg\n").expect("band");
        let bt = got.find("BT\n").expect("text");
        assert!(sel < bt, "selection sits behind the text: {got}");
        assert!(
            !got.contains("0 g\n"),
            "a selection suppresses the caret: {got}"
        );
    }

    #[test]
    fn an_empty_field_with_a_caret_still_emits_a_body() {
        let highlight = Highlight {
            caret: Some(geom::rect(101.0, 101.0, 101.4, 129.0)),
            selection: Vec::new(),
        };
        let got = body_with(&widget_of("Tx", &[]), Some(&highlight)).expect("caret-only body");
        assert!(got.contains("/Tx BMC\n"), "{got}");
        assert!(got.contains("re\nf\n"), "{got}");
        assert!(!got.contains("BT\n"), "{got}");
    }

    /// A value mixing Latin and Hebrew is set in **two** faces: the `/DA`
    /// font writes the Latin, and the second face writes the Hebrew under its
    /// own alias, with a `Tf` at each crossing.
    ///
    /// The dictionary the second face adds is in the appearance's own
    /// resources beside the first — `AddFontToAnnotDict`
    /// (`core/fpdfdoc/cpdf_bafontmap.cpp:318-357`) — because a `Tf` naming a
    /// resource the stream does not carry sets no font at all.
    #[test]
    fn a_value_the_da_font_cannot_write_switches_to_a_second_face() {
        let cache = pdfrum_font::FontCache::new();
        let options = pdfrum_font::SubstitutionOptions::default();
        let mut ctx = pdfrum_page::BuildContext::with_substitution(options);
        let fonts = crate::ap::FormFonts::load(&catalog(), &NoResolve, &mut ctx);
        let substitute = fonts
            .substitute(pdfrum_font::subst::Charset::Hebrew)
            .expect("a Hebrew substitute");

        let face =
            pdfrum_font::Font::load_standard(pdfrum_font::subst::StandardFont::Helvetica, &cache);
        let charset = crate::ap::font_map::font_charset(&face);
        let width = |code: u32| {
            if crate::ap::font_map::da_font_writes(&face, charset, code) {
                TextFont::char_width(&face, code)
            } else {
                crate::ap::font_map::substitute_width(substitute.font, code)
            }
        };
        let font = TextFont {
            metrics: TextFont::metrics_of(&face, &width),
            font: &face,
        };
        // `ab` in the `/DA` font, then two Hebrew letters in the second.
        //
        // Written UTF-16BE behind a byte-order mark, which is the only
        // spelling a `/V` has for a code point outside PDFDocEncoding — the
        // `bug_725389` fixture writes its Hebrew the same way. A UTF-8 `/V`
        // would be read one byte per character and never reach the second
        // face at all.
        let widget = widget_of("Tx", &[("V", utf16("ab\u{5D0}\u{5D1}"))]);
        let body = super::generate(
            &widget,
            &catalog(),
            &font,
            Some(substitute),
            &NoResolve,
            None,
            None,
        )
        .expect("a body");
        // Compared as **bytes**. A code-page byte is not valid UTF-8, so
        // reading the stream as text replaces it with a replacement character
        // and the assertions below could not tell 0xE0 from 0xD0 — which is
        // the whole difference this change makes.
        let stream = body.stream.clone();
        let has = |needle: &[u8]| stream.windows(needle.len()).any(|w| w == needle);

        // Both faces set text, so both name themselves.
        assert!(has(b"/Helv 12 Tf\n"), "{stream:02X?}");
        let mut tf = b"/".to_vec();
        tf.extend_from_slice(substitute.alias.as_bytes());
        tf.extend_from_slice(b" 12 Tf\n");
        assert!(has(&tf), "{stream:02X?}");
        // Each Hebrew letter is written as its code-page byte — aleph 0xE0,
        // bet 0xE1, spelled in octal because a show operand's high bytes are
        // — and not as the low byte of its code point, which would be 0xD0
        // and 0xD1 and is the mojibake this replaces.
        assert!(has(b"\\340"), "aleph as 0xE0: {stream:02X?}");
        assert!(has(b"\\341"), "bet as 0xE1: {stream:02X?}");
        assert!(!has(b"\\320"), "no low-byte aleph: {stream:02X?}");
        assert!(!has(b"\\321"), "no low-byte bet: {stream:02X?}");

        let resources = body.font_resources.expect("font resources");
        assert!(
            resources.contains_key(&pdfrum_object::Name::from("Helv")),
            "{resources:?}"
        );
        assert!(resources.contains_key(substitute.alias), "{resources:?}");
    }

    /// A value the `/DA` font can write entirely is one run in one face, and
    /// the second face's resource is the only thing the substitute adds.
    #[test]
    fn a_latin_value_writes_no_font_switch_even_with_a_substitute_in_hand() {
        let cache = pdfrum_font::FontCache::new();
        let options = pdfrum_font::SubstitutionOptions::default();
        let mut ctx = pdfrum_page::BuildContext::with_substitution(options);
        let fonts = crate::ap::FormFonts::load(&catalog(), &NoResolve, &mut ctx);
        let substitute = fonts
            .substitute(pdfrum_font::subst::Charset::Hebrew)
            .expect("a Hebrew substitute");
        let face =
            pdfrum_font::Font::load_standard(pdfrum_font::subst::StandardFont::Helvetica, &cache);
        let width = |code: u32| TextFont::char_width(&face, code);
        let font = TextFont {
            metrics: TextFont::metrics_of(&face, &width),
            font: &face,
        };
        let widget = widget_of("Tx", &[("V", text("Hello"))]);
        let offered = super::generate(
            &widget,
            &catalog(),
            &font,
            Some(substitute),
            &NoResolve,
            None,
            None,
        )
        .expect("a body");
        let plain = super::generate(&widget, &catalog(), &font, None, &NoResolve, None, None)
            .expect("a body");
        assert_eq!(
            offered.stream, plain.stream,
            "a substitute nothing reaches writes the same stream"
        );
    }

    #[test]
    fn a_live_text_field_draws_what_the_session_holds_rather_than_its_value() {
        // The whole point of the override: an empty field being typed into
        // must show the typing, and a field whose `/V` still reads the old
        // value must not show it.
        let empty = widget_of("Tx", &[]);
        assert_eq!(body(&empty), None, "an empty field draws nothing");
        let typed = live_body(
            &empty,
            &LiveState {
                text: "Hello",
                ..LiveState::default()
            },
        )
        .expect("a live body");
        assert!(typed.contains("(Hello) Tj\n"), "{typed}");

        let stale = widget_of("Tx", &[("V", text("stored"))]);
        let live = live_body(
            &stale,
            &LiveState {
                text: "edited",
                ..LiveState::default()
            },
        )
        .expect("a live body");
        assert!(live.contains("(edited) Tj\n"), "{live}");
        assert!(!live.contains("stored"), "{live}");
    }

    #[test]
    fn a_live_text_field_still_obeys_the_flags_its_dictionary_sets() {
        // The override replaces the *value*, not the layout: a password field
        // bullets the live text exactly as it bullets the stored one, and a
        // comb field still places one character per cell.
        let secret = live_body(
            &widget_of("Tx", &[("V", text("old")), ("Ff", Object::Int(1 << 13))]),
            &LiveState {
                text: "abcd",
                ..LiveState::default()
            },
        )
        .expect("a live body");
        assert!(secret.contains("(****) Tj\n"), "{secret}");
        assert!(!secret.contains("abcd"), "{secret}");

        let comb = live_body(
            &widget_of(
                "Tx",
                &[
                    ("V", text("z")),
                    ("Ff", Object::Int(1 << 24)),
                    ("MaxLen", Object::Int(4)),
                ],
            ),
            &LiveState {
                text: "xy",
                ..LiveState::default()
            },
        )
        .expect("a live body");
        assert_eq!(comb.matches(" Tj\n").count(), 2, "{comb}");
        assert!(
            comb.contains("(x) Tj\n") && comb.contains("(y) Tj\n"),
            "{comb}"
        );
    }

    /// An editable combo box whose options do not contain what is typed.
    fn editable_combo() -> Dict {
        widget_of(
            "Ch",
            &[
                // The combo flag plus bit 19, "the value may be edited".
                ("Ff", Object::Int((1 << 17) | (1 << 18))),
                (
                    "Opt",
                    Object::Array(Array::of([
                        strings(&["a", "Apple"]),
                        strings(&["b", "Banana"]),
                    ])),
                ),
                ("V", text("b")),
            ],
        )
    }

    #[test]
    fn a_live_combo_box_draws_the_typed_text_rather_than_an_options_label() {
        let combo = editable_combo();
        let stored = body(&combo).expect("a body");
        assert!(stored.contains("(Banana) Tj\n"), "{stored}");

        let typed = live_body(
            &combo,
            &LiveState {
                text: "Bana",
                ..LiveState::default()
            },
        )
        .expect("a live body");
        assert!(typed.contains("(Bana) Tj\n"), "{typed}");
        assert!(!typed.contains("(Banana) Tj\n"), "{typed}");
        // The drop button is chrome, not value, so it is drawn either way.
        assert!(typed.contains("0.862745 g\n"), "{typed}");
    }

    /// A three-row list box with nothing selected and no `/TI`.
    fn list_of_three() -> Dict {
        widget_of("Ch", &[("Opt", strings(&["Ant", "Bee", "Cat"]))])
    }

    #[test]
    fn a_live_list_box_paints_the_sessions_selection_not_the_files() {
        let list = list_of_three();
        let stored = body(&list).expect("a body");
        // Nothing selected in the file at all.
        assert!(!stored.contains("0 0.2 0.443137 rg\n"), "{stored}");

        let live = live_body(
            &list,
            &LiveState {
                selected: &[1, 2],
                ..LiveState::default()
            },
        )
        .expect("a live body");
        assert_eq!(live.matches("0 0.2 0.443137 rg\n").count(), 2, "{live}");
        // Two white rows and one in the `/DA`'s black.
        assert_eq!(live.matches("1 g\n").count(), 2, "{live}");
        assert_eq!(live.matches("BT\n").count(), 3, "{live}");
    }

    #[test]
    fn a_live_selection_overrides_a_stored_one_rather_than_adding_to_it() {
        let list = widget_of(
            "Ch",
            &[("Opt", strings(&["Ant", "Bee", "Cat"])), ("V", text("Ant"))],
        );
        let live = live_body(
            &list,
            &LiveState {
                selected: &[2],
                ..LiveState::default()
            },
        )
        .expect("a live body");
        assert_eq!(live.matches("0 0.2 0.443137 rg\n").count(), 1, "{live}");
        // The band belongs to the last row, so the white text drawn after it
        // is `Cat` and `Ant` stays black.
        let band = live.find("0 0.2 0.443137 rg\n").expect("a band");
        assert!(live.find("(Cat) Tj\n") > Some(band), "{live}");
        assert!(live.find("(Ant) Tj\n") < Some(band), "{live}");
    }

    #[test]
    fn a_live_list_box_starts_at_the_sessions_top_row_not_its_ti() {
        // `/TI` says one; the session has scrolled to the third.
        let list = widget_of(
            "Ch",
            &[
                ("Opt", strings(&["Ant", "Bee", "Cat"])),
                ("TI", Object::Int(1)),
            ],
        );
        let stored = body(&list).expect("a body");
        assert_eq!(stored.matches("BT\n").count(), 2, "{stored}");

        let live = live_body(
            &list,
            &LiveState {
                top_visible: 2,
                ..LiveState::default()
            },
        )
        .expect("a live body");
        assert_eq!(live.matches("BT\n").count(), 1, "{live}");
        assert!(live.contains("(Cat) Tj\n"), "{live}");
        assert!(!live.contains("(Bee) Tj\n"), "{live}");

        // And a session resting at the top shows every row, `/TI` or no.
        let unscrolled = live_body(&list, &LiveState::default()).expect("a live body");
        assert_eq!(unscrolled.matches("BT\n").count(), 3, "{unscrolled}");
    }

    /// The first `Td`'s two operands, which are where a shift shows up.
    fn first_move(stream: &str) -> (f32, f32) {
        let line = stream
            .lines()
            .find(|line| line.ends_with(" Td"))
            .unwrap_or_else(|| panic!("no Td in {stream}"));
        let mut parts = line.split_whitespace();
        let x: f32 = parts.next().and_then(|n| n.parse().ok()).expect("an x");
        let y: f32 = parts.next().and_then(|n| n.parse().ok()).expect("a y");
        (x, y)
    }

    #[test]
    fn scrolling_shifts_the_drawn_text_by_the_scroll_offset() {
        // Upstream subtracts the scroll's distance from its resting seed on
        // the way from the layout to the drawn point, so the text moves the
        // other way from the number.
        let field = widget_of("Tx", &[("V", text("Hello"))]);
        let (rest_x, rest_y) = first_move(&body(&field).expect("a body"));
        let scrolled = live_body(
            &field,
            &LiveState {
                text: "Hello",
                scroll: (7.0, 3.0),
                ..LiveState::default()
            },
        )
        .expect("a live body");
        let (x, y) = first_move(&scrolled);
        assert!((x - (rest_x - 7.0)).abs() < 1e-3, "{x} against {rest_x}");
        assert!((y - (rest_y - 3.0)).abs() < 1e-3, "{y} against {rest_y}");
    }

    #[test]
    fn an_unscrolled_live_edit_draws_where_the_stored_value_would() {
        // A zero scroll is not merely close to no scroll: it is the same
        // stream, which is what lets the focused path reuse the geometry the
        // unfocused one pinned.
        let field = widget_of("Tx", &[("V", text("Hello"))]);
        let stored = body(&field).expect("a body");
        let live = live_body(
            &field,
            &LiveState {
                text: "Hello",
                ..LiveState::default()
            },
        )
        .expect("a live body");
        assert_eq!(stored, live);
    }

    #[test]
    fn a_scrolled_list_box_moves_its_rows_and_their_selection_bands_together() {
        let live = live_body(
            &list_of_three(),
            &LiveState {
                selected: &[0],
                scroll: (0.0, 5.0),
                ..LiveState::default()
            },
        )
        .expect("a live body");
        let rest = live_body(
            &list_of_three(),
            &LiveState {
                selected: &[0],
                ..LiveState::default()
            },
        )
        .expect("a live body");
        assert_ne!(live, rest, "a scroll must move the rows");
        let (_, y) = first_move(&live);
        let (_, rest_y) = first_move(&rest);
        assert!((y - (rest_y - 5.0)).abs() < 1e-3, "{y} against {rest_y}");
        // The band is still exactly one row tall and still there.
        assert_eq!(live.matches("0 0.2 0.443137 rg\n").count(), 1, "{live}");
    }

    #[test]
    fn none_and_a_default_live_state_agree_on_every_widget_fixture() {
        // The complement of the byte-identity golden: `None` pins the stored
        // path against a captured stream, and this pins the *live* path
        // against `None` wherever the file itself stores nothing to override.
        // Every fixture that *does* store something legitimately differs — a
        // default `LiveState` is an empty field with nothing selected resting
        // at row zero, which is a different appearance from `Hello` or from a
        // list box whose `/V` picks a row — so the agreeing set is named
        // rather than left to silence.
        //
        // A **list box** left the agreeing set when the scroll bar's width
        // started being reserved, because that reservation is precisely a
        // live-versus-stored difference and applies however empty the live
        // state is: `ch_list_i` is 12 units narrower live, by design. See
        // `LIST_SCROLLBAR_WIDTH`, and `only_a_live_list_box_keeps_the_scroll_bars_width_clear`
        // for the pin.
        for (name, widget) in widget_fixtures() {
            if !matches!(name, "tx_empty" | "btn") {
                continue;
            }
            let live = LiveState::default();
            assert_eq!(
                body_live(&widget, None, Some(&live)),
                body_live(&widget, None, None),
                "{name} moved under an empty live state"
            );
        }
    }
}
