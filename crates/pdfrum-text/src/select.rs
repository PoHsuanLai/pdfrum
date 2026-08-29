//! Selection helpers over the character list
//! (`docs/design/pdfrum-text.md` §1.16).
//!
//! These answer the questions a viewer asks while a user drags a cursor:
//! which boxes cover this run of characters, which character is under this
//! point, what text is inside this rectangle. None of them touches the
//! search-facing text — they read the character list directly.

use crate::charinfo::{CharBox, CharType, ObjectIndex};
use kurbo::{Point, Rect, Size};

/// The boxes covering a run of characters (`GetRectArray`).
///
/// One box per run of consecutive characters sharing a text object, so a line
/// set in two fonts yields two boxes. Generated characters and boxes too
/// small to see are skipped. A box is pushed **unconditionally at the end**,
/// so a run in which every character was skipped still yields one — an
/// all-zero rectangle, which the upstream tests pin.
#[must_use]
pub fn rects(chars: &[CharBox], start: usize, count: Option<usize>) -> Vec<Rect> {
    let mut out = Vec::new();
    let total = chars.len();
    if start >= total {
        return out;
    }
    // `None` is the C++'s negative count: "to the end".
    let count = match count {
        Some(0) => return out,
        Some(count) if start + count <= total => count,
        _ => total - start,
    };

    let mut object: Option<ObjectIndex> = None;
    let mut seen_any = false;
    let mut rect = Rect::ZERO;
    let mut new_rect = true;
    for info in chars.get(start..start + count).unwrap_or_default() {
        if info.char_type == CharType::Generated {
            continue;
        }
        if info.char_box.width() < 0.01 || info.char_box.height() < 0.01 {
            continue;
        }
        if !seen_any {
            object = info.object;
            seen_any = true;
        }
        if object != info.object {
            out.push(rect);
            object = info.object;
            new_rect = true;
        }
        if new_rect {
            new_rect = false;
            rect = normalize(info.char_box);
            continue;
        }
        rect = union(rect, info.char_box);
    }
    out.push(rect);
    out
}

/// The character under a point (`GetIndexAtPos`).
///
/// Exact containment wins outright and returns the **first** such character.
/// Failing that, and only when a tolerance was given, the nearest character
/// whose box expanded by half the tolerance still contains the point, scored
/// by the sum of the distances to the nearest edges.
#[must_use]
pub fn index_at(chars: &[CharBox], point: Point, tolerance: Size) -> Option<usize> {
    let mut nearest = None;
    // The C++ starts the two running distances at 5000 apiece, so a character
    // only ever wins if its combined distance is under ten thousand.
    let mut best = 10000.0f64;
    for (index, info) in chars.iter().enumerate() {
        if contains(info.char_box, point) {
            return Some(index);
        }
        if tolerance.width <= 0.0 && tolerance.height <= 0.0 {
            continue;
        }
        let rect = normalize(info.char_box);
        let expanded = Rect::new(
            rect.x0 - tolerance.width / 2.0,
            rect.y0 - tolerance.height / 2.0,
            rect.x1 + tolerance.width / 2.0,
            rect.y1 + tolerance.height / 2.0,
        );
        if !contains(expanded, point) {
            continue;
        }
        let dx = (point.x - rect.x0).abs().min((point.x - rect.x1).abs());
        let dy = (point.y - rect.y0).abs().min((point.y - rect.y1).abs());
        if dx + dy < best {
            best = dx + dy;
            nearest = Some(index);
        }
    }
    nearest
}

/// The text of every character a predicate accepts
/// (`GetTextByPredicate`).
///
/// Characters the predicate rejects are not simply skipped: a space bridges a
/// gap, and any other rejected character arms a line feed that the next
/// accepted character on a different baseline turns into a `\r\n`. That is
/// what makes a rectangle selection across two lines come back with the break
/// in it.
fn text_by(chars: &[CharBox], accept: impl Fn(&CharBox) -> bool) -> String {
    let mut out = String::new();
    let mut baseline = 0.0f64;
    let mut had_previous = false;
    let mut pending_feed = false;
    for info in chars {
        if accept(info) {
            if (baseline - info.origin.y).abs() > 0.0 && !had_previous && pending_feed {
                baseline = info.origin.y;
                if !out.is_empty() {
                    out.push_str("\r\n");
                }
            }
            had_previous = true;
            pending_feed = false;
            if info.unicode != 0
                && let Some(ch) = char::from_u32(info.unicode)
            {
                out.push(ch);
            }
        } else if info.unicode == u32::from(b' ') {
            if had_previous {
                out.push(' ');
                had_previous = false;
                pending_feed = false;
            }
        } else {
            had_previous = false;
            pending_feed = true;
        }
    }
    out
}

/// The text inside a rectangle (`GetTextByRect`).
#[must_use]
pub fn text_in_rect(chars: &[CharBox], rect: Rect) -> String {
    text_by(chars, |info| intersects(rect, info.char_box))
}

/// The text one text object drew (`GetTextByObject`).
#[must_use]
pub fn text_of_object(chars: &[CharBox], object: ObjectIndex) -> String {
    text_by(chars, |info| info.object == Some(object))
}

fn normalize(rect: Rect) -> Rect {
    Rect::new(
        rect.x0.min(rect.x1),
        rect.y0.min(rect.y1),
        rect.x0.max(rect.x1),
        rect.y0.max(rect.y1),
    )
}

fn union(a: Rect, b: Rect) -> Rect {
    Rect::new(
        a.x0.min(b.x0),
        a.y0.min(b.y0),
        a.x1.max(b.x1),
        a.y1.max(b.y1),
    )
}

fn contains(rect: Rect, point: Point) -> bool {
    point.x >= rect.x0 && point.x <= rect.x1 && point.y >= rect.y0 && point.y <= rect.y1
}

/// Whether two rectangles overlap with positive area, which is what
/// `IsRectIntersect` asks.
fn intersects(a: Rect, b: Rect) -> bool {
    let x0 = a.x0.max(b.x0);
    let y0 = a.y0.max(b.y0);
    let x1 = a.x1.min(b.x1);
    let y1 = a.y1.min(b.y1);
    x1 > x0 && y1 > y0
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
    use kurbo::Affine;
    use pdfrum_font::CharCode;

    fn info(ch: char, object: u32, x: f64) -> CharBox {
        CharBox {
            char_type: CharType::Normal,
            unicode: u32::from(ch),
            code: Some(CharCode(u32::from(ch))),
            origin: Point::new(x, 0.0),
            char_box: Rect::new(x, 0.0, x + 5.0, 10.0),
            loose_char_box: Rect::new(x, 0.0, x + 5.0, 10.0),
            matrix: Affine::IDENTITY,
            object: Some(ObjectIndex(object)),
            font_size: 10.0,
            angle: 0.0,
        }
    }

    fn run() -> Vec<CharBox> {
        vec![
            info('a', 0, 0.0),
            info('b', 0, 5.0),
            info('c', 1, 10.0),
            info('d', 1, 15.0),
        ]
    }

    #[test]
    fn one_rect_per_run_of_characters_sharing_an_object() {
        let chars = run();
        let boxes = rects(&chars, 0, None);
        assert_eq!(boxes.len(), 2);
        assert_eq!(boxes[0], Rect::new(0.0, 0.0, 10.0, 10.0));
        assert_eq!(boxes[1], Rect::new(10.0, 0.0, 20.0, 10.0));
    }

    #[test]
    fn a_run_that_skips_everything_still_pushes_one_rect() {
        // Every character generated: none contributes, but the final push is
        // unconditional, so an all-zero box comes back.
        let mut chars = run();
        for info in &mut chars {
            info.char_type = CharType::Generated;
        }
        assert_eq!(rects(&chars, 0, None), [Rect::ZERO]);
    }

    #[test]
    fn a_zero_count_or_out_of_range_start_yields_nothing() {
        let chars = run();
        assert!(rects(&chars, 0, Some(0)).is_empty());
        assert!(rects(&chars, 99, None).is_empty());
        assert!(rects(&[], 0, None).is_empty());
        // A count past the end is clamped rather than refused.
        assert_eq!(rects(&chars, 2, Some(500)).len(), 1);
    }

    #[test]
    fn exact_containment_wins_and_returns_the_first_match() {
        let chars = run();
        assert_eq!(index_at(&chars, Point::new(2.0, 5.0), Size::ZERO), Some(0));
        assert_eq!(index_at(&chars, Point::new(12.0, 5.0), Size::ZERO), Some(2));
        // With no tolerance, a point outside every box finds nothing.
        assert_eq!(index_at(&chars, Point::new(100.0, 100.0), Size::ZERO), None);
    }

    #[test]
    fn a_tolerance_finds_the_nearest_box() {
        let chars = run();
        // Just left of the first box, within a generous tolerance.
        let found = index_at(&chars, Point::new(-1.0, 5.0), Size::new(10.0, 10.0));
        assert_eq!(found, Some(0));
        // Far outside even the expanded boxes.
        assert_eq!(
            index_at(&chars, Point::new(-100.0, 5.0), Size::new(10.0, 10.0)),
            None
        );
    }

    #[test]
    fn text_by_object_returns_only_that_objects_characters() {
        let chars = run();
        assert_eq!(text_of_object(&chars, ObjectIndex(0)), "ab");
        assert_eq!(text_of_object(&chars, ObjectIndex(1)), "cd");
        assert_eq!(text_of_object(&chars, ObjectIndex(9)), "");
    }

    #[test]
    fn text_by_rect_returns_the_characters_it_overlaps() {
        let chars = run();
        assert_eq!(text_in_rect(&chars, Rect::new(0.0, 0.0, 9.0, 10.0)), "ab");
        assert_eq!(
            text_in_rect(&chars, Rect::new(0.0, 0.0, 100.0, 10.0)),
            "abcd"
        );
        assert_eq!(text_in_rect(&chars, Rect::new(50.0, 50.0, 60.0, 60.0)), "");
    }

    #[test]
    fn a_rejected_space_is_emitted_rather_than_skipped() {
        // A space the predicate rejects is not dropped: it is written out, so
        // a selection that covers two words separated by a space the caller
        // did not ask for still reads as two words.
        let mut chars = run();
        chars[1] = info(' ', 5, 5.0);
        assert_eq!(text_of_object(&chars, ObjectIndex(0)), "a ");
        // But only once, and only when something preceded it.
        let mut chars = run();
        chars[0] = info(' ', 5, 0.0);
        assert_eq!(text_of_object(&chars, ObjectIndex(0)), "b");
    }

    #[test]
    fn intersection_needs_positive_area() {
        // Touching edges do not intersect.
        assert!(!intersects(
            Rect::new(0.0, 0.0, 5.0, 5.0),
            Rect::new(5.0, 0.0, 10.0, 5.0)
        ));
        assert!(intersects(
            Rect::new(0.0, 0.0, 5.0, 5.0),
            Rect::new(4.0, 0.0, 10.0, 5.0)
        ));
    }
}
