//! Painting a page's annotation appearances onto the page.
//!
//! `pdfium_test --png` seeds its render flags with `FPDF_ANNOT`
//! unconditionally (`pdfium_test.cc:220`), so an annotation's `/AP` form is
//! part of the page image rather than an overlay a viewer adds. But it does
//! not all arrive by one route, and the split is the thing to know:
//!
//! - **Pass A** is the page render. `CPDFSDK_RenderPage` builds a
//!   `CPDF_AnnotList` and calls `DisplayAnnots(..., bShowWidget=false)`, whose
//!   `DisplayPass` skips every `/Widget` (`cpdf_annotlist.cpp:250-253`). So
//!   the page render draws the *non*-widgets.
//! - **Pass B** is `FPDF_FFLDraw`, which `pdfium_test` calls after every
//!   bitmap render with no flag guard at all (`pdfium_test.cc:1045-1050`).
//!   That is where widgets are drawn, one at a time through their own
//!   one-layer render context.
//!
//! Both end in the same `AnnotGetMatrix` arithmetic and both draw the normal
//! appearance, so one traversal reproduces them — but **their visibility
//! tests differ**, and merging them would be wrong:
//!
//! | test | Pass A (`DisplayPass`) | Pass B (`CPDFSDK_BAAnnot::IsVisible`) |
//! |---|---|---|
//! | `kInvisible` (bit 1) | not tested | **suppresses** |
//! | `kHidden` (bit 2) | suppresses | suppresses |
//! | `kPrint` (bit 3) | required when printing | not tested |
//! | `kNoView` (bit 6) | suppresses on screen | suppresses |
//!
//! `is_visible` therefore keys on the subtype: a widget goes through Pass B's
//! rules and everything else through Pass A's, which is the only place the
//! `kInvisible` bit is read.
//!
//! Two more decisions change pixels and neither is obvious from the spec:
//!
//! - **The appearance is placed by fitting, not by translating.**
//!   `CFX_Matrix::MatchRect` (`fx_coordinates.cpp:430`) fits the form's
//!   `/BBox` — mapped through the form's own `/Matrix` and re-bounded — into
//!   the annotation's `/Rect`, so a form whose `BBox` is a different size
//!   from the rect is *scaled* to it. A degenerate axis takes scale **1**, not
//!   zero, and the skew terms are forced to zero whatever `/Matrix` said.
//! - **Order is `/Annots` order.** Pass B stable-sorts by a layout order that
//!   is 1 for pop-ups and 5 for everything else, so among non-pop-ups nothing
//!   moves. Later annotations paint over earlier ones with no z-ordering of
//!   their own; `bug_1304714.in` stacks three widgets to pin exactly that.
//!
//! A pop-up is in the list but never painted: `ShouldDrawAnnotation` requires
//! `open_state_`, which only a mouse click sets and `pdfium_test` never does.

use pdfrum_common::{Diagnostics, Limits};
use pdfrum_object::{ByteSpan, Dict, Resolve, Stream};
use pdfrum_page::{BuildContext, Page, Resources, build_form_object};

use crate::annot::appearance::{ApMode, annot_ap, annot_matrix};
use crate::annot::{AnnotList, Annotation, Subtype};
use crate::ap;
use crate::names;

/// Appends every visible annotation's appearance to a built page.
///
/// The page's own resources are the fallback for an appearance form that
/// declares none: `CPDF_Annot::GetAPForm` constructs its `CPDF_Form` with
/// `pPage->GetMutableResources()`, so a form with no `/Resources` of its own
/// resolves names against the page.
pub fn overlay<R: Resolve>(
    page: &mut Page,
    page_dict: &Dict,
    catalog: &Dict,
    r: &R,
    ctx: &mut BuildContext,
    limits: &Limits,
    diags: &mut Diagnostics,
) {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a page width beyond f32 has already lost meaning, and the \
                  value only places a synthesized pop-up, which never paints"
    )]
    let page_width = page.crop_box.width() as f32;
    let list = AnnotList::load(page_dict, page_width, r);
    let resources = Resources::for_page(page.resources.clone());
    // `CPDF_Annot`'s constructor runs `GenerateAPIfNeeded`, so an annotation
    // that arrives without a usable `/AP /N` is given one *before* anything
    // asks it to draw.
    //
    // The *text-bearing* variant, because `GenerateAPIfNeeded` reaches
    // `GenerateFreeTextAP` on the same constructor as every other generator
    // (`cpdf_generateap.cpp:1603`) — there is no second pass, and no route by
    // which a free-text annotation is described but not drawn. Taking the
    // font-less walk here painted a synthesized free-text appearance as
    // nothing at all while `--annot` reported it in full, which is exactly
    // the shape that let it survive: the tier that compares text matched.
    //
    // The stock Helvetica stands in for whatever the `/DA` names, on the same
    // reasoning the dump path records: the generator wants the *metrics*, and
    // a non-embedded `/DA` font is substituted to this face anyway.
    let font =
        pdfrum_font::Font::load_standard(pdfrum_font::subst::StandardFont::Helvetica, &ctx.fonts);
    let width = |code: u32| ap::TextFont::char_width(&font, code);
    let text_font = ap::TextFont {
        metrics: ap::TextFont::metrics_of(&font, &width),
        font: &font,
    };
    let generated =
        ap::generate_appearances_with_text(page_dict, catalog, Some(&text_font), r, diags);

    for (slot, annot) in list.annots.iter().enumerate() {
        if !is_visible(annot) {
            continue;
        }
        let index = list.source_indices.get(slot).copied().unwrap_or(slot);
        // A generated appearance may also move the rectangle it draws into:
        // a text markup annotation with a generated AP is placed at its
        // quadrilaterals' bounding box rather than at its `/Rect`
        // (`CPDF_Annot::RectForDrawing`).
        let (form, placed) = if let Some(made) = generated.get(index) {
            let mut placed = annot.clone();
            placed.rect = generated.rect(index, annot.rect_for_drawing(true));
            (
                Stream::new(ap::stream_dict(made), ByteSpan::from(made.stream.clone())),
                placed,
            )
        } else {
            // `kNormal` in both passes, and `bFallbackToNormal` is a no-op
            // when the mode already is normal.
            let Some(form) = annot_ap(&annot.dict, ApMode::Normal, false, r) else {
                // No appearance to draw — but a widget's highlight is painted
                // *after* the appearance and independently of it
                // (`CFFL_InteractiveFormFiller::OnDraw`,
                // `cffl_interactiveformfiller.cpp:85-94`), so a field with no
                // `/AP` at all still tints. `password.in` is nothing but two
                // such fields.
                if let Some(object) = highlight(annot, r, limits, diags) {
                    page.objects.push(object);
                }
                continue;
            };
            (form, annot.clone())
        };
        // Placed in *page* space: `render_page` composes its own page matrix
        // on top, which is the `mtUser2Device` the C++ concatenates last. So
        // an identity here is what keeps a rotated or cropped page placing
        // annotations exactly as it places content.
        let matrix = annot_matrix(&placed, &form.dict, 0, kurbo::Affine::IDENTITY, r);
        if !matrix.as_coeffs().iter().all(|c| c.is_finite()) {
            continue;
        }
        if let Some(object) = build_form_object(&form, matrix, &resources, r, ctx, limits, diags) {
            page.objects.push(object);
        }
        if let Some(object) = highlight(annot, r, limits, diags) {
            page.objects.push(object);
        }
    }
}

/// The form-field highlight `pdfium_test` paints over every fillable widget.
///
/// This is a **host** decision, not a document one, and it is why so many
/// otherwise-correct form pages differ by a flat tint over every field.
/// `pdfium_test` opens with
///
/// ```text
/// FPDF_SetFormFieldHighlightColor(form.get(), FPDF_FORMFIELD_UNKNOWN, 0xFFE4DD);
/// FPDF_SetFormFieldHighlightAlpha(form.get(), 100);
/// ```
///
/// (`pdfium_test.cc:1776-1777`), and `CPDFSDK_Widget::DrawShadow`
/// (`cpdfsdk_widget.cpp:982-1006`) then fills the widget's `/Rect` with that
/// colour at that alpha once the appearance is down.
///
/// Three things about it are easy to get wrong:
///
/// - **`FX_COLORREF` is BGR.** `0xFFE4DD` is blue `0xFF`, green `0xE4`, red
///   `0xDD` — a pale blue, not the pink the hex reads as. Over white at
///   100/255 that is `(241, 244, 255)`, which is exactly what the goldens
///   carry.
/// - **It is a hard-edged integer rect.** `FillRect` takes an `FX_RECT` and
///   `ToFxRect` **truncates** every edge (`fx_coordinates.cpp:324-327`), so
///   the tint covers `[floor(left), floor(right))` with no antialiasing on
///   any side. A `/Rect` of `[100 100 200 130]` on a 200-tall page tints
///   device rows 70..99 and columns 100..199 — 30 x 100 pixels exactly.
/// - **It is gated on the *field's* flags, not the annotation's.**
///   `IsReadOnly` reads `form_flags::kReadOnly`, bit **0** of the inherited
///   `/Ff` (`constants/form_flags.h:13`), where the annotation's own
///   `ReadOnly` is bit 6 of `/F`. A push button never tints
///   (`IsFillingAllowed`), and neither does a widget with no `/FT` to
///   classify, whose field type is `kUnknown` and which `IsNeedHighLight`
///   refuses outright. A **signature** field is excluded one level higher
///   still: `CPDFSDK_Widget::OnDraw` short-circuits it to `DrawAppearance`
///   and never calls the form filler at all.
fn highlight<R: Resolve>(
    annot: &Annotation,
    r: &R,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> Option<pdfrum_page::PageObject> {
    if annot.subtype != Subtype::Widget {
        return None;
    }
    let field_type = crate::form::attr::field_attr(&annot.dict, names::FT, r, limits, diags)
        .map(|value| value.to_byte_string())
        .unwrap_or_default();
    let flags = crate::form::FieldFlags(
        crate::form::attr::field_attr(&annot.dict, names::FF, r, limits, diags)
            .and_then(|value| value.as_int())
            .unwrap_or(0),
    );
    // `IsNeedHighLight(kUnknown)` is false, so a widget whose `/FT` names no
    // field type is not tinted at all.
    let kind = crate::form::FieldKind::classify(&field_type, flags)?;
    // A push button is refused by `IsFillingAllowed`; a signature widget
    // never reaches the form filler at all, because `CPDFSDK_Widget::OnDraw`
    // short-circuits it to `DrawAppearance` and returns
    // (`cpdfsdk_widget.cpp:719-724`). Six corpus signature files say so, four
    // of them byte-exact.
    if flags.is_read_only()
        || matches!(
            kind,
            crate::form::FieldKind::Button | crate::form::FieldKind::Signature
        )
    {
        return None;
    }
    // A signature widget never reaches the form filler at all:
    // `CPDFSDK_Widget::OnDraw` draws its appearance and *returns*
    // (`cpdfsdk_widget.cpp:719-724`), so the highlight that every other
    // fillable field gets is never painted over it. Six corpus signature
    // files say so, four of them byte-exact.
    let rect = annot.rect;
    // `CFX_FloatRect::Normalize` before `ToFxRect`: `GetRect` hands back a
    // normalized rectangle, and a `/Rect` written corner-first would
    // otherwise truncate to an empty one.
    let rect = kurbo::Rect::new(
        rect.x0.min(rect.x1),
        rect.y0.min(rect.y1),
        rect.x0.max(rect.x1),
        rect.y0.max(rect.y1),
    );
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return None;
    }
    Some(pdfrum_page::PageObject::Path(Box::new(
        pdfrum_page::Content {
            object: pdfrum_page::PathObject {
                path: kurbo::Shape::to_path(&rect, 0.1),
                matrix: kurbo::Affine::IDENTITY,
                fill_rule: pdfrum_page::FillRule::Winding,
                stroke: false,
            },
            state: highlight_state(),
            marks: pdfrum_page::state::ContentMarks::default(),
            content_stream: -1,
        },
    )))
}

/// The graphics state the highlight rectangle fills under.
///
/// The colour is `0xFFE4DD` read as BGR, and the alpha is the 100/255 the
/// host asks for. It reaches the fill as `/ca` rather than as an alpha in the
/// colour word because that is where this engine keeps a constant alpha, and
/// the two are the same source-over multiply — `FillRect`'s `CompositeRect`
/// and an ordinary alpha fill differ in *antialiasing*, which the hard-edged
/// rectangle already settles, not in arithmetic.
fn highlight_state() -> pdfrum_page::GraphicsState {
    /// `0xFFE4DD` as an `FX_COLORREF`: blue high, then green, then red.
    const HIGHLIGHT_BGR: u32 = 0x00FF_E4DD;
    /// `FPDF_SetFormFieldHighlightAlpha(form.get(), 100)`.
    const HIGHLIGHT_ALPHA: f32 = 100.0 / 255.0;

    #[allow(clippy::cast_precision_loss)]
    let channel = |shift: u32| ((HIGHLIGHT_BGR >> shift) & 0xff) as f32 / 255.0;
    let mut fill = pdfrum_page::ColorValue::default();
    fill.set_space(std::sync::Arc::new(pdfrum_page::ColorSpace::DeviceRgb));
    // Red is the low byte and blue the high one: `FX_COLORREF` is BGR, so
    // `0xFFE4DD` is a pale blue rather than the pink it reads as.
    fill.set_components(&[channel(0), channel(8), channel(16)]);
    pdfrum_page::GraphicsState {
        fill,
        general: pdfrum_page::state::GeneralState {
            fill_alpha: HIGHLIGHT_ALPHA,
            ..pdfrum_page::state::GeneralState::default()
        },
        ..pdfrum_page::GraphicsState::default()
    }
}

/// Whether an annotation is painted at all on a screen render.
///
/// The two passes disagree about `kInvisible`, so the subtype picks which
/// test applies. Neither pass reads `kPrint` here, because
/// `pdfium_test --png` is not printing: Pass A's `kPrint` requirement is
/// gated on `bPrinting`, and Pass B has no print check at all.
fn is_visible(annot: &Annotation) -> bool {
    if annot.subtype == Subtype::Popup {
        // Drawn only when open, and nothing opens one.
        return false;
    }
    let flags = annot.flags;
    if flags.is_hidden() || flags.no_view() {
        return false;
    }
    // `CPDFSDK_BAAnnot::IsVisible` adds `kInvisible`, and only widgets reach
    // it — Pass A never tests that bit.
    if annot.subtype == Subtype::Widget && flags.0 & 1 != 0 {
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::{highlight, highlight_state, is_visible};
    use crate::annot::{AnnotFlags, Annotation, Subtype};
    use pdfrum_common::{Diagnostics, Limits};
    use pdfrum_object::{Dict, Name, NoResolve, Object};

    fn annot(subtype: Subtype, flags: i64) -> Annotation {
        let mut annot = Annotation::read(&Dict::new(), &NoResolve);
        annot.flags = AnnotFlags(flags);
        annot.subtype = subtype;
        annot
    }

    /// A widget over `[100 100 200 130]` with the given field type and flags.
    fn widget(field_type: &str, ff: i64) -> Annotation {
        let dict = Dict::from_pairs([
            (Name::from("Subtype"), Object::Name(Name::from("Widget"))),
            (Name::from("FT"), Object::Name(Name::from(field_type))),
            (Name::from("Ff"), Object::Int(ff)),
            (
                Name::from("Rect"),
                Object::Array(pdfrum_object::Array::of([
                    Object::Int(100),
                    Object::Int(100),
                    Object::Int(200),
                    Object::Int(130),
                ])),
            ),
        ]);
        Annotation::read(&dict, &NoResolve)
    }

    fn tinted(annot: &Annotation) -> bool {
        let (limits, mut diags) = (Limits::default(), Diagnostics::default());
        highlight(annot, &NoResolve, &limits, &mut diags).is_some()
    }

    /// `0xFFE4DD` is an `FX_COLORREF`, so it is **BGR**: a pale blue. Over
    /// white at alpha 100/255 that is `(241, 244, 255)`, which is what every
    /// form golden in the corpus carries over its fields.
    #[test]
    fn the_highlight_colour_is_bgr_and_composites_to_the_goldens_tint() {
        let state = highlight_state();
        let rgb = state.fill.to_rgb().expect("a resolved colour");
        assert_eq!(rgb.to_bytes(), [0xDD, 0xE4, 0xFF]);
        #[allow(clippy::cast_possible_truncation)]
        let alpha = (state.general.fill_alpha * 255.0) as i32;
        assert_eq!(alpha, 100);
        // The truncating `AlphaMerge` upstream composites it with.
        let over_white = |c: i32| ((255 * (255 - alpha)) + c * alpha) / 255;
        assert_eq!(
            [over_white(0xDD), over_white(0xE4), over_white(0xFF)],
            [241, 244, 255]
        );
    }

    #[test]
    fn every_fillable_field_type_is_tinted() {
        for ft in ["Tx", "Ch"] {
            assert!(tinted(&widget(ft, 0)), "{ft}");
        }
        // A check box and a radio button are both `/Btn` without bit 17.
        assert!(tinted(&widget("Btn", 0)));
        assert!(tinted(&widget("Btn", 1 << 15)), "radio");
    }

    #[test]
    fn the_three_kinds_of_field_that_are_never_tinted() {
        // A push button: `IsFillingAllowed` refuses it.
        assert!(!tinted(&widget("Btn", 1 << 16)));
        // A signature: `CPDFSDK_Widget::OnDraw` never reaches the form filler.
        assert!(!tinted(&widget("Sig", 0)));
        // A read-only field, on the *form* flag — bit 0 of `/Ff`, not the
        // annotation's own `ReadOnly` at bit 6 of `/F`.
        assert!(!tinted(&widget("Tx", 1)));
        let mut not_read_only = widget("Tx", 0);
        not_read_only.flags = AnnotFlags(64);
        assert!(
            tinted(&not_read_only),
            "the annotation's ReadOnly bit is a different flag word"
        );
    }

    #[test]
    fn a_widget_with_no_field_type_is_not_tinted() {
        // `IsNeedHighLight(kUnknown)` is false, and a widget with no `/FT`
        // classifies to nothing.
        let bare = Dict::from_pairs([(Name::from("Subtype"), Object::Name(Name::from("Widget")))]);
        assert!(!tinted(&Annotation::read(&bare, &NoResolve)));
        // And nothing that is not a widget is ever tinted.
        assert!(!tinted(&annot(Subtype::Square, 0)));
    }

    #[test]
    fn hidden_and_noview_suppress_in_both_passes_and_print_does_not() {
        for subtype in [Subtype::Widget, Subtype::Square] {
            assert!(is_visible(&annot(subtype, 0)), "{subtype:?}");
            assert!(is_visible(&annot(subtype, 4)), "Print alone still shows");
            assert!(!is_visible(&annot(subtype, 2)), "Hidden");
            assert!(!is_visible(&annot(subtype, 32)), "NoView");
            assert!(!is_visible(&annot(subtype, 4 | 32)), "NoView beats Print");
        }
    }

    #[test]
    fn invisible_suppresses_a_widget_and_only_a_widget() {
        // Pass B tests `kInvisible`; Pass A does not. A square carrying the
        // bit still draws, and a widget carrying it does not.
        assert!(!is_visible(&annot(Subtype::Widget, 1)));
        assert!(is_visible(&annot(Subtype::Square, 1)));
    }

    #[test]
    fn a_popup_is_never_painted() {
        assert!(!is_visible(&annot(Subtype::Popup, 0)));
    }
}
