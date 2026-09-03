//! The `event` object: what a script reads to learn why it was run.
//!
//! # It is one object, re-`Initialize`d, never rebuilt
//!
//! A realm holds one `event`, and every trigger overwrites **every** field
//! before running its script. So a Validate script never sees the selection a
//! preceding Keystroke left, and a field a trigger does not set reads its
//! reset value rather than the last trigger's.
//!
//! # Twenty properties in four shapes
//!
//! The shape is the observable behaviour, and the goldens assert all four:
//!
//! - **read-only** — reading answers the field, writing throws
//!   `Operation not supported.` Thirteen of the twenty.
//! - **read/write** — `change`, `rc`, `value`, and the two selection indices,
//!   each with its own coercion and its own gate.
//! - **silently discarded** — the three `rich*` names read `undefined`,
//!   accept any assignment, and keep reading `undefined`.
//! - **throwing on read too** — `fieldFull` outside a Keystroke event, and
//!   `source`/`target` when there is no field to attach.
//!
//! # Why accessors rather than data properties
//!
//! A non-writable data property is a **silent** no-op on assignment in sloppy
//! mode, and every one of these goldens asserts a *thrown* message. Only an
//! accessor whose setter throws produces that, so each of the twenty is an
//! accessor pair.

use boa_engine::object::ObjectInitializer;
use boa_engine::property::Attribute;
use boa_engine::{Context, JsArgs, JsError, JsResult, JsValue};

use super::bind::{NOT_SUPPORTED, define_accessor, host, qualified, string_of};

/// The message a property answers when the object it names is gone.
const BAD_OBJECT: &str = "Object no longer exists.";

/// `JSMessage::kInvalidSetError` — what `event.value = true` answers.
const INVALID_SET: &str = "Set not possible, invalid or unknown.";

/// Which trigger built the current `event`.
///
/// The **kind is what decides the shape**, not just the values: `fieldFull`
/// throws unless the kind is Keystroke, the two selection indices read
/// `undefined` unless it is, and `type` — which gates `value` — is `Field`
/// for the ten field kinds and something else for the rest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum EventKind {
    /// No trigger has run: the realm's initial state.
    #[default]
    Unknown,
    /// `/AA /E` — the pointer entered the widget.
    MouseEnter,
    /// `/AA /X` — it left.
    MouseExit,
    /// `/AA /D` — a button went down over it.
    MouseDown,
    /// `/AA /U` — a button came up over it.
    MouseUp,
    /// `/AA /Fo` — it took the keyboard.
    Focus,
    /// `/AA /Bl` — it lost the keyboard.
    Blur,
    /// `/AA /K`, with or without a commit imminent.
    Keystroke,
    /// `/AA /V`.
    Validate,
    /// `/AA /C`.
    Calculate,
    /// `/AA /F`.
    Format,
}

impl EventKind {
    /// `event.name` — the string a script reads to branch on the trigger.
    ///
    /// The two mouse-button names carry a **space**, and that spelling is in
    /// the expected bytes.
    pub(crate) fn name(self) -> &'static str {
        match self {
            EventKind::Unknown => "",
            EventKind::MouseEnter => "Mouse Enter",
            EventKind::MouseExit => "Mouse Exit",
            EventKind::MouseDown => "Mouse Down",
            EventKind::MouseUp => "Mouse Up",
            EventKind::Focus => "Focus",
            EventKind::Blur => "Blur",
            EventKind::Keystroke => "Keystroke",
            EventKind::Validate => "Validate",
            EventKind::Calculate => "Calculate",
            EventKind::Format => "Format",
        }
    }

    /// `event.type` — `Field` for every trigger a widget's `/AA` can carry.
    ///
    /// The document- and page-level kinds answer `Doc` and `Page`, and this
    /// crate fires neither, so the only two answers reachable here are
    /// `Field` and the empty string.
    pub(crate) fn kind_type(self) -> &'static str {
        match self {
            EventKind::Unknown => "",
            _ => "Field",
        }
    }

    /// Whether a script provoked by this trigger counts as a user gesture.
    ///
    /// Three kinds do, and `Doc.submitForm`/`Doc.print` are gated on it — a
    /// mouse *enter* cannot submit a form and a mouse *down* can.
    pub(crate) fn is_user_gesture(self) -> bool {
        matches!(
            self,
            EventKind::MouseDown | EventKind::MouseUp | EventKind::Keystroke
        )
    }
}

/// The live `event`, as plain data the accessors read and write.
///
/// Every field is reset on every trigger, which is `Initialize`'s whole job;
/// [`EventState::initialize`] is that function and the only way to start a
/// trigger.
#[allow(
    clippy::struct_excessive_bools,
    reason = "each flag is one JavaScript property a script reads — \
              `event.keyDown`, `event.modifier`, `event.shift`, \
              `event.willCommit`, `event.fieldFull`, `event.rc`, and the \
              liveness of `event.value`. They are the `event` object's own \
              surface, not a state machine, and folding any pair into an \
              enum would put a name between the accessor and the value it \
              answers."
)]
#[derive(Debug, Clone, Default)]
pub(crate) struct EventState {
    /// Which trigger. Decides the shape of half the properties.
    pub(crate) kind: EventKind,
    /// `event.targetName` — the target field's fully-qualified name.
    pub(crate) target_name: String,
    /// `event.source`'s name. **Only Calculate sets it**, so everywhere else
    /// `event.source` attaches to the empty name.
    pub(crate) source_name: String,
    /// `event.value` — the field's value, or the text a formatter rewrites.
    pub(crate) value: String,
    /// Whether `value` is live at all. `false` makes both halves of
    /// `event.value` throw `Object no longer exists.`, which is what the six
    /// mouse and focus kinds do.
    pub(crate) has_value: bool,
    /// `event.change` — the text being inserted.
    pub(crate) change: String,
    /// `event.changeEx` — read-only, and only Keystroke and Validate set it.
    pub(crate) change_ex: String,
    /// `event.commitKey` — `-1` reset, `0` for Keystroke and Format.
    pub(crate) commit_key: i32,
    /// `event.keyDown`.
    pub(crate) key_down: bool,
    /// `event.modifier` — whether a modifier key was held.
    pub(crate) modifier: bool,
    /// `event.shift`.
    pub(crate) shift: bool,
    /// `event.selStart`, live only for a Keystroke.
    pub(crate) sel_start: i32,
    /// `event.selEnd`, live only for a Keystroke.
    pub(crate) sel_end: i32,
    /// `event.willCommit`.
    pub(crate) will_commit: bool,
    /// `event.fieldFull` — reading it throws outside a Keystroke.
    pub(crate) field_full: bool,
    /// `event.rc` — the accept flag a Keystroke, Validate or Calculate
    /// script clears.
    ///
    /// **Resets to `false`, not `true`**, and only those three kinds point it
    /// at a live flag: upstream's `rc_du_` is the dummy the getter falls back
    /// to when no caller passed a `bool*`, and `Initialize` sets it false. So
    /// a Format script reads `event.rc == false` before touching it, which
    /// `event_properties_expected.txt` asserts on its own first `rc` line —
    /// and the three kinds that *do* read it back start from `true`, because
    /// their caller's own flag does.
    pub(crate) rc: bool,
    /// Where the target field sits in the document's `/Fields` list, so
    /// `event.target` can be a real `Field` object.
    pub(crate) target_index: Option<u32>,
    /// The same for `event.source`.
    pub(crate) source_index: Option<u32>,
}

impl EventState {
    /// `CJS_EventContext::Initialize`: every field back to its reset value,
    /// then the kind.
    ///
    /// **`commit_key` resets to `-1`, not `0`**, and `rc` resets to `false`
    /// for every kind whose caller does not read it back.
    pub(crate) fn initialize(kind: EventKind) -> EventState {
        EventState {
            kind,
            target_name: String::new(),
            source_name: String::new(),
            value: String::new(),
            has_value: false,
            change: String::new(),
            change_ex: String::new(),
            commit_key: -1,
            key_down: false,
            modifier: false,
            shift: false,
            sel_start: 0,
            sel_end: 0,
            will_commit: false,
            field_full: false,
            // The three kinds whose caller reads `rc` back overwrite this
            // with `true`; for the rest it is upstream's dummy, which
            // `Initialize` leaves false.
            rc: matches!(
                kind,
                EventKind::Keystroke | EventKind::Validate | EventKind::Calculate
            ),
            target_index: None,
            source_index: None,
        }
    }
}

/// The live event, or a default one for a realm nothing has triggered.
fn read<T>(context: &Context, f: impl FnOnce(&EventState) -> T) -> Option<T> {
    let host = host(context)?;
    let state = host.borrow();
    Some(f(&state.event))
}

/// Mutates the live event.
fn write(context: &Context, f: impl FnOnce(&mut EventState)) {
    if let Some(host) = host(context) {
        f(&mut host.borrow_mut().event);
    }
}

/// The error every read-only setter throws, qualified by its property name.
fn read_only(member: &str) -> JsError {
    qualified(&format!("event.{member}"), NOT_SUPPORTED)
}

/// A read-only property's setter: one line, its own name.
macro_rules! declined {
    ($fn_name:ident, $member:literal) => {
        fn $fn_name(_t: &JsValue, _a: &[JsValue], _c: &mut Context) -> JsResult<JsValue> {
            Err(read_only($member))
        }
    };
}

declined!(no_change_ex, "changeEx");
declined!(no_commit_key, "commitKey");
declined!(no_field_full, "fieldFull");
declined!(no_key_down, "keyDown");
declined!(no_modifier, "modifier");
declined!(no_name, "name");
declined!(no_shift, "shift");
declined!(no_source, "source");
declined!(no_target, "target");
declined!(no_target_name, "targetName");
declined!(no_type, "type");
declined!(no_will_commit, "willCommit");

/// A read-only getter over one plain field of the state.
macro_rules! plain {
    ($fn_name:ident, $slot:ident, $wrap:expr) => {
        #[allow(clippy::redundant_closure_call)]
        fn $fn_name(_t: &JsValue, _a: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
            let value = read(context, |event| event.$slot.clone()).unwrap_or_default();
            Ok(($wrap)(value))
        }
    };
}

plain!(get_change_ex, change_ex, |v: String| JsValue::from(
    boa_engine::js_string!(v)
));
plain!(get_commit_key, commit_key, JsValue::from);
plain!(get_key_down, key_down, JsValue::from);
plain!(get_modifier, modifier, JsValue::from);
plain!(get_shift, shift, JsValue::from);
plain!(get_will_commit, will_commit, JsValue::from);
plain!(get_target_name, target_name, |v: String| JsValue::from(
    boa_engine::js_string!(v)
));

/// `event.name`, which is the kind's own spelling.
#[allow(clippy::unnecessary_wraps)]
fn get_name(_t: &JsValue, _a: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let name = read(context, |event| event.kind.name()).unwrap_or("");
    Ok(JsValue::from(boa_engine::js_string!(name)))
}

/// `event.type` — `Field` for every trigger this crate fires.
#[allow(clippy::unnecessary_wraps)]
fn get_type(_t: &JsValue, _a: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let kind = read(context, |event| event.kind.kind_type()).unwrap_or("");
    Ok(JsValue::from(boa_engine::js_string!(kind)))
}

/// `event.change` — read/write, and the setter **ignores a non-string**.
///
/// `event.change = 3` succeeds, yields 3 to the assignment expression, and
/// leaves the change as it was: the setter's whole body is guarded by
/// `if (vp->IsString())`.
#[allow(clippy::unnecessary_wraps)]
fn get_change(_t: &JsValue, _a: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let change = read(context, |event| event.change.clone()).unwrap_or_default();
    Ok(JsValue::from(boa_engine::js_string!(change)))
}

fn set_change(_t: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let value = args.get_or_undefined(0).clone();
    if value.is_string() {
        let text = string_of(&value, context)?;
        write(context, |event| event.change = text);
    }
    Ok(JsValue::undefined())
}

/// `event.rc` — read/write, and the setter **coerces to truthiness**.
///
/// `event.rc = 'boo'` yields the string to the assignment expression and
/// reads back `true`, because the slot is a `bool`.
#[allow(clippy::unnecessary_wraps)]
fn get_rc(_t: &JsValue, _a: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::from(
        read(context, |event| event.rc).unwrap_or(false),
    ))
}

#[allow(clippy::unnecessary_wraps)]
fn set_rc(_t: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let value = args.get_or_undefined(0).to_boolean();
    write(context, |event| event.rc = value);
    Ok(JsValue::undefined())
}

/// The three `rich*` names: `undefined` on read, accepted and dropped on
/// write.
///
/// Upstream's getter and setter are both a bare `return Success();` — the
/// getter's empty result is `undefined`, and the setter records nothing. So
/// `event.richValue = 'boo'` yields `'boo'` to the assignment expression, as
/// every assignment does, and `event.richValue` still reads `undefined`.
#[allow(clippy::unnecessary_wraps)]
fn rich_noop(_t: &JsValue, _a: &[JsValue], _c: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::undefined())
}

/// `event.fieldFull` — **the read throws outside a Keystroke**.
///
/// The message is upstream's own bare `unrecognized event`, which is not one
/// of the `JSMessage` table's and is quoted verbatim by the golden.
fn get_field_full(_t: &JsValue, _a: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let Some((kind, full)) = read(context, |event| (event.kind, event.field_full)) else {
        return Err(qualified("event.fieldFull", "unrecognized event"));
    };
    if kind != EventKind::Keystroke {
        return Err(qualified("event.fieldFull", "unrecognized event"));
    }
    Ok(JsValue::from(full))
}

/// `event.selStart` and `event.selEnd` — live for a Keystroke, `undefined`
/// otherwise, and **the setter is silently dropped** outside one.
macro_rules! selection {
    ($get:ident, $set:ident, $slot:ident) => {
        #[allow(clippy::unnecessary_wraps)]
        fn $get(_t: &JsValue, _a: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
            let Some((kind, value)) = read(context, |event| (event.kind, event.$slot)) else {
                return Ok(JsValue::undefined());
            };
            if kind != EventKind::Keystroke {
                return Ok(JsValue::undefined());
            }
            Ok(JsValue::from(value))
        }

        fn $set(_t: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
            let kind = read(context, |event| event.kind).unwrap_or_default();
            if kind == EventKind::Keystroke {
                let value = args.get_or_undefined(0).to_i32(context)?;
                write(context, |event| event.$slot = value);
            }
            Ok(JsValue::undefined())
        }
    };
}

selection!(get_sel_start, set_sel_start, sel_start);
selection!(get_sel_end, set_sel_end, sel_end);

/// `event.value` — read/write, behind **two gates and a type check**.
///
/// The type must be `Field`, the value must be live, and an assignment of
/// `null`, `undefined` or a boolean is refused with
/// `Set not possible, invalid or unknown.` A number is *not* refused: it is
/// stringified, which is why `event.value = 2` reads back `2`.
fn get_value(_t: &JsValue, _a: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let Some((kind, has_value, value)) = read(context, |event| {
        (event.kind, event.has_value, event.value.clone())
    }) else {
        return Err(qualified("event.value", BAD_OBJECT));
    };
    if kind.kind_type() != "Field" {
        return Err(qualified("event.value", "Bad event type."));
    }
    if !has_value {
        return Err(qualified("event.value", BAD_OBJECT));
    }
    Ok(JsValue::from(boa_engine::js_string!(value)))
}

fn set_value(_t: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let Some((kind, has_value)) = read(context, |event| (event.kind, event.has_value)) else {
        return Err(qualified("event.value", BAD_OBJECT));
    };
    if kind.kind_type() != "Field" {
        return Err(qualified("event.value", "Bad event type."));
    }
    if !has_value {
        return Err(qualified("event.value", BAD_OBJECT));
    }
    let incoming = args.get_or_undefined(0).clone();
    if incoming.is_null_or_undefined() || incoming.is_boolean() {
        return Err(qualified("event.value", INVALID_SET));
    }
    let text = string_of(&incoming, context)?;
    write(context, |event| event.value = text);
    Ok(JsValue::undefined())
}

/// `event.target` and `event.source` — a live `Field` object each.
///
/// Both attach to a **name**, so a kind that set no source name attaches to
/// the empty one — which is an object rather than an error, because the
/// empty name reaches the form's root node. That is the oracle's shape, and
/// it is why `event.source` stringifies as `[object Object]` on a Format
/// event that never had a source.
macro_rules! field_of {
    ($fn_name:ident, $name_slot:ident, $index_slot:ident, $member:literal) => {
        fn $fn_name(_t: &JsValue, _a: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
            let Some((name, index)) = read(context, |event| {
                (event.$name_slot.clone(), event.$index_slot)
            }) else {
                return Err(qualified(concat!("event.", $member), BAD_OBJECT));
            };
            let index = index.and_then(|i| usize::try_from(i).ok()).unwrap_or(0);
            super::field::build(index, &name, context).map(JsValue::from)
        }
    };
}

field_of!(get_target, target_name, target_index, "target");
field_of!(get_source, source_name, source_index, "source");

/// Installs `event` as a global, with all twenty accessors.
///
/// The object exists from the start of the realm rather than being created by
/// the first trigger, because a document-open script may read `event.name`
/// before any field has fired — and it reads the empty string, not a
/// `ReferenceError`.
pub(crate) fn install(context: &mut Context) -> JsResult<()> {
    let event = ObjectInitializer::new(context).build();
    let properties: [(&str, super::af::Bound, super::af::Bound); 20] = [
        ("change", get_change, set_change),
        ("changeEx", get_change_ex, no_change_ex),
        ("commitKey", get_commit_key, no_commit_key),
        ("fieldFull", get_field_full, no_field_full),
        ("keyDown", get_key_down, no_key_down),
        ("modifier", get_modifier, no_modifier),
        ("name", get_name, no_name),
        ("rc", get_rc, set_rc),
        ("richChange", rich_noop, rich_noop),
        ("richChangeEx", rich_noop, rich_noop),
        ("richValue", rich_noop, rich_noop),
        ("selEnd", get_sel_end, set_sel_end),
        ("selStart", get_sel_start, set_sel_start),
        ("shift", get_shift, no_shift),
        ("source", get_source, no_source),
        ("target", get_target, no_target),
        ("targetName", get_target_name, no_target_name),
        ("type", get_type, no_type),
        ("value", get_value, set_value),
        ("willCommit", get_will_commit, no_will_commit),
    ];
    for (name, get, set) in properties {
        define_accessor(&event, context, name, get, set)?;
    }
    context.register_global_property(
        boa_engine::js_string!("event"),
        JsValue::from(event),
        Attribute::all(),
    )?;
    Ok(())
}
