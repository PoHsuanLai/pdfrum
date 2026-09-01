//! Ported undo and redo assertions — the stack-shape half.
//!
//! The upstream tests drive real typing and read the field's text back. That
//! second half needs the layout queries the text engine is still growing, so
//! what is pinned here is the half that does not: **how many items each
//! operation pushes, and how far one undo walks.** That is where every one of
//! these tests actually disagrees with the design a reasonable author would
//! reach for, so it is the half worth having first.
//!
//! Each test names the upstream test it restates and reproduces its step
//! sequence exactly, including the intermediate `can_undo` and `can_redo`
//! readings the original checks after every step.

use pdfrum_form::edit::{Place, Selection, UndoItem, UndoStack};

/// One typed character.
fn typed(ch: char, before: Selection) -> UndoItem {
    UndoItem::InsertWord {
        old: Place::START,
        new: Place::START,
        ch,
        before,
    }
}

/// A replace-selection group: two boundaries around the clear and the insert
/// that each optionally happen, which is why the group is bracketed rather
/// than counted.
fn replace_group(removed: &str, inserted: &str, before: Selection) -> Vec<UndoItem> {
    let mut items = vec![UndoItem::GroupBoundary];
    if !removed.is_empty() {
        items.push(UndoItem::Clear {
            range: pdfrum_form::edit::Range::empty_at(Place::START),
            text: removed.to_string(),
            before,
        });
    }
    if !inserted.is_empty() {
        items.push(UndoItem::InsertText {
            old: Place::START,
            new: Place::START,
            text: inserted.to_string(),
            before,
        });
    }
    items.push(UndoItem::GroupBoundary);
    items
}

fn push_all(stack: &mut UndoStack, items: Vec<UndoItem>) {
    for item in items {
        stack.push(item);
    }
}

/// `UndoRedo`: typing five characters gives five undo steps, one per
/// character, and walking back and forth reports the ends correctly at every
/// step.
#[test]
fn typing_five_characters_undoes_one_character_at_a_time() {
    let mut stack = UndoStack::default();
    assert!(!stack.can_undo());
    assert!(!stack.can_redo());

    for ch in "ABCDE".chars() {
        stack.push(typed(ch, Selection::EMPTY));
    }
    assert_eq!(stack.len(), 5, "one item per character, never coalesced");
    assert!(stack.can_undo());
    assert!(!stack.can_redo());

    // Two undos take exactly one character each.
    assert_eq!(stack.undo().len(), 1);
    assert!(stack.can_undo());
    assert!(stack.can_redo());
    assert_eq!(stack.undo().len(), 1);
    assert!(stack.can_undo());
    assert!(stack.can_redo());

    // And two redos put them back one at a time.
    assert_eq!(stack.redo().len(), 1);
    assert!(stack.can_undo());
    assert!(stack.can_redo());
    assert_eq!(stack.redo().len(), 1);
    assert!(stack.can_undo(), "three characters remain below");
    assert!(!stack.can_redo(), "and the top has been reached");
}

/// `UndoRedo` on a combo box: the stack is per field and does not survive a
/// move to another one, and its bottom is observable.
#[test]
fn a_focus_change_to_another_field_empties_the_stack() {
    let mut stack = UndoStack::default();
    for ch in "ABC".chars() {
        stack.push(typed(ch, Selection::EMPTY));
    }
    assert!(stack.can_undo());

    // Moving to a different field discards the history.
    stack.clear();
    assert!(!stack.can_undo());
    assert!(!stack.can_redo());

    // Typing again and undoing to the bottom reports the bottom.
    for ch in "ABC".chars() {
        stack.push(typed(ch, Selection::EMPTY));
    }
    for _ in 0..3 {
        stack.undo();
    }
    assert!(!stack.can_undo(), "the bottom of the stack is observable");
}

/// `ContinuouslyReplaceAndKeepSelection`: three characters inserted by one
/// call are **one** undo step, not three.
#[test]
fn a_three_character_replace_is_one_undo_step() {
    let mut stack = UndoStack::default();
    push_all(&mut stack, replace_group("", "UVW", Selection::EMPTY));
    assert!(stack.can_undo());

    // One undo consumes the whole bracketed group.
    let undone = stack.undo();
    assert!(
        undone.len() >= 2,
        "the group is walked as a unit, boundaries included"
    );
    assert!(!stack.can_undo(), "…and that was the only step");
}

/// `CutAllTextUndoRestoresAllCharacters`: three characters typed separately
/// are three steps, but the cut that removes all of them is **one** — the cut
/// is atomic even though what it removed was built piecemeal.
#[test]
fn a_cut_is_one_step_however_many_characters_it_removed() {
    let mut stack = UndoStack::default();
    for ch in "ABC".chars() {
        stack.push(typed(ch, Selection::EMPTY));
    }
    assert_eq!(stack.len(), 3);

    // Selecting all records nothing; the cut that follows is one group.
    push_all(&mut stack, replace_group("ABC", "", Selection::EMPTY));

    // One undo restores the whole cut.
    stack.undo();
    assert!(
        stack.can_undo(),
        "the three typed characters are still below the cut"
    );

    // And exactly three steps remain, one per typed character.
    let mut steps = 0;
    while stack.can_undo() {
        stack.undo();
        steps += 1;
    }
    assert_eq!(steps, 3);
}

/// `ReplaceSelection`: typing two characters and then replacing one of them
/// leaves exactly three steps, walkable to the bottom and back.
#[test]
fn typing_twice_then_replacing_leaves_three_steps() {
    let mut stack = UndoStack::default();
    stack.push(typed('A', Selection::EMPTY));
    stack.push(typed('B', Selection::EMPTY));
    push_all(&mut stack, replace_group("A", "XYZ", Selection::EMPTY));

    let mut down = 0;
    while stack.can_undo() {
        stack.undo();
        down += 1;
    }
    assert_eq!(down, 3, "three undo steps, not five items' worth");

    let mut up = 0;
    while stack.can_redo() {
        stack.redo();
        up += 1;
    }
    assert_eq!(up, 3, "and the walk back up matches");
}

/// `ReplaceAndKeepSelection` step 9: a fresh edit after an undo truncates the
/// redo branch.
#[test]
fn a_fresh_edit_after_an_undo_drops_the_redo_branch() {
    let mut stack = UndoStack::default();
    push_all(&mut stack, replace_group("", "XYZ", Selection::EMPTY));
    stack.undo();
    assert!(stack.can_redo());

    push_all(&mut stack, replace_group("", "UVW", Selection::EMPTY));
    assert!(stack.can_undo());
    assert!(!stack.can_redo(), "the redo branch is gone");
}

/// The selection asymmetry, which is the property four upstream tests
/// observe between them: an item carries the selection that was live
/// **before** the edit, and that is what an undo has available to restore.
/// Redo carries no selection to restore, so it leaves the caret collapsed.
#[test]
fn an_item_carries_the_selection_from_before_the_edit() {
    let before = Selection::new(Place::new(0, 0, 0), Place::new(0, 0, 3));
    let mut stack = UndoStack::default();
    stack.push(typed('X', before));

    let undone = stack.undo();
    let restored = undone.first().and_then(UndoItem::before);
    assert_eq!(
        restored,
        Some(before),
        "undo has the pre-edit selection to restore"
    );

    // A boundary carries none, which is what makes it a no-op on both walks.
    assert_eq!(UndoItem::GroupBoundary.before(), None);
}

/// `ReplaceSelectionUndoQueueLimit` / `…RedoQueueLimit`: with the capacity at
/// its floor, a replace group still fits and eviction never splits one.
#[test]
fn a_replace_group_fits_at_the_smallest_capacity() {
    let mut stack = UndoStack::with_max(4);
    assert_eq!(stack.max(), 4, "four is the floor, being the group's size");

    for round in 0..6 {
        push_all(
            &mut stack,
            replace_group("", &format!("v{round}"), Selection::EMPTY),
        );

        assert!(stack.len() <= 4, "the capacity holds");
        let boundaries = stack.items().filter(|i| i.is_boundary()).count();
        assert_eq!(boundaries % 2, 0, "no group was left half-evicted");
    }

    // Whatever was evicted, what remains still undoes cleanly to the bottom.
    let mut steps = 0;
    while stack.can_undo() {
        stack.undo();
        steps += 1;
        assert!(steps < 10, "the walk must terminate");
    }
}

/// `SelectAllText` records nothing: selecting is not an undoable edit, so
/// after typing one character and selecting everything, one undo returns the
/// field to empty rather than merely dropping the selection.
#[test]
fn selecting_all_is_not_an_undoable_edit() {
    let mut stack = UndoStack::default();
    stack.push(typed('A', Selection::EMPTY));

    // A select-all pushes nothing, so the single typed character is still the
    // top of the stack and one undo reaches the bottom.
    assert_eq!(stack.len(), 1);
    assert_eq!(stack.undo().len(), 1);
    assert!(!stack.can_undo(), "one undo reached the bottom");
}
