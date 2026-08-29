//! Tiling patterns, `/PatternType 1` (ISO 32000-1 §8.7.3.2).
//!
//! A content stream repeated on a grid. Three behaviours are worth naming
//! because each one silently makes a pattern paint nothing:
//!
//! - **`/XStep` and `/YStep` are absolute-valued at load**, so a negative
//!   step becomes positive and its sign never reaches the tiler.
//! - **A zero or non-finite step draws nothing at all.** Negatives cannot
//!   reach that test, having already been made positive, so the only way to
//!   trip it is a literal zero, a missing key (which reads as zero), or an
//!   infinity.
//! - **Tile indices that do not fit an `i32` abort the whole pattern.** This
//!   is what stops an absurdly small step from asking for an unbounded number
//!   of tiles.
//!
//! A degenerate `/BBox` does *not* abort: the cell is clamped to one pixel
//! by one and still drawn.

use crate::names;
use kurbo::{Affine, Rect};
use pdfrum_common::{DiagKind, Diagnostics, Limits, Severity};
use pdfrum_object::{ByteSpan, Dict, Resolve, Stream};

/// A `/PatternType 1` pattern.
#[derive(Debug, Clone, PartialEq)]
pub struct TilingPattern {
    /// Whether the tile supplies its own colour (`/PaintType 1`). Anything
    /// else, including `/PaintType 2`, a zero, and a missing key, is
    /// uncoloured and takes its colour from the `scn` operands.
    pub colored: bool,
    /// The horizontal spacing, already absolute-valued.
    pub x_step: f32,
    /// The vertical spacing, already absolute-valued.
    pub y_step: f32,
    /// The tile's clipping rectangle. Requires exactly four elements or it is
    /// all zeros.
    pub bbox: Rect,
    /// The pattern's space composed with the parent matrix.
    pub matrix: Affine,
    /// The tile's `/Resources`.
    pub resources: Option<Dict>,
    /// The tile's content stream, still filtered.
    pub content: ByteSpan,
}

/// The range of tile indices covering a clip rectangle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TileRange {
    /// Lowest column index, inclusive.
    pub min_col: i32,
    /// Highest column index, inclusive.
    pub max_col: i32,
    /// Lowest row index, inclusive.
    pub min_row: i32,
    /// Highest row index, inclusive.
    pub max_row: i32,
}

impl TilingPattern {
    /// Load from the stream a `/Pattern` resource names.
    pub(super) fn load<R: Resolve>(
        stream: &Stream,
        matrix: Affine,
        r: &R,
        limits: &Limits,
        diags: &mut Diagnostics,
    ) -> Self {
        let _ = (limits, diags);
        let dict = &stream.dict;
        Self {
            colored: dict.int(names::PAINT_TYPE, r) == Some(1),
            // Absolute-valued here, so the tiler never sees a negative.
            x_step: dict.number(names::X_STEP, r).unwrap_or(0.0).abs(),
            y_step: dict.number(names::Y_STEP, r).unwrap_or(0.0).abs(),
            // Exactly four elements or an all-zero rectangle.
            bbox: dict
                .array(names::BBOX, r)
                .filter(|a| a.len() == 4)
                .map_or(Rect::ZERO, |a| a.as_rect()),
            matrix,
            resources: dict.dict(names::RESOURCES, r),
            content: stream.data.clone(),
        }
    }

    /// Whether the steps allow the pattern to be drawn at all.
    ///
    /// A zero or non-finite step means it paints nothing — silently, in the
    /// C++, and with a diagnostic here.
    #[must_use]
    pub fn steps_are_drawable(&self) -> bool {
        self.x_step.is_finite()
            && self.y_step.is_finite()
            && self.x_step != 0.0
            && self.y_step != 0.0
    }

    /// The tile indices covering `clip`, in the pattern's own space.
    ///
    /// `None` when the steps are unusable or an index does not fit an `i32`,
    /// both of which abort the whole pattern.
    #[must_use]
    pub fn tile_range(&self, clip: Rect, diags: &mut Diagnostics) -> Option<TileRange> {
        if !self.steps_are_drawable() {
            diags.record(Severity::Suspicious, DiagKind::TilingStepInvalid, None);
            return None;
        }
        let x_step = f64::from(self.x_step);
        let y_step = f64::from(self.y_step);
        // Each bound is a *checked* float-to-integer conversion; any failure
        // aborts, which is what bounds a tiny step's tile count.
        let to_i32 = |v: f64| -> Option<i32> {
            if !v.is_finite() || v < f64::from(i32::MIN) || v > f64::from(i32::MAX) {
                return None;
            }
            #[expect(
                clippy::cast_possible_truncation,
                reason = "the range check above is the C++'s checked conversion"
            )]
            Some(v as i32)
        };
        let range = (|| {
            Some(TileRange {
                min_col: to_i32(((clip.x0 - self.bbox.x1) / x_step).ceil())?,
                max_col: to_i32(((clip.x1 - self.bbox.x0) / x_step).floor())?,
                min_row: to_i32(((clip.y0 - self.bbox.y1) / y_step).ceil())?,
                max_row: to_i32(((clip.y1 - self.bbox.y0) / y_step).floor())?,
            })
        })();
        if range.is_none() {
            diags.record(Severity::Suspicious, DiagKind::TilingRangeOverflow, None);
        }
        range
    }

    /// The tile's pixel size in the device space `matrix` maps to, clamped to
    /// at least one by one.
    ///
    /// A degenerate `/BBox` still produces a tile; only a size that will not
    /// fit an `i32` aborts.
    #[must_use]
    pub fn cell_size(&self, to_device: Affine) -> Option<(i32, i32)> {
        let cell = (to_device * self.matrix).transform_rect_bbox(self.bbox);
        let width = cell.width().ceil();
        let height = cell.height().ceil();
        if !width.is_finite()
            || !height.is_finite()
            || width > f64::from(i32::MAX)
            || height > f64::from(i32::MAX)
        {
            return None;
        }
        #[expect(
            clippy::cast_possible_truncation,
            reason = "the range check above is the C++'s checked conversion"
        )]
        let (w, h) = (width as i32, height as i32);
        Some((w.max(1), h.max(1)))
    }
}

#[cfg(test)]
mod tests {
    // Test fixtures quote the oracle's own vectors, compare floats exactly
    // where the behaviour being pinned is exact, and index arrays whose
    // length the fixture itself fixes.
    #![allow(
        clippy::unreadable_literal,
        clippy::float_cmp,
        clippy::indexing_slicing,
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        reason = "test fixtures quote oracle vectors verbatim and compare exactly"
    )]

    use super::TilingPattern;
    use kurbo::{Affine, Rect};
    use pdfrum_common::{DiagKind, Diagnostics};
    use pdfrum_object::ByteSpan;

    fn pattern(x_step: f32, y_step: f32) -> TilingPattern {
        TilingPattern {
            colored: true,
            x_step,
            y_step,
            bbox: Rect::new(0.0, 0.0, 10.0, 10.0),
            matrix: Affine::IDENTITY,
            resources: None,
            content: ByteSpan::empty(),
        }
    }

    #[test]
    fn zero_and_non_finite_steps_draw_nothing() {
        let mut diags = Diagnostics::default();
        assert!(!pattern(0.0, 10.0).steps_are_drawable());
        assert!(!pattern(10.0, 0.0).steps_are_drawable());
        assert!(!pattern(f32::NAN, 10.0).steps_are_drawable());
        assert!(!pattern(f32::INFINITY, 10.0).steps_are_drawable());
        assert!(pattern(10.0, 10.0).steps_are_drawable());
        assert!(
            pattern(0.0, 10.0)
                .tile_range(Rect::new(0.0, 0.0, 100.0, 100.0), &mut diags)
                .is_none()
        );
        assert!(diags.contains(&DiagKind::TilingStepInvalid));
    }

    #[test]
    fn a_tiny_step_aborts_rather_than_asking_for_endless_tiles() {
        let mut diags = Diagnostics::default();
        let p = pattern(1e-30, 1e-30);
        assert!(
            p.tile_range(Rect::new(0.0, 0.0, 100.0, 100.0), &mut diags)
                .is_none()
        );
        assert!(diags.contains(&DiagKind::TilingRangeOverflow));
    }

    #[test]
    fn a_reasonable_step_covers_the_clip() {
        let mut diags = Diagnostics::default();
        let p = pattern(10.0, 10.0);
        let range = p
            .tile_range(Rect::new(0.0, 0.0, 100.0, 100.0), &mut diags)
            .expect("a tile range");
        assert!(range.min_col <= 0);
        assert!(range.max_col >= 9);
        assert!(diags.is_empty());
    }

    #[test]
    fn a_degenerate_bbox_still_yields_a_one_pixel_cell() {
        let p = TilingPattern {
            bbox: Rect::ZERO,
            ..pattern(10.0, 10.0)
        };
        assert_eq!(p.cell_size(Affine::IDENTITY), Some((1, 1)));
        // A negative-extent bbox likewise.
        let p = TilingPattern {
            bbox: Rect::new(10.0, 10.0, 0.0, 0.0),
            ..pattern(10.0, 10.0)
        };
        let (w, h) = p.cell_size(Affine::IDENTITY).expect("a cell");
        assert!(w >= 1 && h >= 1);
    }

    #[test]
    fn an_enormous_cell_aborts() {
        let p = TilingPattern {
            bbox: Rect::new(0.0, 0.0, 1e30, 1e30),
            ..pattern(10.0, 10.0)
        };
        assert!(p.cell_size(Affine::IDENTITY).is_none());
    }
}
