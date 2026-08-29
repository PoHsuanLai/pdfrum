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

/// The colour behind a selected list-box row, as the byte triple the source
/// writes rather than the fraction it equals.
const SELECTION_FILL: Color = Color::Rgb(0.0, 51.0 / 255.0, 113.0 / 255.0);

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
#[must_use]
pub fn generate<R: Resolve>(
    dict: &Dict,
    catalog: &Dict,
    font: &TextFont<'_>,
    r: &R,
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

    let mut out = Content::new();
    match kind {
        Kind::Text => text_field(&mut out, dict, client, &appearance, color, font, r),
        Kind::Combo => combo_box(&mut out, dict, client, &appearance, color, font, r),
        Kind::List => list_box(&mut out, dict, client, &appearance, color, font, r),
        Kind::Button => push_button(&mut out, dict, client, &appearance, color, font, r),
    }
    if out.is_empty() {
        return None;
    }
    Some(Body {
        stream: out.into_bytes(),
        font_resources: font_resources(&appearance, form.as_ref(), r),
    })
}

/// The one-entry font dictionary a body's `Tf` names.
///
/// The stream names a resource; without this the operator resolves to nothing
/// and the text draws as blank paper. The entry is the form's `/DR /Font`
/// entry under that name, or a stock Helvetica when there is none.
fn font_resources<R: Resolve>(
    appearance: &freetext::Appearance,
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
    Some(Dict::from_pairs([(name, Object::Dict(entry))]))
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
fn set_text(
    text: &str,
    config: &vt::Config,
    font: &TextFont<'_>,
    offset_centred: bool,
    grouping: vt::edit_ap::Grouping,
    alias: &[u8],
) -> (String, Rect) {
    let layout = vt::layout(text, config, &font.metrics);
    let content = layout.content_rect_pdf(config.plate);
    let offset = vertical_offset(offset_centred, config.plate, content);
    let written = vt::edit_ap::generate(
        &layout,
        config,
        &font.metrics,
        offset,
        grouping,
        alias,
        |code| font.encode(code),
    );
    (written, content)
}

/// Wraps a body in the markers and the clip a text field and a combo box
/// share.
///
/// The clip is written **only on overflow** — a body that fits its plate has
/// no `W n` in its stream at all — and the colour operator sits inside the
/// text object rather than before it.
fn wrap_text(out: &mut Content, plate: Rect, content: Rect, color: Color, written: &str) {
    if written.is_empty() {
        return;
    }
    out.raw("/Tx BMC\nq\n");
    if geom::width(content) > geom::width(plate) || geom::height(content) > geom::height(plate) {
        out.rect(plate, Float::Shortest);
        out.raw("re\nW\nn\n");
    }
    out.raw("BT\n");
    out.raw(&color_op_via(color, PaintOp::Fill, Float::G6));
    out.raw(written);
    out.raw("ET\nQ\nEMC\n");
}

/// A text field's body, and the comb separators that precede it.
fn text_field<R: Resolve>(
    out: &mut Content,
    dict: &Dict,
    client: Rect,
    appearance: &freetext::Appearance,
    color: Color,
    font: &TextFont<'_>,
    r: &R,
) {
    let flags = flags(dict, r);
    let multi_line = flags & FLAG_MULTILINE != 0;
    let comb = flags & FLAG_COMB != 0;
    let max_len = inherited(dict, names::MAX_LEN, r)
        .and_then(|value| value.as_int())
        .unwrap_or(0);
    let value = field_value(dict, r);

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
        !multi_line,
        if comb {
            vt::edit_ap::Grouping::PerCharacter
        } else {
            vt::edit_ap::Grouping::Continuous
        },
        &appearance.font_name,
    );
    wrap_text(out, client, content, color, &written);
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
fn push_button<R: Resolve>(
    out: &mut Content,
    dict: &Dict,
    client: Rect,
    appearance: &freetext::Appearance,
    color: Color,
    font: &TextFont<'_>,
    r: &R,
) {
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
        true,
        vt::edit_ap::Grouping::Continuous,
        &appearance.font_name,
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
fn combo_box<R: Resolve>(
    out: &mut Content,
    dict: &Dict,
    client: Rect,
    appearance: &freetext::Appearance,
    color: Color,
    font: &TextFont<'_>,
    r: &R,
) {
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

    // The **label** of the selected option, or the value itself when nothing
    // is selected — which is how a combo box whose `/V` names no option still
    // shows what the file says.
    let options = options(dict, r);
    let text = match selected_indices(dict, &options, r).first().copied() {
        Some(index) => options
            .get(index)
            .map(|option| option.label.clone())
            .unwrap_or_default(),
        None => field_value(dict, r),
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
        true,
        vt::edit_ap::Grouping::Continuous,
        &appearance.font_name,
    );
    wrap_text(out, plate, content, color, &written);
    out.raw(&shapes::drop_button(button));
}

/// A list box's body: every option from `/TI` down.
fn list_box<R: Resolve>(
    out: &mut Content,
    dict: &Dict,
    client: Rect,
    appearance: &freetext::Appearance,
    color: Color,
    font: &TextFont<'_>,
    r: &R,
) {
    let options = options(dict, r);
    let selected = selected_indices(dict, &options, r);
    let top = usize::try_from(
        inherited(dict, names::TI, r)
            .and_then(|value| value.as_int())
            .unwrap_or(0),
    )
    .unwrap_or(0);

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
            (0.0, y),
            vt::edit_ap::Grouping::Continuous,
            &appearance.font_name,
            |code| font.encode(code),
        );
        if selected.contains(&index) {
            rows.raw("q\n");
            rows.raw(&color_op_via(SELECTION_FILL, PaintOp::Fill, Float::G6));
            rows.rect(
                geom::rect(geom::left(client), y - height, geom::right(client), y),
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
    use super::{Choice, Kind, field_value, options, selected_indices};
    use crate::ap::{TextFont, freetext};
    use pdfrum_object::{Array, Dict, Name, NoResolve, Object, PdfString};

    fn dict(pairs: &[(&str, Object)]) -> Dict {
        Dict::from_pairs(
            pairs
                .iter()
                .map(|(k, v)| (Name::from(*k), v.clone()))
                .collect::<Vec<_>>(),
        )
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
        let cache = pdfrum_font::FontCache::new();
        let face =
            pdfrum_font::Font::load_standard(pdfrum_font::subst::StandardFont::Helvetica, &cache);
        let width = |code: u32| TextFont::char_width(&face, code);
        let font = TextFont {
            metrics: TextFont::metrics_of(&face, &width),
            font: &face,
        };
        super::generate(widget, &catalog(), &font, &NoResolve)
            .map(|body| String::from_utf8_lossy(&body.stream).into_owned())
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
}
