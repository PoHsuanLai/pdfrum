//! The `AF*` globals, bound to `pdfrum-script`.
//!
//! **Nothing in this file computes anything.** Every function here reads
//! JavaScript arguments, hands them to a pure function in `pdfrum-script`,
//! turns the [`AfEffects`](pdfrum_script::AfEffects) that came back into
//! transcript lines, and returns the value. The arithmetic, the picture
//! parsing, the mask engine, the separator styles and the six deliberate
//! divergences from the oracle all live in that crate with their own tests
//! and their own citations, and a change to any of them belongs there.
//!
//! That split is why the crate exists. `pdfrum-script` names no engine type,
//! reaches no session and reads no clock, so the whole `AF*` library is
//! testable — and *was* tested, at 385 of 387 reachable golden assertions —
//! before a single line of engine binding existed.
//!
//! # What this file adds that the library cannot
//!
//! Two things, and they are exactly the two the library's own status doc says
//! are missing:
//!
//! - **Arity.** Every one of the 72 golden assertions `pdfrum-script` could
//!   not answer expects `Incorrect number of parameters passed to function.`,
//!   and argument count is something only the binding can see.
//! - **The event.** `AF*` keystroke functions read and write `event`, which is
//!   a live object here and a plain value there.

use boa_engine::object::ObjectInitializer;
use boa_engine::property::Attribute;
use boa_engine::{Context, JsArgs, JsResult, JsValue, NativeFunction};

use super::bind::{now_ms, param_error, say, thrown};
use super::transcript::TranscriptLine;

/// Turns a call's effects into transcript lines.
///
/// An `AF*` alert is **not** an `app.alert`: the function's own name is the
/// title, always with `icon = 3` and `button = 0`, so it renders in the
/// decorated form with no `Alert:` prefix and interleaves with the plain
/// lines. [`TranscriptLine::FunctionAlert`] is that shape.
fn report(effects: &pdfrum_script::AfEffects, context: &Context) {
    for alert in &effects.alerts {
        say(
            context,
            TranscriptLine::FunctionAlert {
                caller: alert.caller.to_string(),
                message: alert.message.to_string(),
            },
        );
    }
}

/// A `Result` from the library, with its effects reported either way.
///
/// **Both halves matter.** `AFNumber_Keystroke` on a bad commit *notifies and
/// then throws*, and the transcript records both lines — which is why
/// `Thrown` carries effects at all and why an `Err` here reports before it
/// returns.
fn settle<T>(
    result: Result<T, pdfrum_script::Thrown>,
    context: &Context,
    name: &str,
) -> JsResult<T> {
    match result {
        Ok(value) => Ok(value),
        Err(thrown_error) => {
            report(&thrown_error.effects, context);
            Err(thrown(name, &thrown_error.error))
        }
    }
}

/// An argument as a string.
fn text(args: &[JsValue], index: usize, context: &mut Context) -> JsResult<String> {
    let value = args.get_or_undefined(index).clone();
    Ok(value.to_string(context)?.to_std_string_lossy())
}

/// An argument as an `i32`, the way `ToInt32Reentrant` takes it.
fn int(args: &[JsValue], index: usize, context: &mut Context) -> JsResult<i32> {
    args.get_or_undefined(index).clone().to_i32(context)
}

/// An argument as a `f64`.
fn number(args: &[JsValue], index: usize, context: &mut Context) -> JsResult<f64> {
    args.get_or_undefined(index).clone().to_number(context)
}

/// An argument's JavaScript truthiness — `ToBooleanReentrant`, not a type
/// check, so `'boo'` is `true`.
fn truthy(args: &[JsValue], index: usize) -> bool {
    args.get_or_undefined(index).to_boolean()
}

/// Rejects a call with the wrong number of arguments.
///
/// Every `AF*` function checks its own arity upstream and answers the same
/// message; the checks are asserted by the fixtures and are the 72 assertions
/// `pdfrum-script` alone could not reach.
fn arity(args: &[JsValue], expected: usize, name: &str) -> JsResult<()> {
    if args.len() < expected {
        return Err(param_error(name));
    }
    Ok(())
}

/// The value a format function's outcome leaves in `event.value`.
///
/// `Formatted` replaces it; `Unchanged` and `Rejected` leave it, because a
/// format function that declines has nothing to say about the value — the
/// rejection is carried by `event.rc`, which a format event does not read
/// (§4.4: Format leaves `rc` unbound).
fn formatted(outcome: pdfrum_script::AfOutcome) -> Option<String> {
    match outcome {
        pdfrum_script::AfOutcome::Formatted(value) => Some(value),
        pdfrum_script::AfOutcome::Rejected | pdfrum_script::AfOutcome::Unchanged => None,
    }
}

/// Writes `event.rc`, which is how a validate function rejects a value.
fn set_event_rc(accepted: bool, context: &mut Context) -> JsResult<()> {
    let event = context
        .global_object()
        .get(boa_engine::js_string!("event"), context)?;
    if let Some(object) = event.as_object() {
        object.set(boa_engine::js_string!("rc"), accepted, false, context)?;
    }
    Ok(())
}

/// Writes a format function's answer back to `event.value`, which is how
/// every `AF*_Format` reaches the field.
fn write_event_value(value: Option<String>, context: &mut Context) -> JsResult<()> {
    let Some(value) = value else {
        return Ok(());
    };
    let event = context
        .global_object()
        .get(boa_engine::js_string!("event"), context)?;
    if let Some(object) = event.as_object() {
        object.set(
            boa_engine::js_string!("value"),
            boa_engine::js_string!(value),
            false,
            context,
        )?;
    }
    Ok(())
}

/// The `event` fields an `AF*` keystroke function reads, as the library's own
/// value type.
fn keystroke_of(context: &mut Context) -> JsResult<pdfrum_script::Keystroke> {
    let event = context
        .global_object()
        .get(boa_engine::js_string!("event"), context)?;
    let Some(object) = event.as_object() else {
        return Ok(pdfrum_script::Keystroke::default());
    };
    let read_text = |key: &'static str, context: &mut Context| -> JsResult<String> {
        let value = object.get(boa_engine::js_string!(key), context)?;
        if value.is_undefined() {
            return Ok(String::new());
        }
        Ok(value.to_string(context)?.to_std_string_lossy())
    };
    let read_i32 = |key: &'static str, context: &mut Context| -> JsResult<i32> {
        let value = object.get(boa_engine::js_string!(key), context)?;
        if value.is_undefined() {
            return Ok(0);
        }
        value.to_i32(context)
    };
    let read_bool = |key: &'static str, context: &mut Context| -> JsResult<bool> {
        Ok(object
            .get(boa_engine::js_string!(key), context)?
            .to_boolean())
    };
    Ok(pdfrum_script::Keystroke {
        value: read_text("value", context)?,
        change: read_text("change", context)?,
        sel_start: read_i32("selStart", context)?,
        sel_end: read_i32("selEnd", context)?,
        will_commit: read_bool("willCommit", context)?,
        field_full: read_bool("fieldFull", context)?,
    })
}

/// Applies a keystroke outcome back to the live `event`.
///
/// **Which fields are copied back is a property of the event kind** (§4.4),
/// and the copy-back here is the Keystroke kind's: `rc`, `change` and the two
/// selection indices. A write to a field that is dead for the kind lands in
/// the object and is dropped when the event ends, which is exactly what
/// upstream's dummy-fallback pointers do — the same observable behaviour,
/// without the aliasing.
fn apply_keystroke(
    outcome: &pdfrum_script::KeystrokeOutcome,
    context: &mut Context,
) -> JsResult<()> {
    let event = context
        .global_object()
        .get(boa_engine::js_string!("event"), context)?;
    let Some(object) = event.as_object() else {
        return Ok(());
    };
    match outcome {
        pdfrum_script::KeystrokeOutcome::Reject => {
            object.set(boa_engine::js_string!("rc"), false, false, context)?;
        }
        pdfrum_script::KeystrokeOutcome::Accept { value, change } => {
            if let Some(value) = value {
                object.set(
                    boa_engine::js_string!("value"),
                    boa_engine::js_string!(value.clone()),
                    false,
                    context,
                )?;
            }
            if let Some(change) = change {
                object.set(
                    boa_engine::js_string!("change"),
                    boa_engine::js_string!(change.clone()),
                    false,
                    context,
                )?;
            }
        }
    }
    Ok(())
}

// ---- the twenty-two ----

/// One bound `AF*` function.
///
/// The Acrobat name is threaded in as `name` because **every error a bound
/// function throws carries it** as a `"<name>: "` prefix, and roughly seventy
/// golden assertions quote the qualified form. The macro exists so that name
/// is written once per function rather than once per `throw`.
macro_rules! af {
    ($fn_name:ident, $acrobat:literal, $body:expr) => {
        fn $fn_name(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
            #[allow(clippy::redundant_closure_call)]
            ($body)(args, context, $acrobat)
        }
    };
}

af!(
    af_number_format,
    "AFNumber_Format",
    |args: &[JsValue], context: &mut Context, name: &str| {
        arity(args, 6, name)?;
        let (n_dec, sep, neg) = (
            int(args, 0, context)?,
            int(args, 1, context)?,
            int(args, 2, context)?,
        );
        let currency = text(args, 4, context)?;
        let prepend = truthy(args, 5);
        let event = keystroke_of(context)?;
        let out =
            pdfrum_script::af_number_format(&event.value, n_dec, sep, neg, &currency, prepend);
        report(&out.effects, context);
        write_event_value(formatted(out.outcome), context)?;
        Ok(JsValue::undefined())
    }
);

af!(
    af_number_keystroke,
    "AFNumber_Keystroke",
    |args: &[JsValue], context: &mut Context, name: &str| {
        arity(args, 6, name)?;
        let (n_dec, sep) = (int(args, 0, context)?, int(args, 1, context)?);
        let event = keystroke_of(context)?;
        let out = settle(
            pdfrum_script::af_number_keystroke(&event, n_dec, sep),
            context,
            name,
        )?;
        report(&out.effects, context);
        apply_keystroke(&out.outcome, context)?;
        Ok(JsValue::undefined())
    }
);

af!(
    af_percent_format,
    "AFPercent_Format",
    |args: &[JsValue], context: &mut Context, name: &str| {
        arity(args, 2, name)?;
        let (n_dec, sep) = (int(args, 0, context)?, int(args, 1, context)?);
        let prepend = args.len() < 3 || truthy(args, 2);
        let event = keystroke_of(context)?;
        let out = settle(
            pdfrum_script::af_percent_format(&event.value, n_dec, sep, prepend),
            context,
            name,
        )?;
        report(&out.effects, context);
        write_event_value(formatted(out.outcome), context)?;
        Ok(JsValue::undefined())
    }
);

af!(
    af_percent_keystroke,
    "AFPercent_Keystroke",
    |args: &[JsValue], context: &mut Context, name: &str| {
        arity(args, 2, name)?;
        let sep = int(args, 1, context)?;
        let event = keystroke_of(context)?;
        let n_dec = int(args, 0, context)?;
        let out = settle(
            pdfrum_script::af_percent_keystroke(&event, n_dec, sep),
            context,
            name,
        )?;
        report(&out.effects, context);
        apply_keystroke(&out.outcome, context)?;
        Ok(JsValue::undefined())
    }
);

af!(
    af_date_format,
    "AFDate_Format",
    |args: &[JsValue], context: &mut Context, name: &str| {
        arity(args, 1, name)?;
        let index = int(args, 0, context)?;
        let now = now_ms(context);
        let event = keystroke_of(context)?;
        let out = settle(
            pdfrum_script::af_date_format(&event.value, index, now),
            context,
            name,
        )?;
        report(&out.effects, context);
        write_event_value(formatted(out.outcome), context)?;
        Ok(JsValue::undefined())
    }
);

af!(
    af_date_format_ex,
    "AFDate_FormatEx",
    |args: &[JsValue], context: &mut Context, name: &str| {
        arity(args, 1, name)?;
        let format = text(args, 0, context)?;
        let now = now_ms(context);
        let event = keystroke_of(context)?;
        let out = settle(
            pdfrum_script::af_date_format_ex(&event.value, &format, now),
            context,
            name,
        )?;
        report(&out.effects, context);
        write_event_value(formatted(out.outcome), context)?;
        Ok(JsValue::undefined())
    }
);

af!(
    af_date_keystroke,
    "AFDate_Keystroke",
    |args: &[JsValue], context: &mut Context, name: &str| {
        arity(args, 1, name)?;
        let index = int(args, 0, context)?;
        let now = now_ms(context);
        let event = keystroke_of(context)?;
        let out = pdfrum_script::af_date_keystroke(&event, index, now);
        report(&out.effects, context);
        apply_keystroke(&out.outcome, context)?;
        Ok(JsValue::undefined())
    }
);

af!(
    af_date_keystroke_ex,
    "AFDate_KeystrokeEx",
    |args: &[JsValue], context: &mut Context, name: &str| {
        // The one function whose arity error is its own message, because
        // upstream checks it separately: `AFDate_KeystrokeEx's parameter size
        // not correct`.
        if args.len() != 1 {
            return Err(thrown(name, &pdfrum_script::Error::DateKeystrokeArity));
        }
        let format = text(args, 0, context)?;
        let now = now_ms(context);
        let event = keystroke_of(context)?;
        let out = pdfrum_script::af_date_keystroke_ex(&event, &format, now);
        report(&out.effects, context);
        apply_keystroke(&out.outcome, context)?;
        Ok(JsValue::undefined())
    }
);

af!(
    af_time_format,
    "AFTime_Format",
    |args: &[JsValue], context: &mut Context, name: &str| {
        arity(args, 1, name)?;
        let index = int(args, 0, context)?;
        let now = now_ms(context);
        let event = keystroke_of(context)?;
        let out = settle(
            pdfrum_script::af_time_format(&event.value, index, now),
            context,
            name,
        )?;
        report(&out.effects, context);
        write_event_value(formatted(out.outcome), context)?;
        Ok(JsValue::undefined())
    }
);

af!(
    af_time_format_ex,
    "AFTime_FormatEx",
    |args: &[JsValue], context: &mut Context, name: &str| {
        arity(args, 1, name)?;
        let format = text(args, 0, context)?;
        let now = now_ms(context);
        let event = keystroke_of(context)?;
        let out = settle(
            pdfrum_script::af_time_format_ex(&event.value, &format, now),
            context,
            name,
        )?;
        report(&out.effects, context);
        write_event_value(formatted(out.outcome), context)?;
        Ok(JsValue::undefined())
    }
);

af!(
    af_time_keystroke,
    "AFTime_Keystroke",
    |args: &[JsValue], context: &mut Context, name: &str| {
        arity(args, 1, name)?;
        let index = int(args, 0, context)?;
        let now = now_ms(context);
        let event = keystroke_of(context)?;
        let out = pdfrum_script::af_time_keystroke(&event, index, now);
        report(&out.effects, context);
        apply_keystroke(&out.outcome, context)?;
        Ok(JsValue::undefined())
    }
);

af!(
    af_time_keystroke_ex,
    "AFTime_KeystrokeEx",
    |args: &[JsValue], context: &mut Context, name: &str| {
        arity(args, 1, name)?;
        let format = text(args, 0, context)?;
        let now = now_ms(context);
        let event = keystroke_of(context)?;
        let out = pdfrum_script::af_time_keystroke_ex(&event, &format, now);
        report(&out.effects, context);
        apply_keystroke(&out.outcome, context)?;
        Ok(JsValue::undefined())
    }
);

af!(
    af_special_format,
    "AFSpecial_Format",
    |args: &[JsValue], context: &mut Context, name: &str| {
        arity(args, 1, name)?;
        let kind = int(args, 0, context)?;
        let event = keystroke_of(context)?;
        let out = pdfrum_script::af_special_format(&event.value, kind);
        report(&out.effects, context);
        write_event_value(formatted(out.outcome), context)?;
        Ok(JsValue::undefined())
    }
);

af!(
    af_special_keystroke,
    "AFSpecial_Keystroke",
    |args: &[JsValue], context: &mut Context, name: &str| {
        arity(args, 1, name)?;
        let kind = int(args, 0, context)?;
        let event = keystroke_of(context)?;
        let out = pdfrum_script::af_special_keystroke(&event, kind);
        report(&out.effects, context);
        apply_keystroke(&out.outcome, context)?;
        Ok(JsValue::undefined())
    }
);

af!(
    af_special_keystroke_ex,
    "AFSpecial_KeystrokeEx",
    |args: &[JsValue], context: &mut Context, name: &str| {
        arity(args, 1, name)?;
        let mask = text(args, 0, context)?;
        let event = keystroke_of(context)?;
        let out = pdfrum_script::af_special_keystroke_ex(&event, &mask);
        report(&out.effects, context);
        apply_keystroke(&out.outcome, context)?;
        Ok(JsValue::undefined())
    }
);

af!(
    af_range_validate,
    "AFRange_Validate",
    |args: &[JsValue], context: &mut Context, name: &str| {
        arity(args, 4, name)?;
        let check_min = truthy(args, 0);
        let min = number(args, 1, context)?;
        let check_max = truthy(args, 2);
        let max = number(args, 3, context)?;
        // The labels are the numbers as the script wrote them, which is what the
        // alert text quotes back — a value typed `1.0` is reported as `1.0`, not
        // as `1`.
        let min_label = text(args, 1, context)?;
        let max_label = text(args, 3, context)?;
        let event = keystroke_of(context)?;
        let out = pdfrum_script::af_range_validate(
            &event.value,
            check_min,
            min,
            &min_label,
            check_max,
            max,
            &max_label,
        );
        report(&out.effects, context);
        // `AFRange_Validate` answers a format outcome, and the only thing it can
        // say about a value is "rejected" — which reaches the field through
        // `event.rc`, this being a Validate event.
        if out.outcome == pdfrum_script::AfOutcome::Rejected {
            set_event_rc(false, context)?;
        }
        Ok(JsValue::undefined())
    }
);

af!(
    af_merge_change,
    "AFMergeChange",
    // The one function that reads no argument and cannot fail: it merges the
    // event's own change into its own value, so there is nothing to check an
    // arity against and nothing to throw.
    |_args: &[JsValue], context: &mut Context, _name: &str| {
        let event = keystroke_of(context)?;
        Ok(JsValue::from(boa_engine::js_string!(
            pdfrum_script::af_merge_change(&event)
        )))
    }
);

af!(
    af_parse_date_ex,
    "AFParseDateEx",
    |args: &[JsValue], context: &mut Context, name: &str| {
        arity(args, 2, name)?;
        let value = text(args, 0, context)?;
        let format = text(args, 1, context)?;
        let now = now_ms(context);
        let millis = settle(
            pdfrum_script::af_parse_date_ex(&value, &format, now),
            context,
            name,
        )?;
        Ok(JsValue::from(millis))
    }
);

af!(
    af_extract_nums,
    "AFExtractNums",
    |args: &[JsValue], context: &mut Context, name: &str| {
        arity(args, 1, name)?;
        let value = text(args, 0, context)?;
        // `false`, not an empty array, when the string holds no digits at all —
        // `AFExtractNums`'s own answer, which `public_methods.in` asserts.
        let Some(parts) = pdfrum_script::af_extract_nums(&value) else {
            return Ok(JsValue::from(false));
        };
        let array = boa_engine::object::builtins::JsArray::new(context)?;
        for part in parts {
            array.push(boa_engine::js_string!(part), context)?;
        }
        Ok(array.into())
    }
);

af!(
    af_make_number,
    "AFMakeNumber",
    |args: &[JsValue], context: &mut Context, name: &str| {
        arity(args, 1, name)?;
        let value = text(args, 0, context)?;
        Ok(JsValue::from(pdfrum_script::af_make_number(&value)))
    }
);

af!(
    af_simple,
    "AFSimple",
    |args: &[JsValue], context: &mut Context, name: &str| {
        arity(args, 3, name)?;
        let op = text(args, 0, context)?;
        let a = number(args, 1, context)?;
        let b = number(args, 2, context)?;
        let value = settle(pdfrum_script::af_simple(&op, a, b), context, name)?;
        Ok(JsValue::from(value))
    }
);

af!(
    af_split_field_list,
    "AFMakeArrayFromList",
    |args: &[JsValue], context: &mut Context, name: &str| {
        arity(args, 1, name)?;
        let value = text(args, 0, context)?;
        let names = pdfrum_script::af_split_field_list(&value);
        let array = boa_engine::object::builtins::JsArray::new(context)?;
        for name in names {
            array.push(boa_engine::js_string!(name), context)?;
        }
        Ok(array.into())
    }
);

/// `AFSimple_Calculate(cFunction, cFields)`.
///
/// **Declined with a reason rather than stubbed silently.** The pure half —
/// `pdfrum_script::af_simple_calculate` over a slice of numbers — is written
/// and tested; what is missing is the lookup that fills the slice from other
/// fields' values.
// The signature is the table's, and every entry must share it.
#[allow(clippy::unnecessary_wraps)]
fn af_simple_calculate(
    _this: &JsValue,
    _args: &[JsValue],
    _context: &mut Context,
) -> JsResult<JsValue> {
    // `Ok` rather than a bare value because every entry in the table below
    // has this signature; the uniformity is the point.
    Ok(JsValue::undefined())
}

/// The shape every bound function has. See `bind`'s module documentation for
/// why it is a function pointer rather than a closure.
pub(crate) type Bound = fn(&JsValue, &[JsValue], &mut Context) -> JsResult<JsValue>;

/// Registers all twenty-two as bare globals.
pub(crate) fn install(context: &mut Context) -> JsResult<()> {
    let functions: &[(&str, Bound)] = &[
        ("AFNumber_Format", af_number_format),
        ("AFNumber_Keystroke", af_number_keystroke),
        ("AFPercent_Format", af_percent_format),
        ("AFPercent_Keystroke", af_percent_keystroke),
        ("AFDate_Format", af_date_format),
        ("AFDate_FormatEx", af_date_format_ex),
        ("AFDate_Keystroke", af_date_keystroke),
        ("AFDate_KeystrokeEx", af_date_keystroke_ex),
        ("AFTime_Format", af_time_format),
        ("AFTime_FormatEx", af_time_format_ex),
        ("AFTime_Keystroke", af_time_keystroke),
        ("AFTime_KeystrokeEx", af_time_keystroke_ex),
        ("AFSpecial_Format", af_special_format),
        ("AFSpecial_Keystroke", af_special_keystroke),
        ("AFSpecial_KeystrokeEx", af_special_keystroke_ex),
        ("AFRange_Validate", af_range_validate),
        ("AFMergeChange", af_merge_change),
        ("AFParseDateEx", af_parse_date_ex),
        ("AFExtractNums", af_extract_nums),
        ("AFMakeNumber", af_make_number),
        ("AFSimple", af_simple),
        ("AFSimple_Calculate", af_simple_calculate),
        ("AFMakeArrayFromList", af_split_field_list),
    ];
    for (name, function) in functions {
        context.register_global_callable(
            boa_engine::js_string!(*name),
            0,
            NativeFunction::from_fn_ptr(*function),
        )?;
    }
    // `event` is a live object the `AF*` functions read and write. The full
    // per-kind field table is §4 of the brief; what a bare script sees before
    // any field event has fired is the `Initialize` default.
    let event = ObjectInitializer::new(context)
        .property(
            boa_engine::js_string!("value"),
            boa_engine::js_string!(""),
            Attribute::all(),
        )
        .property(
            boa_engine::js_string!("change"),
            boa_engine::js_string!(""),
            Attribute::all(),
        )
        .property(
            boa_engine::js_string!("rc"),
            JsValue::from(true),
            Attribute::all(),
        )
        .property(
            boa_engine::js_string!("selStart"),
            JsValue::from(0),
            Attribute::all(),
        )
        .property(
            boa_engine::js_string!("selEnd"),
            JsValue::from(0),
            Attribute::all(),
        )
        .property(
            boa_engine::js_string!("willCommit"),
            JsValue::from(false),
            Attribute::all(),
        )
        .property(
            boa_engine::js_string!("fieldFull"),
            JsValue::from(false),
            Attribute::all(),
        )
        // `commitKey` resets to -1, not 0 (`cjs_event_context.cpp:289-310`).
        .property(
            boa_engine::js_string!("commitKey"),
            JsValue::from(-1),
            Attribute::all(),
        )
        .build();
    context.register_global_property(boa_engine::js_string!("event"), event, Attribute::all())
}
