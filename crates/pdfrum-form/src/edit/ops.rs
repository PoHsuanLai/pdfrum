//! The editing operations, and the postlude they share.
//!
//! **Text is the source of truth; the layout is derived.** Every mutation
//! edits a `String` and re-lays it out wholesale, so the two cannot disagree.
//!
//! A [`Place`] is meaningless without the layout it indexes, so every
//! operation converts to a **flat character index** first, edits there and
//! converts back — which is why undo can replay against a layout that
//! intervening edits have reshaped.
//!
//! Every mutation ends the same way: re-lay out, collapse the selection onto
//! the caret, re-seed the sticky column — except a vertical move.

use pdfrum_doc::ap::field_body::Highlight;
use pdfrum_doc::vt::{self, Layout, Metrics};

use super::place::{Place, Range};
use super::select::Selection;
use super::undo::{UndoItem, UndoStack};

/// A text field's editing state.
///
/// The invariant is one sentence: `caret` and both ends of `selection` are
/// places in `layout`, and `layout` is what laying `text` out again would
/// produce.
#[derive(Debug, Clone, PartialEq)]
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
    ///
    /// A **distance**, not a position: `LiveState::shift` negates it to move
    /// the drawn text. Upstream's `scroll_pos_point_` is the same quantity
    /// seeded at `rcPlate.left`, so ours is upstream's minus that — see
    /// [`scroll_to_caret`].
    pub scroll: (f32, f32),
    /// Whether the view follows the caret out of the plate.
    ///
    /// True for any text field **without** the `DoNotScroll` flag —
    /// single-line and multi-line alike. It gates every writer of the scroll
    /// position, so a field that declines it never moves its view at all,
    /// however far past the plate the caret goes.
    pub auto_scroll: bool,
    /// The vertical alignment offset the text is *drawn* with.
    ///
    /// A single-line field is drawn vertically centred in its plate, so its
    /// glyphs sit below where the layout placed them. Every geometric query —
    /// turning a click into a place, a place into a caret rectangle — has to
    /// be told about that shift or it works against the wrong box: a click at
    /// a real field's mid-height falls *below* the content and clamps to the
    /// end of the text, silently, putting the caret at the end of the field
    /// instead of where it was clicked.
    ///
    /// It is recomputed on every relayout because it depends on the content's
    /// height, which an edit changes.
    pub offset: (f32, f32),
    /// Whether the field draws its text vertically centred.
    pub centred: bool,
    /// The undo stack.
    pub undo: UndoStack,
}

/// The text as the layout will hold it, given whether the field is
/// multiline.
///
/// A **single-line** field has no way to represent a line break, so every one
/// is **removed** — not replaced with a space, and not treated as a
/// terminator that truncates the rest. `"Foo\nBar"` is `"FooBar"`, and
/// `"Foo\n"` is `"Foo"`. A `\r\n` or `\n\r` pair is one break rather than
/// two, which is why the pairs are consumed together.
///
/// A multiline field keeps its breaks, normalized to `'\n'` so that the four
/// spellings of one break compare equal.
///
/// This is not a convenience: the layout engine already applies exactly this
/// rule when it lays the text out, so a control that stored the raw string
/// would disagree with its own layout about how many characters it has.
#[must_use]
pub fn normalize_breaks(text: &str, multi_line: bool) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut index = 0;
    while index < chars.len() {
        let ch = chars.get(index).copied().unwrap_or('\0');
        match ch {
            '\r' | '\n' => {
                // The partner of a pair is consumed with it, so a `\r\n` is
                // one break and not two empty lines.
                let partner = if ch == '\r' { '\n' } else { '\r' };
                if chars.get(index + 1) == Some(&partner) {
                    index += 1;
                }
                if multi_line {
                    out.push('\n');
                }
            }
            // A tab is set as a space, which the layout also does.
            '\t' => out.push(' '),
            _ => out.push(ch),
        }
        index += 1;
    }
    out
}

/// The offset a field's text is drawn with.
///
/// Top alignment shifts nothing; centred alignment shifts by half the slack
/// between the content and the plate. The same rule the appearance path uses,
/// because the two must agree — a caret computed against a different offset
/// from the one the glyphs were drawn with lands in the wrong place.
#[must_use]
pub fn vertical_offset(centred: bool, config: &vt::Config, layout: &Layout) -> (f32, f32) {
    if !centred {
        return (0.0, 0.0);
    }
    let content = layout.content_rect_pdf(config.plate);
    (
        0.0,
        (pdfrum_doc::geom::height(content) - pdfrum_doc::geom::height(config.plate)) * 0.5,
    )
}

impl TextEdit {
    /// An edit control over `text`, laid out with `config`.
    ///
    /// `centred` says whether the field draws its text vertically centred,
    /// which a single-line field does and a multiline one does not.
    ///
    /// The text is **normalized to what the layout will hold** before it is
    /// stored — see [`normalize_breaks`]. Storing the caller's string
    /// unchanged would break the invariant this type exists to keep: a
    /// single-line field's layout silently drops line breaks, so a raw
    /// `"Foo\nBar"` would report seven characters while the layout held six,
    /// and every index derived from one would miss in the other.
    #[must_use]
    pub fn new(
        text: impl Into<String>,
        config: &vt::Config,
        metrics: &Metrics<'_>,
        centred: bool,
    ) -> TextEdit {
        let text = normalize_breaks(&text.into(), config.multi_line);
        let layout = vt::layout(&text, config, metrics);
        let caret = vt::hit::begin_place(&layout);
        let offset = vertical_offset(centred, config, &layout);
        TextEdit {
            text,
            layout,
            caret,
            previous_caret: caret,
            selection: Selection::collapsed_at(caret),
            sticky_x: 0.0,
            scroll: (0.0, 0.0),
            // The permissive default, because it is what every caller in this
            // crate wants: the one field flag that clears it is read where the
            // field's config is, and `route.rs` sets it from there.
            auto_scroll: true,
            centred,
            offset,
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

    /// Selects a **signed** character range, the way an embedder asks for one.
    ///
    /// The signs are not an accident of the C API and are not clamping: they
    /// are three distinct instructions sharing one signature, and the order
    /// they are tested in is what makes them unambiguous.
    ///
    /// - `(0, negative)` selects **everything**. This is the documented
    ///   spelling of "to the end", and it is tested first, so it wins over
    ///   the rule below even though its end is also negative.
    /// - `(negative, anything)` selects **nothing**. A negative *start* is
    ///   not clamped to zero — it clears the selection outright, so
    ///   `(-8, -1)` is empty rather than the whole field.
    /// - otherwise the two are ordered and used as they are, so `(23, 12)`
    ///   and `(12, 23)` select the same run. An end past the text clamps to
    ///   its end, which is ordinary index saturation rather than a fourth
    ///   rule.
    pub fn set_selection(&mut self, start: i32, end: i32) {
        if start == 0 && end < 0 {
            self.select_all();
            return;
        }
        if start < 0 {
            self.select_none();
            return;
        }
        let len = self.len_chars();
        let clamp = |index: i32| usize::try_from(index).unwrap_or(0).min(len);
        let (from, to) = if start < end {
            (clamp(start), clamp(end))
        } else {
            (clamp(end), clamp(start))
        };
        let begin = vt::hit::place_of_word_index(&self.layout, from);
        let finish = vt::hit::place_of_word_index(&self.layout, to);
        self.selection = Selection::new(begin, finish);
        self.previous_caret = self.caret;
        self.caret = finish;
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

/// Whether an insertion that first removes `[from, to)` is refused because
/// the field is full.
///
/// The removal happens **first**, and the overflow test is asked of the text
/// with the selection already gone. Typing over a full field's entire
/// contents therefore works, which is the behaviour a user relies on to
/// correct an overfull field; asking before the removal would break it.
///
/// The removal is measured rather than performed: a trial layout of the text
/// as it would stand answers the same question without an undo item or a
/// caret move to roll back.
fn insertion_is_refused(
    edit: &TextEdit,
    config: &vt::Config,
    metrics: &Metrics<'_>,
    from: usize,
    to: usize,
) -> bool {
    if edit.auto_scroll || config.char_array > 0 {
        return false;
    }
    if from == to {
        return is_text_overflow(edit, config);
    }
    let mut trial = String::with_capacity(edit.text.len());
    trial.extend(edit.text.chars().take(from));
    trial.extend(edit.text.chars().skip(to));
    let layout = vt::layout(
        &normalize_breaks(&trial, config.multi_line),
        config,
        metrics,
    );
    layout_overflows(&layout, config)
}

/// Whether the plate is already full, so that no further text is accepted.
///
/// True when the field can neither scroll nor overflow **and** its content is
/// bigger than its plate; every insertion returns without mutating when it
/// is. ISO 32000-1 Table 228 says the same about `DoNotScroll`: once the field
/// is full, no further text is accepted. Without this gate a `DoNotScroll`
/// field keeps taking characters, the caret walks off the plate, and the extra
/// text sits invisibly in the value.
///
/// **The check is made *before* the character**, so the one that first makes
/// the content exceed the plate is accepted and the **next** is refused.
///
/// Overflow is a **comb field's** property and only a comb field's, so a comb
/// never refuses — which is why the `char_array` test comes first here.
#[must_use]
pub fn is_text_overflow(edit: &TextEdit, config: &vt::Config) -> bool {
    if edit.auto_scroll || config.char_array > 0 {
        return false;
    }
    layout_overflows(&edit.layout, config)
}

/// The two size comparisons `IsTextOverflow` makes once its two flags have
/// let it through, against a layout that need not be the control's own.
///
/// Split out so [`insertion_is_refused`] can ask the question of the text as
/// it *would* stand after a selection is removed, which is the order upstream
/// runs the two operations in.
fn layout_overflows(layout: &vt::Layout, config: &vt::Config) -> bool {
    let plate = config.plate;
    let content = layout.content_rect_pdf(plate);
    // The multi-line branch needs more than one line before a taller content
    // counts: a single line taller than its plate is the ordinary case for a
    // field whose font does not quite fit, and upstream declines to lock
    // those out.
    let lines: usize = layout
        .sections
        .iter()
        .map(|section| section.lines.len())
        .sum();
    if config.multi_line
        && lines > 1
        && is_float_bigger(
            pdfrum_doc::geom::height(content),
            pdfrum_doc::geom::height(plate),
        )
    {
        return true;
    }
    is_float_bigger(
        pdfrum_doc::geom::width(content),
        pdfrum_doc::geom::width(plate),
    )
}

/// Replaces a flat character range with `insert`, recording one undo item.
///
/// The single mutation every other one is written in terms of. `max_len`
/// truncates the insertion — never the field — so inserting a long string
/// into a nearly-full field puts in as much as fits and leaves the rest of
/// the text alone.
#[allow(clippy::too_many_arguments)] // The one primitive; every other
// mutation is a short call into it, so the arguments live here rather than
// being spread across five near-identical bodies.
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
///
/// A field that has filled its plate and may not scroll refuses outright —
/// see [`is_text_overflow`], which is `InsertWord`'s and `InsertReturn`'s
/// first statement upstream.
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
    if insertion_is_refused(edit, config, metrics, from, to) {
        return false;
    }
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
///
/// A `DoNotScroll` field already at its plate's edge refuses the insertion
/// half — see [`is_text_overflow`] — but the *removal* half still runs
/// upstream, because `ReplaceSelection` clears before it inserts. Deleting a
/// selection therefore always works, however full the field is.
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
    let text = if insertion_is_refused(edit, config, metrics, from, to) {
        ""
    } else {
        text
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
    let text = if insertion_is_refused(edit, config, metrics, from, to) {
        ""
    } else {
        text
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
    // The offset depends on the content's height, which the edit just moved.
    edit.offset = vertical_offset(edit.centred, config, &edit.layout);
}

/// The shared tail of every mutation: re-seed the column a vertical move
/// aims for, then bring the caret back into view.
fn settle(edit: &mut TextEdit, config: &vt::Config, metrics: &Metrics<'_>) {
    edit.sticky_x = caret_x(edit, config, metrics);
    scroll_to_caret(edit, config, metrics);
}

/// Scrolls a multiline field's view vertically by a wheel notch, answering
/// whether it moved.
///
/// **A `DoNotScroll` field does not move.** [`TextEdit::auto_scroll`] gates
/// every writer of the vertical scroll position, the wheel included, and such
/// a field is given no scrollbar to drag either — it simply does not pan.
///
/// The step is a quarter of the plate per notch, and the position is clamped
/// to the slack between content and plate, so a field with nothing to scroll
/// answers `false` rather than accumulating an offset it cannot use.
pub fn scroll_by(edit: &mut TextEdit, config: &vt::Config, delta_y: i32) -> bool {
    if !edit.auto_scroll || delta_y == 0 {
        return false;
    }
    let content = edit.layout.content_rect_pdf(config.plate);
    let slack = pdfrum_doc::geom::height(content) - pdfrum_doc::geom::height(config.plate);
    if slack <= 0.0 {
        return false;
    }
    let step = pdfrum_doc::geom::height(config.plate) * 0.25;
    let was = edit.scroll.1;
    #[expect(
        clippy::cast_precision_loss,
        reason = "a wheel delta is a small notch count"
    )]
    let by = -(delta_y as f32) * step;
    edit.scroll.1 = (edit.scroll.1 + by).clamp(0.0, slack);
    (edit.scroll.1 - was).abs() > f32::EPSILON
}

/// Scrolls the view so the caret is inside the plate.
///
/// Run after every mutation and every caret move; without it a field whose
/// text outruns its plate keeps drawing from the first character and hides
/// the caret entirely.
///
/// The three comparisons carry a `0.0001` tolerance rather than a raw `<`: a
/// caret landing exactly on the plate edge must count as *inside*, or a field
/// scrolls by a whole advance on a rounding error. This is the opposite of
/// the hit test's tie-break, which is raw.
///
/// Only the **horizontal** half moves the view here; the wheel
/// ([`scroll_by`]) is the only thing that moves the vertical offset.
pub fn scroll_to_caret(edit: &mut TextEdit, config: &vt::Config, metrics: &Metrics<'_>) {
    if !edit.auto_scroll {
        return;
    }
    let plate = config.plate;
    let (left, width) = (
        pdfrum_doc::geom::left(plate),
        pdfrum_doc::geom::width(plate),
    );
    if is_float_equal(left, pdfrum_doc::geom::right(plate)) {
        return;
    }
    // `SetScrollLimit` (`cpwl_edit_impl.cpp:1207-1234`) runs first, and a
    // plate wider than its content pins the view at the origin — which is
    // what keeps a short value left-aligned rather than drifting.
    let content = edit.layout.content_rect_pdf(plate);
    let (content_left, content_right) = (
        pdfrum_doc::geom::left(content),
        pdfrum_doc::geom::right(content),
    );
    if width > pdfrum_doc::geom::width(content) {
        edit.scroll.0 = 0.0;
    } else {
        // Not `f32::clamp`: `SetScrollLimit` (`cpwl_edit_impl.cpp:1215-1220`)
        // tests each bound with `FXSYS_IsFloatSmaller`/`IsFloatBigger`, so a
        // value within 0.0001 of one is left where it is rather than being
        // snapped onto it. Raw `<=`/`>=` would pull a rounding-edge caret a
        // hair further than upstream does.
        let (low, high) = (content_left - left, content_right - left - width);
        if is_float_smaller(edit.scroll.0, low) {
            edit.scroll.0 = low;
        } else if is_float_bigger(edit.scroll.0, high) {
            edit.scroll.0 = high;
        }
    }

    let head = caret_x(edit, config, metrics);
    let head_edit = head - edit.scroll.0;
    if is_float_smaller(head_edit, left) || is_float_equal(head_edit, left) {
        edit.scroll.0 = head - left;
    } else if is_float_smaller(left + width, head_edit) {
        edit.scroll.0 = head - left - width;
    }
}

/// Equal within the oracle's `0.0001` float tolerance.
fn is_float_equal(a: f32, b: f32) -> bool {
    (a - b).abs() < 0.0001
}

/// Strictly smaller by more than the oracle's `0.0001` float tolerance.
fn is_float_smaller(a: f32, b: f32) -> bool {
    a < b && !is_float_equal(a, b)
}

/// Strictly bigger by more than the oracle's `0.0001` float tolerance.
fn is_float_bigger(a: f32, b: f32) -> bool {
    a > b && !is_float_equal(a, b)
}

/// The caret's horizontal position in layout space.
#[must_use]
pub fn caret_x(edit: &TextEdit, config: &vt::Config, metrics: &Metrics<'_>) -> f32 {
    let point = vt::hit::point_at_place(
        &edit.layout,
        config.plate,
        config,
        metrics,
        edit.offset,
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

/// The place a click at `point` selects.
///
/// `point` is in PDF user space. The field's own drawing offset is applied,
/// which is what makes a click at a field's visible mid-height land on the
/// character under the pointer rather than clamping to the end of the text.
#[must_use]
pub fn place_at_point(
    edit: &TextEdit,
    config: &vt::Config,
    metrics: &Metrics<'_>,
    point: kurbo::Point,
) -> Place {
    vt::hit::place_at_point(
        &edit.layout,
        config.plate,
        config,
        metrics,
        edit.offset,
        point,
    )
}

/// Moves the caret to a click, collapsing the selection and dropping a fresh
/// anchor there.
pub fn click_at(
    edit: &mut TextEdit,
    config: &vt::Config,
    metrics: &Metrics<'_>,
    point: kurbo::Point,
) {
    let place = place_at_point(edit, config, metrics, point);
    edit.previous_caret = edit.caret;
    edit.caret = place;
    edit.selection = Selection::collapsed_at(place);
    edit.sticky_x = caret_x(edit, config, metrics);
    scroll_to_caret(edit, config, metrics);
}

/// Extends the selection to a point, keeping the anchor — a mouse drag.
pub fn drag_to(
    edit: &mut TextEdit,
    config: &vt::Config,
    metrics: &Metrics<'_>,
    point: kurbo::Point,
) {
    let place = place_at_point(edit, config, metrics, point);
    edit.previous_caret = edit.caret;
    edit.caret = place;
    edit.selection.set_active(place);
    scroll_to_caret(edit, config, metrics);
}

/// Selects the whole line under a point — what a double click does.
///
/// The whole line, deliberately, and not the word under the pointer: a double
/// click in a field holding `"Hello World"` selects all of it.
pub fn select_line_at(
    edit: &mut TextEdit,
    config: &vt::Config,
    metrics: &Metrics<'_>,
    point: kurbo::Point,
) {
    let place = place_at_point(edit, config, metrics, point);
    let (begin, end) = line_bounds(edit, place);
    edit.selection = Selection::new(begin, end);
    edit.previous_caret = edit.caret;
    edit.caret = end;
    scroll_to_caret(edit, config, metrics);
}

/// The first and last places of the line a place sits on.
fn line_bounds(edit: &TextEdit, place: Place) -> (Place, Place) {
    let begin = Place::new(place.section, place.line, None);
    let end = edit
        .layout
        .sections
        .get(place.section as usize)
        .and_then(|section| section.lines.get(place.line as usize))
        .map_or(begin, |line| {
            Place::new(place.section, place.line, line.last_word())
        });
    (begin, end)
}

/// The overlay a focused field draws: its caret, or its selection bands.
///
/// A field showing a selection shows **no** caret — the two are alternatives,
/// not additions, which is what the two `form_textfield_selected_*` goldens
/// pin against the two `form_textfield_focused_*` ones.
#[must_use]
pub fn highlight(
    edit: &TextEdit,
    config: &vt::Config,
    metrics: &Metrics<'_>,
    caret_width: f32,
) -> Highlight {
    if edit.has_selection() {
        return Highlight {
            caret: None,
            selection: selection_bands(edit, config, metrics),
        };
    }
    Highlight {
        caret: Some(vt::hit::caret_rect(
            &edit.layout,
            config.plate,
            config,
            metrics,
            edit.offset,
            edit.caret,
            caret_width,
        )),
        selection: Vec::new(),
    }
}

/// The rectangles a selection paints behind its text: **one per selected
/// word**, merged where they touch.
///
/// # Why per word, and not per line
///
/// The obvious implementation spans one rectangle from the caret at the
/// selection's start to the caret at its end, per line. That is right for a
/// left-to-right line and **wrong for every other kind**, because it assumes
/// the carets advance monotonically with the word index. They do not: on a
/// right-to-left line the layout places word 1 at the line's right edge and
/// the last word at its left, so the two endpoint carets are the *interior*
/// of the run rather than its extremes, and the band collapses to roughly one
/// character. Measured on `form_textfield_selected_rtl`, whose ten Hebrew
/// characters produced a six-unit band where the oracle paints fifty.
///
/// Filling one rectangle per **word** — its own extent at the line's ascent
/// and descent — is direction-agnostic by construction, which is why it needs
/// no right-to-left case. Built from the carets that bound each word, whose
/// min and max are that word's extent whichever way the line runs. Touching
/// rectangles are merged so a contiguous run is one fill.
///
/// A **section break** is skipped rather than filled. It occupies an index in
/// the flat numbering — the tokenizer counted it, so undo and the caret both
/// need it to — but it is not a word, and filling it would produce a rectangle
/// spanning from the end of one line to the start of the next.
fn selection_bands(
    edit: &TextEdit,
    config: &vt::Config,
    metrics: &Metrics<'_>,
) -> Vec<kurbo::Rect> {
    let range = edit.selection.range();
    let (from, to) = (
        vt::hit::word_index_of_place(&edit.layout, range.begin()),
        vt::hit::word_index_of_place(&edit.layout, range.end()),
    );
    if from >= to {
        return Vec::new();
    }

    let caret_at = |index: usize| {
        vt::hit::caret_rect(
            &edit.layout,
            config.plate,
            config,
            metrics,
            edit.offset,
            vt::hit::place_of_word_index(&edit.layout, index),
            0.0,
        )
    };

    let mut bands: Vec<kurbo::Rect> = Vec::new();
    for index in from..to {
        if is_section_break(edit, index) {
            continue;
        }
        let word = word_band(index, &caret_at);
        match bands.last_mut() {
            // Merge into the run being built when the two rectangles share an
            // edge and a line. `union` would also swallow a gap; touching is
            // the condition, so a band never covers a word outside the
            // selection.
            Some(last) if touches(*last, word) => *last = last.union(word),
            _ => bands.push(word),
        }
    }
    bands
}

/// Whether a flat index names a **section break** rather than a character.
///
/// The break between two paragraphs counts as one index, so the place at
/// `index` and the place at `index + 1` land on different lines. That is the
/// signal, and it needs no access to the tokenizer's own bookkeeping.
fn is_section_break(edit: &TextEdit, index: usize) -> bool {
    let here = vt::hit::place_of_word_index(&edit.layout, index);
    let next = vt::hit::place_of_word_index(&edit.layout, index + 1);
    here.section != next.section || here.line != next.line
}

/// One selected word's rectangle: `[x, x + width]` at the line's ascent and
/// descent.
///
/// Built from the two carets that bound the word, whose min and max are its
/// extent whichever way the line runs. That is the whole rule, and it needs no
/// right-to-left case of its own — which is the point of taking it per word
/// rather than per line.
fn word_band(index: usize, caret_at: &impl Fn(usize) -> kurbo::Rect) -> kurbo::Rect {
    let (before, after) = (caret_at(index), caret_at(index + 1));
    kurbo::Rect::new(
        before.x0.min(after.x0),
        before.y0.min(after.y0),
        before.x1.max(after.x1),
        before.y1.max(after.y1),
    )
}

/// Whether two word rectangles sit on one line and share a vertical edge.
///
/// Either order counts, because a right-to-left run's next word is to the
/// *left* of the one before it. The tolerance is a thousandth of a unit:
/// consecutive words' carets come from the same accumulated advance, so they
/// agree exactly in principle and to within rounding in practice.
fn touches(a: kurbo::Rect, b: kurbo::Rect) -> bool {
    const EPSILON: f64 = 1e-3;
    let same_line = (a.y0 - b.y0).abs() < EPSILON && (a.y1 - b.y1).abs() < EPSILON;
    let adjacent = (a.x1 - b.x0).abs() < EPSILON || (b.x1 - a.x0).abs() < EPSILON;
    same_line && adjacent
}
