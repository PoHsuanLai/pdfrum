//! N-up imposition: several source pages onto one sheet.
//!
//! Each source page becomes a Form `XObject`, and the sheet's content stream is
//! one `q … cm … Do Q` fragment per slot.
//!
//! # Slots fill visually, but are numbered from the bottom
//!
//! PDF's origin is bottom-left and reading order is top-left, so the slot
//! index has to be flipped: sub-page *i* of an `x` by `y` grid lands at
//! column `i % x` and row `y − (i / x) − 1`. Get that wrong and a 2×2
//! imposition comes out upside down rather than obviously broken.
//!
//! # Centering happens on exactly one axis
//!
//! The scale is uniform — the larger of the two ratios would crop — so one
//! axis has slack and the other does not. Only the slack one is centred, and
//! the comparison is a strict `>`, which sends the exact-fit tie down the
//! branch that adds nothing. There is **no clamping**: a source page smaller
//! than its slot is scaled *up* to fill it.

use pdfrum_common::kurbo::Affine;

use crate::content::num::write_matrix;

/// Where one sub-page lands inside its sheet, and how much it is scaled.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PageEdit {
    /// The bottom-left corner of the sub-page, in sheet coordinates.
    pub(crate) start_x: f32,
    /// See [`PageEdit::start_x`].
    pub(crate) start_y: f32,
    /// The uniform scale, which may be greater than 1.
    pub(crate) scale: f32,
}

impl PageEdit {
    /// The matrix a `cm` operator writes: scale, then translate.
    #[must_use]
    pub(crate) fn matrix(self) -> Affine {
        Affine::new([
            f64::from(self.scale),
            0.0,
            0.0,
            f64::from(self.scale),
            f64::from(self.start_x),
            f64::from(self.start_y),
        ])
    }
}

/// A grid of sub-pages on a sheet of a fixed size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct NupGrid {
    /// Sheet width in points.
    pub(crate) sheet_width: f32,
    /// Sheet height in points.
    pub(crate) sheet_height: f32,
    /// Columns.
    pub(crate) x: u32,
    /// Rows.
    pub(crate) y: u32,
}

impl NupGrid {
    /// The size of one slot.
    #[must_use]
    pub(crate) fn slot_size(self) -> (f32, f32) {
        // A grid dimension past 2^24 would lose precision here, and would
        // also give every slot a sub-pixel width — the geometry is
        // meaningless long before the cast is.
        (
            self.sheet_width / as_f32(self.x.max(1)),
            self.sheet_height / as_f32(self.y.max(1)),
        )
    }

    /// How many sub-pages fit on one sheet.
    #[must_use]
    pub(crate) fn per_sheet(self) -> u32 {
        self.x.saturating_mul(self.y)
    }

    /// The column and bottom-origin row of sub-page `index` on its sheet.
    ///
    /// Reading order runs left to right, top to bottom; the row is flipped
    /// because PDF counts up from the bottom.
    #[must_use]
    pub(crate) fn slot(self, index: u32) -> (u32, u32) {
        let x = self.x.max(1);
        let column = index % x;
        let row_from_top = index / x;
        let row = self.y.saturating_sub(row_from_top).saturating_sub(1);
        (column, row)
    }

    /// Where a source page of `(width, height)` lands in slot `index`.
    #[must_use]
    pub(crate) fn edit(self, index: u32, page_width: f32, page_height: f32) -> PageEdit {
        let (slot_w, slot_h) = self.slot_size();
        let (column, row) = self.slot(index);
        let mut start_x = as_f32(column) * slot_w;
        let mut start_y = as_f32(row) * slot_h;

        // A degenerate source page cannot be scaled to fit anything.
        if page_width <= 0.0 || page_height <= 0.0 {
            return PageEdit {
                start_x,
                start_y,
                scale: 1.0,
            };
        }

        let ratio_x = slot_w / page_width;
        let ratio_y = slot_h / page_height;
        // Uniform: the smaller ratio, so nothing is cropped. No clamping —
        // a page smaller than its slot is scaled up.
        let scale = ratio_x.min(ratio_y);

        // Centre the slack axis only. The strict `>` sends an exact tie down
        // the else branch, which adds zero.
        if ratio_x > ratio_y {
            start_x += (slot_w - page_width * scale) / 2.0;
        } else {
            start_y += (slot_h - page_height * scale) / 2.0;
        }

        PageEdit {
            start_x,
            start_y,
            scale,
        }
    }
}

/// A grid coordinate as a float, saturating where precision would be lost.
///
/// Exact below 2^24, which is far past any grid a sheet of paper admits.
fn as_f32(value: u32) -> f32 {
    #[expect(
        clippy::cast_precision_loss,
        reason = "a grid dimension past 2^24 gives every slot a sub-pixel \
                  width; the geometry is meaningless long before the cast is"
    )]
    {
        value as f32
    }
}

/// The content fragment that draws one sub-page.
///
/// Exactly `q\n{matrix} cm\n/{name} Do Q\n` — note the newline *after* `cm`
/// and that `Q` sits on the same line as `Do`. Fragments are concatenated
/// with no separator; each one's trailing newline is the only delimiter.
#[must_use]
pub(crate) fn sub_page_fragment(name: &str, edit: PageEdit) -> String {
    let mut out = String::from("q\n");
    write_matrix(&mut out, edit.matrix());
    out.push_str(" cm\n/");
    out.push_str(name);
    out.push_str(" Do Q\n");
    out
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::float_cmp,
        reason = "these assertions pin exact values a writer must produce"
    )]
    use super::{NupGrid, PageEdit, sub_page_fragment};

    fn grid(width: f32, height: f32, x: u32, y: u32) -> NupGrid {
        NupGrid {
            sheet_width: width,
            sheet_height: height,
            x,
            y,
        }
    }

    // cpdf_npagetooneexporter_unittest.cpp:10-19 — the whole file, one test,
    // and it pins the layout and the number spelling at once.
    #[test]
    fn the_sub_page_fragment_golden() {
        let edit = PageEdit {
            start_x: 0.000_001,
            start_y: 1_000_000_000_000.0,
            scale: 0.5,
        };
        assert_eq!(
            sub_page_fragment("foo", edit),
            "q\n.5 0 0 .5 .000001 1000000000000 cm\n/foo Do Q\n"
        );
    }

    // The flip: reading order in, bottom-origin slot out.
    #[test]
    fn slots_fill_left_to_right_top_to_bottom() {
        let g = grid(600.0, 600.0, 3, 2);
        // Top row, left to right.
        assert_eq!(g.slot(0), (0, 1));
        assert_eq!(g.slot(1), (1, 1));
        assert_eq!(g.slot(2), (2, 1));
        // Bottom row.
        assert_eq!(g.slot(3), (0, 0));
        assert_eq!(g.slot(4), (1, 0));
        assert_eq!(g.slot(5), (2, 0));
    }

    #[test]
    fn a_single_column_stacks_downwards() {
        let g = grid(600.0, 900.0, 1, 3);
        assert_eq!(g.slot(0), (0, 2));
        assert_eq!(g.slot(1), (0, 1));
        assert_eq!(g.slot(2), (0, 0));
    }

    // ImportNPages (:108): the sheet counts.
    #[test]
    fn sheet_counts_round_up() {
        let sheets = |g: NupGrid, pages: usize| pages.div_ceil(g.per_sheet().max(1) as usize);
        assert_eq!(sheets(grid(612.0, 792.0, 2, 1), 5), 3);
        assert_eq!(sheets(grid(612.0, 792.0, 5, 1), 5), 1);
        assert_eq!(sheets(grid(792.0, 612.0, 8, 1), 5), 1);
        assert_eq!(sheets(grid(792.0, 612.0, 128, 1), 5), 1);
        assert_eq!(sheets(grid(792.0, 612.0, 3, 1), 5), 2);
    }

    // A source page the same shape as its slot fits exactly and is centred
    // on neither axis.
    #[test]
    fn an_exact_fit_is_centred_on_neither_axis() {
        let g = grid(600.0, 600.0, 2, 2);
        // Slot 2 is the bottom-left: column 0, row 0, so its origin is (0,0).
        let edit = g.edit(2, 300.0, 300.0);
        assert_eq!(edit.scale, 1.0);
        assert_eq!((edit.start_x, edit.start_y), (0.0, 0.0));
    }

    // A page wider than its slot's aspect leaves vertical slack, which is
    // the axis that gets centred.
    #[test]
    fn the_slack_axis_is_the_one_centred() {
        let g = grid(600.0, 600.0, 2, 2);
        // A 300x150 page in a 300x300 slot: scale 1, 150 points of slack up.
        let edit = g.edit(2, 300.0, 150.0);
        assert_eq!(edit.scale, 1.0);
        assert_eq!(edit.start_x, 0.0, "the tight axis is not moved");
        assert_eq!(edit.start_y, 75.0, "the slack axis is centred");

        // The other way round: a 150x300 page leaves horizontal slack.
        let edit = g.edit(2, 150.0, 300.0);
        assert_eq!(edit.start_x, 75.0);
        assert_eq!(edit.start_y, 0.0);
    }

    // No clamping: a small page is scaled up to fill its slot.
    #[test]
    fn a_page_smaller_than_its_slot_is_scaled_up() {
        let g = grid(600.0, 600.0, 2, 2);
        let edit = g.edit(2, 150.0, 150.0);
        assert_eq!(edit.scale, 2.0);
    }

    #[test]
    fn a_page_larger_than_its_slot_is_scaled_down_uniformly() {
        let g = grid(600.0, 600.0, 2, 2);
        // 600x300 into a 300x300 slot: the width binds, so scale is 0.5.
        let edit = g.edit(2, 600.0, 300.0);
        assert_eq!(edit.scale, 0.5);
        // The height then occupies 150 of 300, so 75 points of slack.
        assert_eq!(edit.start_y, 75.0);
    }

    // The slot offsets accumulate: slot 1 of a 2x2 sits half a sheet across.
    #[test]
    fn slot_offsets_place_the_sub_page_on_the_sheet() {
        let g = grid(600.0, 600.0, 2, 2);
        // Slot 1 is the top-right: column 1, row 1.
        let edit = g.edit(1, 300.0, 300.0);
        assert_eq!((edit.start_x, edit.start_y), (300.0, 300.0));
    }

    #[test]
    fn a_degenerate_source_page_is_placed_unscaled() {
        let g = grid(600.0, 600.0, 2, 2);
        let edit = g.edit(0, 0.0, 300.0);
        assert_eq!(edit.scale, 1.0);
    }

    #[test]
    fn the_matrix_scales_then_translates() {
        let edit = PageEdit {
            start_x: 10.0,
            start_y: 20.0,
            scale: 0.5,
        };
        assert_eq!(edit.matrix().as_coeffs(), [0.5, 0.0, 0.0, 0.5, 10.0, 20.0]);
    }

    #[test]
    fn slot_size_divides_the_sheet() {
        assert_eq!(grid(612.0, 792.0, 2, 1).slot_size(), (306.0, 792.0));
        assert_eq!(grid(600.0, 600.0, 3, 2).slot_size(), (200.0, 300.0));
    }
}
