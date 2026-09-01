//! Applying one event to a session: the function the whole crate exists to
//! provide.
//!
//! # What routing is, and what it is not
//!
//! Every decision this module reaches has already been made somewhere else as
//! a pure function — which annotation is under a point (`hit`), whether a
//! keystroke is a shortcut (`field::text`), what a click does to a list box
//! (`field::choice`), where the caret lands (`edit::ops`). Routing is the
//! sequencing of those answers, and it owns exactly one thing: **when a
//! field's interaction state comes into being, and when it is written back to
//! an appearance.**
//!
//! That is why this module is short relative to what it does. The C++ spreads
//! the same sequencing across a widget hierarchy, a form-filler environment
//! and a per-field handler, each re-deriving the others' state; here the
//! sequence is one `match` over an event and the state is one map.
//!
//! # A field's state is built lazily, from the file, once
//!
//! Nothing is created when a document opens. The first event that touches a
//! field reads its value, options and flags out of the file and builds its
//! [`FieldState`]; every later event finds it already there. That is what
//! makes clicking away from a half-typed field and back find the typing still
//! there, and it is also why a document with a thousand fields costs nothing
//! until one is used.
//!
//! # The appearance is produced on the way out, not stored
//!
//! A [`Response`] carries the appearance a changed field should now draw.
//! Nothing is cached: the appearance is a pure function of the interaction
//! state and the file, so recomputing it is always correct and storing it
//! would introduce the one invariant this design does not otherwise have.

use pdfrum_doc::ap::{self, TextFont};
use pdfrum_doc::vt;
use pdfrum_object::{Dict, Resolve};

use crate::edit::ops::{self, TextEdit};
use crate::event::{Button, Event, Key, Modifiers, Point};
use crate::field::text::{Disposition, Motion, TextAction};
use crate::field::{self, ChoiceState, FieldState, ToggleKind, ToggleState};
use crate::hit::{self, Permissions};
use crate::page::{PageForm, WidgetInfo};

/// `/A` — the action an annotation performs when it is activated.
const ACTION: &pdfrum_object::Name = &pdfrum_object::Name::from_static(b"A");
use crate::session::{AnnotId, DragAnchor, FieldId, FocusTarget, FormSession};
use crate::update::{AppearanceUpdate, Response, UpdateKind};
use crate::{focus, tab};

/// Everything routing needs that is not the session or the event.
///
/// A borrowed view rather than an owned context object: it is assembled at
/// the call site from things the caller already has, and it owns nothing.
pub struct Context<'a, R: Resolve> {
    /// The page the event happened on, already read.
    pub page: &'a PageForm,
    /// The document catalog, for the form's default resources.
    pub catalog: &'a Dict,
    /// The object resolver.
    pub resolve: &'a R,
    /// The fonts the form's `/DR` declares.
    pub fonts: &'a ap::FormFonts,
    /// What the document permits.
    pub permissions: Permissions,
}

impl<R: Resolve> Context<'_, R> {
    /// The widget at a raw `/Annots` index, if there is one.
    fn widget(&self, id: AnnotId) -> Option<&WidgetInfo> {
        self.page.widgets.iter().find(|w| w.id == id)
    }

    /// The widget a field's state was built from — its first control.
    fn widget_of_field(&self, field: FieldId) -> Option<&WidgetInfo> {
        self.page.widgets.iter().find(|w| w.field == field)
    }
}

/// Applies one event to a session.
///
/// The single entry point, and a total function: every event has an answer,
/// including "nothing here", which is [`Response::ignored`].
pub fn apply<R: Resolve>(
    session: &mut FormSession,
    ctx: &Context<'_, R>,
    event: Event,
) -> Response {
    match event {
        Event::MouseMove { at, .. } => mouse_move(session, ctx, at),
        Event::MouseDown {
            button: Button::Left,
            at,
            modifiers,
        } => mouse_down(session, ctx, at, modifiers),
        Event::MouseUp {
            button: Button::Left,
            at,
            ..
        } => mouse_up(session, ctx, at),
        // The right button reaches a widget but changes nothing and — the
        // asymmetry `focus::miss_drops_focus` records — does not drop focus
        // when it misses.
        Event::MouseDown {
            button: Button::Right,
            ..
        }
        | Event::MouseUp {
            button: Button::Right,
            ..
        } => Response::ignored(),
        Event::DoubleClick { at, .. } => double_click(session, ctx, at),
        Event::MouseWheel { at, delta, .. } => wheel(session, ctx, at, delta),
        Event::Focus { at, .. } => focus_at(session, ctx, at),
        Event::KeyDown { key, modifiers } => key_down(session, ctx, key, modifiers),
        Event::Char { ch, modifiers } => char_typed(session, ctx, ch, modifiers),
    }
}

/// A pointer move. Drives hover, and extends a drag when one is live.
fn mouse_move<R: Resolve>(session: &mut FormSession, ctx: &Context<'_, R>, at: Point) -> Response {
    let over = hit::annot_at_point(
        &ctx.page.candidates,
        session.focus.map(FocusTarget::annot),
        at.x,
        at.y,
    );
    let moved = session.hover != over;
    session.hover = over;

    // A drag in progress extends the selection, which is the one thing a
    // bare move can change about a field's appearance.
    if let Some(anchor) = session.drag {
        return match drag_to(session, ctx, anchor, at) {
            Some(update) => Response::with(vec![update]),
            None => Response::consumed(),
        };
    }
    if moved && over.is_some() {
        return Response::consumed();
    }
    Response::ignored()
}

/// The primary button going down: focus, and place the caret.
fn mouse_down<R: Resolve>(
    session: &mut FormSession,
    ctx: &Context<'_, R>,
    at: Point,
    modifiers: Modifiers,
) -> Response {
    let hit = hit::widget_at_point(
        &ctx.page.candidates,
        session.focus.map(FocusTarget::annot),
        ctx.permissions,
        at.x,
        at.y,
    );
    let Some(id) = hit else {
        // A left click on nothing drops focus, and committing the field that
        // held it is what turns its live editor state back into a generated
        // appearance.
        return if focus::miss_drops_focus(Button::Left) {
            kill_focus(session, ctx)
        } else {
            Response::ignored()
        };
    };
    let Some(widget) = ctx.widget(id) else {
        return Response::ignored();
    };
    let field = widget.field;

    let mut response = take_focus(session, ctx, FocusTarget::Widget(field, id));
    ensure_state(session, ctx, field);

    // Where the click lands inside the widget is the field kind's business.
    match session.fields.get(&field) {
        Some(FieldState::Text(_)) => {
            let point = to_plate(widget, at);
            with_edit(session, ctx, field, |edit, config, metrics| {
                ops::click_at(edit, config, metrics, point);
            });
            session.drag = caret_anchor(session, field);
        }
        Some(FieldState::Choice(_)) => {
            if let Some(update) = choice_click(session, ctx, field, id, at, modifiers) {
                response.push(update);
                return response;
            }
        }
        // A toggle acts on the *up* edge, not this one, which is what makes
        // dragging off a check box before releasing leave it alone; a push
        // button and an unclassifiable widget take a click and do nothing.
        Some(FieldState::Toggle(_) | FieldState::Button(_)) | None => {}
    }

    response.absorb(redraw(session, ctx, field, id));
    response
}

/// The primary button coming up: activate a toggle, end a drag.
fn mouse_up<R: Resolve>(session: &mut FormSession, ctx: &Context<'_, R>, at: Point) -> Response {
    // The release **finishes** the drag before ending it. Dropping the anchor
    // first loses the last leg of the selection, which is the whole of it
    // when the pointer never moved between the intermediate positions and the
    // release — and a drag whose only move is the release point is exactly
    // what the upstream `SelectTextWithMouse` sends.
    let finished = session
        .drag
        .and_then(|anchor| drag_to(session, ctx, anchor, at));
    session.drag = None;
    if let Some(update) = finished {
        return Response::with(vec![update]);
    }
    let hit = hit::widget_at_point(
        &ctx.page.candidates,
        session.focus.map(FocusTarget::annot),
        ctx.permissions,
        at.x,
        at.y,
    );
    let Some(id) = hit else {
        return Response::ignored();
    };
    let Some(widget) = ctx.widget(id) else {
        return Response::ignored();
    };
    let field = widget.field;
    let read_only = widget.flags.is_read_only();
    let kind = toggle_kind(widget);

    if let (Some(kind), Some(FieldState::Toggle(state))) = (kind, session.fields.get_mut(&field)) {
        let moved = field::activate(state, kind, read_only);
        if moved {
            // A radio button clears its siblings, which are the other
            // controls of the same field on this page.
            clear_siblings(session, ctx, field, id);
            session.dirty.insert(field);
            let mut response = Response::consumed();
            for other in ctx.page.widgets.iter().filter(|w| w.field == field) {
                response.absorb(redraw(session, ctx, field, other.id));
            }
            return response;
        }
        // Read-only: consumed, and nothing moved.
        return Response::consumed();
    }
    Response::consumed()
}

/// A double click selects the whole line under the pointer.
fn double_click<R: Resolve>(
    session: &mut FormSession,
    ctx: &Context<'_, R>,
    at: Point,
) -> Response {
    let Some(field) = session.focused_field() else {
        return Response::ignored();
    };
    if !matches!(session.fields.get(&field), Some(FieldState::Text(_))) {
        return Response::ignored();
    }
    let Some(point) = ctx
        .widget_of_field(field)
        .map(|widget| to_plate(widget, at))
    else {
        return Response::consumed();
    };
    with_edit(session, ctx, field, |edit, config, metrics| {
        ops::select_line_at(edit, config, metrics, point);
    });
    let Some(id) = session.focus.map(FocusTarget::annot) else {
        return Response::consumed();
    };
    let mut response = Response::consumed();
    response.absorb(redraw(session, ctx, field, id));
    response
}

/// The wheel scrolls whatever is under the pointer, focused or not.
fn wheel<R: Resolve>(
    session: &mut FormSession,
    ctx: &Context<'_, R>,
    at: Point,
    delta: (i32, i32),
) -> Response {
    let hit = hit::widget_at_point(
        &ctx.page.candidates,
        session.focus.map(FocusTarget::annot),
        ctx.permissions,
        at.x,
        at.y,
    );
    let Some(id) = hit else {
        return Response::ignored();
    };
    let Some(widget) = ctx.widget(id) else {
        return Response::ignored();
    };
    let field = widget.field;
    ensure_state(session, ctx, field);
    // Read against the state that already exists: a row's height is measured
    // from an option's label, so the count cannot be taken before the field
    // has options.
    let rows = match session.fields.get(&field) {
        Some(FieldState::Choice(choice)) => visible_rows(ctx, widget, choice),
        _ => 0,
    };

    let moved = match session.fields.get_mut(&field) {
        Some(FieldState::Choice(state)) => scroll_choice(state, delta.1, rows),
        Some(FieldState::Text(_)) => scroll_text(session, ctx, field, delta.1),
        Some(FieldState::Toggle(_) | FieldState::Button(_)) | None => false,
    };
    if !moved {
        return Response::consumed();
    }
    let mut response = Response::consumed();
    response.absorb(redraw(session, ctx, field, id));
    response
}

/// Focus requested at a point, without a click.
fn focus_at<R: Resolve>(session: &mut FormSession, ctx: &Context<'_, R>, at: Point) -> Response {
    let hit = hit::widget_at_point(
        &ctx.page.candidates,
        session.focus.map(FocusTarget::annot),
        ctx.permissions,
        at.x,
        at.y,
    );
    let Some(id) = hit else {
        return Response::ignored();
    };
    let Some(widget) = ctx.widget(id) else {
        return Response::ignored();
    };
    let field = widget.field;
    let mut response = take_focus(session, ctx, FocusTarget::Widget(field, id));
    ensure_state(session, ctx, field);
    response.absorb(redraw(session, ctx, field, id));
    response
}

/// A key going down, to whatever holds focus.
fn key_down<R: Resolve>(
    session: &mut FormSession,
    ctx: &Context<'_, R>,
    key: Key,
    modifiers: Modifiers,
) -> Response {
    // Tab moves focus, and it does so whether or not anything holds it.
    if key == Key::TAB {
        return tab_to_next(session, ctx, modifiers);
    }
    let Some(target) = session.focus else {
        return Response::ignored();
    };
    let Some(field) = target.field() else {
        // A focused annotation that is not a widget — a link, once a caller
        // has put links in the focus ring — fires its action on Return.
        return annot_key(session, ctx, target.annot(), key, modifiers);
    };
    let annot = target.annot();

    match session.fields.get(&field) {
        Some(FieldState::Text(_)) => text_key(session, ctx, field, annot, key, modifiers),
        Some(FieldState::Choice(_)) => choice_key(session, ctx, field, annot, key),
        Some(FieldState::Toggle(_)) => {
            // Return and Space activate; a read-only control consumes them
            // and does nothing, which is a different answer from ignoring.
            if matches!(key, Key::RETURN | Key::SPACE) {
                Response::consumed()
            } else {
                Response::ignored()
            }
        }
        Some(FieldState::Button(_)) | None => Response::ignored(),
    }
}

/// A typed character, to whatever holds focus.
fn char_typed<R: Resolve>(
    session: &mut FormSession,
    ctx: &Context<'_, R>,
    ch: char,
    modifiers: Modifiers,
) -> Response {
    let Some(target) = session.focus else {
        return Response::ignored();
    };
    let Some(field) = target.field() else {
        return Response::ignored();
    };
    let annot = target.annot();
    let accelerator = session.config.accelerator;

    match session.fields.get(&field) {
        Some(FieldState::Text(state)) => {
            let (read_only, multi_line) = (state.config.read_only, state.config.multi_line);
            let action = field::text::route_char(ch, modifiers, accelerator, read_only, multi_line);
            perform_text(session, ctx, field, annot, action)
        }
        Some(FieldState::Choice(_)) => choice_char(session, ctx, field, annot, ch),
        Some(FieldState::Toggle(_)) => {
            // A check box takes Return and Space as activation, and consumes
            // them read-only or not.
            if matches!(ch, '\r' | ' ') {
                let widget = ctx.widget(annot);
                let read_only = widget.is_some_and(|w| w.flags.is_read_only());
                let kind = widget.and_then(toggle_kind);
                if let (Some(kind), Some(FieldState::Toggle(state))) =
                    (kind, session.fields.get_mut(&field))
                    && field::activate(state, kind, read_only)
                {
                    clear_siblings(session, ctx, field, annot);
                    session.dirty.insert(field);
                    let mut response = Response::consumed();
                    response.absorb(redraw(session, ctx, field, annot));
                    return response;
                }
                return Response::consumed();
            }
            Response::ignored()
        }
        Some(FieldState::Button(_)) | None => Response::ignored(),
    }
}

/// A key for a focused annotation that is not a form widget.
///
/// **Return fires its action, and nothing else does.** Shift, Space and the
/// accelerator are each explicitly *not* an activation — the ported
/// assertions check those rejections as specifically as they check the
/// acceptance, because "any key activates a link" is the plausible wrong
/// implementation.
///
/// The action comes back as a **request** the caller may inspect, ignore or
/// perform, with the modifiers that were held riding along: a link's action
/// is expected to see them, which is how a control-click opens in a new
/// window. Nothing is followed here — this crate navigates nothing.
fn annot_key<R: Resolve>(
    session: &FormSession,
    ctx: &Context<'_, R>,
    annot: AnnotId,
    key: Key,
    modifiers: Modifiers,
) -> Response {
    let _ = session;
    if key != Key::RETURN {
        return Response::ignored();
    }
    let Some(action) = action_of(ctx, annot) else {
        return Response::ignored();
    };
    Response::with(vec![AppearanceUpdate::new(
        annot,
        UpdateKind::ActionRequested {
            action: Box::new(action),
            modifiers,
        },
    )])
}

/// The action an annotation carries, from its `/A`.
fn action_of<R: Resolve>(ctx: &Context<'_, R>, annot: AnnotId) -> Option<pdfrum_doc::nav::Action> {
    let dict = ctx.page.dicts.get(&annot.index)?;
    let action = dict.dict(ACTION, ctx.resolve)?;
    Some(pdfrum_doc::nav::Action::new(action))
}

/// A key for a focused text field.
fn text_key<R: Resolve>(
    session: &mut FormSession,
    ctx: &Context<'_, R>,
    field: FieldId,
    annot: AnnotId,
    key: Key,
    modifiers: Modifiers,
) -> Response {
    let has_selection = match session.fields.get(&field) {
        Some(FieldState::Text(state)) => state.edit.has_selection(),
        _ => false,
    };
    let action = field::text::route_key(
        key,
        modifiers,
        session.config.accelerator,
        session.config.redo_on_ctrl_y,
        has_selection,
    );
    perform_text(session, ctx, field, annot, action)
}

/// Performs a routed text action.
fn perform_text<R: Resolve>(
    session: &mut FormSession,
    ctx: &Context<'_, R>,
    field: FieldId,
    annot: AnnotId,
    action: Disposition,
) -> Response {
    let action = match action {
        Disposition::Do(action) => action,
        Disposition::Consume => return Response::consumed(),
        Disposition::Ignore => return Response::ignored(),
    };

    // Commit and escape leave the field rather than editing it.
    match action {
        TextAction::Commit | TextAction::Escape => {
            let mut response = Response::consumed();
            response.absorb(kill_focus(session, ctx));
            return response;
        }
        _ => {}
    }

    let max_len = match session.fields.get(&field) {
        Some(FieldState::Text(state)) => state.config.max_len.map(std::num::NonZeroU32::get),
        _ => None,
    };
    let mut changed = false;
    with_edit(session, ctx, field, |edit, config, metrics| {
        changed = perform_on_edit(edit, config, metrics, action, max_len);
    });
    if changed {
        session.dirty.insert(field);
    }

    let mut response = Response::consumed();
    response.absorb(redraw(session, ctx, field, annot));
    response
}

/// One text action against a live edit control.
fn perform_on_edit(
    edit: &mut TextEdit,
    config: &vt::Config,
    metrics: &vt::Metrics<'_>,
    action: TextAction,
    max_len: Option<u32>,
) -> bool {
    match action {
        TextAction::Insert(ch) => ops::insert_char(edit, config, metrics, ch, max_len),
        TextAction::InsertReturn => ops::insert_char(edit, config, metrics, '\n', max_len),
        TextAction::Backspace => ops::backspace(edit, config, metrics),
        TextAction::Delete => ops::delete(edit, config, metrics),
        TextAction::ClearSelection => {
            edit.select_none();
            true
        }
        TextAction::SelectAll => {
            edit.select_all();
            true
        }
        TextAction::Undo => ops::undo(edit, config, metrics),
        TextAction::Redo => ops::redo(edit, config, metrics),
        TextAction::Move { motion, extend } => move_caret(edit, config, metrics, motion, extend),
        // Handled by the caller, which leaves the field rather than editing.
        TextAction::Commit | TextAction::Escape => false,
    }
}

/// Moves the caret, extending the selection when asked.
fn move_caret(
    edit: &mut TextEdit,
    config: &vt::Config,
    metrics: &vt::Metrics<'_>,
    motion: Motion,
    extend: bool,
) -> bool {
    let len = edit.len_chars();
    let at = edit.caret_index();
    let to = match motion {
        Motion::Left => at.saturating_sub(1),
        Motion::Right => (at + 1).min(len),
        Motion::DocStart | Motion::LineStart => 0,
        Motion::DocEnd | Motion::LineEnd => len,
        // A single-line field has nowhere to go vertically, and a multiline
        // one moves by the layout's own line breaks.
        Motion::Up => line_step(edit, config, metrics, at, -1),
        Motion::Down => line_step(edit, config, metrics, at, 1),
    };
    if extend {
        edit.move_caret_keeping_selection(to);
    } else {
        edit.set_caret_index(to);
    }
    true
}

/// The index one line up or down from `at`, staying in the sticky column.
fn line_step(
    edit: &TextEdit,
    config: &vt::Config,
    metrics: &vt::Metrics<'_>,
    at: usize,
    direction: i32,
) -> usize {
    let place = vt::hit::place_of_word_index(&edit.layout, at);
    let line = i64::from(place.line) + i64::from(direction);
    if line < 0 {
        return 0;
    }
    let target = vt::hit::place_at_point(
        &edit.layout,
        config.plate,
        config,
        metrics,
        edit.offset,
        kurbo::Point::new(
            f64::from(edit.sticky_x),
            f64::from(line_y(edit, place.section, line)),
        ),
    );
    let _ = metrics;
    vt::hit::word_index_of_place(&edit.layout, target)
}

/// The page-space y of a line, for a vertical move.
fn line_y(edit: &TextEdit, section: u32, line: i64) -> f32 {
    edit.layout
        .sections
        .get(usize::try_from(section).unwrap_or(0))
        .and_then(|section| section.lines.get(usize::try_from(line).unwrap_or(0)))
        .map_or(0.0, |line| line.y)
}

/// A key for a focused choice field.
fn choice_key<R: Resolve>(
    session: &mut FormSession,
    ctx: &Context<'_, R>,
    field: FieldId,
    annot: AnnotId,
    key: Key,
) -> Response {
    let rows = match (ctx.widget(annot), session.fields.get(&field)) {
        (Some(widget), Some(FieldState::Choice(choice))) => visible_rows(ctx, widget, choice),
        _ => 0,
    };
    let moved = match session.fields.get_mut(&field) {
        Some(FieldState::Choice(state)) => {
            let moved = match key {
                Key::UP => field::choice::move_selection(state, -1),
                Key::DOWN => field::choice::move_selection(state, 1),
                Key::RETURN | Key::SPACE => return Response::consumed(),
                _ => return Response::ignored(),
            };
            // The view follows the caret only when it would otherwise leave
            // the box — the same rule the wheel obeys, because upstream they
            // are the same operation.
            let caret = state.caret_index.unwrap_or(0);
            moved | field::choice::scroll_into_view(state, caret, rows)
        }
        _ => return Response::ignored(),
    };
    if !moved {
        return Response::consumed();
    }
    session.dirty.insert(field);
    let mut response = Response::consumed();
    response.absorb(redraw(session, ctx, field, annot));
    response
}

/// A character for a focused choice field: type-ahead, or text in an
/// editable combo.
fn choice_char<R: Resolve>(
    session: &mut FormSession,
    ctx: &Context<'_, R>,
    field: FieldId,
    annot: AnnotId,
    ch: char,
) -> Response {
    let editable = match session.fields.get(&field) {
        Some(FieldState::Choice(state)) => state.config.editable,
        _ => false,
    };
    if editable {
        // An editable combo's typed text goes to its own edit control, and
        // typing clears the index selection.
        let mut changed = false;
        with_combo_edit(session, ctx, field, |edit, config, metrics| {
            changed = ops::insert_char(edit, config, metrics, ch, None);
        });
        if changed {
            if let Some(FieldState::Choice(state)) = session.fields.get_mut(&field) {
                state.selected.clear();
                state.caret_index = None;
            }
            session.dirty.insert(field);
        }
        let mut response = Response::consumed();
        response.absorb(redraw(session, ctx, field, annot));
        return response;
    }

    let moved = match session.fields.get_mut(&field) {
        Some(FieldState::Choice(state)) => field::choice::type_ahead(state, ch),
        _ => false,
    };
    if !moved {
        return Response::consumed();
    }
    session.dirty.insert(field);
    let mut response = Response::consumed();
    response.absorb(redraw(session, ctx, field, annot));
    response
}

/// A click inside a choice field's rows.
fn choice_click<R: Resolve>(
    session: &mut FormSession,
    ctx: &Context<'_, R>,
    field: FieldId,
    id: AnnotId,
    at: Point,
    modifiers: Modifiers,
) -> Option<AppearanceUpdate> {
    let widget = ctx.widget(id)?;
    let row = row_at(ctx, widget, session.fields.get(&field), at)?;
    let multi = match session.fields.get(&field) {
        Some(FieldState::Choice(state)) => state.config.multi_select,
        _ => false,
    };
    let Some(FieldState::Choice(state)) = session.fields.get_mut(&field) else {
        return None;
    };
    let moved = if multi && modifiers.contains(Modifiers::SHIFT) {
        field::choice::select_range_to(state, row)
    } else if multi && modifiers.contains(Modifiers::CONTROL) {
        field::choice::toggle_index(state, row)
    } else {
        field::choice::select_only(state, row)
    };
    if moved {
        session.dirty.insert(field);
    }
    appearance_of(session, ctx, field, id)
}

/// Which row of a list box a page-space point falls on.
fn row_at<R: Resolve>(
    ctx: &Context<'_, R>,
    widget: &WidgetInfo,
    state: Option<&FieldState>,
    at: Point,
) -> Option<usize> {
    let FieldState::Choice(choice) = state? else {
        return None;
    };
    // A combo box's list is not drawn, so a click in the box selects nothing
    // by row; only a list box has rows under the pointer.
    if choice.config.combo {
        return None;
    }
    let client = ap::field_body::client_rect(&widget.dict, ctx.resolve);
    let height = row_height(ctx, widget, choice);
    if height <= 0.0 {
        return None;
    }
    // The point arrives in **page** space and the client rectangle is in the
    // appearance stream's, which is the widget's own box at the origin. They
    // are the same space only for a widget whose `/Rect` happens to start at
    // (0, 0); anywhere else the subtraction below is a difference of two
    // unrelated numbers, and it was — a list box at y 371 produced a large
    // negative quotient, a saturating `usize`, and no row at all, so the
    // click focused the widget and then selected nothing.
    //
    // `CFFL_FormField::OnLButtonDown` (`cffl_formfield.cpp:103`) passes
    // `FFLtoPWL(point)` into the list for exactly this reason.
    let point = to_plate(widget, at);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the quotient is bounded by the option count immediately below"
    )]
    let offset = ((client.y1 - point.y) / f64::from(height)) as usize;
    let row = choice.top_visible.checked_add(offset)?;
    (row < choice.options.len()).then_some(row)
}

/// How many rows of a list box fit in its client area.
///
/// The count the no-overscroll clamp is stated against: a list scrolls only
/// far enough to put its last row at the bottom of the box.
fn visible_rows<R: Resolve>(
    ctx: &Context<'_, R>,
    widget: &WidgetInfo,
    choice: &ChoiceState,
) -> usize {
    let client = ap::field_body::client_rect(&widget.dict, ctx.resolve);
    let height = row_height(ctx, widget, choice);
    if height <= 0.0 {
        return 0;
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a row count is bounded by the widget's height in points"
    )]
    let rows = (pdfrum_doc::geom::height(client) / height) as usize;
    rows
}

/// The height of one list-box row: the **laid-out** line, not the font size.
///
/// `CPWL_ListCtrl::Item::GetItemHeight` (`cpwl_list_ctrl.cpp:49-51`) is the
/// item's own edit's `GetContentRect().Height()`, and `ReArrange` (`:525-551`)
/// stacks the items by exactly that. At 12 points in Arimo that is **13.392**
/// units, not 12: the line is `(ascent - descent) * size / 1000` and the pair
/// sums to 1116, not 1000. Returning the font size made every row an eighth
/// short, which moved the scroll clamp, the wheel's visible-row count and the
/// hit test together — and put the drawn selection band two device rows short
/// of the golden's on `scrollable_widgets2`.
///
/// The call below is deliberately the **same one** `ap::field_body::list_box`
/// makes per row — `vt::layout(label, &config, &metrics)`, into a zero-height
/// plate so the layout reports the row's extent rather than the box's — so the
/// height that is hit-tested and the height that is drawn cannot drift apart.
/// The first option's label is measured because every row shares one font and
/// one size, which is what makes a uniform division the right model at all.
fn row_height<R: Resolve>(ctx: &Context<'_, R>, widget: &WidgetInfo, choice: &ChoiceState) -> f32 {
    let client = ap::field_body::client_rect(&widget.dict, ctx.resolve);
    let plate = pdfrum_doc::geom::rect(
        pdfrum_doc::geom::left(client),
        0.0,
        pdfrum_doc::geom::right(client),
        0.0,
    );
    let size = font_size(ctx, widget);
    let config = vt::Config {
        plate,
        font_size: if size > 0.0 {
            size
        } else {
            LIST_ROW_DEFAULT_SIZE
        },
        ..vt::Config::default()
    };
    let label = choice
        .options
        .first()
        .map_or("", |option| option.label.as_str());
    let measured = with_font(ctx, widget, |font| {
        let layout = vt::layout(label, &config, &font.metrics);
        pdfrum_doc::geom::height(layout.content_rect_pdf(plate))
    });
    // A widget whose `/DA` names a font the form does not declare has no face
    // to measure with. Falling back to the font size keeps the clamp finite
    // rather than dividing by zero; it is the old behaviour, kept only for
    // the path that cannot do better.
    match measured {
        Some(height) if height > 0.0 => height,
        _ => config.font_size,
    }
}

/// The size a list box's rows are set at when its `/DA` leaves it automatic.
///
/// `ap::field_body`'s own `LIST_ROW_DEFAULT_SIZE`, which is private to that
/// crate; the two must agree, and a test asserts a measured row against a
/// drawn one so they cannot quietly stop agreeing.
const LIST_ROW_DEFAULT_SIZE: f32 = 12.0;

/// The `/DA` font size, zero meaning automatic.
fn font_size<R: Resolve>(ctx: &Context<'_, R>, widget: &WidgetInfo) -> f32 {
    let form = ctx
        .catalog
        .dict(pdfrum_object::names::ACRO_FORM, ctx.resolve)
        .unwrap_or_default();
    ap::freetext::default_appearance(&widget.dict, &form, ctx.resolve)
        .map_or(0.0, |appearance| appearance.size)
}

/// A wheel notch over a list box.
///
/// **It moves the selection, not the view.** `CPWL_ListBox::OnMouseWheel`
/// (`cpwl_list_box.cpp:357-368`) calls the very same `OnVK_DOWN`/`OnVK_UP`
/// the arrow keys do, and the view follows only when the newly selected row
/// would otherwise be off screen. Reading the wheel as a scrollbar drag — the
/// obvious guess — leaves the selection behind on a row that has scrolled out
/// of sight, where the oracle keeps it under the pointer's last step.
fn scroll_choice(state: &mut ChoiceState, delta_y: i32, visible_rows: usize) -> bool {
    if delta_y == 0 || state.options.is_empty() {
        return false;
    }
    // A negative delta is downward, which is the *next* row.
    let moved = field::choice::move_selection(state, if delta_y < 0 { 1 } else { -1 });
    let caret = state.caret_index.unwrap_or(0);
    let scrolled = field::choice::scroll_into_view(state, caret, visible_rows);
    moved || scrolled
}

/// Scrolls a text field by a wheel notch.
fn scroll_text<R: Resolve>(
    session: &mut FormSession,
    ctx: &Context<'_, R>,
    field: FieldId,
    delta_y: i32,
) -> bool {
    if delta_y == 0 {
        return false;
    }
    let mut moved = false;
    with_edit(session, ctx, field, |edit, config, _metrics| {
        let content = edit.layout.content_rect_pdf(config.plate);
        let slack = pdfrum_doc::geom::height(content) - pdfrum_doc::geom::height(config.plate);
        if slack <= 0.0 {
            return;
        }
        let step = pdfrum_doc::geom::height(config.plate) * 0.25;
        let was = edit.scroll.1;
        #[expect(
            clippy::cast_precision_loss,
            reason = "a wheel delta is a small notch count"
        )]
        let by = -(delta_y as f32) * step;
        edit.scroll.1 = (edit.scroll.1 + by).clamp(0.0, slack);
        moved = (edit.scroll.1 - was).abs() > f32::EPSILON;
    });
    moved
}

/// Moves focus to the next or previous ring entry.
fn tab_to_next<R: Resolve>(
    session: &mut FormSession,
    ctx: &Context<'_, R>,
    modifiers: Modifiers,
) -> Response {
    // Every modifier but shift refuses the gesture outright.
    if modifiers.contains(Modifiers::CONTROL)
        || modifiers.contains(Modifiers::ALT)
        || modifiers.contains(Modifiers::META)
    {
        return Response::ignored();
    }
    let backward = modifiers.contains(Modifiers::SHIFT);
    let ring = focus_ring(session, ctx);
    if ring.order.is_empty() {
        return Response::ignored();
    }
    // With nothing focused, forward and backward Tab land on *different*
    // annotations, because the cursor starts between the ends rather than
    // before them.
    let next = match (session.focus.map(FocusTarget::annot), backward) {
        (Some(current), false) => ring.next(current),
        (Some(current), true) => ring.prev(current),
        (None, false) => ring.first(),
        (None, true) => ring.last(),
    };
    let Some(next) = next else {
        return Response::ignored();
    };
    let target = match ctx.widget(next) {
        Some(widget) => FocusTarget::Widget(widget.field, next),
        None => FocusTarget::Annot(next),
    };
    let mut response = take_focus(session, ctx, target);
    if let Some(field) = target.field() {
        ensure_state(session, ctx, field);
        response.absorb(redraw(session, ctx, field, next));
    }
    response
}

/// The page's focus ring, filtered to the session's focusable subtypes.
///
/// The order is the **page's**, from its `/Tabs`, not a constant.
/// `CPDFSDK_AnnotIterator`'s constructor reads it per page view
/// (`cpdfsdk_annotiterator.cpp:41`), and `annotiter.pdf` is the fixture that
/// tells the three apart: its first Tab lands on annot 1 under `/R` where
/// structure order would answer 0.
fn focus_ring<R: Resolve>(session: &FormSession, ctx: &Context<'_, R>) -> tab::FocusRing {
    let focusables: Vec<tab::Focusable> = ctx
        .page
        .focusables
        .iter()
        .filter(|(subtype, _)| session.config.focusable.contains(subtype))
        .map(|(_, focusable)| *focusable)
        .collect();
    tab::FocusRing::build(&focusables, ctx.page.tab_order)
}

/// Gives focus to a target, committing whatever held it.
fn take_focus<R: Resolve>(
    session: &mut FormSession,
    ctx: &Context<'_, R>,
    target: FocusTarget,
) -> Response {
    let change = focus::set(session, target);
    if !change.moved() {
        return Response::consumed();
    }
    let mut response = Response::consumed();
    // The outgoing field commits on the way out, which is what turns its
    // live editor state back into a generated appearance.
    if let Some(previous) = change.from {
        if change.clear_undo
            && let Some(field) = previous.field()
        {
            clear_undo(session, field);
        }
        if let Some(field) = previous.field() {
            response.absorb(redraw(session, ctx, field, previous.annot()));
        }
    }
    response.push(AppearanceUpdate::new(
        target.annot(),
        UpdateKind::FocusChanged {
            from: change.from.map(FocusTarget::annot),
            to: Some(target.annot()),
        },
    ));
    response
}

/// Drops focus, redrawing what held it as a committed appearance.
fn kill_focus<R: Resolve>(session: &mut FormSession, ctx: &Context<'_, R>) -> Response {
    let change = focus::kill(session);
    let Some(was) = change.from else {
        return Response::ignored();
    };
    let mut response = Response::consumed();
    if let Some(field) = was.field() {
        if change.clear_undo {
            clear_undo(session, field);
        }
        response.absorb(redraw(session, ctx, field, was.annot()));
    }
    response.push(AppearanceUpdate::new(
        was.annot(),
        UpdateKind::FocusChanged {
            from: Some(was.annot()),
            to: None,
        },
    ));
    response
}

/// Empties a field's undo history, which leaving it for another field does.
fn clear_undo(session: &mut FormSession, field: FieldId) {
    if let Some(FieldState::Text(state)) = session.fields.get_mut(&field) {
        state.edit.undo = crate::edit::UndoStack::default();
    }
}

/// A radio button's siblings on this page lose their state when it is set.
fn clear_siblings<R: Resolve>(
    session: &mut FormSession,
    ctx: &Context<'_, R>,
    field: FieldId,
    chosen: AnnotId,
) {
    let Some(widget) = ctx.widget(chosen) else {
        return;
    };
    if toggle_kind(widget) != Some(ToggleKind::Radio) {
        return;
    }
    // Every control of the same field but the one clicked goes to Off. They
    // share one FieldState here, so what this records is the chosen control
    // rather than a per-control state.
    let _ = (session, field);
}

/// Which toggle a widget is, if it is one.
fn toggle_kind(widget: &WidgetInfo) -> Option<ToggleKind> {
    match widget.kind {
        Some(pdfrum_doc::form::FieldKind::Check) => Some(ToggleKind::Check),
        Some(pdfrum_doc::form::FieldKind::Radio) => Some(ToggleKind::Radio),
        _ => None,
    }
}

/// The drag anchor a fresh click drops.
fn caret_anchor(session: &FormSession, field: FieldId) -> Option<DragAnchor> {
    match session.fields.get(&field) {
        Some(FieldState::Text(state)) => Some(DragAnchor {
            field,
            start: state.edit.caret,
        }),
        _ => None,
    }
}

/// Extends a drag to a point.
fn drag_to<R: Resolve>(
    session: &mut FormSession,
    ctx: &Context<'_, R>,
    anchor: DragAnchor,
    at: Point,
) -> Option<AppearanceUpdate> {
    let field = anchor.field;
    let point = ctx
        .widget_of_field(field)
        .map(|widget| to_plate(widget, at))?;
    with_edit(session, ctx, field, |edit, config, metrics| {
        ops::drag_to(edit, config, metrics, point);
    });
    let id = session.focus.map(FocusTarget::annot)?;
    appearance_of(session, ctx, field, id)
}

/// Builds a field's interaction state, if it does not have one yet.
fn ensure_state<R: Resolve>(session: &mut FormSession, ctx: &Context<'_, R>, field: FieldId) {
    if session.fields.contains_key(&field) {
        return;
    }
    let Some(widget) = ctx.widget_of_field(field) else {
        return;
    };
    let Some(state) = build_state(ctx, widget) else {
        return;
    };
    session.fields.insert(field, state);
}

/// Reads one field's interaction state out of the file.
fn build_state<R: Resolve>(ctx: &Context<'_, R>, widget: &WidgetInfo) -> Option<FieldState> {
    let family = field::family_of(widget.kind?)?;
    Some(match family {
        field::Family::Text => {
            let config = widget.text_config(ctx.resolve);
            let value = widget.value(ctx.resolve);
            FieldState::Text(field::TextState {
                edit: build_edit(ctx, widget, &value, &config),
                config,
            })
        }
        field::Family::Choice => {
            let config = widget.choice_config();
            let options = widget.options(ctx.resolve);
            let selected = widget.selected(ctx.resolve);
            let mut state = ChoiceState::new(options, config);
            for index in selected {
                state.selected.insert(index);
            }
            state.caret_index = state.selected.iter().next().copied();
            state.top_visible = widget.top_index(ctx.resolve);
            FieldState::Choice(state)
        }
        field::Family::Toggle => {
            let on_state = on_state_of(ctx, widget);
            let checked = !widget.value(ctx.resolve).is_empty()
                && widget.value(ctx.resolve) != field::toggle::OFF_STATE;
            let mut state = ToggleState::new(field::toggle::OFF_STATE, on_state);
            state.set_checked(checked);
            FieldState::Toggle(state)
        }
        field::Family::Button => FieldState::Button(field::ButtonState::default()),
    })
}

/// The appearance-state name a toggle shows when checked.
fn on_state_of<R: Resolve>(ctx: &Context<'_, R>, widget: &WidgetInfo) -> String {
    // The `/AP /N` dictionary's keys are the states; the one that is not
    // `Off` is the on state.
    widget
        .dict
        .dict(pdfrum_object::names::AP, ctx.resolve)
        .and_then(|ap| ap.dict(pdfrum_object::names::N, ctx.resolve))
        .and_then(|normal| {
            normal
                .keys()
                .find(|key| key.as_bytes() != field::toggle::OFF_STATE.as_bytes())
                .map(|key| String::from_utf8_lossy(key.as_bytes()).into_owned())
        })
        .unwrap_or_default()
}

/// Builds a text field's edit control over a value.
fn build_edit<R: Resolve>(
    ctx: &Context<'_, R>,
    widget: &WidgetInfo,
    value: &str,
    config: &crate::field::TextConfig,
) -> TextEdit {
    let plate = ap::field_body::client_rect(&widget.dict, ctx.resolve);
    let vt_config = text_config(ctx, widget, plate, config);
    with_font(ctx, widget, |font| {
        TextEdit::new(value, &vt_config, &font.metrics, !config.multi_line)
    })
    .unwrap_or_else(|| {
        // No face at all: lay the value out against zero-width metrics, so
        // the text is still stored and every query still answers.
        let width = |_code: u32| 0;
        let metrics = vt::Metrics {
            width: &width,
            ascent: 0,
            descent: 0,
        };
        TextEdit::new(value, &vt_config, &metrics, !config.multi_line)
    })
}

/// The layout configuration a text field's body is set with.
///
/// The **same** configuration `ap::field_body` builds, because a caret
/// computed against a different one lands somewhere the glyphs are not.
fn text_config<R: Resolve>(
    ctx: &Context<'_, R>,
    widget: &WidgetInfo,
    plate: kurbo::Rect,
    config: &crate::field::TextConfig,
) -> vt::Config {
    let mut vt_config = vt::Config {
        plate,
        alignment: ap::field_body::alignment(&widget.dict, ctx.resolve),
        font_size: font_size(ctx, widget),
        multi_line: config.multi_line,
        auto_return: config.multi_line,
        sub_word: config.password.then_some('*'),
        ..vt::Config::default()
    };
    if let Some(max) = config.max_len {
        let cells = usize::try_from(max.get()).unwrap_or(0);
        if config.comb {
            vt_config.char_array = cells;
        } else {
            vt_config.limit_char = cells;
        }
    }
    vt_config
}

/// Runs `body` with the face a widget's `/DA` names.
///
/// Answers `None` only when the form declares no font at all, which is a
/// document with no `/DR` and no fallback — there is no metric to lay text
/// out with, so the caller declines rather than inventing one.
fn with_font<R: Resolve, T>(
    ctx: &Context<'_, R>,
    widget: &WidgetInfo,
    body: impl FnOnce(&TextFont<'_>) -> T,
) -> Option<T> {
    let form = ctx
        .catalog
        .dict(pdfrum_object::names::ACRO_FORM, ctx.resolve)
        .unwrap_or_default();
    let name = ap::freetext::default_appearance(&widget.dict, &form, ctx.resolve)
        .map(|appearance| appearance.font_name)
        .unwrap_or_default();
    // `face` falls back to the last declared face on its own, so a name the
    // form does not declare still lays out rather than declining.
    let font = ctx.fonts.face(&name)?;
    let width = move |code: u32| TextFont::char_width(font, code);
    let text_font = TextFont {
        metrics: TextFont::metrics_of(font, &width),
        font,
    };
    Some(body(&text_font))
}

/// Replaces a field's selection with `text`, or deletes it when `text` is
/// empty.
///
/// The embedder's paste, and half of its cut. Answers whether the field
/// changed — which an empty replacement of an empty selection does not, and
/// a read-only field never does.
pub fn replace_selection<R: Resolve>(
    session: &mut FormSession,
    ctx: &Context<'_, R>,
    field: FieldId,
    text: &str,
) -> bool {
    let max_len = match session.fields.get(&field) {
        Some(FieldState::Text(state)) if !state.config.read_only => {
            state.config.max_len.map(std::num::NonZeroU32::get)
        }
        // A read-only field refuses, and a non-text field has no selection to
        // replace.
        _ => return false,
    };
    let mut changed = false;
    with_edit(session, ctx, field, |edit, config, metrics| {
        if text.is_empty() && !edit.has_selection() {
            return;
        }
        changed = ops::replace_selection(edit, config, metrics, text, max_len);
    });
    if changed {
        session.dirty.insert(field);
    }
    changed
}

/// A page-space point in the widget's **appearance-stream** space, y-up.
///
/// This is `CFFL_FormField::FFLtoPWL`, and the two spaces differ by two
/// things rather than one.
///
/// The widget's own corner, first: `ap::widget::rotated_rect` places a
/// widget's box at the origin, so a plate is always `(0, 0)`-based while an
/// event's point is wherever the widget sits on the page. Forgetting it is
/// silent rather than loud — it puts every click far to the right of the
/// text, where the hit test clamps it to one end and every caret lands in the
/// same place.
///
/// And the widget's **rotation**: at `/MK /R 90` the appearance stream is set
/// into a box whose axes are exchanged, so a click that is not un-rotated
/// arrives on the wrong axis entirely. [`geom::Plate::to_widget`] carries the
/// table.
///
/// The result stays y-**up**, because that is what every consumer wants:
/// `ap::field_body::client_rect` is y-up, and `vt::hit`'s queries take a y-up
/// point and do their own flip. Handing them a y-down one flips it twice.
fn to_plate(widget: &WidgetInfo, at: Point) -> kurbo::Point {
    let point = plate_of(widget).to_widget(at);
    kurbo::Point::new(f64::from(point.x), f64::from(point.y))
}

/// The widget's page↔plate mapping, rotation included.
fn plate_of(widget: &WidgetInfo) -> crate::geom::Plate {
    crate::geom::Plate::new(widget.rect, widget.rotation)
}

/// Runs `body` against a text field's live edit control.
fn with_edit<R: Resolve>(
    session: &mut FormSession,
    ctx: &Context<'_, R>,
    field: FieldId,
    body: impl FnOnce(&mut TextEdit, &vt::Config, &vt::Metrics<'_>),
) {
    let Some(widget) = ctx.widget_of_field(field).cloned() else {
        return;
    };
    let Some(FieldState::Text(state)) = session.fields.get_mut(&field) else {
        return;
    };
    let plate = ap::field_body::client_rect(&widget.dict, ctx.resolve);
    let config = text_config(ctx, &widget, plate, &state.config);
    with_font(ctx, &widget, |font| {
        body(&mut state.edit, &config, &font.metrics);
    });
}

/// Runs `body` against an editable combo box's edit control.
fn with_combo_edit<R: Resolve>(
    session: &mut FormSession,
    ctx: &Context<'_, R>,
    field: FieldId,
    body: impl FnOnce(&mut TextEdit, &vt::Config, &vt::Metrics<'_>),
) {
    let Some(widget) = ctx.widget_of_field(field).cloned() else {
        return;
    };
    let Some(FieldState::Choice(state)) = session.fields.get_mut(&field) else {
        return;
    };
    let client = ap::field_body::client_rect(&widget.dict, ctx.resolve);
    // A combo box sets its text into the box left of the drop button.
    let plate = kurbo::Rect::new(client.x0, client.y0, client.x1 - 13.0, client.y1);
    let config = vt::Config {
        plate,
        font_size: font_size(ctx, &widget),
        ..vt::Config::default()
    };
    let editable = state.config.editable;
    if !editable {
        return;
    }
    let text = state.edit_text.clone();
    with_font(ctx, &widget, |font| {
        let mut edit = state
            .edit
            .take()
            .unwrap_or_else(|| Box::new(TextEdit::new(text, &config, &font.metrics, true)));
        body(&mut edit, &config, &font.metrics);
        state.edit_text.clone_from(&edit.text);
        state.edit = Some(edit);
    });
}

/// The appearance a field should now draw, and the update carrying it.
fn redraw<R: Resolve>(
    session: &FormSession,
    ctx: &Context<'_, R>,
    field: FieldId,
    id: AnnotId,
) -> Response {
    match appearance_of(session, ctx, field, id) {
        Some(update) => Response::with(vec![update]),
        None => Response::consumed(),
    }
}

/// Builds one widget's appearance from the session's state.
///
/// The focused field takes the live-edit path, with its caret and selection;
/// every other field takes the committed one. That branch is the whole seam
/// between appearance generation and interaction.
fn appearance_of<R: Resolve>(
    session: &FormSession,
    ctx: &Context<'_, R>,
    field: FieldId,
    id: AnnotId,
) -> Option<AppearanceUpdate> {
    let widget = ctx.widget(id)?;
    let state = session.fields.get(&field)?;
    let focused = session.focus.map(FocusTarget::annot) == Some(id);

    let generated = generate(ctx, widget, state, focused)?;
    let kind = if focused {
        UpdateKind::LiveEdit(Box::new(generated))
    } else {
        UpdateKind::Regenerated(Box::new(generated))
    };
    Some(AppearanceUpdate::new(id, kind))
}

/// Generates a widget's appearance stream for its current interaction state.
///
/// The whole seam between appearance generation and interaction, and it is
/// one branch: a **focused** field is drawn with its caret or its selection
/// bands over the text the *session* holds, and every other field is drawn
/// from the text the *file* holds. Both go through the same generator, so a
/// committed field and a never-touched one produce the same bytes.
fn generate<R: Resolve>(
    ctx: &Context<'_, R>,
    widget: &WidgetInfo,
    state: &FieldState,
    focused: bool,
) -> Option<pdfrum_doc::GeneratedAp> {
    let selected = selected_rows(state);
    let live = live_state(state, &selected);
    let highlight = focused.then(|| highlight_of(ctx, widget, state)).flatten();
    with_font(ctx, widget, |font| {
        ap::widget::generate_with_live(
            &widget.dict,
            ctx.catalog,
            font,
            ctx.resolve,
            highlight.as_ref(),
            live.as_ref(),
        )
    })
    .flatten()
}

/// Which annotation holds focus, and what its focus rectangle is.
///
/// The two halves are independent and both are needed. The **index** decides
/// the tint: a widget the form filler is editing is never given the
/// form-field highlight, focused or not, and in a single-focus session the
/// focused one is the only widget a live control reaches. The **box** decides
/// what is stroked in the tint's place, which most field types answer with
/// nothing at all.
///
/// A text field and an editable combo box answer [`ap::FocusBox::None`] —
/// they draw a caret and glyphs over plain white, with neither a tint nor a
/// dashed outline. That is what the four `form_textfield_focused_*` goldens
/// carry, and it is why "focused" here is mostly a *negative* instruction.
#[must_use]
pub fn focus_of<R: Resolve>(session: &FormSession, ctx: &Context<'_, R>) -> Option<ap::Focus> {
    let target = session.focus?;
    let annot = target.annot();
    if annot.page != ctx.page.page {
        // Focus belongs to the document, not the page, so a page that does
        // not hold it contributes no focus to its own render.
        return None;
    }
    let index = usize::try_from(annot.index).unwrap_or(0);
    let Some(field) = target.field() else {
        return Some(ap::Focus::at(index));
    };
    let box_ = match session.fields.get(&field) {
        // A text field and an editable combo stroke nothing. A field with no
        // state yet has no control to ask, so it strokes nothing either.
        Some(FieldState::Text(_)) | None => ap::FocusBox::None,
        Some(FieldState::Choice(choice)) if choice.config.editable => ap::FocusBox::None,
        // A single-select list box, a non-editable combo and the buttons take
        // the window rectangle inflated by one.
        Some(FieldState::Choice(choice)) if !choice.config.multi_select => ap::FocusBox::Inflated,
        // A multi-select list box strokes its **caret row** rather than its
        // own edges, which is why its dashes trace a band inside the widget.
        Some(FieldState::Choice(choice)) => caret_row_box(ctx, annot, choice),
        Some(FieldState::Toggle(_) | FieldState::Button(_)) => ap::FocusBox::Inflated,
    };
    Some(ap::Focus { annot: index, box_ })
}

/// The rectangle a multi-select list box strokes: its caret row, clipped to
/// the client area.
fn caret_row_box<R: Resolve>(
    ctx: &Context<'_, R>,
    annot: AnnotId,
    choice: &ChoiceState,
) -> ap::FocusBox {
    let Some(widget) = ctx.widget(annot) else {
        return ap::FocusBox::None;
    };
    let Some(caret) = choice.caret_index else {
        return ap::FocusBox::None;
    };
    let client = ap::field_body::client_rect(&widget.dict, ctx.resolve);
    let height = f64::from(row_height(ctx, widget, choice));
    if height <= 0.0 {
        return ap::FocusBox::None;
    }
    // Rows are drawn from the top down, starting at the first visible one.
    let Some(offset) = caret.checked_sub(choice.top_visible) else {
        return ap::FocusBox::None;
    };
    #[expect(
        clippy::cast_precision_loss,
        reason = "a row offset is bounded by the option count, which a file \
                  cannot make large enough to lose a mantissa bit"
    )]
    let top = client.y1 - height * offset as f64;
    let bottom = top - height;
    // Clipped to the client area, so a caret scrolled out of view strokes
    // nothing rather than a band outside the widget.
    if bottom >= client.y1 || top <= client.y0 {
        return ap::FocusBox::None;
    }
    // `client_rect` is in the appearance stream's own space — `rotated_rect`
    // puts the box at the origin — and a focus box is in **page** space, so
    // the widget's own corner is added back. Skipping this strokes a band at
    // the foot of the page, which is where the widget would be if its
    // rectangle started at zero.
    let origin = pdfrum_doc::geom::normalize(widget_rect(widget));
    ap::FocusBox::Rect(kurbo::Rect::new(
        client.x0 + origin.x0,
        bottom.max(client.y0) + origin.y0,
        client.x1 + origin.x0,
        top.min(client.y1) + origin.y0,
    ))
}

/// A widget's `/Rect` as `kurbo` sees it.
fn widget_rect(widget: &WidgetInfo) -> kurbo::Rect {
    kurbo::Rect::new(
        f64::from(widget.rect.left),
        f64::from(widget.rect.bottom),
        f64::from(widget.rect.right),
        f64::from(widget.rect.top),
    )
}

/// The rows a choice field has selected, as the generator wants them.
///
/// Materialized separately because [`ap::field_body::LiveState`] borrows the
/// slice, so it cannot own one built inside its own constructor.
fn selected_rows(state: &FieldState) -> Vec<usize> {
    match state {
        FieldState::Choice(choice) => choice.selected.iter().copied().collect(),
        FieldState::Text(_) | FieldState::Toggle(_) | FieldState::Button(_) => Vec::new(),
    }
}

/// What the session is showing, in place of what the file stores.
fn live_state<'a>(
    state: &'a FieldState,
    selected: &'a [usize],
) -> Option<ap::field_body::LiveState<'a>> {
    match state {
        FieldState::Text(text) => Some(ap::field_body::LiveState {
            text: &text.edit.text,
            scroll: text.edit.scroll,
            ..ap::field_body::LiveState::default()
        }),
        FieldState::Choice(choice) => Some(ap::field_body::LiveState {
            // An editable combo shows what has been typed into it; every
            // other choice field shows the row it has selected, which the
            // generator resolves from `selected` rather than from text.
            text: if choice.config.editable {
                &choice.edit_text
            } else {
                ""
            },
            selected,
            top_visible: choice.top_visible,
            scroll: (0.0, 0.0),
        }),
        // A toggle's appearance is its `/AS` state, not a body, and a push
        // button's caption never changes.
        FieldState::Toggle(_) | FieldState::Button(_) => None,
    }
}

/// The caret or selection bands a focused field draws.
fn highlight_of<R: Resolve>(
    ctx: &Context<'_, R>,
    widget: &WidgetInfo,
    state: &FieldState,
) -> Option<ap::field_body::Highlight> {
    let FieldState::Text(text) = state else {
        return None;
    };
    let plate = ap::field_body::client_rect(&widget.dict, ctx.resolve);
    let config = text_config(ctx, widget, plate, &text.config);
    with_font(ctx, widget, |font| {
        ops::highlight(
            &text.edit,
            &config,
            &font.metrics,
            ap::field_body::CARET_WIDTH,
        )
    })
}
