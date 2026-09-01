//! The undo stack (SPEC §15.4).
//!
//! # Why this is not the obvious design
//!
//! The tempting model — an item remembers the text before and after, and undo
//! and redo restore their respective snapshots — is wrong here in four
//! separate ways that ported assertions catch:
//!
//! - **Undo restores the selection that was live before the edit; redo does
//!   not.** After an undo the caret is back inside whatever was selected when
//!   the user typed over it; after the matching redo the selection is empty.
//!   A symmetric design cannot express that.
//! - **An item replays the inverse operation against the live layout**, so a
//!   position is recomputed rather than remembered. A stored offset drifts as
//!   soon as an intervening operation rewraps a line.
//! - **Granularity is per keystroke going in, but per call going out**: one
//!   typed character is one item, while a paste, a cut or a delete of a
//!   selection is exactly one item however many characters it moved.
//! - **Grouping is a pair of sentinels, not a begin/end API.** A variable
//!   length run is bracketed by two [`UndoItem::GroupBoundary`] markers, and
//!   undo walks past members until it consumes the matching one.
//!
//! Head eviction is group-atomic: dropping the oldest entry to make room
//! drops a whole group when the oldest entry opens one, so the stack can
//! never hold a boundary whose partner has been evicted. That is why the
//! capacity has a floor of four — the worst-case group is a sentinel, a
//! clear, an insert and a sentinel.

use std::collections::VecDeque;

use super::place::{Place, Range};
use super::select::Selection;

/// One reversible edit, stored as enough to replay its inverse.
///
/// Every variant but the boundary carries `before`: the selection that was
/// live when the edit was made, which undo restores and redo does not.
#[derive(Debug, Clone, PartialEq)]
pub enum UndoItem {
    /// One character was inserted.
    InsertWord {
        /// The caret before the insert.
        old: Place,
        /// The caret after it.
        new: Place,
        /// The character.
        ch: char,
        /// The selection that was live before the edit.
        before: Selection,
    },
    /// A paragraph break was inserted.
    InsertReturn {
        /// The caret before the insert.
        old: Place,
        /// The caret after it.
        new: Place,
        /// The selection that was live before the edit.
        before: Selection,
    },
    /// The character before the caret was deleted.
    Backspace {
        /// The caret before the delete.
        old: Place,
        /// The caret after it.
        new: Place,
        /// The character that was removed.
        ch: char,
        /// Whether what was removed was a paragraph break rather than a
        /// character. Snapshotted at record time rather than re-derived at
        /// replay time, which is the one place two upstream mechanisms are
        /// collapsed into the one that cannot disagree with itself.
        section_break: bool,
        /// The selection that was live before the edit.
        before: Selection,
    },
    /// The character after the caret was deleted.
    Delete {
        /// The caret before the delete.
        old: Place,
        /// The caret after it.
        new: Place,
        /// The character that was removed.
        ch: char,
        /// Whether what was removed was a paragraph break.
        section_break: bool,
        /// The selection that was live before the edit.
        before: Selection,
    },
    /// A range was removed.
    Clear {
        /// What was removed.
        range: Range,
        /// The text that was in it.
        text: String,
        /// The selection that was live before the edit.
        before: Selection,
    },
    /// A run of text was inserted.
    InsertText {
        /// The caret before the insert.
        old: Place,
        /// The caret after it.
        new: Place,
        /// The text.
        text: String,
        /// The selection that was live before the edit.
        before: Selection,
    },
    /// A group boundary. Undo and redo continue past members until the
    /// matching boundary is consumed.
    GroupBoundary,
}

impl UndoItem {
    /// Whether this item is a group boundary rather than an edit.
    #[must_use]
    pub fn is_boundary(&self) -> bool {
        matches!(self, UndoItem::GroupBoundary)
    }

    /// The selection that was live before this edit, if it is an edit.
    #[must_use]
    pub fn before(&self) -> Option<Selection> {
        match self {
            UndoItem::InsertWord { before, .. }
            | UndoItem::InsertReturn { before, .. }
            | UndoItem::Backspace { before, .. }
            | UndoItem::Delete { before, .. }
            | UndoItem::Clear { before, .. }
            | UndoItem::InsertText { before, .. } => Some(*before),
            UndoItem::GroupBoundary => None,
        }
    }
}

/// A bounded, linear undo stack.
///
/// `items[..pos]` are undoable and `items[pos..]` are redoable, so a fresh
/// edit truncating the redo branch is one `truncate`.
#[derive(Debug, Clone)]
pub struct UndoStack {
    items: VecDeque<UndoItem>,
    pos: usize,
    max: usize,
    enabled: bool,
}

impl UndoStack {
    /// The default capacity.
    pub const DEFAULT_MAX: u32 = 10_000;

    /// The smallest capacity that can hold the worst-case group: a boundary,
    /// a clear, an insert and a boundary.
    pub const MIN_MAX: u32 = 4;

    /// An empty, enabled stack holding at most `max` items.
    ///
    /// `max` is clamped **up** to [`UndoStack::MIN_MAX`], because a capacity
    /// below the worst-case group size could only be satisfied by evicting
    /// half a group, and a half-evicted group is not a state this type
    /// admits.
    #[must_use]
    pub fn with_max(max: u32) -> UndoStack {
        UndoStack {
            items: VecDeque::new(),
            pos: 0,
            max: max.max(UndoStack::MIN_MAX) as usize,
            enabled: true,
        }
    }

    /// Whether the stack records anything at all.
    ///
    /// A disabled stack receives no items, boundaries included — the check
    /// happens once, at the single entry point, rather than at each of the
    /// several call sites that would otherwise each have to remember it.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Turns recording on or off. Does not discard what is already recorded.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// The capacity.
    #[must_use]
    pub fn max(&self) -> usize {
        self.max
    }

    /// How many items are stored, undoable and redoable together.
    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether nothing is stored.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Whether there is anything to undo.
    #[must_use]
    pub fn can_undo(&self) -> bool {
        self.pos > 0
    }

    /// Whether there is anything to redo.
    #[must_use]
    pub fn can_redo(&self) -> bool {
        self.pos < self.items.len()
    }

    /// Forgets everything. What a focus change to another field does.
    pub fn clear(&mut self) {
        self.items.clear();
        self.pos = 0;
    }

    /// Records one item, truncating the redo branch and evicting from the
    /// head if the stack is full.
    ///
    /// A disabled stack ignores the call.
    pub fn push(&mut self, item: UndoItem) {
        if !self.enabled {
            return;
        }
        self.items.truncate(self.pos);
        self.evict_to_fit();
        self.items.push_back(item);
        self.pos = self.items.len();
    }

    /// Drops whole groups from the head until one more item fits.
    ///
    /// Dropping a lone item is one `pop_front`; dropping a group's opening
    /// boundary takes everything up to and including its partner, so the
    /// stack never holds an unmatched boundary.
    fn evict_to_fit(&mut self) {
        while self.items.len() >= self.max {
            let opened_group = matches!(self.items.front(), Some(UndoItem::GroupBoundary));
            self.items.pop_front();
            if opened_group {
                while let Some(item) = self.items.pop_front() {
                    if item.is_boundary() {
                        break;
                    }
                }
            }
            if self.items.is_empty() {
                break;
            }
        }
        self.pos = self.items.len();
    }

    /// The items one `undo` would replay, oldest first, and moves the cursor
    /// past them.
    ///
    /// Exactly one item when the top of the stack is an ordinary edit. When
    /// it is a group's closing boundary, the whole group: the walk continues
    /// until it has consumed the opening boundary.
    ///
    /// Returns an empty slice when there is nothing to undo, which is the
    /// same answer as "the cursor is at the bottom".
    pub fn undo(&mut self) -> Vec<UndoItem> {
        let mut taken = Vec::new();
        let mut first = true;
        while self.pos > 0 {
            self.pos -= 1;
            let Some(item) = self.items.get(self.pos) else {
                break;
            };
            let is_boundary = item.is_boundary();
            taken.push(item.clone());
            if first {
                first = false;
                if !is_boundary {
                    break;
                }
            } else if is_boundary {
                break;
            }
        }
        taken
    }

    /// The items one `redo` would replay, oldest first, and moves the cursor
    /// past them. The mirror of [`UndoStack::undo`].
    pub fn redo(&mut self) -> Vec<UndoItem> {
        let mut taken = Vec::new();
        let mut first = true;
        while self.pos < self.items.len() {
            let Some(item) = self.items.get(self.pos) else {
                break;
            };
            let is_boundary = item.is_boundary();
            taken.push(item.clone());
            self.pos += 1;
            if first {
                first = false;
                if !is_boundary {
                    break;
                }
            } else if is_boundary {
                break;
            }
        }
        taken
    }

    /// The stored items, oldest first. For tests and diagnostics.
    pub fn items(&self) -> impl Iterator<Item = &UndoItem> {
        self.items.iter()
    }

    /// How many items are undoable — the cursor's position.
    #[must_use]
    pub fn position(&self) -> usize {
        self.pos
    }
}

impl Default for UndoStack {
    fn default() -> UndoStack {
        UndoStack::with_max(UndoStack::DEFAULT_MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(ch: char) -> UndoItem {
        UndoItem::InsertWord {
            old: Place::START,
            new: Place::START,
            ch,
            before: Selection::EMPTY,
        }
    }

    fn chars_of(items: &[UndoItem]) -> Vec<char> {
        items
            .iter()
            .filter_map(|i| match i {
                UndoItem::InsertWord { ch, .. } => Some(*ch),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_fresh_stack_can_neither_undo_nor_redo() {
        let stack = UndoStack::default();
        assert!(!stack.can_undo());
        assert!(!stack.can_redo());
    }

    /// Typing n characters pushes exactly n items, and each undo takes one.
    #[test]
    fn one_typed_character_is_one_item() {
        let mut stack = UndoStack::default();
        for ch in "ABCDE".chars() {
            stack.push(word(ch));
        }
        assert_eq!(stack.len(), 5);
        assert_eq!(chars_of(&stack.undo()), vec!['E']);
        assert_eq!(chars_of(&stack.undo()), vec!['D']);
        assert!(stack.can_undo());
        assert!(stack.can_redo());
        assert_eq!(chars_of(&stack.redo()), vec!['D']);
        assert_eq!(chars_of(&stack.redo()), vec!['E']);
        assert!(!stack.can_redo());
        assert!(stack.can_undo());
    }

    /// The stack bottom is observable: after undoing every item, `can_undo`
    /// is false rather than merely unhelpful.
    #[test]
    fn undoing_to_the_bottom_reports_the_bottom() {
        let mut stack = UndoStack::default();
        for ch in "ABC".chars() {
            stack.push(word(ch));
        }
        for _ in 0..3 {
            assert!(stack.can_undo());
            stack.undo();
        }
        assert!(!stack.can_undo());
        assert!(stack.undo().is_empty());
    }

    /// A bracketed run is undone as a unit however many members it has.
    #[test]
    fn a_group_undoes_and_redoes_as_one_step() {
        let mut stack = UndoStack::default();
        stack.push(word('A'));
        stack.push(UndoItem::GroupBoundary);
        stack.push(word('X'));
        stack.push(word('Y'));
        stack.push(word('Z'));
        stack.push(UndoItem::GroupBoundary);

        let undone = stack.undo();
        assert_eq!(chars_of(&undone), vec!['Z', 'Y', 'X']);
        assert!(stack.can_undo());
        assert_eq!(chars_of(&stack.undo()), vec!['A']);
        assert!(!stack.can_undo());

        assert_eq!(chars_of(&stack.redo()), vec!['A']);
        assert_eq!(chars_of(&stack.redo()), vec!['X', 'Y', 'Z']);
        assert!(!stack.can_redo());
    }

    /// A new edit after an undo truncates the redo branch.
    #[test]
    fn a_fresh_edit_drops_the_redo_branch() {
        let mut stack = UndoStack::default();
        stack.push(word('A'));
        stack.push(word('B'));
        stack.undo();
        assert!(stack.can_redo());

        stack.push(word('C'));
        assert!(stack.can_undo());
        assert!(!stack.can_redo());
        assert_eq!(chars_of(&stack.undo()), vec!['C']);
    }

    /// The capacity floor exists so the worst-case group always fits.
    #[test]
    fn capacity_is_clamped_up_to_the_worst_case_group() {
        assert_eq!(UndoStack::with_max(0).max(), 4);
        assert_eq!(UndoStack::with_max(1).max(), 4);
        assert_eq!(UndoStack::with_max(4).max(), 4);
        assert_eq!(UndoStack::with_max(9).max(), 9);
    }

    /// Eviction takes whole groups, so no unmatched boundary can survive.
    #[test]
    fn eviction_never_leaves_half_a_group() {
        let mut stack = UndoStack::with_max(4);
        stack.push(UndoItem::GroupBoundary);
        stack.push(word('X'));
        stack.push(word('Y'));
        stack.push(UndoItem::GroupBoundary);
        assert_eq!(stack.len(), 4);

        // One more item does not fit; the whole group leaves together.
        stack.push(word('Z'));
        let boundaries = stack.items().filter(|i| i.is_boundary()).count();
        assert_eq!(boundaries % 2, 0, "an unmatched boundary survived");
        assert_eq!(
            chars_of(&stack.items().cloned().collect::<Vec<_>>()),
            vec!['Z']
        );
    }

    /// However the stack is filled, it never holds an unmatched boundary and
    /// never exceeds its capacity.
    #[test]
    fn eviction_holds_the_invariants_over_many_shapes() {
        for max in [4u32, 5, 7] {
            for seed in 0..64u32 {
                let mut stack = UndoStack::with_max(max);
                let mut bits = seed;
                for n in 0..20u32 {
                    if bits & 1 == 1 {
                        stack.push(UndoItem::GroupBoundary);
                        stack.push(word('a'));
                        stack.push(UndoItem::GroupBoundary);
                    } else {
                        stack.push(word(char::from_u32('a' as u32 + n % 26).unwrap_or('a')));
                    }
                    bits >>= 1;
                    if bits == 0 {
                        bits = seed | 1;
                    }

                    assert!(stack.len() <= max as usize, "capacity exceeded");
                    let boundaries = stack.items().filter(|i| i.is_boundary()).count();
                    assert_eq!(
                        boundaries % 2,
                        0,
                        "unmatched boundary at max={max} seed={seed}"
                    );
                }
            }
        }
    }

    /// A disabled stack takes nothing, boundaries included.
    #[test]
    fn a_disabled_stack_records_nothing() {
        let mut stack = UndoStack::default();
        stack.set_enabled(false);
        stack.push(word('A'));
        stack.push(UndoItem::GroupBoundary);
        assert!(stack.is_empty());
        assert!(!stack.can_undo());
    }

    /// A focus change to another field empties the stack.
    #[test]
    fn clear_forgets_both_branches() {
        let mut stack = UndoStack::default();
        stack.push(word('A'));
        stack.push(word('B'));
        stack.undo();
        stack.clear();
        assert!(!stack.can_undo());
        assert!(!stack.can_redo());
        assert!(stack.is_empty());
    }
}
