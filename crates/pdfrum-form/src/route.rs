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

use crate::cascade::{Cascade, FieldRef, Keystroke, KeystrokeOutcome};
use crate::commit;
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
    cascade: &mut dyn Cascade,
    event: Event,
) -> Response {
    match event {
        Event::MouseMove { at, .. } => mouse_move(session, ctx, at),
        Event::MouseDown {
            button: Button::Left,
            at,
            modifiers,
        } => mouse_down(session, ctx, cascade, at, modifiers),
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
        Event::MouseWheel {
            at,
            delta,
            modifiers,
        } => wheel(session, ctx, at, delta, modifiers),
        Event::Focus { at, .. } => focus_at(session, ctx, cascade, at),
        Event::KeyDown { key, modifiers } => key_down(session, ctx, cascade, key, modifiers),
        Event::Char { ch, modifiers } => char_typed(session, ctx, cascade, ch, modifiers),
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

    // An open dropdown carries `Styles::kListboxHoverSel`
    // (`cpwl_combo_box.cpp:210-211`), whose whole effect in
    // `CPWL_ListBox::OnMouseMove` (`cpwl_list_box.cpp:167-181`) is to select
    // the row under the pointer. It is recorded as *hover* rather than folded
    // into the selection because dismissing the list must leave the stored
    // value alone — which is exactly what `bug_736695_4` renders.
    if let Some(response) = hover_in_popup(session, ctx, at) {
        return response;
    }

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
    cascade: &mut dyn Cascade,
    at: Point,
    modifiers: Modifiers,
) -> Response {
    // An open dropdown is a **window in front of the page**, so it is tested
    // before the annotations under it. Upstream this is not a special case at
    // all — `CPWL_Wnd::OnLButtonDown` walks its children first, and the list
    // is a child — but here the widget hit test is containment over
    // `/Annots`, which the list is not in. Without this the same click read
    // as a miss and killed focus (see `popup_hit`).
    if let Some((field, annot, index)) = popup_hit(session, ctx, at) {
        return press_in_popup(session, ctx, field, annot, index);
    }
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
            kill_focus(session, ctx, cascade)
        } else {
            Response::ignored()
        };
    };
    let Some(widget) = ctx.widget(id) else {
        return Response::ignored();
    };
    let field = widget.field;

    let mut response = take_focus(session, ctx, cascade, FocusTarget::Widget(field, id));
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
            // `CPWL_CBButton::OnLButtonDown` (`cpwl_cbbutton.cpp:65-75`)
            // notifies its parent, and `CPWL_ComboBox::NotifyLButtonDown`
            // (`cpwl_combo_box.cpp:497-503`) is `SetPopup(!is_popup_)` — a
            // **toggle**, so a second click on the button shuts the list it
            // opened. A click anywhere else in the box does not.
            if toggle_popup_at(session, ctx, field, id, at) {
                response.absorb(redraw(session, ctx, field, id));
                return response;
            }
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
    // The release inside an open dropdown is what *commits* the row — see
    // `release_in_popup` for why the press only hovers it.
    if let Some((field, annot, index)) = popup_hit(session, ctx, at) {
        return release_in_popup(session, ctx, field, annot, index);
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
    // A double click selects the **whole field**, not the line under the
    // pointer. `CPWL_Edit::OnLButtonDblClk` (`cpwl_edit.cpp:636-644`) calls
    // `edit_impl_->SelectAll()`; the embeddertest's comment says "the entire
    // line" and its field is single-line, so the two agree there and only
    // there. A multiline field is where the wrong reading shows.
    //
    // The point is still needed: upstream selects only when the click is
    // inside the client area (or the field overflows), so a double click on
    // the border selects nothing.
    let Some(widget) = ctx.widget_of_field(field) else {
        return Response::consumed();
    };
    let point = to_plate(widget, at);
    let client =
        pdfrum_doc::geom::normalize(ap::field_body::client_rect(&widget.dict, ctx.resolve));
    // `CFX_FloatRect::Contains` (`fx_coordinates.cpp:229-234`) is inclusive on
    // all four edges, where `kurbo::Rect::contains` is half-open — a click
    // exactly on the client's top or right edge selects upstream and would
    // not here.
    let inside = point.x >= client.x0
        && point.x <= client.x1
        && point.y >= client.y0
        && point.y <= client.y1;
    if !inside {
        return Response::consumed();
    }
    with_edit(session, ctx, field, |edit, _config, _metrics| {
        edit.select_all();
    });
    let Some(id) = session.focus.map(FocusTarget::annot) else {
        return Response::consumed();
    };
    let mut response = Response::consumed();
    response.absorb(redraw(session, ctx, field, id));
    response
}

/// The wheel scrolls whatever is under the pointer, focused or not.
///
/// A **list box** moves its selection rather than its view, with the wheel's
/// own Shift and Control passed through — `CPWL_ListBox::OnMouseWheel`
/// (`cpwl_list_box.cpp:357-368`) hands `IsSHIFTKeyDown`/`IsCTRLKeyDown` to the
/// same `OnVK_DOWN`/`OnVK_UP` the arrow keys call, and those flags change what
/// a multi-select list does with the row it lands on.
///
/// A **combo box** does nothing. It has no `OnMouseWheel` of its own, so it
/// falls through to `CPWL_Wnd::OnMouseWheel` (`cpwl_wnd.cpp:412-429`), which
/// returns false unless a child holds the keyboard capture — and a closed
/// combo's list window is not shown, so nothing moves. Treating it as a list
/// would let a wheel notch silently change a committed value.
fn wheel<R: Resolve>(
    session: &mut FormSession,
    ctx: &Context<'_, R>,
    at: Point,
    delta: (i32, i32),
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
        // A combo box is not a list under the wheel; see this function's docs.
        Some(FieldState::Choice(state)) if state.config.combo => false,
        Some(FieldState::Choice(state)) => scroll_choice(state, delta.1, rows, modifiers),
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
fn focus_at<R: Resolve>(
    session: &mut FormSession,
    ctx: &Context<'_, R>,
    cascade: &mut dyn Cascade,
    at: Point,
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
    let mut response = take_focus(session, ctx, cascade, FocusTarget::Widget(field, id));
    ensure_state(session, ctx, field);
    response.absorb(redraw(session, ctx, field, id));
    response
}

/// A key going down, to whatever holds focus.
fn key_down<R: Resolve>(
    session: &mut FormSession,
    ctx: &Context<'_, R>,
    cascade: &mut dyn Cascade,
    key: Key,
    modifiers: Modifiers,
) -> Response {
    // Tab moves focus, and it does so whether or not anything holds it.
    if key == Key::TAB {
        return tab_to_next(session, ctx, cascade, modifiers);
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
        Some(FieldState::Text(_)) => text_key(session, ctx, cascade, field, annot, key, modifiers),
        Some(FieldState::Choice(_)) => choice_key(session, ctx, field, annot, key, modifiers),
        Some(FieldState::Toggle(_)) => {
            // Return and Space activate; a read-only control consumes them
            // and does nothing, which is a different answer from ignoring.
            if matches!(key, Key::RETURN | Key::SPACE) {
                Response::consumed()
            } else {
                Response::ignored()
            }
        }
        // `[oracle-bug]` a focused push button fires its `/A` on Return, the
        // same activation a focused link gets. `CFFL_PushButton` has no
        // `OnChar` override (where `CFFL_TextField` does, at
        // `cffl_textfield.cpp:116-140`), so
        // `fpdf_formfill_embeddertest.cpp:3658-3667` asserts `DoURIAction`
        // `.Times(0)` and `ASSERT_FALSE(FORM_OnChar(…, kReturn, 0))` — both
        // marked `TODO(crbug.com/1028991)` saying they should be one and
        // true — while the adjacent `LinkActionInvokeTest` (`:3670-3690`)
        // asserts `.Times(4)` and `ASSERT_TRUE` for a link. §12.6.3 table 196
        // performs an annotation's `/A` when it is *activated*, and a keyboard
        // activation of a tab-focused button is one. pdf.js gets it free by
        // rendering push buttons as `<a>` (`annotation_layer.js:2129-2137`).
        Some(FieldState::Button(_)) => annot_key(session, ctx, annot, key, modifiers),
        None => Response::ignored(),
    }
}

/// A typed character, to whatever holds focus.
fn char_typed<R: Resolve>(
    session: &mut FormSession,
    ctx: &Context<'_, R>,
    cascade: &mut dyn Cascade,
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
            perform_text(session, ctx, cascade, field, annot, action)
        }
        Some(FieldState::Choice(_)) => choice_char(session, ctx, cascade, field, annot, ch),
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
        // `[oracle-bug]` The char path too: the upstream assertion is on
        // `FORM_OnChar(…, kReturn, 0)` (`fpdf_formfill_embeddertest.cpp:3667`),
        // so a typed Return activates a focused push button exactly as the
        // key path does. See `key_down`.
        Some(FieldState::Button(_)) if ch == '\r' => {
            annot_key(session, ctx, annot, Key::RETURN, modifiers)
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
    cascade: &mut dyn Cascade,
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
    perform_text(session, ctx, cascade, field, annot, action)
}

/// Performs a routed text action.
fn perform_text<R: Resolve>(
    session: &mut FormSession,
    ctx: &Context<'_, R>,
    cascade: &mut dyn Cascade,
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
            response.absorb(kill_focus(session, ctx, cascade));
            return response;
        }
        _ => {}
    }

    // The per-character keystroke hook, before the edit is applied.
    //
    // `CFFL_InteractiveFormFiller::OnChar` gathers the field action and runs
    // `/AA /K` with `willCommit` false (`cffl_interactiveformfiller.cpp:1020`)
    // ahead of the insertion, then applies `SetSelection` and
    // `ReplaceSelection` from what the script left behind
    // (`cffl_textfield.cpp:216-222`). A refusal is "do nothing": the character
    // is dropped and the field is unchanged, which is `:1052`'s
    // `RecreatePWLWindowFromSavedState` restated.
    let action = match keystroke_hook(session, ctx, cascade, field, action) {
        Keyed::Perform(action) => action,
        Keyed::Refused => return Response::consumed(),
        Keyed::Rewrote => {
            session.dirty.insert(field);
            let mut response = Response::consumed();
            response.absorb(redraw(session, ctx, field, annot));
            return response;
        }
    };

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

/// What the per-character keystroke hook left for routing to do.
enum Keyed {
    /// Go ahead with this action, unchanged.
    Perform(TextAction),
    /// The hook rewrote the text; it is already applied, so edit nothing more.
    Rewrote,
    /// The hook refused. The character is dropped and the field is unchanged.
    Refused,
}

/// Offers a text action to the per-character keystroke hook.
///
/// # Which actions reach a script, and which do not
///
/// Only the ones that **change the text**: an insertion, a return, and the
/// two deletions. Caret movement, selection, undo, redo and scrolling never
/// build a `CFFL_FieldAction` upstream, because `OnChar` and `OnKeyDown` gate
/// the whole gathering on there being a change to describe — a script that
/// saw arrow keys would be seeing keystrokes the specification says a
/// keystroke event is not about.
///
/// A hook that rewrites `change` is answered by *replacing the selection*
/// with what it returned, which is `SetActionData`
/// (`fpdfsdk/formfiller/cffl_textfield.cpp:216-222`) — `SetSelection` then
/// `ReplaceSelection` — rather than by re-running the original action.
fn keystroke_hook<R: Resolve>(
    session: &mut FormSession,
    ctx: &Context<'_, R>,
    cascade: &mut dyn Cascade,
    field: FieldId,
    action: TextAction,
) -> Keyed {
    let change = match action {
        TextAction::Insert(ch) => ch.to_string(),
        TextAction::InsertReturn => "\n".to_string(),
        // A deletion is a keystroke whose change is empty; the selection it
        // replaces is what the edit control already holds.
        TextAction::Backspace | TextAction::Delete => String::new(),
        _ => return Keyed::Perform(action),
    };
    let Some(reference) = field_ref(ctx, field) else {
        return Keyed::Perform(action);
    };
    let Some(FieldState::Text(state)) = session.fields.get(&field) else {
        return Keyed::Perform(action);
    };
    let offered = Keystroke::of(&state.edit, change);

    match cascade.keystroke(&reference, offered.clone()) {
        KeystrokeOutcome::Reject => Keyed::Refused,
        KeystrokeOutcome::Accept(back) if back == offered => Keyed::Perform(action),
        KeystrokeOutcome::Accept(back) => {
            // The script moved the caret, rewrote the text, or both. Apply
            // what it left rather than what was offered.
            set_field_text(session, field, &back.applied());
            Keyed::Rewrote
        }
    }
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
    // Upstream runs `ScrollToCaret` after every one of these, which is what
    // lets an arrow key walk off the visible end of a long value and bring
    // the view with it.
    ops::scroll_to_caret(edit, config, metrics);
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
///
/// The arrow keys carry their modifiers for the same reason the wheel does:
/// `CPWL_ListBox::OnKeyDown` (`cpwl_list_box.cpp:103-119`) hands
/// `IsSHIFTKeyDown`/`IsCTRLKeyDown` to the very `OnVK_UP`/`OnVK_DOWN` that
/// `OnMouseWheel` calls, so the two gestures are one operation upstream and
/// must not diverge here.
fn choice_key<R: Resolve>(
    session: &mut FormSession,
    ctx: &Context<'_, R>,
    field: FieldId,
    annot: AnnotId,
    key: Key,
    modifiers: Modifiers,
) -> Response {
    let rows = match (ctx.widget(annot), session.fields.get(&field)) {
        (Some(widget), Some(FieldState::Choice(choice))) => visible_rows(ctx, widget, choice),
        _ => 0,
    };
    let moved = match session.fields.get_mut(&field) {
        Some(FieldState::Choice(state)) => {
            let (shift, ctrl) = (
                modifiers.contains(Modifiers::SHIFT),
                modifiers.contains(Modifiers::CONTROL),
            );
            let moved = match key {
                Key::UP => field::choice::move_caret_by(state, -1, shift, ctrl),
                Key::DOWN => field::choice::move_caret_by(state, 1, shift, ctrl),
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
    cascade: &mut dyn Cascade,
    field: FieldId,
    annot: AnnotId,
    ch: char,
) -> Response {
    let (combo, editable) = match session.fields.get(&field) {
        Some(FieldState::Choice(state)) => (state.config.combo, state.config.editable),
        _ => (false, false),
    };
    // `CPWL_ComboBox::OnChar` (`cpwl_combo_box.cpp:441-497`) reads two
    // characters before anything else, and they are **not** symmetric:
    //
    // - `Return` **toggles** the list, editable or not, and then re-reads the
    //   current row into the edit half — so a second Return shuts what the
    //   first opened;
    // - `Space` opens it, only on a **gated** combo, and only when it is
    //   shut. An editable combo's Space falls through and types a space.
    //
    // Both return `true` whatever happened, which is why the responses below
    // are consumed even where the list refused to move.
    if combo && matches!(ch, '\r' | '\n') {
        return toggle_popup_by_key(session, ctx, field, annot);
    }
    if combo && !editable && ch == ' ' {
        let shut = matches!(
            session.fields.get(&field),
            Some(FieldState::Choice(state)) if !state.popup_open
        );
        if shut {
            return toggle_popup_by_key(session, ctx, field, annot);
        }
        return Response::consumed();
    }
    if editable {
        // The keystroke hook sees an editable combo's typing exactly as it
        // sees a text field's: `CFFL_ComboBox` builds the same
        // `CFFL_FieldAction` from its edit half
        // (`fpdfsdk/formfiller/cffl_combobox.cpp:180-196`).
        if let Some(reference) = field_ref(ctx, field)
            && let Some(FieldState::Choice(state)) = session.fields.get(&field)
            && let Some(edit) = state.edit.as_ref()
        {
            let offered = Keystroke::of(edit, ch.to_string());
            match cascade.keystroke(&reference, offered.clone()) {
                KeystrokeOutcome::Reject => return Response::consumed(),
                KeystrokeOutcome::Accept(back) if back != offered => {
                    set_field_text(session, field, &back.applied());
                    if let Some(FieldState::Choice(state)) = session.fields.get_mut(&field) {
                        state.selected.clear();
                        state.caret_index = None;
                        state.edit = None;
                    }
                    session.dirty.insert(field);
                    let mut response = Response::consumed();
                    response.absorb(redraw(session, ctx, field, annot));
                    return response;
                }
                KeystrokeOutcome::Accept(_) => {}
            }
        }
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
    // A combo box's own box has no rows — `row_at` says so — so the drop
    // button, handled by the caller, is the only thing a click in one does.
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

/// The width of a combo box's drop button, in PDF units.
///
/// `kDefaultButtonWidth` (`fpdfsdk/pwl/cpwl_combo_box.cpp:22`), the same
/// constant `ap::shapes::drop_button` draws with. `RepositionChildWnd`
/// (`:284-289`) places it as the right `kDefaultButtonWidth` of the client
/// rectangle, clamped to the client's left edge for a widget narrower than
/// the button — which is why the `max` below is not decoration.
const DROP_BUTTON_WIDTH: f32 = 13.0;

/// Whether a page-space point is inside a combo box's drop button.
///
/// The button is a child window in *plate* space, so the point is mapped
/// through the widget's own rotation before it is compared: a `/MK /R 90`
/// combo has its button on the top edge as drawn, not the right one.
fn on_drop_button<R: Resolve>(ctx: &Context<'_, R>, widget: &WidgetInfo, at: Point) -> bool {
    let client = ap::field_body::client_rect(&widget.dict, ctx.resolve);
    let (left, right) = (
        pdfrum_doc::geom::left(client),
        pdfrum_doc::geom::right(client),
    );
    let edge = (right - DROP_BUTTON_WIDTH).max(left);
    let point = to_plate(widget, at);
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a plate coordinate is a widget-sized number"
    )]
    let (x, y) = (point.x as f32, point.y as f32);
    x >= edge
        && x <= right
        && y >= pdfrum_doc::geom::bottom(client)
        && y <= pdfrum_doc::geom::top(client)
}

/// Where a combo box's dropdown would be, whether or not it is open.
///
/// [`None`] for anything that is not a combo, and for a combo whose list has
/// no room to open — `SetPopup`'s two "refuse, but report success" exits.
fn popup_geometry<R: Resolve>(
    ctx: &Context<'_, R>,
    widget: &WidgetInfo,
    choice: &ChoiceState,
) -> Option<crate::popup::PopupGeometry> {
    if !choice.config.combo {
        return None;
    }
    crate::popup::place(
        widget.rect,
        ctx.page.page_height,
        choice.options.len(),
        row_height(ctx, widget, choice),
    )
}

/// Opens or closes a combo box's dropdown, reporting whether the state moved.
///
/// `CPWL_ComboBox::SetPopup` (`cpwl_combo_box.cpp:325-377`) and its **three
/// failure returns**, all of which are `true` — the call reports success and
/// changes nothing:
///
/// - no list at all (a field that is not a combo);
/// - a list whose content rectangle has no height (no options);
/// - a `QueryWherePopup` that comes back with nothing (no room on the page).
///
/// So a click on the drop button of a combo with nowhere to open is still
/// *consumed*; it simply leaves the list shut. That is why this returns
/// "did anything move" rather than "did it succeed": the caller wants to know
/// whether to redraw, and the two questions have different answers here.
fn set_popup<R: Resolve>(
    ctx: &Context<'_, R>,
    widget: &WidgetInfo,
    choice: &mut ChoiceState,
    open: bool,
) -> bool {
    if !choice.config.combo || open == choice.popup_open {
        return false;
    }
    if !open {
        choice.popup_open = false;
        choice.hovered = None;
        return true;
    }
    if popup_geometry(ctx, widget, choice).is_none() {
        return false;
    }
    choice.popup_open = true;
    // `RepositionChildWnd` (`cpwl_combo_box.cpp:275`) runs
    // `ScrollToListItem(select_item_)` as it opens, so an already-selected
    // row is scrolled into view rather than the list opening at the top.
    // Recomputed *after* the flag is set because the geometry is the same
    // either way — the popup's size does not depend on whether it is showing.
    if let Some(selected) = choice.selected.iter().next().copied() {
        let rows =
            popup_geometry(ctx, widget, choice).map_or(0, |geometry| geometry.visible_rows());
        field::choice::scroll_into_view(choice, selected, rows);
    }
    true
}

/// Shuts every open dropdown on the page.
///
/// `CPWL_ComboBox::KillFocus` (`cpwl_combo_box.cpp:52-58`) closes the list
/// before the base class drops focus, so nothing survives a focus change —
/// and because a session focuses one field at a time, closing *every* one is
/// the same operation stated without a special case for which field it was.
fn close_all_popups(session: &mut FormSession) -> bool {
    let mut closed = false;
    for state in session.fields.values_mut() {
        if let FieldState::Choice(choice) = state
            && choice.popup_open
        {
            choice.popup_open = false;
            choice.hovered = None;
            closed = true;
        }
    }
    closed
}

/// The field whose dropdown is open on this page, if one is.
///
/// At most one, because opening one takes focus and taking focus closes the
/// last. The walk is over the page's widgets rather than over the session's
/// fields so that a field with a control on two pages answers for the page
/// being asked about.
fn open_popup_of(session: &FormSession, page: &PageForm) -> Option<(FieldId, AnnotId)> {
    page.widgets
        .iter()
        .find_map(|widget| match session.fields.get(&widget.field) {
            Some(FieldState::Choice(choice)) if choice.popup_open => {
                Some((widget.field, widget.id))
            }
            _ => None,
        })
}

/// The open dropdown a page-space point falls inside, with the row it names.
///
/// **This is the routing gap the pixels could not show.** A mouse-down below
/// an open combo is inside the list window, and upstream `CPWL_Wnd::OnLButtonDown`
/// walks the child windows before the widget's own hit test — so it selects a
/// row. Here the widget hit test is rect containment over `/Annots`, and the
/// list is not in `/Annots`, so the same click read as a **miss** and dropped
/// focus. `bug_736695_3` scored 0.997 anyway, because a wrong 150×15 box is
/// under the SSIM floor; the selection it never made is invisible to the
/// metric and plain in the state.
fn popup_hit<R: Resolve>(
    session: &FormSession,
    ctx: &Context<'_, R>,
    at: Point,
) -> Option<(FieldId, AnnotId, usize)> {
    let (field, annot) = open_popup_of(session, ctx.page)?;
    let widget = ctx.widget(annot)?;
    let FieldState::Choice(choice) = session.fields.get(&field)? else {
        return None;
    };
    let geometry = popup_geometry(ctx, widget, choice)?;
    let offset = geometry.row_at(at.x, at.y)?;
    let index = crate::popup::option_at(choice, offset)?;
    Some((field, annot, index))
}

/// A pointer move over an open dropdown, which hover-selects a row.
///
/// [`None`] when the pointer is not over an open list, which is what lets
/// `mouse_move` fall through to hover and drag as before. A move that stays
/// on the same row answers `Some(consumed)` with no update: the pointer is
/// still inside a window, so the event is not the page's, but nothing was
/// repainted.
fn hover_in_popup<R: Resolve>(
    session: &mut FormSession,
    ctx: &Context<'_, R>,
    at: Point,
) -> Option<Response> {
    let (field, annot, index) = popup_hit(session, ctx, at)?;
    let moved = match session.fields.get_mut(&field) {
        Some(FieldState::Choice(choice)) => {
            let moved = choice.hovered != Some(index);
            choice.hovered = Some(index);
            moved
        }
        _ => false,
    };
    if !moved {
        return Some(Response::consumed());
    }
    let mut response = Response::consumed();
    response.absorb(redraw(session, ctx, field, annot));
    Some(response)
}

/// A left-down on the drop button, reporting whether the list moved.
///
/// Returns `false` for a click that is not on the button, which is what lets
/// the caller fall through to the ordinary click handling; a click that *is*
/// on it but cannot open the list — no options, no room — also answers
/// `false`, because nothing moved and there is nothing to redraw. Either way
/// the click stays consumed: the widget took focus before this ran.
fn toggle_popup_at<R: Resolve>(
    session: &mut FormSession,
    ctx: &Context<'_, R>,
    field: FieldId,
    id: AnnotId,
    at: Point,
) -> bool {
    let Some(widget) = ctx.widget(id) else {
        return false;
    };
    let combo = matches!(
        session.fields.get(&field),
        Some(FieldState::Choice(choice)) if choice.config.combo
    );
    if !combo || !on_drop_button(ctx, widget, at) {
        return false;
    }
    let Some(FieldState::Choice(choice)) = session.fields.get_mut(&field) else {
        return false;
    };
    let open = !choice.popup_open;
    // Split so the immutable `ctx.widget` borrow and the mutable state borrow
    // do not overlap: `set_popup` needs both, and the widget is `ctx`'s.
    let mut taken = std::mem::take(choice);
    let moved = set_popup(ctx, widget, &mut taken, open);
    if let Some(FieldState::Choice(choice)) = session.fields.get_mut(&field) {
        *choice = taken;
    }
    moved
}

/// `Return` (or a gated combo's `Space`) toggling the dropdown.
///
/// The keyboard spelling of `toggle_popup_at`, without a point to test. The
/// response is always consumed — `CPWL_ComboBox::OnChar`'s two early cases
/// return `true` even when `SetPopup` refused — so a list with nowhere to
/// open still swallows the key rather than letting it type a character.
fn toggle_popup_by_key<R: Resolve>(
    session: &mut FormSession,
    ctx: &Context<'_, R>,
    field: FieldId,
    annot: AnnotId,
) -> Response {
    let Some(widget) = ctx.widget(annot) else {
        return Response::consumed();
    };
    let Some(FieldState::Choice(choice)) = session.fields.get_mut(&field) else {
        return Response::consumed();
    };
    let open = !choice.popup_open;
    let mut taken = std::mem::take(choice);
    let moved = set_popup(ctx, widget, &mut taken, open);
    if let Some(FieldState::Choice(choice)) = session.fields.get_mut(&field) {
        *choice = taken;
    }
    if !moved {
        return Response::consumed();
    }
    let mut response = Response::consumed();
    response.absorb(redraw(session, ctx, field, annot));
    response
}

/// A left-down inside an open dropdown's rows.
///
/// **Down hovers, up selects.** `CPWL_ListBox::OnLButtonDown`
/// (`cpwl_list_box.cpp:137-150`) runs `list_ctrl_->OnMouseDown`, which moves
/// the list control's own selection; the *commit* — copying the row's text
/// into the edit half and shutting the list — is
/// `CPWL_ComboBox::NotifyLButtonUp` (`cpwl_combo_box.cpp:505-516`), reached
/// from `CPWL_CBListBox::OnLButtonUp`. Splitting them matters because a press
/// that drags off the list before releasing must leave the field alone.
fn press_in_popup<R: Resolve>(
    session: &mut FormSession,
    ctx: &Context<'_, R>,
    field: FieldId,
    annot: AnnotId,
    index: usize,
) -> Response {
    if let Some(FieldState::Choice(choice)) = session.fields.get_mut(&field) {
        choice.hovered = Some(index);
    }
    let mut response = Response::consumed();
    response.absorb(redraw(session, ctx, field, annot));
    response
}

/// A left-up inside an open dropdown's rows: the row is chosen.
///
/// `CPWL_ComboBox::NotifyLButtonUp` (`cpwl_combo_box.cpp:505-516`) in order —
/// `SetSelectText()`, `SelectAllText()`, `edit_->SetFocus()`,
/// `SetPopup(false)`. The first is what makes the chosen row the field's
/// value, and it routes through `ReplaceSelection` upstream so **every combo
/// selection is undoable**; here `select_only` is the same state change over
/// this crate's model. The last is why the list shuts on release rather than
/// on press.
fn release_in_popup<R: Resolve>(
    session: &mut FormSession,
    ctx: &Context<'_, R>,
    field: FieldId,
    annot: AnnotId,
    index: usize,
) -> Response {
    let label = match session.fields.get_mut(&field) {
        Some(FieldState::Choice(choice)) => {
            field::choice::select_only(choice, index);
            choice.popup_open = false;
            choice.hovered = None;
            choice
                .options
                .get(index)
                .map(|option| option.label.clone())
                .unwrap_or_default()
        }
        _ => return Response::consumed(),
    };
    // An editable combo shows the chosen row in its text half, which is what
    // `SetSelectText`'s `edit_->ReplaceSelection(list_->GetText())` puts
    // there. A gated one has no text half and reads its label from the
    // selection instead.
    set_combo_text(session, ctx, field, label);
    session.dirty.insert(field);
    let mut response = Response::consumed();
    response.absorb(redraw(session, ctx, field, annot));
    response
}

/// `SetSelectText()` then `SelectAllText()`: the chosen row's label into the
/// combo's text half, **left selected**.
///
/// Both halves matter and the second is the one that shows.
/// `CPWL_ComboBox::SetSelectText` (`cpwl_combo_box.cpp:518-523`) is
/// `SelectAllText(); ReplaceSelection(list_->GetText()); SelectAllText();` —
/// it *ends* on a select-all — and `NotifyLButtonUp` (`:505-516`) then calls
/// `SelectAllText()` again for good measure. So after choosing a row the
/// text half holds that row's label with **every character selected**, which
/// is why `bug_736695_3`'s golden shows `Spain` as white glyphs on a navy
/// band rather than as black text on white.
///
/// The control is rebuilt from the label rather than edited in place: the
/// whole text is being replaced, so there is nothing of the old one to keep,
/// and `with_combo_edit` is the one place that knows the plate and the face
/// to build it with.
fn set_combo_text<R: Resolve>(
    session: &mut FormSession,
    ctx: &Context<'_, R>,
    field: FieldId,
    label: String,
) {
    let editable = matches!(
        session.fields.get(&field),
        Some(FieldState::Choice(choice)) if choice.config.editable
    );
    if !editable {
        return;
    }
    if let Some(FieldState::Choice(choice)) = session.fields.get_mut(&field) {
        choice.edit_text = label;
        // Dropped rather than rewritten, so `with_combo_edit` below lays the
        // new label out from scratch: one place the text can come from
        // instead of two that must agree.
        choice.edit = None;
    }
    with_combo_edit(session, ctx, field, |edit, _config, _metrics| {
        edit.select_all();
    });
}

/// What a host draws for one page's open dropdown, if one is open.
///
/// The **state-plus-geometry** half of the M14 chrome ruling (STYLE §2b): the
/// library says where the list is and what is in it, and the host paints it
/// on its own schedule. Nothing here is a callback and nothing is a trait —
/// a viewer that never asks is never told, and one that asks twice gets the
/// same answer.
///
/// [`None`] when nothing on the page has its dropdown open, which is the
/// common case: only a click on a drop button, a `Return` or a `Space` on a
/// gated combo opens one.
#[must_use]
pub fn popup_view<R: Resolve>(
    session: &FormSession,
    ctx: &Context<'_, R>,
) -> Option<crate::popup::PopupView> {
    let (field, annot) = open_popup_of(session, ctx.page)?;
    let widget = ctx.widget(annot)?;
    let FieldState::Choice(choice) = session.fields.get(&field)? else {
        return None;
    };
    let geometry = popup_geometry(ctx, widget, choice)?;
    Some(crate::popup::PopupView {
        annot,
        anchor: widget.rect,
        geometry,
        options: choice
            .options
            .iter()
            .map(|option| option.label.clone())
            .collect(),
        selected: choice.selected.iter().next().copied(),
        hovered: choice.hovered,
        top_visible: choice.top_visible,
        edit_text: choice.config.editable.then(|| choice.edit_text.clone()),
    })
}

/// How far one scrollable control has scrolled, in rows.
///
/// The second value getter the chrome ruling asks for, and the reason it is
/// keyed by annotation rather than carried on [`popup_view`]: a **list box**
/// scrolls without any dropdown being open, and its scroll bar is the host's
/// to draw for exactly the same reason the dropdown is.
///
/// [`None`] for an annotation that is not a choice widget, or one the session
/// has never built state for.
#[must_use]
pub fn scroll_view<R: Resolve>(
    session: &FormSession,
    ctx: &Context<'_, R>,
    annot: AnnotId,
) -> Option<crate::popup::ScrollView> {
    let widget = ctx.widget(annot)?;
    let FieldState::Choice(choice) = session.fields.get(&widget.field)? else {
        return None;
    };
    // An open dropdown scrolls in its own window, which is taller than the
    // widget; a closed combo and a list box scroll inside the widget's box.
    let visible = match popup_geometry(ctx, widget, choice).filter(|_| choice.popup_open) {
        Some(geometry) => geometry.visible_rows(),
        None => visible_rows(ctx, widget, choice),
    };
    Some(crate::popup::ScrollView {
        top_visible: choice.top_visible,
        visible_rows: visible,
        total: choice.options.len(),
    })
}

/// The host reporting that the user picked a row of an open dropdown.
///
/// The **intent** half of the chrome ruling: a host that drew the list from
/// [`popup_view`] tells the session what was chosen, and the session does
/// what a click on that row would have done — select it, shut the list, and
/// hand back the widget's new appearance. Exactly `NotifyLButtonUp`'s
/// sequence, reachable without synthesizing a click at coordinates the host
/// would have to compute backwards from the geometry it was given.
///
/// An index past the end of the options is ignored, and the response is
/// [`Response::ignored`] — a host cannot corrupt a field by miscounting.
pub fn choose<R: Resolve>(
    session: &mut FormSession,
    ctx: &Context<'_, R>,
    cascade: &mut dyn Cascade,
    annot: AnnotId,
    index: usize,
) -> Response {
    // The cascade is taken but not spent here, and that is upstream's shape
    // rather than an omission: `CFFL_ComboBox::SaveData`
    // (`fpdfsdk/formfiller/cffl_combobox.cpp:90`) runs only from
    // `CommitData`, which only `KillFocusForAnnot` calls — so choosing a row
    // changes the selection and the scripts run when the field is left.
    // The parameter is on the signature because this is one of the three
    // entry points that *can* reach a commit (PLAN §M15's E1 ruling), and a
    // caller must not have to discover later that it needs one.
    let _ = &cascade;
    let Some(widget) = ctx.widget(annot) else {
        return Response::ignored();
    };
    let field = widget.field;
    let in_range = matches!(
        session.fields.get(&field),
        Some(FieldState::Choice(choice)) if index < choice.options.len()
    );
    if !in_range {
        return Response::ignored();
    }
    release_in_popup(session, ctx, field, annot, index)
}

/// The host reporting that an open dropdown was dismissed without a choice.
///
/// `SetPopup(false)`, and nothing else: the stored selection is untouched,
/// which is what `bug_736695_4` asserts by hovering a row, clicking away, and
/// rendering a field that never changed.
///
/// [`Response::ignored`] when that annotation had no dropdown open, so a host
/// may call it unconditionally.
pub fn close_popup<R: Resolve>(
    session: &mut FormSession,
    ctx: &Context<'_, R>,
    annot: AnnotId,
) -> Response {
    let Some(widget) = ctx.widget(annot) else {
        return Response::ignored();
    };
    let field = widget.field;
    let closed = match session.fields.get_mut(&field) {
        Some(FieldState::Choice(choice)) if choice.popup_open => {
            choice.popup_open = false;
            choice.hovered = None;
            true
        }
        _ => false,
    };
    if !closed {
        return Response::ignored();
    }
    let mut response = Response::consumed();
    response.absorb(redraw(session, ctx, field, annot));
    response
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
    let measured = with_font(ctx, widget, |font, _substitute| {
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
fn scroll_choice(
    state: &mut ChoiceState,
    delta_y: i32,
    visible_rows: usize,
    modifiers: Modifiers,
) -> bool {
    if delta_y == 0 || state.options.is_empty() {
        return false;
    }
    // A negative delta is downward, which is the *next* row. The wheel's own
    // modifiers are handed on, because `OnMouseWheel` hands them to `OnVK`
    // and they decide what a multi-select list does with the row.
    let moved = field::choice::move_caret_by(
        state,
        if delta_y < 0 { 1 } else { -1 },
        modifiers.contains(Modifiers::SHIFT),
        modifiers.contains(Modifiers::CONTROL),
    );
    let caret = state.caret_index.unwrap_or(0);
    let scrolled = field::choice::scroll_into_view(state, caret, visible_rows);
    moved || scrolled
}

/// Scrolls a text field by a wheel notch.
///
/// A `DoNotScroll` field does not move, and the gate is upstream's own:
/// `CPWL_EditImpl::SetScrollPosY` (`fpdfsdk/pwl/cpwl_edit_impl.cpp:1175-1178`)
/// returns at its first statement when `enable_scroll_` is clear, so every
/// caller — the wheel included — writes nothing.
/// `CFFL_TextField::GetCreateParam` (`cffl_textfield.cpp:54-57`) withholds
/// `kWindowVScroll` from such a field besides, so upstream draws it no
/// scrollbar to drag either.
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
        moved = ops::scroll_by(edit, config, delta_y);
    });
    moved
}

/// Moves focus to the next or previous ring entry.
fn tab_to_next<R: Resolve>(
    session: &mut FormSession,
    ctx: &Context<'_, R>,
    cascade: &mut dyn Cascade,
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
    let mut response = take_focus(session, ctx, cascade, target);
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

/// How a field a caller is leaving named itself to its scripts.
///
/// `None` for a target that is not a widget, or one this page does not carry
/// — a script cannot be run for a field the routing context cannot see.
fn field_ref<R: Resolve>(ctx: &Context<'_, R>, field: FieldId) -> Option<FieldRef> {
    let widget = ctx.widget_of_field(field)?;
    Some(FieldRef {
        name: widget.name.clone(),
        index: field.0,
    })
}

/// The text a field currently holds in the session, for the commit gate.
///
/// Only the two families that carry text have one: a toggle's value is its
/// `/AS` state and a push button has none, and neither reaches the commit
/// cascade upstream either (`CFFL_CheckBox`/`CFFL_RadioButton` reach
/// `CommitData` through `IsDataChanged`, which their own `SaveData` answers
/// without a keystroke script).
fn edited_text(session: &FormSession, field: FieldId) -> Option<String> {
    match session.fields.get(&field)? {
        FieldState::Text(text) => Some(text.edit.text.clone()),
        FieldState::Choice(choice) if choice.config.editable => Some(choice.edit_text.clone()),
        FieldState::Choice(_) | FieldState::Toggle(_) | FieldState::Button(_) => None,
    }
}

/// Runs the commit cascade for a field that is losing focus.
///
/// This is `CFFL_FormField::KillFocusForAnnot`'s call to `CommitData`
/// (`fpdfsdk/formfiller/cffl_formfield.cpp:306`), which is the *only* place
/// upstream where the script gates run over a whole field value. The answer
/// says whether focus may proceed: see [`commit::CommitOutcome::keeps_focus`]
/// and `commit`'s module documentation for why a refusal keeps the field here
/// where the oracle drops it.
///
/// `None` when nothing ran — a field with no text, or one whose value has not
/// moved — which is the ordinary case and the one that must cost nothing.
fn commit_field<R: Resolve>(
    session: &mut FormSession,
    ctx: &Context<'_, R>,
    cascade: &mut dyn Cascade,
    field: FieldId,
) -> Option<commit::CommitOutcome> {
    let reference = field_ref(ctx, field)?;
    let edited = edited_text(session, field)?;
    let stored = ctx.widget_of_field(field)?.value(ctx.resolve);
    let outcome = commit::run(
        &reference,
        &stored,
        &edited,
        cascade,
        session.config.max_calculate_depth,
    );

    if outcome.reverted {
        // The gate refused: the field goes back to what the document holds.
        set_field_text(session, field, &stored);
        return Some(outcome);
    }
    for (index, value) in &outcome.writes {
        // A calculation names fields by their index in the form's list, which
        // is what `FieldId` is.
        set_field_text(session, FieldId(*index), value);
        session.dirty.insert(FieldId(*index));
    }
    Some(outcome)
}

/// Puts a field's text back to `value`, whichever text-bearing family it is.
fn set_field_text(session: &mut FormSession, field: FieldId, value: &str) {
    match session.fields.get_mut(&field) {
        Some(FieldState::Text(text)) => {
            text.edit.text = value.to_string();
            text.edit.undo = crate::edit::UndoStack::default();
        }
        Some(FieldState::Choice(choice)) if choice.config.editable => {
            choice.edit_text = value.to_string();
        }
        _ => {}
    }
}

/// Gives focus to a target, committing whatever held it.
fn take_focus<R: Resolve>(
    session: &mut FormSession,
    ctx: &Context<'_, R>,
    cascade: &mut dyn Cascade,
    target: FocusTarget,
) -> Response {
    // The outgoing field's scripts run *before* focus moves, because a
    // refusal keeps it — `commit`'s module doc, and A63.
    if let Some(previous) = session.focus.and_then(FocusTarget::field)
        && session.focus != Some(target)
        && let Some(outcome) = commit_field(session, ctx, cascade, previous)
        && outcome.keeps_focus()
    {
        let annot = session.focus.map(FocusTarget::annot);
        let mut response = Response::consumed();
        if let Some(annot) = annot {
            response.absorb(redraw(session, ctx, previous, annot));
        }
        return response;
    }

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
///
/// Public because it is not only a left click's miss path: the embedder's own
/// `FORM_ForceToKillFocus` is the same operation, and a second implementation
/// of it would be a second chance to forget the redraw. Dropping focus is
/// what turns a field's live editor state back into a generated stream
/// (brief §3.3 step 6), so a version that only reported `FocusChanged` would
/// leave the caret and the live text on the page.
pub fn kill_focus<R: Resolve>(
    session: &mut FormSession,
    ctx: &Context<'_, R>,
    cascade: &mut dyn Cascade,
) -> Response {
    // `CPWL_ComboBox::KillFocus` (`cpwl_combo_box.cpp:52-58`) shuts the list
    // *before* the base class drops focus, and returns early if it could not
    // — so a dropdown never outlives the focus that opened it. Run
    // unconditionally, ahead of `focus::kill`, because it must happen even
    // when the outgoing field is not the one that had a list open.
    let closed = close_all_popups(session);
    // The commit runs before focus goes, because a refusal keeps the field
    // (`commit`'s module doc, A63). `FORM_ForceToKillFocus` is the same
    // operation and the same gate: `KillFocusForAnnot` consults `CommitData`
    // first (`fpdfsdk/formfiller/cffl_formfield.cpp:306`).
    if let Some(previous) = session.focus.and_then(FocusTarget::field)
        && let Some(outcome) = commit_field(session, ctx, cascade, previous)
        && outcome.keeps_focus()
    {
        let annot = session.focus.map(FocusTarget::annot);
        let mut response = Response::consumed();
        if let Some(annot) = annot {
            response.absorb(redraw(session, ctx, previous, annot));
        }
        return response;
    }
    let change = focus::kill(session);
    let Some(was) = change.from else {
        // Nothing held focus, but a list may still have been open — a host
        // that opened one through `choose`'s sibling entry points, or a
        // session whose focus was force-killed. Report the redraw rather than
        // leaving a shut list drawn.
        return if closed {
            Response::consumed()
        } else {
            Response::ignored()
        };
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
///
/// `CPDF_FormField::CheckControl` (`cpdf_formfield.cpp:683-716`) walks every
/// control of the field: the one at the clicked index takes its own on state
/// and **every other one is set to `Off`**. Which control is which matters,
/// because two kids of a radio group carry different on-state names — that is
/// how `/V` names the chosen one.
///
/// A field's controls share one [`ToggleState`] here, so the per-control `/AS`
/// that walk writes cannot be stored control by control. What *is* storable is
/// **which** control is the checked one, and that is what this records: a
/// caller reading [`ToggleState::checked_control`] can tell the chosen kid
/// from its siblings, where before the two were indistinguishable.
///
/// # What this still cannot do, and why it is recorded rather than faked
///
/// # And how the difference is drawn
///
/// A toggle's appearance is its `/AS` state, which the generator used to read
/// only from the **widget dictionary** — so a session could record the chosen
/// kid and not show it, which is how this landed in M14's OWED list. It now
/// reads [`ap::widget::LiveInput::appearance_state`] first, and `generate`
/// fills that in from [`ToggleState::state_for_control`]: the chosen kid its
/// own on-state name, every sibling `Off`, and a group nothing has clicked
/// `None`, which is the file's own `/AS` unchanged.
///
/// Recording the chosen control was what made that a one-line change when the
/// seam arrived, rather than a second state model — and it was.
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
    if let Some(FieldState::Toggle(state)) = session.fields.get_mut(&field) {
        state.checked_control = Some(chosen);
    }
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
    let mut edit = with_font(ctx, widget, |font, _substitute| {
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
    });
    // `CFFL_TextField::GetCreateParam` (`cffl_textfield.cpp:54-63`) raises
    // `kEditAutoScroll` for a text field without `DoNotScroll`, multi-line or
    // not, and `CPWL_Edit::OnCreated` (`cpwl_edit.cpp:131`) hands it to the
    // control. This is the one place that knows the flag.
    edit.auto_scroll = config.auto_scroll;
    edit
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

/// Runs `body` with the face a widget's `/DA` names, and the **second face**
/// for the characters that face's charset cannot write.
///
/// Answers `None` only when the form declares no font at all, which is a
/// document with no `/DR` and no fallback — there is no metric to lay text
/// out with, so the caller declines rather than inventing one.
///
/// # Why the width closure has to know about the second face
///
/// This is the same construction `ap::generate_appearances_with_text` makes
/// for the *stored* path, and it is made here rather than borrowed because
/// the closure has to outlive the [`TextFont`] that borrows it.
///
/// The point worth restating is that the substitute enters through the
/// **width closure** and not only through the encoder. A run set in two faces
/// advances by two faces' metrics; measuring it all with the first gives a
/// line the wrong length wherever the second one writes — which is exactly
/// how a Hebrew selection band came to end ten units short of the glyphs it
/// was supposed to cover. `CPDF_BAFontMap::GetWordFontIndex`
/// (`core/fpdfdoc/cpdf_bafontmap.cpp:116-151`) asks the same question per
/// character and knows nothing about whether the character was typed or
/// stored, so the typed path takes the same answer.
fn with_font<R: Resolve, T>(
    ctx: &Context<'_, R>,
    widget: &WidgetInfo,
    body: impl FnOnce(&TextFont<'_>, Option<ap::Substitute<'_>>) -> T,
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
    let da_charset = ap::font_map::font_charset(font);
    let substitute = ap::font_map::SUBSTITUTABLE_CHARSETS
        .iter()
        .find(|charset| **charset != da_charset)
        .and_then(|charset| ctx.fonts.substitute(*charset));
    let width = move |code: u32| match substitute {
        Some(sub) if !ap::font_map::da_font_writes(font, da_charset, code) => {
            ap::font_map::substitute_width(sub.font, code)
        }
        _ => TextFont::char_width(font, code),
    };
    let text_font = TextFont {
        metrics: TextFont::metrics_of(font, &width),
        font,
    };
    Some(body(&text_font, substitute))
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
    with_font(ctx, &widget, |font, _substitute| {
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
    with_font(ctx, &widget, |font, _substitute| {
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
    // A radio group's kids each carry a different on-state name, and a click
    // on one sets that kid's `/AS` to its own name and every sibling's to
    // `Off`. A session holds one record per *field*, so this is the per-kid
    // half of `CheckControl` the record can express — see
    // `ToggleState::state_for_control`, and `clear_siblings` for what puts
    // the chosen control there. `None` means "read the widget's own `/AS`",
    // which is every widget nothing has clicked.
    let as_override = match state {
        FieldState::Toggle(toggle) => toggle.state_for_control(widget.id),
        FieldState::Text(_) | FieldState::Choice(_) | FieldState::Button(_) => None,
    };
    // `LiveInput` is **not** `#[non_exhaustive]`, so this literal has to name
    // every field and a new one upstream is a compile error here rather than a
    // silent default. That is a real cost paid once already — `6e87424`'s
    // `appearance_state` broke this construction site and left `pdfrum-form`
    // failing to build for a period — and the fix is not to spell the literal
    // differently but to mark the struct: whoever adds a sixth field should
    // put `#[non_exhaustive]` on it first, and give it a `Default` so callers
    // outside `pdfrum-doc` can still build one.
    with_font(ctx, widget, |font, substitute| {
        ap::widget::generate_with_live_faces(
            &widget.dict,
            ctx.catalog,
            font,
            ctx.resolve,
            ap::widget::LiveInput {
                caret_and_selection: highlight.as_ref(),
                live: live.as_ref(),
                substitute,
                appearance_state: as_override.map(str::as_bytes),
            },
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
/// The whole table, each row from the control's own `GetFocusRect`, which is
/// what `CFFL_FormField::GetFocusBox` (`cffl_formfield.cpp:480-489`) asks:
///
/// | control | `GetFocusRect` | here |
/// |---|---|---|
/// | text field (`cpwl_edit.cpp:313-315`) | empty | [`ap::FocusBox::None`] |
/// | **any** combo box (`cpwl_combo_box.cpp:321-323`) | empty | [`ap::FocusBox::None`] |
/// | multi-select list (`cpwl_list_box.cpp:227-234`) | the caret item ∩ client | its caret row |
/// | single-select list, check box, radio (`cpwl_wnd.cpp:713-719`) | window inflated by 1 | [`ap::FocusBox::Inflated`] |
/// | push button (`cpwl_special_button.cpp:21-24`) | window **deflated by the border** | [`ap::FocusBox::Rect`] of that box |
///
/// Two rows are easy to get wrong in the same direction, by reaching for the
/// generic `CPWL_Wnd` answer where a subclass overrides it. A combo box
/// returns an empty rectangle **whatever** its custom-text flag says — the
/// override takes no branch at all, so the editable and gated cases are one
/// row, not two. And a push button deflates where the generic answer
/// inflates, which is the opposite sign on the same number.
///
/// So "focused" is mostly a *negative* instruction: it suppresses the tint,
/// and only three of the five controls stroke anything in its place. That is
/// what the four `form_textfield_focused_*` goldens carry — a caret and
/// glyphs over plain white, with no outline of any kind.
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
        // A text field strokes nothing. A field with no state yet has no
        // control to ask, so it strokes nothing either.
        Some(FieldState::Text(_)) | None => ap::FocusBox::None,
        // A combo box strokes nothing whether it is editable or gated:
        // `CPWL_ComboBox::GetFocusRect` returns an empty rectangle
        // unconditionally.
        Some(FieldState::Choice(choice)) if choice.config.combo => ap::FocusBox::None,
        // A single-select list box takes the window rectangle inflated by one
        // — the generic `CPWL_Wnd` answer, which it does not override.
        Some(FieldState::Choice(choice)) if !choice.config.multi_select => ap::FocusBox::Inflated,
        // A multi-select list box strokes its **caret row** rather than its
        // own edges, which is why its dashes trace a band inside the widget.
        Some(FieldState::Choice(choice)) => caret_row_box(ctx, annot, choice),
        // A check box and a radio button take the generic inflation.
        Some(FieldState::Toggle(_)) => ap::FocusBox::Inflated,
        // A push button deflates by its own border instead.
        Some(FieldState::Button(_)) => push_button_box(ctx, annot),
    };
    Some(ap::Focus { annot: index, box_ })
}

/// The rectangle a push button strokes: its window, deflated by the border.
///
/// `CPWL_PushButton::GetFocusRect` (`cpwl_special_button.cpp:21-24`) is
/// `GetWindowRect().GetDeflated(GetBorderWidth(), GetBorderWidth())`, and the
/// deflation is by the border on **each** side — the same `widget_border`
/// width `ap::field_body::client_rect` already reads. Unlike every other row
/// in the table this is a real rectangle rather than a rule, so it is
/// produced in page space, which is what [`ap::FocusBox::Rect`] carries.
fn push_button_box<R: Resolve>(ctx: &Context<'_, R>, annot: AnnotId) -> ap::FocusBox {
    let Some(widget) = ctx.widget(annot) else {
        return ap::FocusBox::None;
    };
    let width = f64::from(ap::widget::widget_border(&widget.dict, ctx.resolve).width);
    let rect = pdfrum_doc::geom::normalize(widget_rect(widget));
    // `CFX_FloatRect::GetDeflated` on a box narrower than twice its border
    // turns it inside out rather than emptying it, and `GetFocusBox` then
    // drops it for not being inside the page. Normalizing keeps the same
    // answer without a second rule.
    ap::FocusBox::Rect(pdfrum_doc::geom::normalize(kurbo::Rect::new(
        rect.x0 + width,
        rect.y0 + width,
        rect.x1 - width,
        rect.y1 - width,
    )))
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
    match state {
        FieldState::Text(text) => {
            let plate = ap::field_body::client_rect(&widget.dict, ctx.resolve);
            let config = text_config(ctx, widget, plate, &text.config);
            with_font(ctx, widget, |font, _substitute| {
                ops::highlight(
                    &text.edit,
                    &config,
                    &font.metrics,
                    ap::field_body::CARET_WIDTH,
                )
            })
        }
        // **An editable combo box has a caret and a selection band too**, and
        // for the same reason a text field does: its text half *is* a
        // `CPWL_Edit` (`cpwl_combo_box.cpp:190-203`), read-only only when the
        // box is gated (`:115-118`). Choosing a row leaves that edit holding
        // the row's label with everything selected, which is what draws the
        // navy band under `bug_736695_3`'s `Spain`; clicking into an empty
        // one leaves a caret, which is the 24-pixel bar at columns 166-167 of
        // `bug_736695_2`'s golden.
        //
        // A **gated** combo answers nothing: its edit is read-only and shows
        // neither, which is `CPWL_Edit::GetFocusRect` returning empty for
        // every combo and `SetCaret` forcing the caret invisible on one that
        // is not focused in its own right.
        FieldState::Choice(choice) if choice.config.editable => {
            let edit = choice.edit.as_deref()?;
            let client = ap::field_body::client_rect(&widget.dict, ctx.resolve);
            // The text sits left of the drop button, which is the plate
            // `with_combo_edit` laid it out in — the band has to be measured
            // in the same box or it lands a button's width off.
            let plate = kurbo::Rect::new(
                client.x0,
                client.y0,
                client.x1 - f64::from(DROP_BUTTON_WIDTH),
                client.y1,
            );
            let config = vt::Config {
                plate,
                font_size: font_size(ctx, widget),
                ..vt::Config::default()
            };
            with_font(ctx, widget, |font, _substitute| {
                ops::highlight(edit, &config, &font.metrics, ap::field_body::CARET_WIDTH)
            })
        }
        FieldState::Choice(_) | FieldState::Toggle(_) | FieldState::Button(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdfrum_object::{Name, NoResolve, Object, PdfString};

    /// A page carrying one `/Tx` widget whose `/DA` names `/Arial`, under a
    /// catalog whose `/AcroForm /DR /Font` declares that name as a bare
    /// non-embedded TrueType — `form_textfield_focused_ltr`'s shape, and the
    /// one that makes the substitution question arise at all.
    fn page_and_catalog() -> (Dict, Dict) {
        let font = Dict::from_pairs([
            (
                pdfrum_object::names::TYPE.clone(),
                Object::Name(Name::from_static(b"Font").clone()),
            ),
            (
                pdfrum_object::names::SUBTYPE.clone(),
                Object::Name(Name::from_static(b"TrueType").clone()),
            ),
            (
                Name::from_static(b"BaseFont").clone(),
                Object::Name(Name::from_static(b"Arial").clone()),
            ),
        ]);
        let catalog = Dict::from_pairs([(
            Name::from_static(b"AcroForm").clone(),
            Object::Dict(Dict::from_pairs([(
                Name::from_static(b"DR").clone(),
                Object::Dict(Dict::from_pairs([(
                    Name::from_static(b"Font").clone(),
                    Object::Dict(Dict::from_pairs([(
                        Name::from_static(b"Arial").clone(),
                        Object::Dict(font),
                    )])),
                )])),
            )])),
        )]);
        let widget = Dict::from_pairs([
            (
                pdfrum_object::names::TYPE.clone(),
                Object::Name(Name::from_static(b"Annot").clone()),
            ),
            (
                pdfrum_object::names::SUBTYPE.clone(),
                Object::Name(Name::from_static(b"Widget").clone()),
            ),
            (
                Name::from_static(b"FT").clone(),
                Object::Name(Name::from_static(b"Tx").clone()),
            ),
            (
                Name::from_static(b"T").clone(),
                Object::Str(PdfString::literal(*b"Text Box")),
            ),
            (
                Name::from_static(b"Rect").clone(),
                Object::Array(
                    [
                        Object::Int(50),
                        Object::Int(40),
                        Object::Int(150),
                        Object::Int(70),
                    ]
                    .into_iter()
                    .collect(),
                ),
            ),
            (
                Name::from_static(b"DA").clone(),
                Object::Str(PdfString::literal(*b"/Arial 12 Tf 0 0 0 rg")),
            ),
        ]);
        let page = Dict::from_pairs([(
            Name::from_static(b"Annots").clone(),
            Object::Array([Object::Dict(widget)].into_iter().collect()),
        )]);
        (page, catalog)
    }

    /// The width closure `with_font` hands the layout **measures a character
    /// the `/DA` font cannot write in the second face**, not in the `/DA`
    /// font's fallback.
    ///
    /// # The defect this pins, which the appearance-stream test cannot
    ///
    /// `8e45897` gave the typed path the second face, and the assertion it
    /// shipped with reads the emitted stream for `/_B1`. That catches a
    /// regression in the *encoder* and misses one in the **widths**: a
    /// version that puts the substitute on `LiveInput` and leaves the closure
    /// on the `/DA` font still writes `/_B1` and still emits the right bytes,
    /// while every advance, caret column and selection band comes out of the
    /// wrong table.
    ///
    /// That is not hypothetical — it is the F2 residue this crate carried:
    /// `form_textfield_selected_rtl`'s band ended at device column 101 with
    /// its glyphs running to 111, ten columns of dark where the oracle's are
    /// white, because Latin advances were measuring a Hebrew run.
    ///
    /// # What upstream does
    ///
    /// `CPWL_EditImpl::Provider::GetCharWidth`
    /// (`fpdfsdk/pwl/cpwl_edit_impl.cpp:124-137`) measures the face
    /// `GetWordFontIndex` (`core/fpdfdoc/cpdf_bafontmap.cpp:116-151`)
    /// selected for that character. Layout and encoding share the one index,
    /// so a character written in the second face is measured in it too.
    ///
    /// The numbers are the two faces' own and are asserted as a **relation**
    /// rather than as constants: which face stands in for `/Arial` depends on
    /// the substitution options, and the claim is that the two differ and
    /// that the closure takes the substitute's.
    #[test]
    fn the_width_closure_measures_an_unwritable_character_in_the_second_face() {
        // Bet, which an Ansi `/DA` font cannot write.
        const BET: u32 = 0x05D1;

        let (page, catalog) = page_and_catalog();
        let resolve = NoResolve;
        let form = crate::page::read(0, &page, &catalog, &resolve);
        let widget = form.widgets.first().expect("one /Tx widget");

        let mut build = pdfrum_page::BuildContext::new();
        let fonts = ap::FormFonts::load(&catalog, &resolve, &mut build);
        let ctx = Context {
            page: &form,
            catalog: &catalog,
            resolve: &resolve,
            fonts: &fonts,
            permissions: hit::Permissions::ALL,
        };

        let da = fonts.face(b"Arial").expect("the /DR declares one face");
        let substitute = fonts
            .substitute(pdfrum_font::subst::Charset::Hebrew)
            .expect("the Hebrew second face loads with no font directory at all");
        assert!(
            !ap::font_map::da_font_writes(da, ap::font_map::font_charset(da), BET),
            "the fixture's own font must be unable to write the character, \
             or the substitution never arises"
        );

        let (da_width, substitute_width) = (
            TextFont::char_width(da, BET),
            ap::font_map::substitute_width(substitute.font, BET),
        );
        assert_ne!(
            da_width, substitute_width,
            "the two faces must disagree, or this test cannot tell them apart"
        );

        let measured = with_font(&ctx, widget, |font, _substitute| {
            (
                (font.metrics.width)(BET),
                (font.metrics.width)(u32::from(b'a')),
            )
        })
        .expect("the widget's /DA resolves to a face");

        assert_eq!(
            measured.0, substitute_width,
            "a character the /DA font cannot write is measured in the second face"
        );
        assert_eq!(
            measured.1,
            TextFont::char_width(da, u32::from(b'a')),
            "and one it can is still measured in the /DA font"
        );
    }
}
