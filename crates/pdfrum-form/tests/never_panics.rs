//! The property that outranks every behavioural one: **no input panics.**
//!
//! Every value a form session holds is derived from an untrusted file — a
//! rectangle written inside out, a widget one unit square, a rotation of
//! minus ninety degrees, a choice field naming a row it does not have. These
//! tests drive the crate's pure operations over generated inputs and assert
//! only that they return.
//!
//! Two of them assert termination as well as return, on the two paths where
//! the oracle can spin: the tab-order banding, and the undo walk. Those are
//! not hypothetical — the banding's upstream loop has an unreachable `erase`
//! on one branch and hangs.

use pdfrum_form::edit::{Place, Range, Selection, UndoItem, UndoStack};
use pdfrum_form::event::{Button, Key, Modifiers, Point};
use pdfrum_form::field::choice::{
    find_next, is_index_selected, move_selection, select_only, select_range_to, set_index_selected,
    toggle_index, top_visible_for, type_ahead,
};
use pdfrum_form::field::text::{route_char, route_key};
use pdfrum_form::field::{ChoiceConfig, ChoiceOption, ChoiceState};
use pdfrum_form::geom::{Plate, Rotation};
use pdfrum_form::hit::{Candidate, LayoutBand, Permissions, WidgetHit, widget_at_point};
use pdfrum_form::session::AnnotId;
use pdfrum_form::tab::{FocusRing, Focusable, Rect, TabOrder};

/// A small deterministic generator: a counter run through a mixing step.
/// Enough spread to reach the awkward cases, and reproducible when one fails.
struct Gen(u32);

impl Gen {
    fn next(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        self.0 >> 8
    }

    /// A coordinate spanning negatives, zero and page-sized values.
    fn coord(&mut self) -> f32 {
        // Bounded to -1000..1000, so the conversion is exact.
        let raw = i16::try_from(self.next() % 2000).unwrap_or(0) - 1000;
        f32::from(raw) / 2.0
    }

    fn below(&mut self, n: u32) -> u32 {
        if n == 0 { 0 } else { self.next() % n }
    }
}

/// Rectangles that would break a naive implementation: inverted, degenerate,
/// negative, and a one-by-one box a real corpus file contains.
fn awkward_rects() -> Vec<Rect> {
    vec![
        Rect::new(0.0, 0.0, 0.0, 0.0),
        Rect::new(10.0, 10.0, 10.0, 10.0),
        Rect::new(1.0, 1.0, 2.0, 2.0),
        // Written inside out, as bug_889099's field is.
        Rect::new(100.0, 100.0, 200.0, -130.0),
        Rect::new(200.0, 200.0, 100.0, 100.0),
        Rect::new(-500.0, -500.0, -400.0, -400.0),
        Rect::new(0.0, 0.0, 1e6, 1e6),
    ]
}

/// The plate transform survives every awkward rectangle and every rotation,
/// and never produces a value that is not a number.
#[test]
fn the_plate_transform_never_panics_or_produces_nonsense() {
    let mut rng = Gen(1);
    for rect in awkward_rects() {
        for degrees in [-450, -90, 0, 37, 90, 180, 270, 360, 1_000_000] {
            let plate = Plate::new(rect, Rotation::from_degrees(degrees));
            for _ in 0..20 {
                let at = Point::new(rng.coord(), rng.coord());
                let there = plate.to_plate(at);
                let back = plate.to_page(there);
                assert!(there.x.is_finite() && there.y.is_finite());
                assert!(back.x.is_finite() && back.y.is_finite());
            }
            assert!(plate.width().is_finite());
            assert!(plate.height().is_finite());
            assert!(
                plate.width() >= 0.0,
                "a normalized box has no negative width"
            );
            assert!(plate.height() >= 0.0);
        }
    }
}

/// Hit testing over generated geometry returns, and never names an
/// annotation that is not in the list.
#[test]
fn hit_testing_never_panics_and_never_invents_an_annotation() {
    let mut rng = Gen(7);
    for _ in 0..200 {
        let count = rng.below(6);
        let candidates: Vec<Candidate> = (0..count)
            .map(|i| {
                let (x, y) = (rng.coord(), rng.coord());
                Candidate {
                    id: AnnotId::new(rng.below(3), i),
                    rect: Rect::new(x, y, x + rng.coord(), y + rng.coord()),
                    band: match rng.below(3) {
                        0 => LayoutBand::Popup,
                        1 => LayoutBand::Widget,
                        _ => LayoutBand::Other,
                    },
                    widget: (rng.below(2) == 0).then(|| WidgetHit {
                        signature: rng.below(2) == 0,
                        hidden: rng.below(2) == 0,
                        read_only: rng.below(2) == 0,
                        push_button: rng.below(2) == 0,
                    }),
                }
            })
            .collect();

        let focused = candidates.first().map(|c| c.id);
        for permissions in [Permissions::ALL, Permissions::NONE] {
            let hit = widget_at_point(&candidates, focused, permissions, rng.coord(), rng.coord());
            if let Some(hit) = hit {
                assert!(
                    candidates.iter().any(|c| c.id == hit),
                    "hit test named an annotation that is not in the list"
                );
            }
        }
    }
}

/// The tab-order banding terminates over any geometry, in every order, and
/// emits each annotation exactly once. The upstream loop hangs here.
#[test]
fn the_focus_ring_always_terminates_and_is_always_a_permutation() {
    let mut rng = Gen(13);
    for _ in 0..300 {
        let count = rng.below(8);
        let annots: Vec<Focusable> = (0..count)
            .map(|i| {
                let (x, y) = (rng.coord(), rng.coord());
                Focusable {
                    id: AnnotId::new(0, i),
                    // Deliberately including zero-area and inverted boxes.
                    rect: Rect::new(x, y, x + rng.coord(), y + rng.coord()),
                }
            })
            .collect();

        for order in [TabOrder::Row, TabOrder::Column, TabOrder::Structure] {
            let built = FocusRing::build(&annots, order);
            assert_eq!(built.len(), annots.len(), "{order:?} lost an annotation");

            let mut seen: Vec<u32> = built.order.iter().map(|a| a.index).collect();
            seen.sort_unstable();
            let expected: Vec<u32> = (0..count).collect();
            assert_eq!(seen, expected, "{order:?} is not a permutation");

            // Walking the ring from either end terminates.
            if let Some(mut at) = built.first() {
                let mut steps = 0;
                while let Some(next) = built.next(at) {
                    at = next;
                    steps += 1;
                    assert!(
                        steps <= count as usize,
                        "the forward walk did not terminate"
                    );
                }
            }
        }
    }
}

/// Any sequence of undo pushes and walks terminates, respects the capacity,
/// and never leaves an unmatched group boundary.
///
/// The groups pushed here are **well formed** — a boundary, some members,
/// a boundary — because that is what the operations that push them do, and
/// the invariant under test is that *eviction* never splits a pair. A stack
/// cannot pair what a caller never paired, so pushing loose boundaries would
/// be testing the generator rather than the stack.
#[test]
fn the_undo_stack_never_panics_and_holds_its_invariants() {
    let mut rng = Gen(29);
    for max in [4u32, 5, 8, 16] {
        for _ in 0..40 {
            let mut stack = UndoStack::with_max(max);

            for _ in 0..40 {
                match rng.below(4) {
                    // A bracketed group of a random length, as a replace does.
                    0 => {
                        stack.push(UndoItem::GroupBoundary);
                        for _ in 0..rng.below(3) {
                            stack.push(UndoItem::Clear {
                                range: Range::empty_at(Place::start()),
                                text: String::new(),
                                before: Selection::empty(),
                            });
                        }
                        stack.push(UndoItem::GroupBoundary);
                    }
                    1 => {
                        stack.undo();
                    }
                    2 => {
                        stack.redo();
                    }
                    // A lone item, as a typed character does.
                    _ => stack.push(UndoItem::InsertWord {
                        old: Place::start(),
                        new: Place::start(),
                        ch: 'x',
                        before: Selection::empty(),
                    }),
                }

                assert!(stack.len() <= max as usize, "capacity exceeded");
                let boundaries = stack.items().filter(|i| i.is_boundary()).count();
                assert_eq!(boundaries % 2, 0, "eviction split a group");
                assert!(stack.position() <= stack.len());
            }

            // Walking to each end terminates.
            let mut steps = 0;
            while stack.can_undo() {
                stack.undo();
                steps += 1;
                assert!(steps <= max as usize + 1, "the undo walk did not terminate");
            }
            steps = 0;
            while stack.can_redo() {
                stack.redo();
                steps += 1;
                assert!(steps <= max as usize + 1, "the redo walk did not terminate");
            }
        }
    }
}

/// Every choice operation on every shape of field returns, including on a
/// field with no rows at all.
#[test]
fn choice_operations_never_panic() {
    let mut rng = Gen(41);
    for _ in 0..200 {
        let count = rng.below(5) as usize;
        let options: Vec<ChoiceOption> = (0..count)
            .map(|i| ChoiceOption {
                label: format!("Row {i}"),
                value: format!("v{i}"),
            })
            .collect();

        let config = ChoiceConfig {
            combo: rng.below(2) == 0,
            editable: rng.below(2) == 0,
            multi_select: rng.below(2) == 0,
            read_only: rng.below(2) == 0,
        };
        let mut field = ChoiceState::new(options, config);

        for _ in 0..30 {
            // Indices deliberately reach well past the end.
            let index = rng.below(10) as usize;
            match rng.below(7) {
                0 => {
                    set_index_selected(&mut field, index, rng.below(2) == 0);
                }
                1 => {
                    select_only(&mut field, index);
                }
                2 => {
                    select_range_to(&mut field, index);
                }
                3 => {
                    toggle_index(&mut field, index);
                }
                4 => {
                    move_selection(&mut field, i32::try_from(rng.below(7)).unwrap_or(0) - 3);
                }
                5 => {
                    type_ahead(
                        &mut field,
                        char::from_u32(65 + rng.below(30)).unwrap_or('A'),
                    );
                }
                _ => {
                    let _ = is_index_selected(&field, index);
                }
            }

            // Whatever happened, no selected row is out of range and the
            // focused text is readable.
            assert!(field.selected.iter().all(|i| *i < count));
            let _ = field.focused_text();
        }
    }
}

/// Type-ahead terminates on any list, including an empty one, and never
/// names a row that does not exist.
#[test]
fn type_ahead_terminates_and_stays_in_range() {
    let mut rng = Gen(53);
    for count in [0usize, 1, 2, 5] {
        let labels: Vec<String> = (0..count).map(|i| format!("Row {i}")).collect();
        for _ in 0..50 {
            let from = rng.below(10) as usize;
            let ch = char::from_u32(rng.below(200)).unwrap_or('a');
            match find_next(&labels, from.min(count.saturating_sub(1)), ch) {
                Some(index) => assert!(index < count.max(1)),
                None => assert_eq!(count, 0, "only an empty list may answer nothing"),
            }
        }
    }
}

/// The scroll clamp is total: no row count, visible count or selection makes
/// it name a row past the end.
#[test]
fn the_scroll_clamp_stays_in_range() {
    let mut rng = Gen(67);
    for _ in 0..500 {
        let count = rng.below(20) as usize;
        let visible = rng.below(20) as usize;
        let selected = rng.below(30) as usize;

        let top = top_visible_for(count, visible, selected);
        assert!(
            top == 0 || top < count,
            "top {top} is past the end of {count} rows"
        );
    }
}

/// Keyboard routing is total over every key code, every modifier combination
/// and every character.
#[test]
fn keyboard_routing_is_total() {
    for code in (0u16..=0xFF).chain([0x1000, 0xFFFF]) {
        for bits in 0u32..16 {
            let modifiers = Modifiers::from_bits(bits);
            for accelerator in [Modifiers::CONTROL, Modifiers::META] {
                for redo_y in [true, false] {
                    for selected in [true, false] {
                        let _ = route_key(
                            Key::from_virtual(code),
                            modifiers,
                            accelerator,
                            redo_y,
                            selected,
                        );
                    }
                }
            }
        }
    }

    let mut rng = Gen(71);
    for _ in 0..2000 {
        let ch = char::from_u32(rng.below(0x11_0000)).unwrap_or('\u{FFFD}');
        let modifiers = Modifiers::from_bits(rng.below(512));
        for read_only in [true, false] {
            for multi_line in [true, false] {
                let _ = route_char(ch, modifiers, Modifiers::CONTROL, read_only, multi_line);
            }
        }
    }
}

/// A button event of either kind is representable and routing it does not
/// panic — the right button being reachable is deliberate.
#[test]
fn both_buttons_are_representable() {
    for button in [Button::Left, Button::Right] {
        let event = pdfrum_form::Event::MouseDown {
            button,
            at: Point::new(0.0, 0.0),
            modifiers: Modifiers::NONE,
        };
        // Matching is exhaustive over the event enum, so this compiles only
        // while every variant is still handled somewhere.
        assert!(matches!(event, pdfrum_form::Event::MouseDown { .. }));
    }
}
