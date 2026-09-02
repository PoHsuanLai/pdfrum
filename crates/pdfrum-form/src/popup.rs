//! The open combo-box dropdown, as **state and geometry** rather than as a
//! window.
//!
//! # Why this is a value and not a callback
//!
//! `fpdfsdk/pwl` is a closed list of five chrome pieces — the caret, the
//! selection band, the focus rectangle, the scroll bar and this dropdown. The
//! first three are baked *inside* the widget's `/Rect`, so they belong in the
//! appearance stream and are already there. The last two are drawn *outside*
//! it, and a library that creates windows to draw them is a library that has
//! taken over the host's job.
//!
//! So this module publishes what the C++'s `CPWL_ComboBox` knows and lets the
//! host paint: **where** the list would be ([`PopupView::geometry`]), **what** is
//! in it ([`PopupView::options`]), **which** row is selected or hovered, and
//! how tall a row is. A host that wants to draw the dropdown has every number
//! it needs; a host that does not want to draw it is not asked to. The
//! reverse direction — the host telling the session what the user did with
//! the list — is [`crate::route`]'s `choose` and `close_popup`, which are
//! ordinary intent methods and not a trait (STYLE §2b, ruled 2026-09-01).
//!
//! # The one thing this is *not*
//!
//! It is not an appearance stream. A widget's `/AP` form is mapped onto its
//! `/Rect` by `CFX_Matrix::MatchRect`, so folding a taller list into it would
//! **scale** the widget's own body rather than overflowing below it. The
//! popup has a bbox of its own, and a caller draws it as a second object.

use crate::field::ChoiceState;
use crate::session::AnnotId;
use crate::tab::Rect;

/// The list's border, in PDF units, on every side.
///
/// `CPWL_ComboBox::CreateListBox` (`fpdfsdk/pwl/cpwl_combo_box.cpp:205-236`)
/// sets `dwBorderWidth = 1` and `BorderStyle::kSolid` on the list it makes,
/// whatever the widget's own `/MK /BW` says: the dropdown is chrome the
/// viewer draws, not something the file describes.
pub const LIST_BORDER: f32 = 1.0;

/// The tallest a dropdown is allowed to grow, in PDF units.
///
/// `kMaxListBoxHeight` (`formfiller/cffl_interactiveformfiller.cpp:706`).
pub const MAX_LIST_HEIGHT: f32 = 140.0;

/// How many rows the minimum popup shows, and the count above which that
/// minimum applies at all.
///
/// `CPWL_ComboBox::SetPopup` (`cpwl_combo_box.cpp:337-340`): a list of **more
/// than three** options may not be clamped below three rows plus its border;
/// a list of three or fewer has no floor and may be squeezed to nothing.
pub const MIN_POPUP_ROWS: usize = 3;

/// Which side of the widget the list opens on.
///
/// `CFFL_InteractiveFormFiller::QueryWherePopup`
/// (`cffl_interactiveformfiller.cpp:670-729`) picks by room: below when the
/// space under the widget can hold the whole list, above when it cannot but
/// the space over it can, and otherwise whichever side is larger.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// The list hangs below the widget, growing downward from its bottom
    /// edge. `bBottom == true`.
    Below,
    /// The list rises above the widget, growing upward from its top edge.
    /// `bBottom == false`.
    Above,
}

/// Where an open dropdown sits and how tall its rows are.
///
/// Pure geometry, in **page space** (PDF user space, y-up) — the same space
/// the widget's `/Rect` is written in and the same space an [`crate::Event`]
/// arrives in, so a host can hit-test a click against [`Self::rect`] without
/// a transform.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PopupGeometry {
    /// The whole list window, border included, in page space.
    ///
    /// A [`kurbo::Rect`] on the way *out*: this is geometry a host paints and
    /// hit-tests against, in the same vocabulary `Page::crop_box` speaks. The
    /// values inside are the crate's `f32` widened losslessly — the placement
    /// arithmetic below is `f32` throughout, and stays that way.
    pub rect: kurbo::Rect,
    /// Which side of the widget it opened on.
    pub placement: Placement,
    /// The height of one row, which is the **laid-out** line height and not
    /// the font size — see [`crate::route`]'s `row_height`.
    pub row_height: f32,
}

/// This crate's `f32` rectangle widened for a caller. Lossless.
pub(crate) fn widen(rect: Rect) -> kurbo::Rect {
    kurbo::Rect::new(
        f64::from(rect.left),
        f64::from(rect.bottom),
        f64::from(rect.right),
        f64::from(rect.top),
    )
}

impl PopupGeometry {
    /// The window as this crate's `f32` rectangle.
    ///
    /// The inverse of [`widen`] over every value [`PopupGeometry::rect`] can
    /// hold, because every one of them was widened from an `f32` here.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "every value in `rect` was widened from an f32 by `widen`"
    )]
    fn narrow(&self) -> Rect {
        Rect::new(
            self.rect.x0 as f32,
            self.rect.y0 as f32,
            self.rect.x1 as f32,
            self.rect.y1 as f32,
        )
    }

    /// The area the rows are drawn in: the window deflated by its border.
    ///
    /// `CPWL_ListBox::GetListRect` (`cpwl_list_box.cpp:352-355`), which is
    /// what `CPWL_ListCtrl::SetPlateRect` is handed and therefore what the
    /// item rectangles are measured against. It is deliberately **not**
    /// `GetClientRect`: that one also subtracts a visible scroll bar's width,
    /// and the rows keep their full width behind one.
    #[must_use]
    pub fn plate(&self) -> kurbo::Rect {
        widen(self.plate_f32())
    }

    /// [`PopupGeometry::plate`] in the crate's own `f32`, which is what the
    /// row arithmetic below measures against.
    pub(crate) fn plate_f32(&self) -> Rect {
        let border = LIST_BORDER;
        let rect = self.narrow();
        // `GetDeflated` on a rectangle narrower than twice its border would
        // turn it inside out. Upstream's `CPWL_Wnd::GetClientRect` guards the
        // same case with `rcWindow.Contains(rcClient)`, answering an empty
        // rectangle when the deflation escaped the window.
        if rect.right - rect.left <= border * 2.0 || rect.top - rect.bottom <= border * 2.0 {
            return Rect::new(rect.left, rect.bottom, rect.left, rect.bottom);
        }
        Rect::new(
            rect.left + border,
            rect.bottom + border,
            rect.right - border,
            rect.top - border,
        )
    }

    /// The rectangle of one visible row, counting `offset` rows down from the
    /// first one showing.
    ///
    /// `CPWL_ListCtrl::ReArrange` (`cpwl_list_ctrl.cpp:525-551`) stacks the
    /// items downward from the plate's top by exactly one row height each,
    /// so this is that stack read back out. A row past the bottom of the
    /// plate still has a rectangle — the caller clips.
    #[must_use]
    pub fn row_rect(&self, offset: usize) -> kurbo::Rect {
        let plate = self.plate_f32();
        // The product is computed in `f64` and narrowed once, so an absurd
        // offset saturates to an off-plate rectangle rather than wrapping.
        #[expect(
            clippy::cast_precision_loss,
            reason = "a row offset past 2^53 has no rectangle worth naming"
        )]
        let down = f64::from(self.row_height) * offset as f64;
        #[expect(
            clippy::cast_possible_truncation,
            reason = "narrowed once, after the multiply, so the clamp is the f32 range"
        )]
        let top = plate.top - down as f32;
        widen(Rect::new(
            plate.left,
            top - self.row_height,
            plate.right,
            top,
        ))
    }

    /// Which visible row a page-space point falls on, or [`None`] when the
    /// point is outside the plate.
    ///
    /// The offset is from the first visible row, so a caller adds
    /// [`ChoiceState::top_visible`] to get an option index.
    pub(crate) fn row_at(&self, x: f32, y: f32) -> Option<usize> {
        let plate = self.plate_f32();
        if x < plate.left || x > plate.right || y < plate.bottom || y > plate.top {
            return None;
        }
        if self.row_height <= 0.0 {
            return None;
        }
        let offset = ((plate.top - y) / self.row_height).floor();
        if !offset.is_finite() || offset < 0.0 {
            return None;
        }
        // Bounded by the plate test above: `plate.top - y` is at most the
        // plate's height, so the quotient is at most the visible row count.
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "non-negative and bounded by the plate height over the row height"
        )]
        let offset = offset as usize;
        Some(offset)
    }

    /// How many rows fit in the plate, whole and partial alike.
    ///
    /// A partial row at the bottom is still drawn — `CPWL_ListBox`'s cull
    /// keeps any item overlapping the plate — so a viewer counting rows to
    /// paint wants the rounded-up count, not the floor.
    #[must_use]
    pub fn visible_rows(&self) -> usize {
        if self.row_height <= 0.0 {
            return 0;
        }
        let plate = self.plate_f32();
        let rows = ((plate.top - plate.bottom) / self.row_height).ceil();
        if !rows.is_finite() || rows <= 0.0 {
            return 0;
        }
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a row count is bounded by the popup's height in points"
        )]
        let rows = rows as usize;
        rows
    }
}

/// Everything a host needs to draw one open dropdown.
///
/// Returned by `FormSession::popup_for_page` and by [`crate::route::popup_view`].
///
/// # Owned, and not borrowed from the session
///
/// The M14 ruling sketched this with a lifetime — `options: &'a [String]`,
/// `edit_text: Option<&'a str>` — and it is owned instead, deliberately. Two
/// reasons, one practical and one from the ruling's own text:
///
/// - The facade's per-page entry point assembles a borrowed
///   [`crate::route::Context`] around a `PageForm` it owns, drives the body,
///   and drops it. A view borrowing the session cannot outlive that scope, so
///   a borrowed `PopupView` is reachable from `pdfrum-form` and **not** from
///   `pdfrum::FormSession` — which is the one caller the ruling names.
/// - STYLE §2b's ruling says the point of exposing state rather than
///   inverting is to avoid "a trait object threaded through the session, a
///   synchronous mid-dispatch callback contract, **or a lifetime on a public
///   type**". An owned view is the ruling's own preference stated plainly.
///
/// The cost is a `Vec<String>` per query, on a query a host makes once per
/// render of a page that has a dropdown open — which is at most one page in a
/// session, and only while a list is showing.
#[derive(Debug, Clone, PartialEq)]
pub struct PopupView {
    /// Which widget the list belongs to, by **raw** `/Annots` index — the
    /// same key space appearance updates and the annotation overlay use.
    pub annot: AnnotId,
    /// The widget's own `/Rect`, page space: the anchor the list hangs from.
    pub anchor: kurbo::Rect,
    /// Where the list is and how tall its rows are.
    pub geometry: PopupGeometry,
    /// The rows' labels, in option order.
    ///
    /// Labels and not values: a list shows [`crate::field::ChoiceOption::label`],
    /// while `value` is what the field *stores* when they differ, and drawing
    /// the latter would put the wrong string on the page for every `/Opt`
    /// entry written as a two-element array.
    pub options: Vec<String>,
    /// Which option is selected, if any.
    ///
    /// A combo box selects at most one — `field::choice::select_only` is the
    /// only path a click into the list takes — so this is an index and not a
    /// set.
    pub selected: Option<usize>,
    /// Which option the pointer is over, if any.
    ///
    /// The list carries `Styles::kListboxHoverSel`
    /// (`cpwl_combo_box.cpp:210-211`), so hovering a row *selects* it
    /// upstream rather than merely tinting it. This reports the pointer's row
    /// so a host can paint the band before the click lands.
    pub hovered: Option<usize>,
    /// The first option currently showing, for a list taller than the popup.
    pub top_visible: usize,
    /// An **editable** combo's text half, which is a typed string rather than
    /// an option's label. [`None`] for a gated combo, which has no text half.
    pub edit_text: Option<String>,
}

impl PopupView {
    /// The label of one visible row, counting `offset` rows down from the
    /// first one showing.
    ///
    /// The pairing for [`PopupGeometry::row_rect`]: the two take the same
    /// offset, so a painter walks `0..visible_rows()` asking each for its
    /// rectangle and its text without doing the `top_visible` arithmetic
    /// itself.
    #[must_use]
    pub fn label_at(&self, offset: usize) -> Option<&str> {
        let index = self.top_visible.checked_add(offset)?;
        self.options.get(index).map(String::as_str)
    }

    /// Whether the row at `offset` visible rows down is the selected one.
    ///
    /// The band `CPWL_ListBox::DrawThisAppearance` (`cpwl_list_box.cpp:66-84`)
    /// fills navy behind and writes white text into. **Hover counts**: the
    /// list is created with `kListboxHoverSel`, so the row under the pointer
    /// is selected as far as the drawing is concerned even though the field's
    /// stored value has not moved.
    #[must_use]
    pub fn is_banded(&self, offset: usize) -> bool {
        let Some(index) = self.top_visible.checked_add(offset) else {
            return false;
        };
        self.hovered == Some(index) || (self.hovered.is_none() && self.selected == Some(index))
    }
}

/// How far a scrollable control has scrolled, in rows.
///
/// A second value getter beside [`PopupView`], for the same reason: a scroll
/// bar is host chrome the library merely *knows about*. It answers for a
/// scrolling **list box** as well as for an open dropdown, which is why it is
/// keyed by annotation rather than carried on the popup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollView {
    /// The first row showing.
    pub top_visible: usize,
    /// How many rows the box can show at once.
    pub visible_rows: usize,
    /// How many rows there are altogether.
    pub total: usize,
}

impl ScrollView {
    /// Whether a scroll bar would be drawn at all.
    ///
    /// `CPWL_ListBox::OnSetScrollInfoY` (`cpwl_list_box.cpp:288-303`) hides
    /// the bar whenever the plate is at least as tall as the content, which
    /// is exactly "every row fits".
    #[must_use]
    pub fn is_scrollable(&self) -> bool {
        self.total > self.visible_rows
    }
}

/// Where a list of `rows` rows would open from a widget whose `/Rect` is
/// `anchor` on a page `page` units tall, and how tall it would be.
///
/// This is `CPWL_ComboBox::SetPopup`'s clamp
/// (`cpwl_combo_box.cpp:325-377`) followed by
/// `CFFL_InteractiveFormFiller::QueryWherePopup`
/// (`cffl_interactiveformfiller.cpp:670-729`), as one function over numbers:
///
/// 1. the list's content is `rows * row_height`, and the window it wants is
///    that plus a border on each side;
/// 2. the **floor** is three rows plus the border, but only when there are
///    more than three options — a shorter list has no floor;
/// 3. `kMaxListBoxHeight` (140) is clamped into `[floor, wanted]`, so a list
///    shorter than 140 units asks for its own height and a longer one asks
///    for 140 — unless the floor pushes back above it;
/// 4. the side is chosen by room: below if the space under the widget can
///    hold that height, else above if the space over it can, else whichever
///    side is larger — and the height becomes that side's room.
///
/// Returns [`None`] when the list would have no height at all, which is
/// `SetPopup`'s two early `return true`s: a zero-height content rectangle,
/// and a `fPopupRet` that comes back non-positive. Both are **refusals to
/// open**, and `SetPopup` reporting `true` for them is why a combo whose list
/// cannot fit stays closed while the click is still consumed.
pub(crate) fn place(
    anchor: Rect,
    page_height: f32,
    rows: usize,
    row_height: f32,
) -> Option<PopupGeometry> {
    if row_height <= 0.0 || rows == 0 {
        return None;
    }
    let border = LIST_BORDER * 2.0;
    // `list_->GetContentRect().Height()`, the stacked items' total.
    #[expect(
        clippy::cast_precision_loss,
        reason = "an option count past 2^24 has already exceeded any page"
    )]
    let content = row_height * rows as f32;
    if content <= 0.0 {
        return None;
    }
    let floor = if rows > MIN_POPUP_ROWS {
        row_height * 3.0 + border
    } else {
        0.0
    };
    let ceiling = content + border;
    // `std::clamp(kMaxListBoxHeight, fPopupMin, fPopupMax)`. Upstream's
    // argument order means the **minimum wins** when the two cross, which is
    // how a four-row list of very tall rows can still be asked to open
    // taller than 140.
    let wanted = MAX_LIST_HEIGHT.max(floor).min(ceiling.max(floor));

    // `rcPageView(0, height, width, 0)` normalized, against the widget's
    // rect: the room above the widget's top and below its bottom. The C++
    // measures against the page's *display* size from the origin and not
    // against a crop box that starts elsewhere, so this does too.
    let above = page_height - anchor.top;
    let below = anchor.bottom;

    let (placement, height) = if below > wanted {
        (Placement::Below, wanted)
    } else if above > wanted {
        (Placement::Above, wanted)
    } else if above > below {
        (Placement::Above, above)
    } else {
        (Placement::Below, below)
    };
    if height <= 0.0 {
        return None;
    }

    // `RepositionChildWnd` (`cpwl_combo_box.cpp:238-283`): the window grows
    // by `fPopupRet` on the chosen side, and the list child takes the grown
    // part — so the list's near edge is the widget's own edge, exactly, and
    // the two never overlap.
    let rect = match placement {
        Placement::Below => Rect::new(
            anchor.left,
            anchor.bottom - height,
            anchor.right,
            anchor.bottom,
        ),
        Placement::Above => Rect::new(anchor.left, anchor.top, anchor.right, anchor.top + height),
    };
    Some(PopupGeometry {
        rect: widen(rect),
        placement,
        row_height,
    })
}

/// The option a click at `offset` visible rows down selects, if the list has
/// one there.
#[must_use]
pub fn option_at(state: &ChoiceState, offset: usize) -> Option<usize> {
    let index = state.top_visible.checked_add(offset)?;
    (index < state.options.len()).then_some(index)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `bug_736695_2`: a two-option combo at `/Rect [165.7 315.9 315.7 330.1]`
    /// on a 342-unit page. There is far more room below (315.9) than above
    /// (11.9), and two rows of 13.392 plus a 2-unit border is 28.784 — well
    /// under both `kMaxListBoxHeight` and the room, so the list opens
    /// downward at its own height.
    #[test]
    fn two_options_open_downward_at_their_own_height() {
        let anchor = Rect::new(165.7, 315.9, 315.7, 330.1);
        let popup = place(anchor, 342.0, 2, 13.392).expect("a two-row list fits");
        assert_eq!(popup.placement, Placement::Below);
        assert!((popup.rect.y1 - 315.9).abs() < 1e-4, "{:?}", popup.rect);
        assert!(
            ((popup.rect.y1 - popup.rect.y0) - 28.784).abs() < 1e-3,
            "{:?}",
            popup.rect
        );
        assert!((popup.rect.x0 - 165.7).abs() < 1e-4);
        assert!((popup.rect.x1 - 315.7).abs() < 1e-4);
    }

    /// `bug_1372651`: three options at `/Rect [70 135 150 155]` on a
    /// 200-unit page. 135 below, 45 above; three rows plus border is 42.176,
    /// which fits below.
    #[test]
    fn three_options_open_downward() {
        let anchor = Rect::new(70.0, 135.0, 150.0, 155.0);
        let popup = place(anchor, 200.0, 3, 13.392).expect("a three-row list fits");
        assert_eq!(popup.placement, Placement::Below);
        assert!(
            ((popup.rect.y1 - popup.rect.y0) - 42.176).abs() < 1e-3,
            "{:?}",
            popup.rect
        );
        assert!((popup.rect.y1 - 135.0).abs() < 1e-4);
    }

    /// A widget near the **bottom** of the page has no room under it, so the
    /// list rises instead — the `bBottom == false` branch.
    #[test]
    fn no_room_below_opens_upward() {
        let anchor = Rect::new(10.0, 4.0, 100.0, 24.0);
        let popup = place(anchor, 200.0, 2, 13.392).expect("there is room above");
        assert_eq!(popup.placement, Placement::Above);
        assert!((popup.rect.y0 - 24.0).abs() < 1e-4, "{:?}", popup.rect);
        assert!(((popup.rect.y1 - popup.rect.y0) - 28.784).abs() < 1e-3);
    }

    /// Squeezed on both sides, the list takes the larger side's room rather
    /// than the height it asked for — upstream's last `if`.
    #[test]
    fn squeezed_takes_the_larger_side_whole() {
        // 6 below, 10 above, on a 36-unit page with a 20-unit widget.
        let anchor = Rect::new(0.0, 6.0, 50.0, 26.0);
        let popup = place(anchor, 36.0, 2, 13.392).expect("ten units is still a popup");
        assert_eq!(popup.placement, Placement::Above);
        assert!(
            ((popup.rect.y1 - popup.rect.y0) - 10.0).abs() < 1e-4,
            "{:?}",
            popup.rect
        );
    }

    /// A long list is capped at `kMaxListBoxHeight` even where the page has
    /// room for all of it.
    #[test]
    fn a_long_list_is_capped_at_one_hundred_and_forty() {
        let anchor = Rect::new(0.0, 400.0, 100.0, 420.0);
        let popup = place(anchor, 800.0, 40, 13.392).expect("a forty-row list opens");
        assert!(
            ((popup.rect.y1 - popup.rect.y0) - 140.0).abs() < 1e-4,
            "{:?}",
            popup.rect
        );
    }

    /// The three-row floor applies only above three options — and it beats
    /// the 140 cap when the rows are tall enough, because upstream clamps the
    /// **constant** into `[min, max]` rather than the other way round.
    #[test]
    fn the_three_row_floor_outranks_the_cap() {
        let anchor = Rect::new(0.0, 400.0, 100.0, 420.0);
        // Four rows of 60 units: the floor is 182, above the 140 cap.
        let popup = place(anchor, 800.0, 4, 60.0).expect("a four-row list opens");
        assert!(
            ((popup.rect.y1 - popup.rect.y0) - 182.0).abs() < 1e-4,
            "{:?}",
            popup.rect
        );
    }

    /// Three options of the same tall rows have **no** floor, so the cap
    /// stands: the boundary is `> 3`, not `>= 3`.
    #[test]
    fn three_tall_options_are_still_capped() {
        let anchor = Rect::new(0.0, 400.0, 100.0, 420.0);
        let popup = place(anchor, 800.0, 3, 60.0).expect("a three-row list opens");
        assert!(
            ((popup.rect.y1 - popup.rect.y0) - 140.0).abs() < 1e-4,
            "{:?}",
            popup.rect
        );
    }

    /// A widget filling the page has nowhere to open, and `place` refuses
    /// rather than returning a zero-height rectangle — `SetPopup`'s
    /// non-positive `fPopupRet` exit.
    #[test]
    fn nowhere_to_open_refuses() {
        let anchor = Rect::new(0.0, 0.0, 100.0, 200.0);
        assert_eq!(place(anchor, 200.0, 2, 13.392), None);
    }

    /// An empty list refuses too — the zero-height content rectangle.
    #[test]
    fn an_empty_list_refuses() {
        let anchor = Rect::new(0.0, 100.0, 100.0, 120.0);
        assert_eq!(place(anchor, 400.0, 0, 13.392), None);
        assert_eq!(place(anchor, 400.0, 2, 0.0), None);
    }

    /// The plate is the window less its border, and rows stack down from the
    /// plate's top.
    #[test]
    fn rows_stack_downward_from_the_plate() {
        let anchor = Rect::new(165.7, 315.9, 315.7, 330.1);
        let popup = place(anchor, 342.0, 2, 13.392).expect("a two-row list fits");
        let plate = popup.plate_f32();
        assert!((plate.top - 314.9).abs() < 1e-4, "{plate:?}");
        assert!((plate.left - 166.7).abs() < 1e-4);
        let first = popup.row_rect(0);
        assert!((first.y1 - 314.9).abs() < 1e-4, "{first:?}");
        assert!((first.y0 - (314.9 - 13.392)).abs() < 1e-3);
        let second = popup.row_rect(1);
        assert!((second.y1 - first.y0).abs() < 1e-6);
    }

    /// A click inside the plate answers its row; one outside answers none.
    #[test]
    fn a_point_finds_its_row() {
        let anchor = Rect::new(165.7, 315.9, 315.7, 330.1);
        let popup = place(anchor, 342.0, 2, 13.392).expect("a two-row list fits");
        // `bug_736695_3` clicks (312, 310), which is the first row.
        assert_eq!(popup.row_at(312.0, 310.0), Some(0));
        assert_eq!(popup.row_at(312.0, 295.0), Some(1));
        // Inside the widget, above the list.
        assert_eq!(popup.row_at(312.0, 324.0), None);
        // Below the list.
        assert_eq!(popup.row_at(312.0, 280.0), None);
        // Left of it.
        assert_eq!(popup.row_at(100.0, 310.0), None);
    }

    /// A scroll view is scrollable exactly when a row does not fit.
    #[test]
    fn scroll_view_reports_whether_it_scrolls() {
        assert!(
            ScrollView {
                top_visible: 0,
                visible_rows: 3,
                total: 9,
            }
            .is_scrollable()
        );
        assert!(
            !ScrollView {
                top_visible: 0,
                visible_rows: 9,
                total: 9,
            }
            .is_scrollable()
        );
    }
}
