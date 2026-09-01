//! The editing operations, and the postlude they share.
//!
//! # Text is the source of truth; the layout is derived
//!
//! Every mutation here edits a `String` and then re-lays it out. That is a
//! deliberate simplification of an engine that rearranges only the affected
//! paragraph, and it is safe for the same reason the incremental version is
//! correct: a layout is a pure function of its text, its configuration and
//! its metrics. Doing it wholesale means the two can never disagree — the
//! property that a partial rearrangement has to be careful to preserve, this
//! design cannot violate.
//!
//! Positions are the bridge. A [`Place`] is meaningless without the layout it
//! indexes, so every operation converts to a **flat character index** first,
//! edits the string there, re-lays out, and converts back. That is also why
//! undo can replay against a layout that intervening edits have reshaped.
//!
//! # One postlude, five callers
//!
//! Every mutation ends the same way: re-lay out, collapse the selection onto
//! the caret, and re-seed the sticky column. The one operation that does not
//! re-seed is a vertical move, because the whole point of the sticky column
//! is to survive a run of them.

use pdfrum_doc::vt::{self, Layout, Metrics};

use super::place::{Place, PlaceExt, Range};
use super::select::Selection;
use super::undo::{UndoItem, UndoStack};

/// A text field's editing state.
///
/// The invariant is one sentence: `caret` and both ends of `selection` are
/// places in `layout`, and `layout` is what laying `text` out again would
/// produce.
#[derive(Debug, Clone)]
pub struct TextEdit {
    /// The text as the user has it, which may differ from the field's stored
    /// value until the edit commits.
    pub text: String,
    /// The laid-out form of that text.
    pub layout: Layout,
    /// Where the caret is.
    pub caret: Place,
    /// The caret's position before the last movement, which a shift-move
    /// anchors on when no anchor exists yet.
    pub previous_caret: Place,
    /// The selection: directional, empty when its ends agree.
    pub selection: Selection,
    /// The column a vertical move aims for, in layout space.
    ///
    /// Re-seeded by every horizontal move and every mutation, and
    /// deliberately **not** by a vertical one — which is what lets a run of
    /// up-arrows through a ragged paragraph keep returning to the column the
    /// caret started in.
    pub sticky_x: f32,
    /// How far the view is scrolled, in layout space.
    pub scroll: (f32, f32),
    /// The undo stack.
    pub undo: UndoStack,
}

impl TextEdit {
    /// An edit control over `text`, laid out with `config`.
    #[must_use]
    pub fn new(text: impl Into<String>, config: &vt::Config, metrics: &Metrics<'_>) -> TextEdit {
        let text = text.into();
        let layout = vt::layout(&text, config, metrics);
        let caret = vt::hit::begin_place(&layout);
        TextEdit {
            text,
            layout,
            caret,
            previous_caret: caret,
            selection: Selection::collapsed_at(caret),
            sticky_x: 0.0,
            scroll: (0.0, 0.0),
            undo: UndoStack::default(),
        }
    }

    /// The caret's flat character index.
    #[must_use]
    pub fn caret_index(&self) -> usize {
        vt::hit::word_index_of_place(&self.layout, self.caret)
    }

    /// The selection's flat character range, ordered.
    #[must_use]
    pub fn selection_indices(&self) -> (usize, usize) {
        let range = self.selection.range();
        (
            vt::hit::word_index_of_place(&self.layout, range.begin()),
            vt::hit::word_index_of_place(&self.layout, range.end()),
        )
    }

    /// The selected text, empty when nothing is selected.
    #[must_use]
    pub fn selected_text(&self) -> String {
        let (from, to) = self.selection_indices();
        slice_chars(&self.text, from, to)
    }

    /// Whether anything is selected.
    #[must_use]
    pub fn has_selection(&self) -> bool {
        !self.selection.is_empty()
    }

    /// How many characters the text holds.
    #[must_use]
    pub fn len_chars(&self) -> usize {
        self.text.chars().count()
    }

    /// Moves the caret to a flat character index, collapsing the selection.
    pub fn set_caret_index(&mut self, index: usize) {
        self.previous_caret = self.caret;
        self.caret = vt::hit::place_of_word_index(&self.layout, index);
        self.selection = Selection::collapsed_at(self.caret);
    }

    /// Moves the caret without touching the selection — what a shift-move
    /// needs, since it must extend from an anchor the caret is leaving.
    pub fn move_caret_keeping_selection(&mut self, index: usize) {
        self.previous_caret = self.caret;
        self.caret = vt::hit::place_of_word_index(&self.layout, index);
    }

    /// Selects everything. Records no undo item — selecting is not an edit.
    pub fn select_all(&mut self) {
        let begin = vt::hit::begin_place(&self.layout);
        let end = vt::hit::end_place(&self.layout);
        self.selection = Selection::new(begin, end);
        self.previous_caret = self.caret;
        self.caret = end;
    }

    /// Drops the selection, leaving a live anchor at the caret.
    pub fn select_none(&mut self) {
        self.selection = Selection::collapsed_at(self.caret);
    }
}

/// How many characters a field will still accept.
///
/// `None` means unlimited. A limit already reached answers zero rather than
/// refusing, because the rule is **truncation, not rejection**: an insert
/// that does not fit is trimmed to what does, and only an insert with no room
/// at all does nothing.
#[must_use]
pub fn room_for(edit: &TextEdit, max_len: Option<u32>, replacing: usize) -> Option<usize> {
    let max = max_len? as usize;
    let after_removal = edit.len_chars().saturating_sub(replacing);
    Some(max.saturating_sub(after_removal))
}

/// Replaces a flat character range with `insert`, recording one undo item.
///
/// The single mutation every other one is written in terms of. `max_len`
/// truncates the insertion — never the field — so inserting a long string
/// into a nearly-full field puts in as much as fits and leaves the rest of
/// the text alone.
pub fn replace_range(
    edit: &mut TextEdit,
    config: &vt::Config,
    metrics: &Metrics<'_>,
    from: usize,
    to: usize,
    insert: &str,
    max_len: Option<u32>,
    record: bool,
) -> bool {
    let (from, to) = (from.min(to), from.max(to));
    let removed = slice_chars(&edit.text, from, to);

    let inserted: String = match room_for(edit, max_len, to.saturating_sub(from)) {
        Some(room) => insert.chars().take(room).collect(),
        None => insert.to_string(),
    };

    if removed.is_empty() && inserted.is_empty() {
        return false;
    }

    let before_selection = edit.selection;
    let old_place = vt::hit::place_of_word_index(&edit.layout, from);

    let mut next = String::with_capacity(edit.text.len());
    next.extend(edit.text.chars().take(from));
    next.push_str(&inserted);
    next.extend(edit.text.chars().skip(to));
    edit.text = next;

    relayout(edit, config, metrics);

    let caret_at = from.saturating_add(inserted.chars().count());
    edit.previous_caret = edit.caret;
    edit.caret = vt::hit::place_of_word_index(&edit.layout, caret_at);
    edit.selection = Selection::collapsed_at(edit.caret);

    if record {
        push_edit(edit, &removed, &inserted, old_place, before_selection);
    }
    settle(edit, config, metrics);
    true
}

/// Records the one item a replacement is worth.
///
/// A pure insertion and a pure deletion each record a single item; a genuine
/// replacement — text removed *and* text put in — records the pair bracketed
/// by boundaries, because undoing it has to do both and a caller must not be
/// able to stop between them.
fn push_edit(
    edit: &mut TextEdit,
    removed: &str,
    inserted: &str,
    old_place: Place,
    before: Selection,
) {
    let new_place = edit.caret;
    match (removed.is_empty(), inserted.is_empty()) {
        (true, false) => edit
            .undo
            .push(insertion_item(old_place, new_place, inserted, before)),
        (false, true) => edit.undo.push(UndoItem::Clear {
            range: Range::new(old_place, new_place),
            text: removed.to_string(),
            before,
        }),
        (false, false) => {
            edit.undo.push(UndoItem::GroupBoundary);
            edit.undo.push(UndoItem::Clear {
                range: Range::new(old_place, old_place),
                text: removed.to_string(),
                before,
            });
            edit.undo
                .push(insertion_item(old_place, new_place, inserted, before));
            edit.undo.push(UndoItem::GroupBoundary);
        }
        (true, true) => {}
    }
}

/// The item a pure insertion records: one character is its own variant, so
/// that typing produces exactly one item per keystroke.
fn insertion_item(old: Place, new: Place, inserted: &str, before: Selection) -> UndoItem {
    let mut chars = inserted.chars();
    match (chars.next(), chars.next()) {
        (Some(ch), None) => UndoItem::InsertWord {
            old,
            new,
            ch,
            before,
        },
        _ => UndoItem::InsertText {
            old,
            new,
            text: inserted.to_string(),
            before,
        },
    }
}

/// Types one character at the caret, replacing any selection.
///
/// Exactly one undo item, whether or not a selection was replaced — which is
/// what makes a run of typing undo one keystroke at a time.
pub fn insert_char(
    edit: &mut TextEdit,
    config: &vt::Config,
    metrics: &Metrics<'_>,
    ch: char,
    max_len: Option<u32>,
) -> bool {
    let (from, to) = if edit.has_selection() {
        edit.selection_indices()
    } else {
        let at = edit.caret_index();
        (at, at)
    };
    let mut buffer = [0u8; 4];
    replace_range(
        edit,
        config,
        metrics,
        from,
        to,
        ch.encode_utf8(&mut buffer),
        max_len,
        true,
    )
}

/// Deletes the selection, or the character **before** the caret.
pub fn backspace(edit: &mut TextEdit, config: &vt::Config, metrics: &Metrics<'_>) -> bool {
    let (from, to) = if edit.has_selection() {
        edit.selection_indices()
    } else {
        let at = edit.caret_index();
        let Some(before) = at.checked_sub(1) else {
            return false;
        };
        (before, at)
    };
    replace_range(edit, config, metrics, from, to, "", None, true)
}

/// Deletes the selection, or the character **after** the caret.
pub fn delete(edit: &mut TextEdit, config: &vt::Config, metrics: &Metrics<'_>) -> bool {
    let (from, to) = if edit.has_selection() {
        edit.selection_indices()
    } else {
        let at = edit.caret_index();
        if at >= edit.len_chars() {
            return false;
        }
        (at, at.saturating_add(1))
    };
    replace_range(edit, config, metrics, from, to, "", None, true)
}

/// Replaces the selection with `text`, leaving the caret after it.
///
/// One undo item however long the text, which is the difference from typing
/// the same characters one at a time.
pub fn replace_selection(
    edit: &mut TextEdit,
    config: &vt::Config,
    metrics: &Metrics<'_>,
    text: &str,
    max_len: Option<u32>,
) -> bool {
    let (from, to) = if edit.has_selection() {
        edit.selection_indices()
    } else {
        let at = edit.caret_index();
        (at, at)
    };
    replace_range(edit, config, metrics, from, to, text, max_len, true)
}

/// Replaces the selection with `text` and **keeps the inserted text
/// selected**.
///
/// The whole difference from [`replace_selection`], and the reason both
/// exist: one leaves a caret, the other leaves a selection over what it just
/// put in.
pub fn replace_and_keep_selection(
    edit: &mut TextEdit,
    config: &vt::Config,
    metrics: &Metrics<'_>,
    text: &str,
    max_len: Option<u32>,
) -> bool {
    let (from, to) = if edit.has_selection() {
        edit.selection_indices()
    } else {
        let at = edit.caret_index();
        (at, at)
    };
    let before = from;
    if !replace_range(edit, config, metrics, from, to, text, max_len, true) {
        return false;
    }
    let after = edit.caret_index();
    let begin = vt::hit::place_of_word_index(&edit.layout, before);
    let end = vt::hit::place_of_word_index(&edit.layout, after);
    edit.selection = Selection::new(begin, end);
    true
}

/// Re-lays the text out and puts the caret back where its index says.
fn relayout(edit: &mut TextEdit, config: &vt::Config, metrics: &Metrics<'_>) {
    edit.layout = vt::layout(&edit.text, config, metrics);
}

/// The shared tail of every mutation: re-seed the column a vertical move
/// aims for.
fn settle(edit: &mut TextEdit, config: &vt::Config, metrics: &Metrics<'_>) {
    edit.sticky_x = caret_x(edit, config, metrics);
}

/// The caret's horizontal position in layout space.
#[must_use]
pub fn caret_x(edit: &TextEdit, config: &vt::Config, metrics: &Metrics<'_>) -> f32 {
    let point = vt::hit::point_at_place(
        &edit.layout,
        config.plate,
        config,
        metrics,
        (0.0, 0.0),
        edit.caret,
    );
    #[allow(clippy::cast_possible_truncation)]
    let x = point.x as f32;
    x
}

/// The characters of `text` from `from` up to `to`, by character index.
fn slice_chars(text: &str, from: usize, to: usize) -> String {
    text.chars()
        .skip(from)
        .take(to.saturating_sub(from))
        .collect()
}

/// Undoes one step, replaying each item's inverse against the live layout.
///
/// Returns whether anything was undone. The selection each item carries from
/// *before* its edit is restored; a redo deliberately restores none.
pub fn undo(edit: &mut TextEdit, config: &vt::Config, metrics: &Metrics<'_>) -> bool {
    let items = edit.undo.undo();
    if items.is_empty() {
        return false;
    }
    let mut restored = None;
    for item in &items {
        restored = item.before().or(restored);
        invert(edit, config, metrics, item);
    }
    if let Some(selection) = restored {
        edit.selection = selection;
        edit.caret = selection.end;
    }
    true
}

/// Redoes one step. Restores text only, leaving the caret collapsed — the
/// asymmetry with [`undo`], and the one four ported assertions observe.
pub fn redo(edit: &mut TextEdit, config: &vt::Config, metrics: &Metrics<'_>) -> bool {
    let items = edit.undo.redo();
    if items.is_empty() {
        return false;
    }
    for item in &items {
        apply(edit, config, metrics, item);
    }
    edit.selection = Selection::collapsed_at(edit.caret);
    true
}

/// Replays one item's inverse. Never records, which is what keeps an undo
/// from pushing the item it is undoing.
fn invert(edit: &mut TextEdit, config: &vt::Config, metrics: &Metrics<'_>, item: &UndoItem) {
    match item {
        UndoItem::InsertWord { old, ch, .. } => {
            let at = vt::hit::word_index_of_place(&edit.layout, *old);
            let mut buffer = [0u8; 4];
            let len = ch.encode_utf8(&mut buffer).chars().count();
            replace_range(edit, config, metrics, at, at + len, "", None, false);
        }
        UndoItem::InsertText { old, text, .. } => {
            let at = vt::hit::word_index_of_place(&edit.layout, *old);
            let len = text.chars().count();
            replace_range(edit, config, metrics, at, at + len, "", None, false);
        }
        UndoItem::InsertReturn { old, .. } => {
            let at = vt::hit::word_index_of_place(&edit.layout, *old);
            replace_range(edit, config, metrics, at, at + 1, "", None, false);
        }
        UndoItem::Clear { range, text, .. } => {
            let at = vt::hit::word_index_of_place(&edit.layout, range.begin());
            replace_range(edit, config, metrics, at, at, text, None, false);
        }
        UndoItem::Backspace { old, ch, .. } | UndoItem::Delete { old, ch, .. } => {
            let at = vt::hit::word_index_of_place(&edit.layout, *old);
            let mut buffer = [0u8; 4];
            replace_range(
                edit,
                config,
                metrics,
                at,
                at,
                ch.encode_utf8(&mut buffer),
                None,
                false,
            );
        }
        UndoItem::GroupBoundary => {}
    }
}

/// Replays one item forwards, for a redo.
fn apply(edit: &mut TextEdit, config: &vt::Config, metrics: &Metrics<'_>, item: &UndoItem) {
    match item {
        UndoItem::InsertWord { old, ch, .. } => {
            let at = vt::hit::word_index_of_place(&edit.layout, *old);
            let mut buffer = [0u8; 4];
            replace_range(
                edit,
                config,
                metrics,
                at,
                at,
                ch.encode_utf8(&mut buffer),
                None,
                false,
            );
        }
        UndoItem::InsertText { old, text, .. } => {
            let at = vt::hit::word_index_of_place(&edit.layout, *old);
            replace_range(edit, config, metrics, at, at, text, None, false);
        }
        UndoItem::InsertReturn { old, .. } => {
            let at = vt::hit::word_index_of_place(&edit.layout, *old);
            replace_range(edit, config, metrics, at, at, "\r", None, false);
        }
        UndoItem::Clear { range, text, .. } => {
            let at = vt::hit::word_index_of_place(&edit.layout, range.begin());
            let len = text.chars().count();
            replace_range(edit, config, metrics, at, at + len, "", None, false);
        }
        UndoItem::Backspace { old, .. } | UndoItem::Delete { old, .. } => {
            let at = vt::hit::word_index_of_place(&edit.layout, *old);
            replace_range(edit, config, metrics, at, at + 1, "", None, false);
        }
        UndoItem::GroupBoundary => {}
    }
}
