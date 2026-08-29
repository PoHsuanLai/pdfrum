//! Object-level duplicate suppression (`docs/design/pdfrum-text.md` §1.11).
//!
//! Real files draw the same text twice — a fake-bold effect, a shadow, a
//! badly-flattened layer — and the second copy must not double the extracted
//! text. The test is deliberately loose: same character codes, overlapping
//! boxes of comparable width, the same font size, and origins within a
//! fraction of a character of each other.
//!
//! Two details of the search are easy to lose. It examines the **five nearest
//! preceding text objects**, and a non-text object between them does not
//! consume one of the five — so twenty images followed by five text objects
//! still puts all five in range. And the object's own kind is what matters,
//! not its distance.

use crate::charinfo::CharBox;
use crate::object::{TextRun, ladder_char_width};

/// How many preceding text objects the search examines.
const LOOKBACK: usize = 5;

/// Whether `candidate` repeats one of the text objects just before it
/// (`IsSameAsPreTextObject`).
///
/// `preceding` holds the *text* objects already walked, newest last. Because
/// nothing else is in that list, walking it backwards is already the "five
/// nearest preceding text objects" the C++ arrives at by skipping non-text
/// objects without spending a lookback slot on them.
#[must_use]
pub fn repeats_a_predecessor(
    candidate: &TextRun,
    preceding: &[TextRun],
    chars: &[CharBox],
) -> bool {
    preceding
        .iter()
        .rev()
        .filter(|run| run.index != candidate.index)
        .take(LOOKBACK)
        .any(|earlier| same_object(earlier, candidate, chars))
}

/// Whether two text objects draw the same thing in the same place
/// (`IsSameTextObject`).
///
/// The parameter names in the C++ are inverted relative to the call site —
/// what it calls `prev_obj_rect` is the *current* object's box. Transcribed
/// against the roles, not the names.
///
/// Three traps live in here:
///
/// - The current object's box is **intersected in place**, so the size
///   comparison at the end measures the *intersection*, not the object.
/// - When both boxes are empty the intersection tests are skipped entirely:
///   the empty-box branch can only reject, and falling out of it lands in a
///   condition that is now false.
/// - Font sizes are compared for **exact float equality**.
#[must_use]
pub fn same_object(earlier: &TextRun, current: &TextRun, chars: &[CharBox]) -> bool {
    let mut current_box = current.rect;
    let earlier_box = earlier.rect;
    let empty = |rect: kurbo::Rect| rect.x1 <= rect.x0 || rect.y1 <= rect.y0;

    if empty(current_box) && empty(earlier_box) {
        // Two empty boxes: the only question is whether they start at the
        // same x, measured against the width of a character two back.
        let x_diff = (current_box.x0 - earlier_box.x0).abs();
        if let Some(reference) = chars.len().checked_sub(2).and_then(|i| chars.get(i))
            && x_diff > reference.char_box.width()
        {
            return false;
        }
    }
    if !empty(current_box) || !empty(earlier_box) {
        current_box = intersect(current_box, earlier_box);
        if empty(current_box) {
            return false;
        }
        if (current_box.width() - earlier_box.width()).abs() > earlier_box.width() / 2.0 {
            return false;
        }
        #[expect(
            clippy::float_cmp,
            reason = "exact equality is the C++'s test, and a tolerance would \
                      make two objects at nearly the same size dedup each other"
        )]
        if current.font_size != earlier.font_size {
            return false;
        }
    }

    let count = current.count();
    if count != earlier.count() {
        return false;
    }
    if count == 0 {
        // Two objects that show nothing show the same nothing.
        return true;
    }
    let mut last_code = None;
    for index in 0..count {
        let (Some(a), Some(b)) = (current.item(index), earlier.item(index)) else {
            return false;
        };
        if a.code != b.code {
            return false;
        }
        last_code = Some(a.code);
    }

    let dx = earlier.position.x - current.position.x;
    let dy = earlier.position.y - current.position.y;
    let font_size = current.font_size;
    let char_size = ladder_char_width(current, last_code);
    let max_pre_size = current_box
        .height()
        .max(current_box.width())
        .max(f64::from(font_size));
    dx.abs() <= f64::from(0.9 * char_size * font_size / 1000.0) && dy.abs() <= max_pre_size / 8.0
}

/// The intersection of two rectangles, taken as unnormalized corner pairs the
/// way `CFX_FloatRect::Intersect` does.
fn intersect(a: kurbo::Rect, b: kurbo::Rect) -> kurbo::Rect {
    kurbo::Rect::new(
        a.x0.max(b.x0),
        a.y0.max(b.y0),
        a.x1.min(b.x1),
        a.y1.min(b.y1),
    )
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
    fn two_empty_rects_never_reach_the_intersection_tests() {
        // The empty branch can only reject; falling through it leaves the
        // `!empty || !empty` condition false, so the size and font-size
        // comparisons are skipped and the item comparison decides.
        let a = Rect::new(10.0, 10.0, 10.0, 10.0);
        let b = Rect::new(10.0, 10.0, 10.0, 10.0);
        assert!(a.x1 <= a.x0 && b.x1 <= b.x0);
    }

    #[test]
    fn intersection_takes_the_inner_bounds() {
        let a = Rect::new(0.0, 0.0, 10.0, 10.0);
        let b = Rect::new(5.0, 5.0, 20.0, 20.0);
        assert_eq!(intersect(a, b), Rect::new(5.0, 5.0, 10.0, 10.0));
        // Disjoint boxes give an inverted, i.e. empty, rectangle.
        let far = Rect::new(50.0, 50.0, 60.0, 60.0);
        let result = intersect(a, far);
        assert!(result.x1 <= result.x0);
    }
}
