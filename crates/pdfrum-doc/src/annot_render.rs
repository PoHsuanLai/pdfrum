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

/// Appends every visible annotation's appearance to a built page.
///
/// The page's own resources are the fallback for an appearance form that
/// declares none: `CPDF_Annot::GetAPForm` constructs its `CPDF_Form` with
/// `pPage->GetMutableResources()`, so a form with no `/Resources` of its own
/// resolves names against the page.
pub fn overlay<R: Resolve>(
    page: &mut Page,
    page_dict: &Dict,
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
    // asks it to draw. The dump path already builds exactly this overlay.
    let generated = ap::generate_appearances(page_dict, r, diags);

    for (slot, annot) in list.annots.iter().enumerate() {
        if !is_visible(annot) {
            continue;
        }
        let index = list.source_indices.get(slot).copied().unwrap_or(slot);
        // A generated appearance may also move the rectangle it draws into:
        // a text markup annotation with a generated AP is placed at its
        // quadrilaterals' bounding box rather than at its `/Rect`
        // (`CPDF_Annot::RectForDrawing`).
        let (form, placed) = match generated.get(index) {
            Some(made) => {
                let mut placed = annot.clone();
                placed.rect = generated.rect(index, annot.rect_for_drawing(true));
                (
                    Stream::new(ap::stream_dict(made), ByteSpan::from(made.stream.clone())),
                    placed,
                )
            }
            // `kNormal` in both passes, and `bFallbackToNormal` is a no-op
            // when the mode already is normal.
            None => match annot_ap(&annot.dict, ApMode::Normal, false, r) {
                Some(form) => (form, annot.clone()),
                None => continue,
            },
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
    use super::is_visible;
    use crate::annot::{AnnotFlags, Annotation, Subtype};
    use pdfrum_object::{Dict, NoResolve};

    fn annot(subtype: Subtype, flags: i64) -> Annotation {
        let mut annot = Annotation::read(&Dict::new(), &NoResolve);
        annot.flags = AnnotFlags(flags);
        annot.subtype = subtype;
        annot
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
