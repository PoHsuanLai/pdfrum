//! Text line flow orientation detection.
//!
//! Estimates whether text runs horizontally or vertically across the page.

// Two guesses, one global and one per object. The global one is made once
// before any character is emitted and is the fallback whenever the per-object
// one cannot decide — which is often, because a one-glyph object has no
// direction of its own.

use crate::object::TextRun;
use pdfrum_page::Page;

/// Which way a line of text runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Orientation {
    /// No guess: the two line-end tests are both skipped for an object whose
    /// orientation resolves to this, so no line break is ever generated from
    /// geometry alone.
    #[default]
    Unknown,
    /// Left to right (or right to left) along a baseline.
    Horizontal,
    /// Top to bottom down a column.
    Vertical,
}

/// The page-global orientation guess (`FindTextlineFlowOrientation`).
///
/// Paints two occupancy masks — one per page axis — from the *page-level*
/// text objects' bounding boxes and asks which axis the text fills. Three
/// details are load-bearing and all look like mistakes:
///
/// - **`line_height` is seeded from the first object that contributes and is
///   never updated.** Not a median, not an average: whatever the first text
///   object's box happens to be tall.
/// - **Text inside form `XObject`s is not counted.** The scan walks the
///   page's own object list, and a form object is not a text object, so a
///   page whose text lives entirely inside forms scans nothing and comes back
///   [`Orientation::Unknown`].
/// - **The page dimensions truncate toward zero**, and so does twice the line
///   height, because both are `int32_t` in the C++.
#[must_use]
pub fn page_flow(page: &Page, runs: &[TextRun]) -> Orientation {
    let (width, height) = page.display_size();
    #[expect(
        clippy::cast_possible_truncation,
        reason = "page dimensions are int32_t in the C++, truncated the same way"
    )]
    let (page_width, page_height) = (width as i32, height as i32);
    if page_width <= 0 || page_height <= 0 {
        return Orientation::Unknown;
    }
    // Both are positive after the guard above.
    let (page_width, page_height) = (
        page_width.unsigned_abs() as usize,
        page_height.unsigned_abs() as usize,
    );

    let mut horizontal = vec![false; page_width];
    let mut vertical = vec![false; page_height];
    let mut line_height = 0.0f64;
    let (mut start_h, mut end_h) = (page_width, 0usize);
    let (mut start_v, mut end_v) = (page_height, 0usize);

    let mut runs = runs.iter();
    for index in crate::object::top_level_text_indices(&page.objects) {
        // Both sequences are ascending in the same flattened numbering, so
        // one forward scan pairs a page-level text object with the run
        // `walk` already built for it. A text object with no font builds no
        // run and is simply not found -- the same objects the old rebuild
        // skipped, since it read `build`'s `None` as an empty rect.
        let Some(run) = runs.find(|run| run.index.0 >= index.0) else {
            break;
        };
        if run.index != index {
            continue;
        }
        let rect = run.rect;
        let clamp = |value: f64, limit: usize| -> usize {
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                clippy::cast_precision_loss,
                reason = "clamped into 0..=limit before the cast, and a page is \
                          a few thousand units wide"
            )]
            let clamped = value.clamp(0.0, limit as f64) as usize;
            clamped
        };
        let (min_h, max_h) = (clamp(rect.x0, page_width), clamp(rect.x1, page_width));
        let (min_v, max_v) = (clamp(rect.y0, page_height), clamp(rect.y1, page_height));
        if min_h >= max_h || min_v >= max_v {
            continue;
        }
        for cell in horizontal.get_mut(min_h..max_h).unwrap_or_default() {
            *cell = true;
        }
        for cell in vertical.get_mut(min_v..max_v).unwrap_or_default() {
            *cell = true;
        }
        start_h = start_h.min(min_h);
        end_h = end_h.max(max_h);
        start_v = start_v.min(min_v);
        end_v = end_v.max(max_v);
        if line_height <= 0.0 {
            line_height = rect.height();
        }
    }

    #[expect(
        clippy::cast_possible_truncation,
        reason = "int32_t truncation in the C++, reproduced"
    )]
    let double_line_height = (2.0 * line_height) as i32;
    let span = |start: usize, end: usize| -> i32 {
        i32::try_from(end).unwrap_or(i32::MAX) - i32::try_from(start).unwrap_or(i32::MAX)
    };
    if span(start_v, end_v) < double_line_height {
        return Orientation::Horizontal;
    }
    if span(start_h, end_h) < double_line_height {
        return Orientation::Vertical;
    }
    let sum_h = filled(&horizontal, start_h, end_h);
    if sum_h > 0.8 {
        return Orientation::Horizontal;
    }
    let sum_v = filled(&vertical, start_v, end_v);
    if sum_h > sum_v {
        Orientation::Horizontal
    } else if sum_h < sum_v {
        Orientation::Vertical
    } else {
        Orientation::Unknown
    }
}

/// The fraction of a mask's cells that are set over `start..end`, or zero for
/// an empty span (`MaskPercentFilled`).
fn filled(mask: &[bool], start: usize, end: usize) -> f32 {
    if start >= end {
        return 0.0;
    }
    let Some(span) = mask.get(start..end) else {
        return 0.0;
    };
    #[expect(
        clippy::cast_precision_loss,
        reason = "a page is a few thousand cells wide; the C++ divides in float too"
    )]
    let ratio = span.iter().filter(|set| **set).count() as f32 / (end - start) as f32;
    ratio
}

/// The orientation one text object suggests (`GetTextObjectWritingMode`).
///
/// Takes the vector from the first glyph's origin to the last, transformed
/// into page space, and asks which axis it is within five degrees of. Both
/// components inside the cone (a diagonal) or both outside means "trust the
/// page-global guess"; exactly one outside picks that axis.
///
/// A run of one glyph or fewer has no vector at all and falls straight
/// through to the page-global guess.
#[must_use]
pub fn object_flow(run: &TextRun, page_flow: Orientation) -> Orientation {
    if run.count() <= 1 {
        return page_flow;
    }
    let (Some(first), Some(last)) = (run.item(0), run.item(run.count() - 1)) else {
        return page_flow;
    };
    // The *linear* part only: the object's own translation cancels out of a
    // difference, and the C++ transforms both origins by the same matrix.
    let first = run.text_matrix * first.origin;
    let last = run.text_matrix * last.origin;
    let dx = (last.x - first.x).abs();
    let dy = (last.y - first.y).abs();
    if dx <= 0.0001 && dy <= 0.0001 {
        return Orientation::Unknown;
    }
    let length = dx.hypot(dy);
    // `CFX_VectorF::Normalize` leaves a very short vector alone, but the
    // guard above already put at least one component past that threshold.
    let (unit_x, unit_y) = if length < 0.0001 {
        (dx, dy)
    } else {
        (dx / length, dy / length)
    };
    // 0.0872 is sin(5 degrees): the cone within which an axis counts as
    // aligned.
    let x_inside = unit_x <= 0.0872;
    if unit_y <= 0.0872 {
        return if x_inside {
            page_flow
        } else {
            Orientation::Horizontal
        };
    }
    if x_inside {
        Orientation::Vertical
    } else {
        page_flow
    }
}

#[cfg(test)]
mod tests {
    // Test fixtures quote the oracle's own vectors, compare floats exactly
    // where the behaviour being pinned is exact, and index arrays whose
    // length the fixture itself fixes.
    #![allow(
        clippy::float_cmp,
        clippy::indexing_slicing,
        clippy::unreadable_literal,
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        reason = "test fixtures quote oracle vectors verbatim and compare exactly"
    )]

    use super::*;
    use kurbo::Rect;

    #[test]
    fn a_zero_sized_page_has_no_orientation() {
        let mut page = Page::empty();
        page.crop_box = Rect::ZERO;
        assert_eq!(page_flow(&page, &[]), Orientation::Unknown);
    }

    #[test]
    fn a_page_with_no_text_objects_comes_back_horizontal() {
        // With nothing scanned the vertical span is `0 - page_height`, a
        // large negative, and twice a zero line height is zero -- so the very
        // first test fires and the answer is Horizontal. The arithmetic
        // matters because a page whose text lives entirely inside form
        // XObjects scans nothing and lands here.
        let page = Page::empty();
        assert_eq!(page_flow(&page, &[]), Orientation::Horizontal);
    }

    #[test]
    fn an_empty_span_is_zero_percent_filled() {
        assert_eq!(filled(&[true, true], 1, 1), 0.0);
        assert_eq!(filled(&[true, true], 2, 1), 0.0);
        assert_eq!(filled(&[true, false, true, true], 0, 4), 0.75);
        // A span past the end reads as empty rather than panicking.
        assert_eq!(filled(&[true], 0, 9), 0.0);
    }
}
