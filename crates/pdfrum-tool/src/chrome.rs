//! Painting the viewer chrome the **library deliberately does not draw**.
//!
//! `pdfrum-form` publishes an open combo box's dropdown as state and geometry
//! ([`pdfrum::PopupView`]) and stops there, because a dropdown is a window and
//! a PDF library has no business creating one. The oracle **is** a host,
//! though, and composites the widget windows over the page bitmap — so to
//! compare against its goldens this tool has to be a host too.
//!
//! This module is that host, and it lives here rather than in a library crate
//! on purpose. It exists to make three conformance rows comparable
//! (`bug_1372651.{in,pdf}` and `bug_736695_2.in`); it is not a drawing API and
//! nothing outside the tool calls it.
//!
//! # Why it is not folded into the widget's `/AP`
//!
//! An annotation's appearance form is **fitted** into the annotation's
//! `/Rect`: its `/BBox` is scaled to the rectangle rather than translated
//! into it. A list hanging twenty-nine units below a fourteen-unit-tall
//! widget would therefore be squeezed into the widget's box rather than
//! overflowing below it. So the popup is a second page object with a bbox of
//! its own, appended after the annotation pass has laid everything else down
//! — which is also the order a host composites its windows in.
//!
//! # What the list looks like
//!
//! A **solid one-unit border**, a background, and — where the widget's own
//! `/MK` leaves them transparent — a black border and a white fill. A
//! selected item takes a navy band, `(0, 51, 113)` opaque, with white text;
//! every other item takes the text colour on the background. Both goldens
//! carry exactly that: `bug_1372651`'s `Item3` band is `(0, 51, 113)` at
//! device rows 92..106, verbatim.
//!
//! The **12-unit scroll-bar reservation does not apply here.** The rectangle
//! the rows are measured against deflates by the border alone, and a
//! scroll bar occupies no width while it is invisible, which it is whenever
//! the content fits. Neither target fixture scrolls.

// The oracle's window classes are where the above was measured.
// CPWL_ComboBox::CreateListBox (fpdfsdk/pwl/cpwl_combo_box.cpp:205-236)
// makes a CPWL_CBListBox and supplies kDefaultBlackColor /
// kDefaultWhiteColor for a transparent /MK.
// CPWL_ListBox::DrawThisAppearance (cpwl_list_box.cpp:45-85) walks the items
// and bands the selected one with ArgbEncode(255, 0, 51, 113).
// GetListRect (cpwl_list_box.cpp:352-355) is what SetPlateRect receives, and
// it deflates by the border alone; only GetClientRect subtracts a bar, and
// GetScrollBarWidth answers zero while the bar is invisible.
// The placement is CFX_Matrix::MatchRect, and FPDF_FFLDraw runs after
// FPDF_RenderPageBitmap.

use pdfrum_common::{Diagnostics, Limits};
use pdfrum_doc::ap;
use pdfrum_object::{Array, ByteSpan, Dict, Name, Object, PdfString, Resolve, Stream};
use pdfrum_page::{BuildContext, Page, Resources, build_form_object_with};

/// The list's border width, in PDF units.
const BORDER_WIDTH: i64 = 1;

/// Appends the open dropdown's window to a built page, after everything else.
///
/// A no-op when no list is open, which is every page in the corpus but three.
///
/// The list is drawn by **describing it as a list box and asking the ordinary
/// appearance generator to draw that**, rather than by emitting operators
/// here. `ap::field_body`'s list-box body already does every part of this —
/// the row stack from the plate's top, the laid-out line height, the navy
/// band, the white text on it, the clip — and it is the code the *widget*
/// path uses, so the popup's rows and a list box's rows cannot drift apart.
/// What this function supplies is the synthetic dictionary that says "a list
/// box, this big, with these options, this row banded".
#[expect(
    clippy::too_many_arguments,
    reason = "the same six inputs plus two sinks `annot_render::overlay_with` \
              carries, for the same reason: bundling them into a context \
              struct is the god-object shape STYLE §1 forbids"
)]
pub fn push_popup<R: Resolve>(
    page: &mut Page,
    popup: &pdfrum::PopupView,
    widget: &Dict,
    catalog: &Dict,
    r: &R,
    ctx: &mut BuildContext,
    limits: &Limits,
    diags: &mut Diagnostics,
) {
    let dict = popup_dict(popup, widget, r);
    let fonts = ap::FormFonts::load(catalog, r, ctx);
    let form = catalog
        .dict(pdfrum_object::names::ACRO_FORM, r)
        .unwrap_or_default();
    // Which face the rows are *measured* with. The generator reads the
    // `/DA` itself for the size and the resource name, so all this decides is
    // the ascent, descent and advances the layout uses.
    //
    // A widget with no `/DA` anywhere — `bug_736695_4.pdf` is exactly that,
    // and so is its form dictionary — falls through to `FormFonts`' own
    // fallback, which is filed under the **empty** name and is the same stock
    // face `ap::field_body::generate` synthesizes for the case. Reaching for
    // `default_appearance` alone and giving up when it answers `None` drew no
    // list at all on that fixture, which is the whole of `bug_736695_2`.
    let alias = ap::freetext::default_appearance(&dict, &form, r)
        .map(|appearance| appearance.font_name)
        .filter(|name| !name.is_empty())
        .unwrap_or_default();
    // The width closure is kept by the caller because it has to outlive the
    // `TextFont` that borrows it — `FormFonts::text_font`'s shape, not this
    // module's choice.
    let Some(face) = fonts.face(&alias) else {
        return;
    };
    let width = |code: u32| ap::TextFont::char_width(face, code);
    let Some(font) = fonts.text_font(&alias, &width) else {
        return;
    };
    let Some(generated) = ap::widget::generate_with_live_faces(
        &dict,
        catalog,
        &font,
        r,
        ap::widget::LiveInput {
            caret_and_selection: None,
            // `None` rather than a `LiveState`, and the difference is the
            // 12-unit scroll-bar reservation: the live path narrows the client
            // rectangle because a `CPWL_ListBox` window always creates a bar,
            // and the popup's rows are measured against `GetListRect`, which
            // never does. Selection and the first visible row travel in the
            // dictionary below instead.
            live: None,
            substitute: fonts.substitute(pdfrum_font::Charset::Hebrew),
            appearance_state: None,
            border_style: None,
            center_rows: true,
        },
    ) else {
        return;
    };

    let stream = Stream::new(
        ap::stream_dict(&generated),
        ByteSpan::from(generated.stream.clone()),
    );
    // The generator writes its bbox at the **origin** — `rotated_rect` is
    // `(0, 0, w, h)`, which is what makes an appearance form independent of
    // where its annotation sits — so the form is placed by the same
    // `CFX_Matrix::MatchRect` the annotation pass uses. The bbox and the
    // destination are the same *size* here, so this is a pure translation and
    // the list is moved rather than scaled: which is the whole reason the
    // popup is a second object with a rectangle of its own instead of a
    // taller `/AP` on the widget, whose rectangle would have squeezed it.
    let resources = Resources::for_page(page.resources.clone());
    // Already page-space `kurbo`: `PopupGeometry::rect` widens on the way out
    // of `pdfrum-form`, so the four `f64::from` calls this used to need are
    // gone .
    let placed = popup.geometry.rect;
    let matrix = pdfrum_doc::geom::match_rect(
        pdfrum_doc::geom::normalize(placed),
        pdfrum_doc::geom::transform_rect(generated.matrix, generated.bbox),
    );
    if !matrix.as_coeffs().iter().all(|c| c.is_finite()) {
        return;
    }
    // `live_edit = true`: the list's rows are drawn with **ClearType**, like
    // every other run the form filler draws. `CPWL_ListBox::DrawThisAppearance`
    // (`cpwl_list_box.cpp:66-84`) sets each row through
    // `CPWL_EditImpl::DrawEdit` → `DrawTextString` (`cpwl_edit_impl.cpp:40-57`),
    // which is the same local `CPDF_RenderOptions` a focused text field's
    // glyphs go through — the popup is a `CPWL_Wnd` and every `CPWL_Wnd`'s text
    // takes that path. Drawing it grey left 419 coloured pixels of the golden's
    // fringes unmatched inside the list alone.
    if let Some(object) =
        build_form_object_with(&stream, matrix, &resources, r, ctx, limits, diags, true)
    {
        page.objects.push(object);
    }
}

/// The scroll bar's width, and the gap the C++ leaves on the right
/// (`CPWL_Wnd::RepositionChildWnd`: `right - kWidth` … `right - 1`).
const SCROLLBAR_WIDTH: f32 = 12.0;
const SCROLLBAR_RIGHT_INSET: f32 = 1.0;
const SCROLLBAR_BUTTON: f32 = 9.0;
const SCROLLBAR_THUMB_MIN: f32 = 2.0;
/// `CPWL_ScrollBar::kTransparency`.
const SCROLLBAR_ALPHA: f32 = 150.0 / 255.0;

/// Paints a list-box scroll bar over the widget, as `CPWL_ScrollBar` does.
///
/// The live appearance already reserved the 12-unit strip; this is the
/// occupant. A no-op when the bar would have no area.
#[expect(
    clippy::too_many_arguments,
    reason = "the same sinks `push_popup` carries, plus the widget and the \
              view the bar is measured from"
)]
pub fn push_scrollbar<R: Resolve>(
    page: &mut Page,
    view: &pdfrum::ScrollView,
    widget: &Dict,
    catalog: &Dict,
    r: &R,
    ctx: &mut BuildContext,
    limits: &Limits,
    diags: &mut Diagnostics,
) {
    let _ = catalog;
    let Some(placed) = scrollbar_rect(widget, r) else {
        return;
    };
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a widget's client is a few hundred PDF units, well inside f32"
    )]
    let (width, height) = (placed.width() as f32, placed.height() as f32);
    if width <= 0.0 || height <= 0.0 {
        return;
    }
    let generated = scrollbar_ap(view, width, height);
    let stream = Stream::new(
        ap::stream_dict(&generated),
        ByteSpan::from(generated.stream.clone()),
    );
    let resources = Resources::for_page(page.resources.clone());
    let matrix = pdfrum_doc::geom::match_rect(
        pdfrum_doc::geom::normalize(placed),
        pdfrum_doc::geom::transform_rect(generated.matrix, generated.bbox),
    );
    if !matrix.as_coeffs().iter().all(|c| c.is_finite()) {
        return;
    }
    if let Some(object) =
        build_form_object_with(&stream, matrix, &resources, r, ctx, limits, diags, true)
    {
        page.objects.push(object);
    }
}

/// The bar's **page-space** rectangle: the 12-unit strip inside the widget's
/// client, inset 1 unit from the right as `RepositionChildWnd` does.
///
/// `client_rect` is appearance space (the widget box at the origin). Mapping
/// through [`pdfrum_doc::geom::match_rect`] is what `push_popup` already
/// does with a page-space destination — without it the bar lands at y≈0
/// instead of inside `/Rect`, which is `scrollable_widgets1`.
fn scrollbar_rect<R: Resolve>(widget: &Dict, r: &R) -> Option<kurbo::Rect> {
    let client = ap::field_body::client_rect(widget, r);
    let right = pdfrum_doc::geom::right(client);
    let left = right - SCROLLBAR_WIDTH;
    let bar = pdfrum_doc::geom::rect(
        left,
        pdfrum_doc::geom::bottom(client),
        right - SCROLLBAR_RIGHT_INSET,
        pdfrum_doc::geom::top(client),
    );
    if pdfrum_doc::geom::width(bar) <= 0.0 || pdfrum_doc::geom::height(bar) <= 0.0 {
        return None;
    }
    let page = widget.rect(pdfrum_object::names::RECT, r);
    let rotation = ap::widget::widget_rotation(widget, r);
    let (ap_w, ap_h) = if rotation.swaps_axes() {
        (
            pdfrum_doc::geom::height(page),
            pdfrum_doc::geom::width(page),
        )
    } else {
        (
            pdfrum_doc::geom::width(page),
            pdfrum_doc::geom::height(page),
        )
    };
    let ap_box = pdfrum_doc::geom::rect(0.0, 0.0, ap_w, ap_h);
    let matrix = pdfrum_doc::geom::match_rect(pdfrum_doc::geom::normalize(page), ap_box);
    Some(pdfrum_doc::geom::transform_rect(matrix, bar))
}

/// The bar as a form whose bbox sits at the origin.
fn scrollbar_ap(view: &pdfrum::ScrollView, width: f32, height: f32) -> ap::GeneratedAp {
    let mut out = String::new();
    out.push_str("q\n/GS1 gs\n");
    // Track: white fill, two grey 1-unit rules inset 2 units.
    out.push_str("1 1 1 rg\n0 0 ");
    push_num(&mut out, width);
    push_num(&mut out, height);
    out.push_str("re f\n0.392 0.392 0.392 RG\n1 w\n");
    let x_left = 2.0_f32.min(width);
    let x_right = (width - 2.0).max(0.0);
    push_num(&mut out, x_left);
    push_num(&mut out, height - 2.0);
    out.push_str("m\n");
    push_num(&mut out, x_left);
    push_num(&mut out, 2.0);
    out.push_str("l S\n");
    push_num(&mut out, x_right);
    push_num(&mut out, height - 2.0);
    out.push_str("m\n");
    push_num(&mut out, x_right);
    push_num(&mut out, 2.0);
    out.push_str("l S\n");

    let button = if height > SCROLLBAR_BUTTON * 2.0 + SCROLLBAR_THUMB_MIN + 2.0 {
        SCROLLBAR_BUTTON
    } else {
        ((height - SCROLLBAR_THUMB_MIN - 2.0) / 2.0).max(0.0)
    };
    if button > 0.0 {
        emit_sb_button(&mut out, 0.0, height - button, width, button, true);
        emit_sb_button(&mut out, 0.0, 0.0, width, button, false);
        emit_sb_thumb(&mut out, view, width, height, button);
    }
    out.push_str("Q\n");

    let mut gs = Dict::new();
    gs.push(Name::from(b"ca".as_slice()), Object::Real(SCROLLBAR_ALPHA));
    gs.push(Name::from(b"CA".as_slice()), Object::Real(SCROLLBAR_ALPHA));
    let mut gs_map = Dict::new();
    gs_map.push(Name::from(b"GS1".as_slice()), Object::Dict(gs));
    let mut resources = Dict::new();
    resources.push(Name::from(b"ExtGState".as_slice()), Object::Dict(gs_map));

    ap::GeneratedAp {
        stream: out.into_bytes(),
        bbox: kurbo::Rect::new(0.0, 0.0, f64::from(width), f64::from(height)),
        matrix: kurbo::Affine::IDENTITY,
        resources,
        rect_override: None,
        as_override: None,
    }
}

fn push_num(out: &mut String, value: f32) {
    let _ = std::fmt::Write::write_fmt(out, format_args!("{value} "));
}

/// One end-cap button: grey border, light fill, white chevron.
fn emit_sb_button(out: &mut String, x: f32, y: f32, w: f32, h: f32, up: bool) {
    out.push_str("0.392 0.392 0.392 RG\n0 w\n");
    push_num(out, x);
    push_num(out, y);
    push_num(out, w);
    push_num(out, h);
    out.push_str("re S\n1 1 1 RG\n1 w\n");
    push_num(out, x + 0.5);
    push_num(out, y + 0.5);
    push_num(out, w - 1.0);
    push_num(out, h - 1.0);
    out.push_str("re S\n");
    // Interior: the `DrawShadow` ramp approximated as a light grey fill.
    out.push_str("0.863 0.863 0.863 rg\n");
    push_num(out, x + 1.0);
    push_num(out, y + 1.0);
    push_num(out, (w - 2.0).max(0.0));
    push_num(out, (h - 2.0).max(0.0));
    out.push_str("re f\n");
    if h <= 6.0 {
        return;
    }
    // Chevron, from `kOffsetsMin` / `kOffsets` in `cpwl_sbbutton.cpp`.
    out.push_str("1 1 1 rg\n");
    let origin_x = x + 1.5;
    let origin_y = y;
    let pts: [(f32, f32); 7] = if up {
        [
            (2.5, 4.0),
            (2.5, 3.0),
            (4.5, 5.0),
            (6.5, 3.0),
            (6.5, 4.0),
            (4.5, 6.0),
            (2.5, 4.0),
        ]
    } else {
        [
            (2.5, 5.0),
            (2.5, 6.0),
            (4.5, 4.0),
            (6.5, 6.0),
            (6.5, 5.0),
            (4.5, 3.0),
            (2.5, 5.0),
        ]
    };
    if let Some(&(dx, dy)) = pts.first() {
        push_num(out, origin_x + dx);
        push_num(out, origin_y + dy);
        out.push_str("m\n");
    }
    for &(dx, dy) in pts.iter().skip(1) {
        push_num(out, origin_x + dx);
        push_num(out, origin_y + dy);
        out.push_str("l\n");
    }
    out.push_str("f\n");
}

/// The thumb, sized from [`pdfrum::ScrollView`] the way `TrueToFace` is.
fn emit_sb_thumb(
    out: &mut String,
    view: &pdfrum::ScrollView,
    width: f32,
    height: f32,
    button: f32,
) {
    let area_bottom = button + 1.0;
    let area_top = height - button - 1.0;
    if area_top - area_bottom <= SCROLLBAR_THUMB_MIN {
        return;
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "a list box's row count is a few dozen, exactly representable in f32"
    )]
    let total = view.total.max(1) as f32;
    #[expect(
        clippy::cast_precision_loss,
        reason = "visible and top_visible are bounded by total"
    )]
    let visible = (view.visible_rows as f32).clamp(1.0, total);
    #[expect(
        clippy::cast_precision_loss,
        reason = "visible and top_visible are bounded by total"
    )]
    let top = (view.top_visible as f32).min(total - visible).max(0.0);
    let span = area_top - area_bottom;
    let fact = total.max(1.0);
    let thumb_top = area_top - top * span / fact;
    let mut thumb_bottom = area_top - (top + visible) * span / fact;
    if thumb_top - thumb_bottom < SCROLLBAR_THUMB_MIN {
        thumb_bottom = thumb_top - SCROLLBAR_THUMB_MIN;
    }
    if thumb_bottom < area_bottom {
        thumb_bottom = area_bottom;
    }
    let h = (thumb_top - thumb_bottom).max(SCROLLBAR_THUMB_MIN);
    out.push_str("0.824 0.824 0.824 rg\n");
    push_num(out, 0.0);
    push_num(out, thumb_bottom);
    push_num(out, width);
    push_num(out, h);
    out.push_str("re f\n");
    if h <= 8.0 {
        return;
    }
    out.push_str("0.471 0.471 0.471 RG\n1 w\n");
    let mid_x = width / 2.0;
    let mid_y = thumb_bottom + h / 2.0;
    let left = mid_x - 2.5;
    let right = mid_x + 2.5;
    let mut y = mid_y - 2.25;
    for _ in 0..3 {
        push_num(out, left);
        push_num(out, y);
        out.push_str("m\n");
        push_num(out, right);
        push_num(out, y);
        out.push_str("l S\n");
        y += 2.0;
    }
}

/// The synthetic widget dictionary that describes the popup as a list box.
///
/// Six keys, each one a fact from the C++ rather than a convenience:
///
/// - `/FT /Ch` with **no combo bit**, so the body generator draws rows rather
///   than one line plus a drop button. The popup *is* a `CPWL_ListBox`;
///   `CPWL_CBListBox` overrides only its mouse handling.
/// - `/Rect` the popup's own window, which is what makes the appearance
///   overflow the widget it hangs from.
/// - `/Opt` the labels, in order — the list shows what the field shows.
/// - `/TI` the first visible row, and `/I` the banded one. The generator reads
///   both from the dictionary on the non-live path, which is exactly the
///   channel a synthetic dictionary can speak through.
/// - `/MK /BG` and `/BC`: the widget's own colours where it declares them, and
///   a white fill with a black border where it leaves them transparent.
/// - `/DA` copied from the widget, so the rows are set in the field's own font
///   at the field's own size — which for an automatic size the generator
///   resolves to `kComboBoxDefaultFontSize`, twelve points.
fn popup_dict<R: Resolve>(popup: &pdfrum::PopupView, widget: &Dict, r: &R) -> Dict {
    let mut dict = Dict::new();
    dict.push(
        pdfrum_object::names::TYPE.clone(),
        Object::Name(Name::from(b"Annot".as_slice())),
    );
    dict.push(
        pdfrum_object::names::SUBTYPE.clone(),
        Object::Name(Name::from(b"Widget".as_slice())),
    );
    dict.push(
        Name::from(b"FT".as_slice()),
        Object::Name(Name::from(b"Ch".as_slice())),
    );
    // No `/Ff` at all: the combo bit is what the *widget* carries and the
    // list must not, and every other choice flag is irrelevant to a window
    // that is drawn once and never interacted with through this dictionary.
    let rect = popup.geometry.rect;
    // `Object::Real` is `f32`, and every value here was widened from an `f32`
    // by `PopupGeometry::rect`, so the narrowing is exact and the `/Rect`
    // array is byte-for-byte what it was before § widened the field.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "every value in PopupGeometry::rect was widened from an f32"
    )]
    let edges = [rect.x0, rect.y0, rect.x1, rect.y1].map(|v| v as f32);
    dict.push(
        pdfrum_object::names::RECT.clone(),
        Object::Array(edges.into_iter().map(Object::Real).collect::<Array>()),
    );
    dict.push(
        Name::from(b"Opt".as_slice()),
        Object::Array(
            popup
                .options
                .iter()
                .map(|label| Object::Str(PdfString::literal(label.clone().into_bytes())))
                .collect::<Array>(),
        ),
    );
    if popup.top_visible > 0 {
        dict.push(
            Name::from(b"TI".as_slice()),
            Object::Int(i64::try_from(popup.top_visible).unwrap_or(0)),
        );
    }
    // The banded row, which is the hovered one where there is one — the list
    // carries `kListboxHoverSel`, so the pointer's row is *the* selection as
    // far as the drawing is concerned.
    //
    // Written as `/V`, the row's own text, and **not** as `/I`. The appearance
    // reader `ap::field_body::selected_indices` is `/V`-first and matches by
    // *value*, which is correct for its own job — the producer it reproduces
    // is `CPDFSDK_AppStream::SetAsListBox`, and records that
    // making it index-first would flip `listbox_form.{in,pdf}` to fail. So an
    // `/I [2]` here would match nothing and band no row at all. `/V` is the
    // channel this reader speaks, and a synthetic dictionary should speak it
    // rather than ask the reader to change.
    //
    // The label doubles as the value because that is what a one-string
    // `/Opt` entry means, and it is what `/Opt` is written as above.
    if let Some(banded) = popup.hovered.or(popup.selected)
        && let Some(label) = popup.options.get(banded)
    {
        dict.push(
            Name::from(b"V".as_slice()),
            Object::Str(PdfString::literal(label.clone().into_bytes())),
        );
    }
    dict.push(
        Name::from(b"MK".as_slice()),
        Object::Dict(list_colors(widget, r)),
    );
    dict.push(Name::from(b"BS".as_slice()), Object::Dict(solid_border()));
    if let Some(da) = widget.get(&Name::from(b"DA".as_slice()), r) {
        dict.push(Name::from(b"DA".as_slice()), da.get().clone());
    }
    dict
}

/// The list's `/MK`: the widget's own colours, with defaults where the widget
/// declares none.
///
/// A **transparent** border colour becomes black and a transparent background
/// becomes white. That is why `bug_736695_4.pdf`'s widget, which carries no
/// `/MK` at all, still draws a black-bordered white list.
// The two defaults are kDefaultBlackColor and kDefaultWhiteColor, supplied at
// cpwl_combo_box.cpp:224-231.
fn list_colors<R: Resolve>(widget: &Dict, r: &R) -> Dict {
    let mk = widget.dict(&Name::from(b"MK".as_slice()), r);
    let mut out = Dict::new();
    let background = mk
        .as_ref()
        .and_then(|mk| mk.array(&Name::from(b"BG".as_slice()), r));
    out.push(
        Name::from(b"BG".as_slice()),
        match background {
            Some(array) if !array.is_empty() => Object::Array(array),
            _ => Object::Array([Object::Real(1.0)].into_iter().collect::<Array>()),
        },
    );
    let border = mk
        .as_ref()
        .and_then(|mk| mk.array(&Name::from(b"BC".as_slice()), r));
    out.push(
        Name::from(b"BC".as_slice()),
        match border {
            Some(array) if !array.is_empty() => Object::Array(array),
            _ => Object::Array([Object::Real(0.0)].into_iter().collect::<Array>()),
        },
    );
    out
}

/// `/BS`: a solid one-unit border, which the list takes whatever the widget's
/// own `/MK /BW` and `/BS /S` say — `lcp.nBorderStyle = BorderStyle::kSolid`
/// and `lcp.dwBorderWidth = 1` are assignments, not inheritances.
fn solid_border() -> Dict {
    let mut bs = Dict::new();
    bs.push(Name::from(b"W".as_slice()), Object::Int(BORDER_WIDTH));
    bs.push(
        Name::from(b"S".as_slice()),
        Object::Name(Name::from(b"S".as_slice())),
    );
    bs
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdfrum::{Placement, PopupGeometry};

    fn view() -> pdfrum::PopupView {
        pdfrum::PopupView {
            annot: pdfrum::AnnotId::new(0, 0),
            anchor: kurbo::Rect::new(70.0, 135.0, 150.0, 155.0),
            geometry: PopupGeometry {
                rect: kurbo::Rect::new(70.0, 92.824, 150.0, 135.0),
                placement: Placement::Below,
                row_height: 13.392,
            },
            options: vec!["Item1".to_owned(), "Item2".to_owned(), "Item3".to_owned()],
            selected: Some(2),
            hovered: None,
            top_visible: 0,
            edit_text: None,
        }
    }

    /// The synthetic dictionary says "list box", not "combo box" — the combo
    /// bit belongs to the widget and would make the generator draw one line
    /// and a drop button instead of three rows.
    #[test]
    fn the_popup_is_described_as_a_list_box() {
        let dict = popup_dict(&view(), &Dict::new(), &pdfrum_object::NoResolve);
        assert_eq!(
            dict.byte_string(&Name::from(b"FT".as_slice()), &pdfrum_object::NoResolve),
            Some(b"Ch".to_vec())
        );
        assert!(
            !dict.contains_key(&Name::from(b"Ff".as_slice())),
            "no /Ff at all, so no combo bit"
        );
    }

    /// The rectangle is the **popup's**, not the widget's — which is the whole
    /// reason this is a second object.
    #[test]
    fn the_rectangle_is_the_popups_own() {
        let dict = popup_dict(&view(), &Dict::new(), &pdfrum_object::NoResolve);
        let rect = dict.rect(pdfrum_object::names::RECT, &pdfrum_object::NoResolve);
        assert!((rect.y1 - 135.0).abs() < 1e-3, "{rect:?}");
        assert!(rect.y0 < 93.0, "the list hangs below the widget: {rect:?}");
    }

    /// A widget with no `/MK` still gets a black-bordered white list, because
    /// `CreateListBox` substitutes its own defaults for transparent colours.
    /// Read back through `Color::from_array`, which is the reader the
    /// generator itself uses, so the test cannot pass on an array the
    /// generator would decline.
    #[test]
    fn a_widget_with_no_colours_gets_the_lists_defaults() {
        let mk = list_colors(&Dict::new(), &pdfrum_object::NoResolve);
        let r = pdfrum_object::NoResolve;
        let bg = mk
            .array(&Name::from(b"BG".as_slice()), &r)
            .expect("a background is always written");
        assert_eq!(
            pdfrum_doc::Color::from_array(&bg),
            pdfrum_doc::Color::Gray(1.0)
        );
        let bc = mk
            .array(&Name::from(b"BC".as_slice()), &r)
            .expect("a border colour is always written");
        assert_eq!(
            pdfrum_doc::Color::from_array(&bc),
            pdfrum_doc::Color::Gray(0.0)
        );
    }

    /// The widget's **own** colours win where it declares them — a list
    /// under a coloured combo takes that combo's fill, not white.
    #[test]
    fn a_widgets_own_colours_are_kept() {
        let mut mk = Dict::new();
        mk.push(
            Name::from(b"BG".as_slice()),
            Object::Array([Object::Real(0.25)].into_iter().collect::<Array>()),
        );
        let mut widget = Dict::new();
        widget.push(Name::from(b"MK".as_slice()), Object::Dict(mk));

        let r = pdfrum_object::NoResolve;
        let out = list_colors(&widget, &r);
        let bg = out
            .array(&Name::from(b"BG".as_slice()), &r)
            .expect("a background is always written");
        assert_eq!(
            pdfrum_doc::Color::from_array(&bg),
            pdfrum_doc::Color::Gray(0.25)
        );
        // The border it did *not* declare still falls back to black.
        let bc = out
            .array(&Name::from(b"BC".as_slice()), &r)
            .expect("a border colour is always written");
        assert_eq!(
            pdfrum_doc::Color::from_array(&bc),
            pdfrum_doc::Color::Gray(0.0)
        );
    }

    /// The hovered row outranks the stored one for the band, because the list
    /// is created with `kListboxHoverSel` and hovering upstream *is*
    /// selecting.
    #[test]
    fn hover_decides_the_band_when_there_is_one() {
        let mut popup = view();
        popup.hovered = Some(0);
        let dict = popup_dict(&popup, &Dict::new(), &pdfrum_object::NoResolve);
        assert_eq!(
            dict.byte_string(&Name::from(b"V".as_slice()), &pdfrum_object::NoResolve),
            Some(b"Item1".to_vec())
        );
    }

    /// With nothing hovered the stored selection bands, which is
    /// `bug_1372651`'s `Item3`.
    #[test]
    fn the_stored_selection_bands_when_nothing_is_hovered() {
        let dict = popup_dict(&view(), &Dict::new(), &pdfrum_object::NoResolve);
        assert_eq!(
            dict.byte_string(&Name::from(b"V".as_slice()), &pdfrum_object::NoResolve),
            Some(b"Item3".to_vec())
        );
    }

    /// The banded row is written as `/V`, the row's **text**, and never as
    /// `/I`. `ap::field_body::selected_indices` is `/V`-first and matches by
    /// value — correct for its own job, which settles — so an
    /// index array would band nothing at all.
    #[test]
    fn the_band_is_written_as_a_value_and_never_as_an_index() {
        let dict = popup_dict(&view(), &Dict::new(), &pdfrum_object::NoResolve);
        assert!(
            !dict.contains_key(&Name::from(b"I".as_slice())),
            "an /I array would match no option and band no row"
        );
    }

    /// The border is always solid and one unit, whatever the widget says —
    /// `lcp.nBorderStyle` and `lcp.dwBorderWidth` are assignments.
    #[test]
    fn the_border_is_always_one_solid_unit() {
        let bs = solid_border();
        let r = pdfrum_object::NoResolve;
        assert_eq!(bs.int(&Name::from(b"W".as_slice()), &r), Some(1));
        assert_eq!(
            bs.byte_string(&Name::from(b"S".as_slice()), &r),
            Some(b"S".to_vec())
        );
    }
}
