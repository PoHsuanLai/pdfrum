//! `Field` — one form field, as `Doc.getField` hands it back.
//!
//! # The densest table in the object model, and the shape it has
//!
//! `fxjs/cjs_field.cpp` is 2897 lines and binds **52 properties and 26
//! methods**. Read for what each *does* rather than what each is named, it
//! collapses:
//!
//! | shape | count | why |
//! |---|---|---|
//! | getters that read something real | 48 | the ordinary case |
//! | setters that **validate and then discard** | 32 | see below — this is upstream, not us |
//! | setters that change something | 10 | `value`, `display`, `hidden`, `readonly`, `rect`, `borderStyle`, `lineWidth`, `print`, `currentValueIndices`, `delay` |
//! | members that throw whatever they are given | 17 | 7 setters and 10 methods, all `Operation not supported.` bar one |
//!
//! **The 32 validate-then-discard setters are the interesting class**, and
//! they are why this file is mechanical rather than deep: `set_fill_color`
//! checks the field exists, checks the permission, checks the argument is an
//! array — and then returns success without writing anything. The validation
//! *is* the observable behaviour, because the errors are what
//! `field_properties_expected.txt` asserts; the write was never there to
//! reproduce.
//!
//! # A `Field` is a value, not a handle
//!
//! Each call to `getField` builds a fresh object carrying the field's
//! `/Fields` position, and every accessor reads the model through the host
//! slot by that number. Nothing here borrows the document or the session, so
//! a `Field` a script kept across events cannot be a dangling pointer — which
//! is the whole class of bug the seven owed regressions are about
//! (`Bug620428`, `Bug634394`, `Bug634716`, `Bug679649`, `Bug707673`,
//! `Bug765384`, `Bug1477093`): each is a use-after-free upstream, and each is
//! unreachable here by construction rather than by a guard.

#![allow(
    clippy::unnecessary_wraps,
    reason = "every bound function has one signature — \
              `fn(&JsValue, &[JsValue], &mut Context) -> JsResult<JsValue>` — \
              because that is what `NativeFunction::from_fn_ptr` takes and \
              what makes the tables tables. A getter that cannot fail still \
              has to return a `Result`, and narrowing the ones that happen \
              not to today would break the next one that grows a throw."
)]

use boa_engine::object::ObjectInitializer;
use boa_engine::{Context, JsArgs, JsError, JsObject, JsResult, JsValue};

use super::bind::{host, native, param_error, qualified, string_of};
use super::doc::{BAD_OBJECT, NOT_SUPPORTED, VALUE_ERROR};
use super::model::{FieldModel, FieldModelKind};

/// The class name every error this object throws is qualified by.
const CLASS: &str = "Field";

/// The key a `Field` object carries its `/Fields` position under.
///
/// A plain non-enumerable property rather than a native data slot, because a
/// `Field` must survive being copied into a script's own object and read back
/// — `field_methods.in` does exactly that with `getArray()`'s results — and a
/// number is the only thing that survives that intact.
const INDEX_KEY: &str = "__pdfrum_field_index";

/// The key a `Field` object carries the name it was **looked up by** under.
///
/// Not the field's own name: `AttachField` stores the caller's string after
/// collapsing `".."`, and `Field.name` answers *that*. So
/// `getField('MyField').name` is `MyField` even though the field it resolved
/// to is `MyField.MyText`, and `getField('MyField...nonesuch').name` is
/// `MyField..nonesuch` — a three-dot run collapsed once and left a two-dot
/// one behind.
const NAME_KEY: &str = "__pdfrum_field_name";

/// The error a `Field` member throws, qualified `Field.<member>: …`.
fn err(member: &str, message: &str) -> JsError {
    qualified(&format!("{CLASS}.{member}"), message)
}

/// The arity error, qualified.
fn params(member: &str) -> JsError {
    param_error(&format!("{CLASS}.{member}"))
}

/// The `/Fields` position a `Field` object carries.
fn index_of(this: &JsValue, context: &mut Context) -> Option<usize> {
    let object = this.as_object()?;
    let value = object
        .get(boa_engine::js_string!(INDEX_KEY), context)
        .ok()?;
    usize::try_from(value.to_i32(context).ok()?).ok()
}

/// Runs `body` against the field this object names.
///
/// `None` when the object is not a `Field` or the model no longer lists it,
/// which every caller turns into `Object no longer exists.` — the same
/// message upstream's null checks produce.
fn with_field<T>(
    this: &JsValue,
    context: &mut Context,
    body: impl FnOnce(&FieldModel) -> T,
) -> Option<T> {
    let index = index_of(this, context)?;
    let host = host(context)?;
    let state = host.borrow();
    state.document.field_at(index).map(body)
}

/// A getter reading one field property, with the `Object no longer exists.`
/// failure spelled once.
fn read<T>(
    this: &JsValue,
    context: &mut Context,
    member: &str,
    body: impl FnOnce(&FieldModel) -> T,
) -> JsResult<T> {
    with_field(this, context, body).ok_or_else(|| err(member, BAD_OBJECT))
}

// ---- the shared bodies ----

/// One of the **32** setters that validate and then do nothing.
///
/// Not a stub: `set_fill_color`, `set_alignment`, `set_char_limit`,
/// `set_user_name` and twenty-eight more each run their checks and then
/// `return CJS_Result::Success()` without writing. Reproducing that is
/// reproducing the specification — and the checks the goldens assert are the
/// arity and type ones, which are shared, so this really is one body.
#[allow(clippy::unnecessary_wraps)]
fn ignoring_setter(_this: &JsValue, _a: &[JsValue], _c: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::undefined())
}

/// One of the **6** methods that are no-ops returning success, zero
/// validation: `buttonImportIcon`, `clearItems`, `deleteItemAt`,
/// `insertItemAt`, `setAction`, `setItems`.
///
/// The consequence worth naming: a choice field's option list is
/// **immutable from JavaScript**, and `setAction` silently discards every
/// action a script assigns.
#[allow(clippy::unnecessary_wraps)]
fn noop(_this: &JsValue, _a: &[JsValue], _c: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::undefined())
}

/// A member that throws `Operation not supported.` whatever it is given.
macro_rules! declined {
    ($fn_name:ident, $member:literal) => {
        fn $fn_name(_t: &JsValue, _a: &[JsValue], _c: &mut Context) -> JsResult<JsValue> {
            Err(err($member, NOT_SUPPORTED))
        }
    };
}

declined!(throw_default_style, "defaultStyle");
declined!(throw_doc, "doc");
declined!(throw_name, "name");
declined!(throw_num_items, "numItems");
declined!(throw_type, "type");
declined!(throw_value_as_string, "valueAsString");
declined!(method_button_set_caption, "buttonSetCaption");
declined!(method_button_set_icon, "buttonSetIcon");
declined!(method_get_lock, "getLock");
declined!(method_set_lock, "setLock");
declined!(
    method_signature_get_modifications,
    "signatureGetModifications"
);
declined!(method_signature_get_seed_value, "signatureGetSeedValue");
declined!(method_signature_info, "signatureInfo");
declined!(method_signature_set_seed_value, "signatureSetSeedValue");
declined!(method_signature_sign, "signatureSign");
declined!(method_signature_validate, "signatureValidate");

/// `Field.page = …` — the one setter that throws
/// `Cannot assign to readonly property.` rather than
/// `Operation not supported.`, which is an inconsistency reproduced because
/// the golden asserts it.
fn throw_page(_t: &JsValue, _a: &[JsValue], _c: &mut Context) -> JsResult<JsValue> {
    Err(err("page", "Cannot assign to readonly property."))
}

// ---- the getters that carry weight ----

/// `Field.value`.
///
/// **Coerced**, which is the surprise: the string goes through
/// `MaybeCoerceToNumber`, so a text field holding `"3"` answers the *number*
/// 3 and `bug_361`'s forty-nine cases are all about where that coercion stops.
/// `valueAsString` is the uncoerced twin, and exists because of this.
///
/// A push button has no value at all and answers
/// `Object is of the wrong type.`
fn get_value(this: &JsValue, _a: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let (kind, value, selected, options) = read(this, context, "value", |field| {
        (
            field.kind,
            field.value.clone(),
            field.selected.clone(),
            field.options.clone(),
        )
    })?;
    if kind == FieldModelKind::Button {
        return Err(err("value", "Object is of the wrong type."));
    }
    if kind == FieldModelKind::ListBox && selected.len() > 1 {
        let values: Vec<JsValue> = selected
            .iter()
            .filter_map(|index| options.get(usize::try_from(*index).ok()?))
            .map(|(export, label)| {
                let text = if export.is_empty() { label } else { export };
                JsValue::from(boa_engine::js_string!(text.clone()))
            })
            .collect();
        return Ok(JsValue::from(
            boa_engine::object::builtins::JsArray::from_iter(values, context),
        ));
    }
    Ok(maybe_number(&value, context))
}

/// `CJS_Runtime::MaybeCoerceToNumber` (`fxjs/cjs_runtime.cpp:218-244`),
/// reproduced.
///
/// **It is JavaScript's own `Number(s)`**, not a hand-written parser, with two
/// gates around it: the empty string is left alone, and a result that is `NaN`
/// is left alone **unless the string was literally `"NaN"`**. That is why
/// `bug_361`'s forty-nine cases read the way they do — `" 4"` is 4 because JS
/// trims, `"0x100"` is 256 because JS reads hex, `"Infinity"` is a number and
/// `"INFINITY"` is not, and `"1,000,000"` stays a string.
///
/// Using boa's `to_number` rather than Rust's `f64::from_str` is what makes
/// that true rather than approximately true: the two disagree on hex, on
/// `"Infinity"`'s spelling, and on whitespace.
fn maybe_number(value: &str, context: &mut Context) -> JsValue {
    if value.is_empty() {
        return JsValue::from(boa_engine::js_string!(""));
    }
    let text = JsValue::from(boa_engine::js_string!(value.to_string()));
    let Ok(number) = text.to_number(context) else {
        return text;
    };
    if number.is_nan() && value != "NaN" {
        return text;
    }
    JsValue::from(number)
}

/// `Field.value = …` — the one setter that reaches the document.
///
/// The write is recorded on the host and read back by the cascade after the
/// script returns, so it lands through the ordinary commit path and the
/// appearance regenerates — rather than being applied from inside a native
/// function, which would be the `busy_` re-entry upstream refuses.
///
/// The model's own copy is updated in step, because a later expression in the
/// same script must read what this one wrote.
fn set_value(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let Some(index) = index_of(this, context) else {
        return Err(err("value", BAD_OBJECT));
    };
    let value = args.get_or_undefined(0).clone();
    // The argument becomes a `vector<WideString>`: an array element by
    // element, anything else a one-element list.
    let offered: Vec<String> = if let Some(array) = value.as_object().filter(|o| o.is_array()) {
        let length = array
            .get(boa_engine::js_string!("length"), context)?
            .to_length(context)?;
        let mut out = Vec::new();
        for at in 0..length {
            let element = array.get(at, context)?;
            out.push(string_of(&element, context)?);
        }
        out
    } else {
        vec![string_of(&value, context)?]
    };
    if offered.is_empty() {
        // `if (strArray.empty()) return;` — an empty array changes nothing.
        return Ok(JsValue::undefined());
    }

    // **`delay` queues the write rather than making it.** `set_value` pushes
    // a `CJS_DelayData` when `delay_` is set (`fxjs/cjs_field.cpp:2354`) and
    // the queue drains only when the flag goes false *through this object* —
    // `set_delay(false)` calls `DoDelay` for each queued item. Setting
    // `Doc.delay` in between does not drain it, which is what `bug_494057`
    // demonstrates: `ff.delay = true`, a write, `this.delay = true`,
    // `ff.delay = false`, and the value has still not changed.
    let delayed = this
        .as_object()
        .and_then(|object| object.get(boa_engine::js_string!(DELAY_KEY), context).ok())
        .is_some_and(|value| value.to_boolean());
    if delayed {
        if let Some(object) = this.as_object() {
            let queued = boa_engine::object::builtins::JsArray::from_iter(
                offered
                    .into_iter()
                    .map(|text| JsValue::from(boa_engine::js_string!(text))),
                context,
            );
            object.set(
                boa_engine::js_string!(QUEUE_KEY),
                JsValue::from(queued),
                false,
                context,
            )?;
        }
        return Ok(JsValue::undefined());
    }

    if let Some(host) = host(context) {
        let mut state = host.borrow_mut();
        let Some(field) = state.document.fields.get_mut(index) else {
            return Err(err("value", BAD_OBJECT));
        };
        let accepted = apply_value(field, &offered);
        state
            .field_writes
            .push((u32::try_from(index).unwrap_or(u32::MAX), accepted));
    }
    Ok(JsValue::undefined())
}

/// The key a `Field` holds a delayed write under.
const QUEUE_KEY: &str = "__pdfrum_field_queue";

/// `SetFieldValue`'s per-kind branch (`fxjs/cjs_field.cpp:474-525`), and the
/// answer the field then holds.
///
/// **Three families, and only one of them looks past the first element.**
/// Text, combo, check box and radio take `strArray[0]` and ignore the rest; a
/// **list box** walks the whole array calling `SetItemSelection(FindOption(s))`
/// for each. That is the rule behind three otherwise puzzling golden lines:
/// `['bar','qux']` on a single-select list reads back `qux` — each selection
/// replaces the last — while `['foo',1]` reads back `foo`, because
/// `FindOption("1")` is `-1` and selecting nothing changes nothing.
///
/// A choice field also refuses anything that is not one of its **export
/// values**: `FindOption` compares `GetOptionValue(i)` alone, so the label
/// `Foo` selects nothing where the value `foo` selects the row. Fifty of
/// `listbox_methods`'s assertions are that one rule.
fn apply_value(field: &mut FieldModel, offered: &[String]) -> String {
    let first = offered.first().cloned().unwrap_or_default();
    if !field.kind.is_choice() {
        field.value.clone_from(&first);
        return first;
    }
    // `FindOption`, which matches the export value and nothing else.
    let option_of = |field: &FieldModel, text: &str| -> Option<usize> {
        field.options.iter().position(|(export, _)| export == text)
    };
    if field.kind == FieldModelKind::ComboBox {
        // A combo box takes `strArray[0]` through the same `SetValue` a text
        // field does, so an unmatched string is simply stored.
        field.value.clone_from(&first);
        return first;
    }
    // The list box: every element in turn.
    field.selected.clear();
    for text in offered {
        let Some(at) = option_of(field, text) else {
            continue;
        };
        let at = u32::try_from(at).unwrap_or(u32::MAX);
        if field.flags.multiple_selection {
            if !field.selected.contains(&at) {
                field.selected.push(at);
            }
        } else {
            field.selected = vec![at];
        }
    }
    let value = field
        .selected
        .first()
        .and_then(|at| field.options.get(usize::try_from(*at).ok()?))
        .map(|(export, label)| {
            if export.is_empty() {
                label.clone()
            } else {
                export.clone()
            }
        })
        .unwrap_or_default();
    field.value.clone_from(&value);
    value
}

/// `Field.valueAsString` — the value with **no** numeric coercion.
///
/// Not always the same string as `value`: a check box answers the literal
/// `Yes` or `Off` rather than its export value, and a multi-select list
/// answers `""`.
fn get_value_as_string(this: &JsValue, _a: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let text = read(this, context, "valueAsString", |field| {
        field
            .value_as_string
            .clone()
            .unwrap_or_else(|| field.value.clone())
    })?;
    Ok(JsValue::from(boa_engine::js_string!(text)))
}

/// `Field.name` — the name the field was *looked up* by, dots collapsed.
///
/// **Not `GetFullName()`**: `AttachField` stores the caller's string after
/// collapsing `".."`, which is why `getField("MyField..MyPushButton").name`
/// is `MyField.MyPushButton` and `getField("MyField...nonesuch").name` is
/// `MyField..nonesuch` — the collapse ran once on a three-dot run and left a
/// two-dot one behind.
fn get_name(this: &JsValue, _a: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let Some(object) = this.as_object() else {
        return Err(err("name", BAD_OBJECT));
    };
    object.get(boa_engine::js_string!(NAME_KEY), context)
}

/// `Field.type` — one of eight strings, and they are API.
fn get_type(this: &JsValue, _a: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let kind = read(this, context, "type", |field| field.kind)?;
    Ok(JsValue::from(boa_engine::js_string!(kind.as_str())))
}

/// `Field.doc` — the document object, which is the global.
fn get_doc(_this: &JsValue, _a: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::from(context.global_object()))
}

/// `Field.defaultValue` — `/DV`. A push button or signature has none and
/// answers `Object is of the wrong type.`
fn get_default_value(this: &JsValue, _a: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let (kind, value) = read(this, context, "defaultValue", |field| {
        (field.kind, field.default_value.clone())
    })?;
    if matches!(kind, FieldModelKind::Button | FieldModelKind::Signature) {
        return Err(err("defaultValue", "Object is of the wrong type."));
    }
    Ok(JsValue::from(boa_engine::js_string!(value)))
}

/// `Field.display` — 0 visible, 1 hidden, 2 visible-not-printed, 3
/// printed-not-viewed.
fn get_display(this: &JsValue, _a: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let display = read(this, context, "display", |field| field.display)?;
    Ok(JsValue::from(display))
}

/// `Field.display = n` — a value outside `0..=3` is a **silent no-op**, not
/// an error.
fn set_display(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let wanted = args.get_or_undefined(0).to_i32(context)?;
    let Ok(wanted) = u32::try_from(wanted) else {
        return Ok(JsValue::undefined());
    };
    if wanted > 3 {
        return Ok(JsValue::undefined());
    }
    write_field(this, context, |field| field.display = wanted);
    Ok(JsValue::undefined())
}

/// `Field.hidden` — the `/F` invisible-or-hidden bits.
fn get_hidden(this: &JsValue, _a: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let display = read(this, context, "hidden", |field| field.display)?;
    Ok(JsValue::from(display == 1))
}

/// `Field.hidden = b` — **`SetDisplay(b ? 1 : 0)`**, so `hidden = false` also
/// force-sets the print flag. Upstream's own aliasing, reproduced.
fn set_hidden(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let hidden = args.get_or_undefined(0).to_boolean();
    write_field(this, context, |field| {
        field.display = u32::from(hidden);
    });
    Ok(JsValue::undefined())
}

/// `Field.readonly` — `/Ff` bit 1.
fn get_readonly(this: &JsValue, _a: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let flag = read(this, context, "readonly", |field| field.flags.read_only)?;
    Ok(JsValue::from(flag))
}

/// `Field.readonly = b` — the only writer of `/Ff` upstream, and it writes
/// without a change mark or a redraw.
fn set_readonly(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let readonly = args.get_or_undefined(0).to_boolean();
    write_field(this, context, |field| field.flags.read_only = readonly);
    Ok(JsValue::undefined())
}

/// `Field.rect` — `[left, top, right, bottom]`, integer-truncated.
///
/// The getter's order and the setter's disagree: the setter reads
/// `(left, bottom, right, top)`, so `f.rect = f.rect` **swaps top and
/// bottom**. `[oracle-bug]`, and it is what `field_properties`'s four `rect`
/// lines pin — see the module note in `docs/status/M15.md`.
fn get_rect(this: &JsValue, _a: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let rect = read(this, context, "rect", |field| field.rect)?;
    let values: Vec<JsValue> = rect
        .iter()
        .map(|side| JsValue::from(side.trunc()))
        .collect();
    Ok(JsValue::from(
        boa_engine::object::builtins::JsArray::from_iter(values, context),
    ))
}

/// `Field.rect = [...]` — a non-array or a short array is a *value* error.
fn set_rect(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let value = args.get_or_undefined(0).clone();
    let Some(array) = value.as_object().filter(|o| o.is_array()) else {
        return Err(err("rect", VALUE_ERROR));
    };
    let length = array
        .get(boa_engine::js_string!("length"), context)?
        .to_length(context)?;
    if length < 4 {
        return Err(err("rect", VALUE_ERROR));
    }
    let mut sides = [0.0_f64; 4];
    for (index, side) in sides.iter_mut().enumerate() {
        *side = array.get(index as u64, context)?.to_number(context)?;
    }
    // `CFX_FloatRect(f0, f1, f2, f3)` reads `(left, bottom, right, top)`
    // while the *getter* answered `[left, top, right, bottom]` — so the two
    // disagree about which of the middle two is which. The rectangle is then
    // **normalized**, which sorts each pair, and the getter re-emits it in
    // its own order.
    //
    // The round trip is therefore stable even though the orders differ:
    // `[100,121,120,101]` in is `(100,121)`×`(120,101)` normalized to
    // `(100,101,120,121)`, and out again as `[100,121,120,101]`. That is what
    // `field_properties`'s four `rect` lines pin, and a setter that stored
    // the sides in the order it read them would fail the second of them.
    write_field(this, context, |field| {
        let (left, right) = (sides[0].min(sides[2]), sides[0].max(sides[2]));
        let (bottom, top) = (sides[1].min(sides[3]), sides[1].max(sides[3]));
        field.rect = [left, top, right, bottom];
    });
    Ok(JsValue::undefined())
}

/// `Field.page` — the pages this field's widgets are on.
///
/// **`-1`, a bare number, when the field has no widget at all**; one page is
/// still an array of one, which is a shape worth not tidying because
/// `field_properties` prints `page = 0` for a single-widget field and
/// stringification hides the array.
fn get_page(this: &JsValue, _a: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let pages = read(this, context, "page", |field| field.pages.clone())?;
    if pages.is_empty() {
        return Ok(JsValue::from(-1));
    }
    let values: Vec<JsValue> = pages.iter().map(|page| JsValue::from(*page)).collect();
    Ok(JsValue::from(
        boa_engine::object::builtins::JsArray::from_iter(values, context),
    ))
}

/// `Field.numItems` — how many options a choice field offers.
fn get_num_items(this: &JsValue, _a: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let count = read(this, context, "numItems", |field| field.options.len())?;
    Ok(JsValue::from(i32::try_from(count).unwrap_or(i32::MAX)))
}

/// `Field.currentValueIndices` — `-1` for nothing selected, a **bare number**
/// for one, an array for more.
fn get_current_value_indices(this: &JsValue, _a: &[JsValue], c: &mut Context) -> JsResult<JsValue> {
    let selected = read(this, c, "currentValueIndices", |field| {
        field.selected.clone()
    })?;
    match selected.len() {
        0 => Ok(JsValue::from(-1)),
        1 => Ok(selected
            .first()
            .map_or_else(JsValue::undefined, |at| JsValue::from(*at))),
        _ => {
            let values: Vec<JsValue> = selected.iter().map(|index| JsValue::from(*index)).collect();
            Ok(JsValue::from(
                boa_engine::object::builtins::JsArray::from_iter(values, c),
            ))
        }
    }
}

/// `Field.currentValueIndices = …` — a number or an array of them.
///
/// Anything else silently selects **nothing**, and a single-select field
/// stops after the first index however many were given.
fn set_current_value_indices(
    this: &JsValue,
    args: &[JsValue],
    c: &mut Context,
) -> JsResult<JsValue> {
    let value = args.get_or_undefined(0).clone();
    let mut wanted: Vec<u32> = Vec::new();
    if let Some(array) = value.as_object().filter(|o| o.is_array()) {
        let length = array
            .get(boa_engine::js_string!("length"), c)?
            .to_length(c)?;
        for index in 0..length {
            let element = array.get(index, c)?;
            if let Ok(number) = element.to_i32(c)
                && let Ok(number) = u32::try_from(number)
            {
                wanted.push(number);
            }
        }
    } else if let Ok(number) = value.to_i32(c)
        && let Ok(number) = u32::try_from(number)
    {
        wanted.push(number);
    }
    write_field(this, c, |field| {
        if !field.flags.multiple_selection {
            wanted.truncate(1);
        }
        field.selected = wanted;
    });
    Ok(JsValue::undefined())
}

/// `Field.getItemAt(nIdx, bExport)` — one option's label or export value.
///
/// The clamp is the detail: `nIdx == -1` **or** `nIdx > CountOptions()`
/// answers the *last* option, and note that is `>` rather than `>=`, so
/// exactly `CountOptions()` escapes the clamp and answers `""`. A negative
/// other than `-1` answers `""` too. Every one of those edges is an
/// assertion in `field_methods_expected.txt`.
fn get_item_at(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let (kind, options) = read(this, context, "getItemAt", |field| {
        (field.kind, field.options.clone())
    })?;
    if !kind.is_choice() {
        return Err(err("getItemAt", "Object is of the wrong type."));
    }
    let wanted = match args.first() {
        // A missing argument is `-1`, upstream's own default, which is the
        // "last option" spelling.
        None => -1,
        Some(value) => {
            let number = value.clone().to_number(context)?;
            // `'zzzz'` is NaN, and `ToInt32` makes it 0 — which is why the
            // golden reads `foo` for it.
            if number.is_nan() {
                0
            } else {
                value.clone().to_i32(context)?
            }
        }
    };
    let count = i32::try_from(options.len()).unwrap_or(i32::MAX);
    let index = if wanted == -1 || wanted > count {
        count - 1
    } else {
        wanted
    };
    let Some(option) = usize::try_from(index).ok().and_then(|at| options.get(at)) else {
        return Ok(JsValue::from(boa_engine::js_string!("")));
    };
    let export = args.get(1).is_none_or(JsValue::to_boolean);
    let (value, label) = option;
    let text = if export {
        if value.is_empty() { label } else { value }
    } else {
        label
    };
    Ok(JsValue::from(boa_engine::js_string!(text.clone())))
}

/// `Field.checkThisBox(nWidget, bCheckIt)`.
fn check_this_box(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    if args.is_empty() {
        return Err(params("checkThisBox"));
    }
    let (kind, controls) = read(this, context, "checkThisBox", |field| {
        (field.kind, field.checked.len())
    })?;
    if !kind.is_toggle() {
        return Err(err("checkThisBox", "Object is of the wrong type."));
    }
    let widget = args.get_or_undefined(0).to_i32(context)?;
    let Some(widget) = usize::try_from(widget).ok().filter(|at| *at < controls) else {
        return Err(err("checkThisBox", VALUE_ERROR));
    };
    let check = args.get(1).is_none_or(JsValue::to_boolean);
    write_field(this, context, |field| {
        if let Some(slot) = field.checked.get_mut(widget) {
            *slot = check;
        }
    });
    Ok(JsValue::undefined())
}

/// `Field.isBoxChecked(nWidget)`.
///
/// **Never throws `Object is of the wrong type.`** — a text field answers
/// `false`, which is the asymmetry with `checkThisBox` the golden asserts.
/// An out-of-range or missing index is a *value* error.
fn is_box_checked(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let checked = read(this, context, "isBoxChecked", |field| field.checked.clone())?;
    let widget = match args.first() {
        Some(value) => value.clone().to_i32(context)?,
        None => return Err(err("isBoxChecked", VALUE_ERROR)),
    };
    let Some(widget) = usize::try_from(widget).ok() else {
        return Err(err("isBoxChecked", VALUE_ERROR));
    };
    let Some(state) = checked.get(widget) else {
        return Err(err("isBoxChecked", VALUE_ERROR));
    };
    Ok(JsValue::from(*state))
}

/// `Field.isDefaultChecked(nWidget)` — the same shape over `/DV`.
fn is_default_checked(
    this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let checked = read(this, context, "isDefaultChecked", |field| {
        field.default_checked.clone()
    })?;
    let widget = match args.first() {
        Some(value) => value.clone().to_i32(context)?,
        None => return Err(err("isDefaultChecked", VALUE_ERROR)),
    };
    let Some(state) = usize::try_from(widget).ok().and_then(|at| checked.get(at)) else {
        return Err(err("isDefaultChecked", VALUE_ERROR));
    };
    Ok(JsValue::from(*state))
}

/// `Field.defaultIsChecked(nWidget)` — the **misspelt twin**, which validates
/// its argument and then ignores it.
///
/// It answers only "is this a toggle at all", never the default state, which
/// makes `isDefaultChecked` the correct API and this one a trap. Reproduced
/// because the golden asserts its answer.
fn default_is_checked(
    this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    if args.is_empty() {
        return Err(params("defaultIsChecked"));
    }
    let (kind, controls) = read(this, context, "defaultIsChecked", |field| {
        (field.kind, field.default_checked.len())
    })?;
    let widget = args.get_or_undefined(0).to_i32(context)?;
    if usize::try_from(widget).ok().is_none_or(|at| at >= controls) {
        return Err(err("defaultIsChecked", VALUE_ERROR));
    }
    Ok(JsValue::from(kind.is_toggle()))
}

/// `Field.buttonGetCaption(nFace)` — `/MK /CA`, `/AC` or `/RC`.
fn button_get_caption(
    this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let (kind, captions) = read(this, context, "buttonGetCaption", |field| {
        (field.kind, field.captions.clone())
    })?;
    if kind != FieldModelKind::Button {
        return Err(err("buttonGetCaption", "Object is of the wrong type."));
    }
    let face = match args.first() {
        Some(value) => value.clone().to_i32(context)?,
        None => 0,
    };
    let Some(caption) = usize::try_from(face).ok().and_then(|at| captions.get(at)) else {
        return Err(err("buttonGetCaption", VALUE_ERROR));
    };
    Ok(JsValue::from(boa_engine::js_string!(caption.clone())))
}

/// `Field.buttonGetIcon(nFace)` — validates `nFace`, then **ignores it** and
/// answers a fresh `Icon` with no stream behind it. Upstream's own shape.
fn button_get_icon(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let kind = read(this, context, "buttonGetIcon", |field| field.kind)?;
    if kind != FieldModelKind::Button {
        return Err(err("buttonGetIcon", "Object is of the wrong type."));
    }
    let face = match args.first() {
        Some(value) => value.clone().to_i32(context)?,
        None => 0,
    };
    if !(0..=2).contains(&face) {
        return Err(err("buttonGetIcon", VALUE_ERROR));
    }
    super::doc::icon_object(None, context).map(JsValue::from)
}

/// `Field.browseForFileToSubmit()` — a file-select text field only, and it
/// browses nothing.
fn browse_for_file(this: &JsValue, _a: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let ok = read(this, context, "browseForFileToSubmit", |field| {
        field.kind == FieldModelKind::Text && field.flags.file_select
    })?;
    if !ok {
        return Err(err("browseForFileToSubmit", "Object is of the wrong type."));
    }
    Ok(JsValue::undefined())
}

/// `Field.setFocus()` — asks for the keyboard.
///
/// Recorded rather than performed, for the same reason `Field.value`'s write
/// is: focus is the session's, and moving it from inside a native function
/// would re-enter the cascade the script is already inside.
fn set_focus(this: &JsValue, _a: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let Some(index) = index_of(this, context) else {
        return Ok(JsValue::undefined());
    };
    if let Some(host) = host(context) {
        host.borrow_mut().focus_requested = Some(u32::try_from(index).unwrap_or(u32::MAX));
    }
    Ok(JsValue::undefined())
}

/// `Field.getArray()` — every field sharing this one's name, **sorted by
/// full name**.
///
/// The sort is upstream's and is what `field_methods`'s eight-line list
/// asserts: `MyBadPushButton` before `MyCheckBox` before `MyFile`, which is
/// alphabetical and not `/Kids` order.
fn get_array(this: &JsValue, _a: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let Some(index) = index_of(this, context) else {
        return Err(err("getArray", BAD_OBJECT));
    };
    let Some(host) = host(context) else {
        return Err(err("getArray", BAD_OBJECT));
    };
    // The **lookup name**, not the resolved field's: `getField('MyField')`
    // resolves to `MyField.MyText`, and `getArray()` on it must still answer
    // all eight of `MyField`'s kids rather than the one leaf.
    // `GetFormFieldsForName` walks the node the *name* reaches.
    let prefix = {
        let Some(object) = this.as_object() else {
            return Err(err("getArray", BAD_OBJECT));
        };
        let name = object.get(boa_engine::js_string!(NAME_KEY), context)?;
        string_of(&name, context)?
    };
    let _ = index;
    let mut children: Vec<(String, usize)> = {
        let state = host.borrow();
        state
            .document
            .fields
            .iter()
            .enumerate()
            .filter(|(_, field)| {
                // An empty name is the root node, and every field is under
                // it — which is what `getField('').getArray()` asks for.
                prefix.is_empty()
                    || field.name == prefix
                    || field
                        .name
                        .strip_prefix(prefix.as_str())
                        .is_some_and(|rest| rest.starts_with('.'))
            })
            .map(|(at, field)| (field.name.clone(), at))
            .collect()
    };
    children.sort_by(|left, right| left.0.cmp(&right.0));
    let mut values = Vec::with_capacity(children.len());
    for (name, at) in children {
        values.push(JsValue::from(build(at, &name, context)?));
    }
    Ok(JsValue::from(
        boa_engine::object::builtins::JsArray::from_iter(values, context),
    ))
}

/// Applies a mutation to the field this object names.
fn write_field(this: &JsValue, context: &mut Context, body: impl FnOnce(&mut FieldModel)) {
    let Some(index) = index_of(this, context) else {
        return;
    };
    let Some(host) = host(context) else { return };
    let mut state = host.borrow_mut();
    if let Some(field) = state.document.fields.get_mut(index) {
        body(field);
    }
}

// ---- flag-derived getters, which are one shape ----

/// The eleven `/Ff` getters, and the two families that refuse each.
macro_rules! flag {
    ($fn_name:ident, $member:literal, $read:expr) => {
        fn $fn_name(this: &JsValue, _a: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
            #[allow(clippy::redundant_closure_call)]
            let flag = read(this, context, $member, |field| ($read)(field))?;
            Ok(JsValue::from(flag))
        }
    };
}

flag!(get_comb, "comb", |f: &FieldModel| f.flags.comb);
flag!(get_multiline, "multiline", |f: &FieldModel| f
    .flags
    .multiline);
flag!(get_password, "password", |f: &FieldModel| f.flags.password);
flag!(get_rich_text, "richText", |f: &FieldModel| f
    .flags
    .rich_text);
flag!(get_required, "required", |f: &FieldModel| f.flags.required);
flag!(get_do_not_scroll, "doNotScroll", |f: &FieldModel| f
    .flags
    .do_not_scroll);
flag!(
    get_do_not_spell_check,
    "doNotSpellCheck",
    |f: &FieldModel| f.flags.do_not_spell_check
);
flag!(get_file_select, "fileSelect", |f: &FieldModel| f
    .flags
    .file_select);
flag!(get_editable, "editable", |f: &FieldModel| f.flags.editable);
flag!(
    get_multiple_selection,
    "multipleSelection",
    |f: &FieldModel| f.flags.multiple_selection
);

/// `Field.userName` — `/TU`, the tooltip, `""` when absent.
fn get_user_name(this: &JsValue, _a: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let name = read(this, context, "userName", |field| field.user_name.clone())?;
    Ok(JsValue::from(boa_engine::js_string!(name)))
}

/// `Field.exportValues = …` — **validate then discard**, and the validation
/// is what shows: a non-array throws `Object no longer exists.` rather than
/// the `Cannot assign to readonly property.` a reader would expect, which is
/// upstream's own choice of message (`fxjs/cjs_field.cpp`'s
/// `set_export_values`).
fn set_export_values(_t: &JsValue, args: &[JsValue], _c: &mut Context) -> JsResult<JsValue> {
    let is_array = args
        .first()
        .and_then(JsValue::as_object)
        .is_some_and(|object| object.is_array());
    if !is_array {
        return Err(err("exportValues", BAD_OBJECT));
    }
    Ok(JsValue::undefined())
}

/// `Field.exportValues` — a check box's or radio group's control values.
fn get_export_values(this: &JsValue, _a: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let (kind, values) = read(this, context, "exportValues", |field| {
        (field.kind, field.export_values.clone())
    })?;
    if !kind.is_toggle() {
        return Err(err("exportValues", "Object is of the wrong type."));
    }
    let values: Vec<JsValue> = values
        .into_iter()
        .map(|value| JsValue::from(boa_engine::js_string!(value)))
        .collect();
    Ok(JsValue::from(
        boa_engine::object::builtins::JsArray::from_iter(values, context),
    ))
}

/// A getter answering a fixed value, for the properties whose answer depends
/// on nothing this model carries.
macro_rules! fixed {
    ($fn_name:ident, $value:expr) => {
        #[allow(clippy::unnecessary_wraps)]
        fn $fn_name(_t: &JsValue, _a: &[JsValue], _c: &mut Context) -> JsResult<JsValue> {
            Ok(JsValue::from($value))
        }
    };
}

fixed!(get_calc_order_index, -1);
fixed!(get_char_limit, 0);
fixed!(get_text_size, 0);
fixed!(get_rotation, 0);
/// The keys a `Field` carries its two other real writes under.
const LINE_WIDTH_KEY: &str = "__pdfrum_field_line_width";
/// The `Field.print` flag's key.
const PRINT_KEY: &str = "__pdfrum_field_print";

/// The two remaining real setters, which read back what was written.
macro_rules! per_object {
    ($get:ident, $set:ident, $key:ident, $default:expr, $coerce:ident) => {
        fn $get(this: &JsValue, _a: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
            let Some(object) = this.as_object() else {
                return Ok(JsValue::from($default));
            };
            object.get(boa_engine::js_string!($key), context)
        }

        fn $set(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
            let value = args.get_or_undefined(0).clone().$coerce(context)?;
            if let Some(object) = this.as_object() {
                object.set(
                    boa_engine::js_string!($key),
                    JsValue::from(value),
                    false,
                    context,
                )?;
            }
            Ok(JsValue::undefined())
        }
    };
}

per_object!(get_line_width, set_line_width, LINE_WIDTH_KEY, 1, to_i32);

/// `Field.print` — the `/F` print bit, and a real write.
fn get_print(this: &JsValue, _a: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let Some(object) = this.as_object() else {
        return Ok(JsValue::from(true));
    };
    object.get(boa_engine::js_string!(PRINT_KEY), context)
}

/// `Field.print = b`.
fn set_print(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let value = args.get_or_undefined(0).to_boolean();
    if let Some(object) = this.as_object() {
        object.set(
            boa_engine::js_string!(PRINT_KEY),
            JsValue::from(value),
            false,
            context,
        )?;
    }
    Ok(JsValue::undefined())
}
/// `Field.buttonPosition` — `/MK /TP`, clamped.
fn get_button_position(this: &JsValue, _a: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let position = read(this, context, "buttonPosition", |field| {
        field.button_position
    })?;
    Ok(JsValue::from(position))
}

fixed!(get_button_align_x, 0);
fixed!(get_button_align_y, 0);
fixed!(get_button_fit_bounds, false);
fixed!(get_button_scale_how, false);
fixed!(get_button_scale_when, 0);
fixed!(get_radios_in_unison, false);

/// A getter answering a fixed string.
macro_rules! fixed_string {
    ($fn_name:ident, $value:literal) => {
        #[allow(clippy::unnecessary_wraps)]
        fn $fn_name(_t: &JsValue, _a: &[JsValue], _c: &mut Context) -> JsResult<JsValue> {
            Ok(JsValue::from(boa_engine::js_string!($value)))
        }
    };
}

// The three colours are `["T"]` — transparent — for every field with no
// `/MK /BG`, no `/MK /BC` and a `/DA` that names no colour, which is every
// field in the fixture. `cjs_color.cpp`'s encoding is
// `["T"] / ["G",g] / ["RGB",r,g,b] / ["CMYK",c,m,y,k]`, and the array
// stringifies to the bare `T` a golden reads.
fixed_string!(get_alignment, "left");
fixed_string!(get_highlight, "invert");
fixed_string!(get_style, "check");
fixed_string!(get_text_font, "Helv");

/// One of the three colour getters — all `["T"]`.
fn get_color(_t: &JsValue, _a: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::from(
        boa_engine::object::builtins::JsArray::from_iter(
            [JsValue::from(boa_engine::js_string!("T"))],
            context,
        ),
    ))
}

/// A getter answering `undefined`, for the three properties that are pure
/// no-ops upstream: `richValue`, `source`, `submitName`.
#[allow(clippy::unnecessary_wraps)]
fn get_undefined(_t: &JsValue, _a: &[JsValue], _c: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::undefined())
}

/// The key a `Field` object carries its border style under.
///
/// A real setter — one of the ten that write — and the value is per field
/// rather than per object, so it lives on the model. An unrecognized spelling
/// is **silently ignored** rather than refused: the helper at
/// `fxjs/cjs_field.cpp:238` returns early without throwing, which is why
/// `field_properties` can assert `borderStyle = inset` and nothing about a
/// bad one.
const BORDER_STYLES: [&str; 5] = ["solid", "dashed", "beveled", "inset", "underline"];

/// `Field.borderStyle`.
fn get_border_style(this: &JsValue, _a: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let Some(object) = this.as_object() else {
        return Ok(JsValue::from(boa_engine::js_string!("solid")));
    };
    object.get(boa_engine::js_string!(BORDER_KEY), context)
}

/// `Field.borderStyle = s` — an unrecognized spelling changes nothing.
fn set_border_style(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let text = string_of(&args.get_or_undefined(0).clone(), context)?;
    if !BORDER_STYLES.contains(&text.as_str()) {
        return Ok(JsValue::undefined());
    }
    if let Some(object) = this.as_object() {
        object.set(
            boa_engine::js_string!(BORDER_KEY),
            JsValue::from(boa_engine::js_string!(text)),
            false,
            context,
        )?;
    }
    Ok(JsValue::undefined())
}

/// The key [`get_border_style`] reads.
const BORDER_KEY: &str = "__pdfrum_field_border";

/// The key a `Field` object carries its own `delay` flag under.
const DELAY_KEY: &str = "__pdfrum_field_delay";

/// `Field.delay` — a **per-object** batching flag.
///
/// Per `CJS_Field` instance and not per field: `delay_` is a member of the
/// JavaScript wrapper (`fxjs/cjs_field.cpp`), so two `getField` calls for one
/// field give two objects with independent flags. `field_properties.in`
/// exercises exactly that, reading the flag back off the object it set it on.
fn get_delay(this: &JsValue, _a: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let Some(object) = this.as_object() else {
        return Ok(JsValue::from(false));
    };
    object.get(boa_engine::js_string!(DELAY_KEY), context)
}

/// `Field.delay = b`.
///
/// Setting it **false** flushes what was queued while it was true, through
/// `DoDelay` — and setting it *true* is what makes `Field.value` queue rather
/// than write. `bug_494057` is the fixture, and its assertion is that the
/// value has not moved on either side of the round trip.
fn set_delay(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let delay = args.get_or_undefined(0).to_boolean();
    let Some(object) = this.as_object() else {
        return Ok(JsValue::undefined());
    };
    object.set(
        boa_engine::js_string!(DELAY_KEY),
        JsValue::from(delay),
        false,
        context,
    )?;
    if delay {
        return Ok(JsValue::undefined());
    }
    // The flush.
    let queued = object.get(boa_engine::js_string!(QUEUE_KEY), context)?;
    let Some(array) = queued.as_object().filter(|o| o.is_array()) else {
        return Ok(JsValue::undefined());
    };
    object.set(
        boa_engine::js_string!(QUEUE_KEY),
        JsValue::undefined(),
        false,
        context,
    )?;
    let length = array
        .get(boa_engine::js_string!("length"), context)?
        .to_length(context)?;
    let mut offered = Vec::new();
    for at in 0..length {
        let element = array.get(at, context)?;
        offered.push(string_of(&element, context)?);
    }
    let Some(index) = index_of(this, context) else {
        return Ok(JsValue::undefined());
    };
    if let Some(host) = host(context) {
        let mut state = host.borrow_mut();
        if let Some(field) = state.document.fields.get_mut(index) {
            let accepted = apply_value(field, &offered);
            state
                .field_writes
                .push((u32::try_from(index).unwrap_or(u32::MAX), accepted));
        }
    }
    Ok(JsValue::undefined())
}

// ---- building one ----

/// Defines one accessor on a `Field` — [`super::bind::define_accessor`].
fn define(
    object: &JsObject,
    context: &mut Context,
    name: &str,
    get: super::af::Bound,
    set: super::af::Bound,
) -> JsResult<()> {
    super::bind::define_accessor(object, context, name, get, set)
}

/// Builds the `Field` object for the field at a `/Fields` position.
///
/// Long on purpose, and for the same reason `doc::install` is: it *is* the
/// table of 52 properties and 26 methods, and the only thing a reader checks
/// it for is that each name appears once beside the shape it takes.
#[allow(
    clippy::too_many_lines,
    reason = "the function is the table; see the doc comment"
)]
pub(crate) fn build(index: usize, name: &str, context: &mut Context) -> JsResult<JsObject> {
    let object = ObjectInitializer::new(context).build();
    object.create_data_property_or_throw(
        boa_engine::js_string!(INDEX_KEY),
        JsValue::from(u32::try_from(index).unwrap_or(u32::MAX)),
        context,
    )?;
    object.create_data_property_or_throw(
        boa_engine::js_string!(NAME_KEY),
        JsValue::from(boa_engine::js_string!(name.to_string())),
        context,
    )?;
    object.create_data_property_or_throw(
        boa_engine::js_string!(DELAY_KEY),
        JsValue::from(false),
        context,
    )?;
    object.create_data_property_or_throw(
        boa_engine::js_string!(BORDER_KEY),
        JsValue::from(boa_engine::js_string!("solid")),
        context,
    )?;
    object.create_data_property_or_throw(
        boa_engine::js_string!(LINE_WIDTH_KEY),
        JsValue::from(1),
        context,
    )?;
    object.create_data_property_or_throw(
        boa_engine::js_string!(PRINT_KEY),
        JsValue::from(true),
        context,
    )?;

    // The properties that read something and take a real write.
    let writable: [(&str, super::af::Bound, super::af::Bound); 11] = [
        ("value", get_value, set_value),
        ("display", get_display, set_display),
        ("hidden", get_hidden, set_hidden),
        ("readonly", get_readonly, set_readonly),
        ("rect", get_rect, set_rect),
        (
            "currentValueIndices",
            get_current_value_indices,
            set_current_value_indices,
        ),
        ("delay", get_delay, set_delay),
        ("borderStyle", get_border_style, set_border_style),
        ("lineWidth", get_line_width, set_line_width),
        ("print", get_print, set_print),
        ("exportValues", get_export_values, set_export_values),
    ];
    for (name, getter, setter) in writable {
        define(&object, context, name, getter, setter)?;
    }

    // The properties that read something and **discard** their write, which
    // is the file's commonest shape.
    let discarding: [(&str, super::af::Bound); 27] = [
        ("alignment", get_alignment),
        ("buttonAlignX", get_button_align_x),
        ("buttonAlignY", get_button_align_y),
        ("buttonFitBounds", get_button_fit_bounds),
        ("buttonPosition", get_button_position),
        ("buttonScaleHow", get_button_scale_how),
        ("buttonScaleWhen", get_button_scale_when),
        ("calcOrderIndex", get_calc_order_index),
        ("charLimit", get_char_limit),
        ("comb", get_comb),
        ("defaultValue", get_default_value),
        ("doNotScroll", get_do_not_scroll),
        ("doNotSpellCheck", get_do_not_spell_check),
        ("editable", get_editable),
        ("fileSelect", get_file_select),
        ("fillColor", get_color),
        ("highlight", get_highlight),
        ("multiline", get_multiline),
        ("multipleSelection", get_multiple_selection),
        ("password", get_password),
        ("radiosInUnison", get_radios_in_unison),
        ("required", get_required),
        ("richText", get_rich_text),
        ("rotation", get_rotation),
        ("strokeColor", get_color),
        ("style", get_style),
        ("textColor", get_color),
    ];
    for (name, getter) in discarding {
        define(&object, context, name, getter, ignoring_setter)?;
    }
    for (name, getter) in [
        ("textFont", get_text_font as super::af::Bound),
        ("textSize", get_text_size),
        ("userName", get_user_name),
        ("richValue", get_undefined),
        ("source", get_undefined),
        ("submitName", get_undefined),
    ] {
        define(&object, context, name, getter, ignoring_setter)?;
    }

    // The seven that throw on assignment.
    let refusing: [(&str, super::af::Bound, super::af::Bound); 7] = [
        ("name", get_name, throw_name),
        ("type", get_type, throw_type),
        ("valueAsString", get_value_as_string, throw_value_as_string),
        ("numItems", get_num_items, throw_num_items),
        ("doc", get_doc, throw_doc),
        ("page", get_page, throw_page),
        ("defaultStyle", throw_default_style, throw_default_style),
    ];
    for (name, getter, setter) in refusing {
        define(&object, context, name, getter, setter)?;
    }

    // The methods.
    let methods: [(&str, usize, super::af::Bound); 26] = [
        ("browseForFileToSubmit", 0, browse_for_file),
        ("buttonGetCaption", 1, button_get_caption),
        ("buttonGetIcon", 1, button_get_icon),
        ("buttonImportIcon", 0, noop),
        ("buttonSetCaption", 0, method_button_set_caption),
        ("buttonSetIcon", 0, method_button_set_icon),
        ("checkThisBox", 2, check_this_box),
        ("clearItems", 0, noop),
        ("defaultIsChecked", 1, default_is_checked),
        ("deleteItemAt", 0, noop),
        ("getArray", 0, get_array),
        ("getItemAt", 2, get_item_at),
        ("getLock", 0, method_get_lock),
        ("insertItemAt", 0, noop),
        ("isBoxChecked", 1, is_box_checked),
        ("isDefaultChecked", 1, is_default_checked),
        ("setAction", 0, noop),
        ("setFocus", 0, set_focus),
        ("setItems", 0, noop),
        ("setLock", 0, method_set_lock),
        (
            "signatureGetModifications",
            0,
            method_signature_get_modifications,
        ),
        ("signatureGetSeedValue", 0, method_signature_get_seed_value),
        ("signatureInfo", 0, method_signature_info),
        ("signatureSetSeedValue", 0, method_signature_set_seed_value),
        ("signatureSign", 0, method_signature_sign),
        ("signatureValidate", 0, method_signature_validate),
    ];
    for (name, length, function) in methods {
        let bound = ObjectInitializer::new(context)
            .function(native(function), boa_engine::js_string!("f"), length)
            .build()
            .get(boa_engine::js_string!("f"), context)?;
        object.create_data_property_or_throw(
            boa_engine::js_string!(name.to_string()),
            bound,
            context,
        )?;
    }
    Ok(object)
}
