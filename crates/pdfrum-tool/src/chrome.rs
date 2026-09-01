//! Painting the viewer chrome the **library deliberately does not draw**.
//!
//! `pdfrum-form` publishes an open combo box's dropdown as state and geometry
//! ([`pdfrum::PopupView`]) and stops there, because a dropdown is a window and
//! a PDF library has no business creating one (STYLE.md §2b, ruled
//! 2026-09-01). The oracle's `pdfium_test` **is** a host, though — it drives
//! `FPDF_FFLDraw`, which composites `CPWL_Wnd::DrawAppearance` over the page
//! bitmap — so to compare against its goldens this tool has to be a host too.
//!
//! This module is that host, and it lives here rather than in a library crate
//! on purpose. It exists to make three conformance rows comparable
//! (`bug_1372651.{in,pdf}` and `bug_736695_2.in`); it is not a drawing API and
//! nothing outside the tool calls it.
//!
//! # Why it is not folded into the widget's `/AP`
//!
//! An annotation's appearance form is placed by `CFX_Matrix::MatchRect`, which
//! **fits** the form's `/BBox` into the annotation's `/Rect`. A list hanging
//! twenty-nine units below a fourteen-unit-tall widget would therefore be
//! *scaled* into the widget's box rather than overflowing below it. So the
//! popup is a second page object with a bbox of its own, appended after the
//! annotation pass has laid everything else down — which is also the order the
//! oracle composites in, since `FPDF_FFLDraw` runs after
//! `FPDF_RenderPageBitmap`.
//!
//! # What the list looks like, from the C++
//!
//! `CPWL_ComboBox::CreateListBox` (`fpdfsdk/pwl/cpwl_combo_box.cpp:205-236`)
//! makes a `CPWL_CBListBox` with a **solid one-unit border**, a background,
//! and — where the widget's own `/MK` leaves them transparent — the defaults
//! `kDefaultBlackColor` for the border and `kDefaultWhiteColor` for the fill.
//! `CPWL_ListBox::DrawThisAppearance` (`cpwl_list_box.cpp:45-85`) then walks
//! the items: a selected one takes a `ArgbEncode(255, 0, 51, 113)` navy band
//! with white text, and every other one takes the text colour on the
//! background. Both goldens carry exactly that — `bug_1372651`'s `Item3` band
//! is `(0, 51, 113)` at device rows 92..106, verbatim.
//!
//! The **12-unit scroll-bar reservation does not apply here.** `GetListRect`
//! (`cpwl_list_box.cpp:352-355`) — which is what `SetPlateRect` receives and
//! therefore what the rows are measured against — deflates by the border
//! alone; only `GetClientRect` subtracts a bar, and `GetScrollBarWidth`
//! answers **zero** while the bar is invisible, which it is whenever the
//! content fits. Neither target fixture scrolls.

use pdfrum_common::{Diagnostics, Limits};
use pdfrum_doc::ap;
use pdfrum_object::{Array, ByteSpan, Dict, Name, Object, PdfString, Resolve, Stream};
use pdfrum_page::{BuildContext, Page, Resources, build_form_object_with};

/// The list's border width, in PDF units — `lcp.dwBorderWidth = 1`.
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
            substitute: fonts.substitute(pdfrum_font::subst::Charset::Hebrew),
            appearance_state: None,
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
    let placed = kurbo::Rect::new(
        f64::from(popup.geometry.rect.left),
        f64::from(popup.geometry.rect.bottom),
        f64::from(popup.geometry.rect.right),
        f64::from(popup.geometry.rect.top),
    );
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
///   `CreateListBox`'s white-and-black defaults where it leaves them
///   transparent (`cpwl_combo_box.cpp:224-231`).
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
    dict.push(
        pdfrum_object::names::RECT.clone(),
        Object::Array(
            [rect.left, rect.bottom, rect.right, rect.top]
                .into_iter()
                .map(Object::Real)
                .collect::<Array>(),
        ),
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
    // is `CPDFSDK_AppStream::SetAsListBox`, and OWED item 7 records that
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

/// The list's `/MK`: the widget's own colours, with `CreateListBox`'s
/// defaults where the widget declares none.
///
/// `cpwl_combo_box.cpp:224-231` — a **transparent** border colour becomes
/// `kDefaultBlackColor` and a transparent background becomes
/// `kDefaultWhiteColor`. That is why `bug_736695_4.pdf`'s widget, which
/// carries no `/MK` at all, still draws a black-bordered white list.
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
            anchor: pdfrum::FormRect::new(70.0, 135.0, 150.0, 155.0),
            geometry: PopupGeometry {
                rect: pdfrum::FormRect::new(70.0, 92.824, 150.0, 135.0),
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
    /// value — correct for its own job, which OWED item 7 settles — so an
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
