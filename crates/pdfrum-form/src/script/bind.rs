//! The object model, bound into one realm.
//!
//! Everything here is `pub(crate)`: the shapes a script sees are not this
//! crate's API, they are the Acrobat API, and the only thing a Rust caller
//! gets back is the transcript.
//!
//! # The binding shape, stated once because it is meant to be copied
//!
//! Every bound function has the same three parts, and a follow-on adding the
//! remaining `app`/`Doc`/`Field` properties should not have to invent any of
//! them:
//!
//! 1. **A free `fn(&JsValue, &[JsValue], &mut Context) -> JsResult<JsValue>`**,
//!    registered with [`boa_engine::NativeFunction::from_fn_ptr`]. Not a
//!    closure: closures must be `Copy` here, which rules out capturing the
//!    host state, and a function pointer reaches it through the context
//!    instead — see (2).
//! 2. **`host(context)` for anything the host must see.** The transcript lives
//!    in boa's own `HostDefined` slot ([`super::host::HostState`]), so a
//!    native function reaches it from the `&mut Context` it already has.
//! 3. **`arg(args, n)` for a missing argument**, which is `undefined` rather
//!    than an error, because that is what a JavaScript call with too few
//!    arguments produces and several fixtures assert the resulting message
//!    rather than a panic.
//!
//! Stubs follow the same shape and return [`unsupported`] or `undefined`. A
//! stub is never a `todo!()` and never a panic: a script calling
//! `app.browseForDoc` must not bring the process down, and upstream's own
//! answer for most of them is "return success and do nothing".

use boa_engine::object::ObjectInitializer;
use boa_engine::property::Attribute;
use boa_engine::{Context, JsArgs, JsError, JsResult, JsValue, NativeFunction};

use super::host::{Host, HostState};
use super::transcript::{DEFAULT_ALERT_TITLE, TranscriptLine};

/// The message every declined method answers with, verbatim.
pub(crate) const NOT_SUPPORTED: &str = "Operation not supported.";

/// `JSMessage::kParamError`, the message sixteen of the goldens assert.
pub(crate) const PARAM_ERROR: &str = "Incorrect number of parameters passed to function.";

/// **Every error a bound function throws carries its own name.**
///
/// The form is `<name>: <message>`, so a golden reads
/// `util.printd: Incorrect number of parameters passed to function.` rather
/// than the bare message. Roughly seventy golden assertions are arity checks
/// and every one quotes the qualified form.
///
/// # And it is thrown as a **bare string**, not an `Error`
///
/// A string primitive, so `'' + e` is the message alone with **no
/// `TypeError: ` prefix** — a `JsNativeError` here would prefix every one of
/// ours and fail every `expectError` assertion. Hence `JsError::from_opaque`
/// over a string.
pub(crate) fn qualified(name: &str, message: &str) -> JsError {
    JsError::from_opaque(JsValue::from(boa_engine::js_string!(format!(
        "{name}: {message}"
    ))))
}

/// The error a declined method throws, qualified by its own name.
fn unsupported(name: &str) -> JsError {
    qualified(name, NOT_SUPPORTED)
}

/// The error a call with too few arguments throws, qualified by its own name.
pub(crate) fn param_error(name: &str) -> JsError {
    qualified(name, PARAM_ERROR)
}

/// The host state, or `None` if the realm was built without one — which
/// cannot happen through [`super::ScriptCascade`] and is answered rather than
/// asserted.
pub(crate) fn host(context: &Context) -> Option<Host> {
    context.get_data::<Host>().cloned()
}

/// Pushes one line onto the transcript.
pub(crate) fn say(context: &Context, line: TranscriptLine) {
    if let Some(host) = host(context) {
        host.borrow_mut().transcript.push(line);
    }
}

/// An argument as a string, the way `ToWideStringReentrant` would take it.
pub(crate) fn string_of(value: &JsValue, context: &mut Context) -> JsResult<String> {
    Ok(value.to_string(context)?.to_std_string_lossy())
}

// ---- app ----

/// `app.alert` — 42 of the 47 fixtures, and the function the milestone is
/// scored on.
///
/// # Argument handling is two shapes, and the second is easy to miss
///
/// `cMsg`, `nIcon`, `nType` and `cTitle` are read positionally — **unless**
/// there is exactly one argument, it is an object, and it is *not* an array,
/// in which case the four are read as named properties off it and the
/// positional reading is discarded entirely. A property that is `undefined`
/// stays "unknown" and takes its default rather than becoming the string
/// `"undefined"`.
///
/// # And three details the goldens pin
///
/// - **An array `cMsg` is joined**, not stringified: `"[" + join(", ") + "]"`.
///   A plain object is not, so it renders `[object Object]`.
/// - A missing `cMsg` throws [`PARAM_ERROR`], and the message is asserted.
/// - The call **returns 0 without erroring** — nothing is prompted and no
///   button is pressed, so the host's answer is 0.
fn app_alert(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let expanded = expand_keywords(args, &["cMsg", "nIcon", "nType", "cTitle"], context)?;
    // `IsExpandedParamKnown` (`fxjs/js_define.cpp:100-105`) asks whether the
    // **slot was filled**, and an explicitly passed `undefined` fills it:
    // `value->IsUndefined()` is one of the six types it accepts. So
    // `app.alert(undefined)` prints the string `undefined` and answers 0,
    // while `app.alert()` and `app.alert({})` — which leave the slot empty —
    // throw. `app_methods_expected.txt` asserts all three.
    let Some(message_value) = expanded.first().cloned().flatten() else {
        return Err(param_error("app.alert"));
    };

    let message = if let Some(array) = message_value.as_object().filter(|o| o.is_array()) {
        let length = array
            .get(boa_engine::js_string!("length"), context)?
            .to_length(context)?;
        let mut parts = Vec::new();
        for index in 0..length {
            let element = array.get(index, context)?;
            parts.push(string_of(&element, context)?);
        }
        format!("[{}]", parts.join(", "))
    } else {
        string_of(&message_value, context)?
    };

    let known = |index: usize| -> Option<JsValue> { expanded.get(index).cloned().flatten() };
    let icon = match known(1) {
        Some(value) => value.to_i32(context)?,
        None => 0,
    };
    let button = match known(2) {
        Some(value) => value.to_i32(context)?,
        None => 0,
    };
    let title = match known(3) {
        Some(value) => string_of(&value, context)?,
        None => DEFAULT_ALERT_TITLE.to_string(),
    };

    say(
        context,
        TranscriptLine::Alert {
            title,
            message,
            icon,
            button,
        },
    );
    // `JS_appAlert`'s return is which button was pressed, and with no
    // form-fill environment it is 0. Nothing here prompts, so it is 0.
    Ok(JsValue::from(0))
}

/// `ExpandKeywordParams`, reproduced.
///
/// Returns exactly `keywords.len()` values, `undefined` where the caller said
/// nothing. The single-object form replaces the positional reading rather
/// than supplementing it — the first slot is cleared first, so
/// `alert({nIcon: 1})` has no message at all and throws.
pub(crate) fn expand_keywords(
    args: &[JsValue],
    keywords: &[&str],
    context: &mut Context,
) -> JsResult<Vec<Option<JsValue>>> {
    let mut out: Vec<Option<JsValue>> = vec![None; keywords.len()];
    for (slot, value) in out.iter_mut().zip(args.iter()) {
        *slot = Some(value.clone());
    }

    // The named form applies only to a lone non-array object.
    let single_object = args
        .first()
        .filter(|_| args.len() == 1)
        .and_then(JsValue::as_object)
        .filter(|object| !object.is_array());
    let Some(object) = single_object else {
        return Ok(out);
    };

    if let Some(first) = out.first_mut() {
        *first = None;
    }
    for (index, keyword) in keywords.iter().enumerate() {
        let value = object.get(boa_engine::js_string!(*keyword), context)?;
        if !value.is_undefined()
            && let Some(slot) = out.get_mut(index)
        {
            *slot = Some(value);
        }
    }
    Ok(out)
}

/// `app.beep(nType)`. One argument, and its absence is an error.
fn app_beep(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    if args.len() != 1 {
        return Err(param_error("app.beep"));
    }
    let kind = args.get_or_undefined(0).to_i32(context)?;
    say(context, TranscriptLine::Beep(kind));
    Ok(JsValue::undefined())
}

/// `app.response(...)`. **Prompts nothing and returns the empty string**; the
/// question goes on the transcript for a host to decide about.
fn app_response(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let expanded = expand_keywords(
        args,
        &["cQuestion", "cTitle", "cDefault", "bPassword", "cLabel"],
        context,
    )?;
    if expanded.first().is_none_or(Option::is_none) {
        return Err(param_error("app.response"));
    }
    let text = |index: usize, context: &mut Context| -> JsResult<String> {
        match expanded.get(index).cloned().flatten() {
            Some(value) => string_of(&value, context),
            None => Ok(String::new()),
        }
    };
    let question = text(0, context)?;
    let title = match expanded.get(1).cloned().flatten() {
        Some(value) => string_of(&value, context)?,
        // `JSMessage::kAlert` is reused as the response dialog's default
        // title, which is why the goldens read `PDF: question` for one call
        // and `title: question` for the other.
        None => "PDF".to_string(),
    };
    let default_value = text(2, context)?;
    let password = expanded
        .get(3)
        .and_then(Clone::clone)
        .is_some_and(|value| value.to_boolean());
    let label = text(4, context)?;

    say(
        context,
        TranscriptLine::Response {
            question,
            title,
            default_value,
            label,
            password,
        },
    );
    // **The host's answer, and the harness's is `No`.** Nothing prompts here;
    // the question went on the transcript for a host to decide about, and the
    // value a script sees is what a host would have replied.
    // `ExampleAppResponse` writes the two UTF-16 code units `N`, `o` into the
    // caller's buffer (`testing/pdfium_test/pdfium_test.cc:356-364`), which
    // seven `app_methods` assertions read back — a harness constant, like
    // `myfile.pdf` and the frozen clock.
    Ok(JsValue::from(boa_engine::js_string!(GOLDEN_RESPONSE)))
}

/// What `app.response` answers when nobody is there to be asked.
///
/// The golden harness's own reply, and the value seven golden assertions read
/// back. An embedder that wants to prompt reads the
/// [`TranscriptLine::Response`] and asks; this is what the library says on its
/// own.
pub(crate) const GOLDEN_RESPONSE: &str = "No";

/// One of the eight `app` methods that are **no-ops returning success**
/// upstream.
///
/// `app.launchURL` is the interesting one and the reason this is a shared body
/// rather than a decline: upstream does not even parse its arguments. There is
/// nothing to decline, because the oracle already declines it.
// The signature is the table's, and every entry must share it.
#[allow(clippy::unnecessary_wraps)]
fn app_noop(_this: &JsValue, _args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    // `Ok` rather than a bare value: every function registered below has this
    // signature, and the uniformity is what keeps the table a table.
    Ok(JsValue::undefined())
}

/// One of the five `app` methods that **already error** upstream with
/// [`NOT_SUPPORTED`] — `execMenuItem`, `newDoc`, `openDoc`, `popUpMenu`,
/// `popUpMenuEx`. Answering the same message is the specified behaviour
/// rather than a stub of one.
macro_rules! declined {
    ($fn_name:ident, $acrobat:literal) => {
        fn $fn_name(
            _this: &JsValue,
            _args: &[JsValue],
            _context: &mut Context,
        ) -> JsResult<JsValue> {
            Err(unsupported($acrobat))
        }
    };
}

declined!(app_exec_menu_item, "app.execMenuItem");
declined!(app_new_doc, "app.newDoc");
declined!(app_open_doc, "app.openDoc");
declined!(app_popup_menu, "app.popUpMenu");
declined!(app_popup_menu_ex, "app.popUpMenuEx");

/// `app.setTimeOut(cExpr, nMilliseconds)` and `app.setInterval`.
///
/// **The script and the interval are recorded; nothing ever fires them.**
///
/// Three facts make that the right answer rather than a shortfall. The
/// machinery upstream is a **process-wide** timer map, which this workspace
/// does not build. A timer that fires while the runtime is blocking is
/// discarded anyway. And a one-shot with `ms == 0` never runs its script at
/// all.
///
/// The registry here is per-session, never global.
fn app_set_timer(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    if args.len() < 2 {
        return Err(param_error("app.setTimeOut"));
    }
    let script = string_of(&args.get_or_undefined(0).clone(), context)?;
    let interval = args.get_or_undefined(1).clone().to_i32(context)?;
    let id = if let Some(host) = host(context) {
        let mut state = host.borrow_mut();
        state.timers.push((script, interval));
        i32::try_from(state.timers.len()).unwrap_or(i32::MAX)
    } else {
        0
    };
    // `CJS_TimerObj` carries an integer id and **nothing else**: no
    // properties, no methods (`fxjs/cjs_timerobj.cpp:20-23`).
    let timer = ObjectInitializer::new(context)
        .property(
            boa_engine::js_string!("timeOut"),
            JsValue::from(id),
            Attribute::all(),
        )
        .build();
    Ok(JsValue::from(timer))
}

// ---- console ----

/// `console.println`. **The argument is discarded**, as upstream discards it,
/// so the line is recorded but renders to nothing.
fn console_println(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let text = match args.first() {
        Some(value) => string_of(&value.clone(), context)?,
        None => String::new(),
    };
    say(context, TranscriptLine::ConsolePrintln(text));
    Ok(JsValue::undefined())
}

// ---- util ----

/// `util.printf(cFormat, ...)` — bound to `pdfrum_script::util_printf`.
///
/// The formatter is not reimplemented here and must not be: it lives in
/// `pdfrum-script` as a pure function over plain data, with its own tests
/// against `util_printf`'s fixture. This function's whole job is turning
/// JavaScript values into that function's argument type and its `Error` back
/// into a thrown exception.
fn util_printf(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let Some(format) = args.first().cloned() else {
        return Err(param_error("util.printf"));
    };
    let format = string_of(&format, context)?;

    let mut values = Vec::new();
    for value in args.iter().skip(1) {
        values.push(printf_arg(value, context)?);
    }
    match pdfrum_script::util_printf(&format, &values) {
        Ok(text) => Ok(JsValue::from(boa_engine::js_string!(text))),
        Err(error) => Err(thrown("util.printf", &error)),
    }
}

/// One `util.printf` argument, as the library's own type.
///
/// A number stays a number and everything else becomes its string, which is
/// what `ToWideStringReentrant` does at the call site upstream — the format
/// specifier decides how it is consumed, not the value's JavaScript type.
fn printf_arg(value: &JsValue, context: &mut Context) -> JsResult<pdfrum_script::PrintfArg> {
    if let Some(number) = value.as_number() {
        return Ok(pdfrum_script::PrintfArg::Double(number));
    }
    Ok(pdfrum_script::PrintfArg::String(string_of(value, context)?))
}

/// `util.printd(cFormat, oDate, bXFAPicture)` — bound to
/// `pdfrum_script::util_printd`.
fn util_printd(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    const NAME: &str = "util.printd";
    if args.len() < 2 {
        return Err(param_error(NAME));
    }
    let format = args.get_or_undefined(0).clone();
    let date = args.get_or_undefined(1).clone();

    // The four gates, in `CJS_Util::printd`'s own order
    // (`fxjs/cjs_util.cpp:172-222`), because each has its own message and
    // `util_printd_expected.txt` asserts all four verbatim.
    //
    // (1) The second argument must be a `Date` **object**, not something
    //     that could become one: `42` and `"clams"` are refused rather than
    //     coerced (`:176-178`).
    let Some(object) = date.as_object().filter(is_date) else {
        return Err(thrown(NAME, &pdfrum_script::Error::NotADate));
    };
    // (2) …and it must not hold NaN, which `new Date(undefined)` does
    //     (`:180-182`).
    let millis = {
        let time = object.get(boa_engine::js_string!("getTime"), context)?;
        let Some(callable) = time.as_callable() else {
            return Err(thrown(NAME, &pdfrum_script::Error::NotADate));
        };
        callable.call(&date, &[], context)?.to_number(context)?
    };
    if millis.is_nan() {
        return Err(thrown(NAME, &pdfrum_script::Error::InvalidDate));
    }
    // `FX_LocalTime` before the components are read — `:185`.
    let millis = to_local_time(millis, context);

    // (3) A numeric first argument selects one of four canned styles, and a
    //     number outside `0..=2` is a *value* error (`:193-214`).
    if let Some(style) = format.as_number() {
        #[allow(clippy::cast_possible_truncation)]
        let style = style as i32;
        return match pdfrum_script::util_printd_style(style, millis) {
            Ok(text) => Ok(JsValue::from(boa_engine::js_string!(text))),
            Err(error) => Err(thrown(NAME, &error)),
        };
    }
    // (4) …and anything that is neither a number nor a string is a *type*
    //     error, which is why `util.printd({clams: 3}, d)` does not
    //     stringify its argument (`:216-217`).
    if !format.is_string() {
        return Err(thrown(NAME, &pdfrum_script::Error::Type));
    }
    // XFA pictures are declined upstream too, with their own message
    // (`:219-222`).
    if args.len() > 2 && args.get_or_undefined(2).to_boolean() {
        return Err(thrown(NAME, &pdfrum_script::Error::NotSupported));
    }

    let format = string_of(&format, context)?;
    match pdfrum_script::util_printd(&format, millis) {
        Ok(text) => Ok(JsValue::from(boa_engine::js_string!(text))),
        Err(error) => Err(thrown(NAME, &error)),
    }
}

/// Whether an object is a `Date` — `fxv8::IsDate`, which is a type test and
/// not a coercion.
fn is_date(object: &boa_engine::JsObject) -> bool {
    object.is::<boa_engine::builtins::date::Date>()
}

/// `util.printx(cFormat, cSource)` — bound to `pdfrum_script::util_printx`.
fn util_printx(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    if args.len() < 2 {
        return Err(param_error("util.printx"));
    }
    let mask = string_of(&args.get_or_undefined(0).clone(), context)?;
    let source = string_of(&args.get_or_undefined(1).clone(), context)?;
    Ok(JsValue::from(boa_engine::js_string!(
        pdfrum_script::util_printx(&mask, &source)
    )))
}

/// `util.scand(cFormat, cDate)` — bound to `pdfrum_script::util_scand`.
///
/// Answers a real `Date`. A string the picture cannot parse answers
/// `undefined` rather than throwing.
fn util_scand(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    if args.len() < 2 {
        return Err(param_error("util.scand"));
    }
    let format = string_of(&args.get_or_undefined(0).clone(), context)?;
    let text = string_of(&args.get_or_undefined(1).clone(), context)?;
    // The library's third parameter is *now*, not an offset: an empty date
    // string means "the current time", which is the one case `util.scand`
    // reads a clock for.
    let now = now_ms(context);
    match pdfrum_script::util_scand(&format, &text, now) {
        Some(millis) => new_date(millis, context),
        None => Ok(JsValue::undefined()),
    }
}

/// `util.byteToChar(n)`. One byte, `0..=255`, as a one-character string;
/// anything else is a value error.
fn util_byte_to_char(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    if args.len() != 1 {
        return Err(param_error("util.byteToChar"));
    }
    let code = args.get_or_undefined(0).to_i32(context)?;
    let Ok(byte) = u8::try_from(code) else {
        return Err(thrown("util.byteToChar", &pdfrum_script::Error::Value));
    };
    let text = char::from(byte).to_string();
    Ok(JsValue::from(boa_engine::js_string!(text)))
}

// ---- shared helpers the AF* bindings will also want ----

/// A `pdfrum-script` error as a thrown JavaScript exception.
///
/// **The messages are API, not diagnostics.** Sixteen of the 46 goldens
/// assert exception text verbatim, which is why `pdfrum_script::Error`'s
/// `Display` carries the oracle's table word for word and this function does
/// nothing but pass it through.
pub(crate) fn thrown(name: &str, error: &pdfrum_script::Error) -> JsError {
    qualified(name, &error.to_string())
}

/// An epoch instant shifted into the viewer's local zone: the standard
/// offset plus a daylight-saving term.
///
/// **`util.printd` prints *local* time, and this is where that happens** — the
/// shift is applied before year, month, day, hour, minute and second are
/// extracted. Missing it is an 8-hour error on every `util_printd` line.
///
/// The daylight term is why the shift is applied here rather than folded into
/// the frozen clock: the clock is one instant, and the offset depends on which
/// instant is being printed.
fn to_local_time(millis: f64, context: &Context) -> f64 {
    let offset = context.get_data::<PrintdOffset>().map_or(0, |o| o.0);
    millis + f64::from(offset) * 1000.0
}

/// The local-time offset, in the context's own data slot.
///
/// A separate value from the `Date` timezone on purpose: the two are an hour
/// apart all summer under the fixture runner, whose replacement `localtime`
/// makes the daylight-saving term always read zero. See
/// `ScriptConfig::printd_offset_secs`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PrintdOffset(pub(crate) i32);

/// A JavaScript `Date` at `millis`.
fn new_date(millis: f64, context: &mut Context) -> JsResult<JsValue> {
    let constructor = context
        .global_object()
        .get(boa_engine::js_string!("Date"), context)?;
    let Some(constructor) = constructor.as_constructor() else {
        return Ok(JsValue::undefined());
    };
    constructor
        .construct(&[JsValue::from(millis)], None, context)
        .map(JsValue::from)
}

/// The realm's frozen clock, in milliseconds since the epoch.
///
/// Every date function in `pdfrum-script` takes "now" as a parameter rather
/// than reading one — which is what makes that crate deterministic — and this
/// is where the parameter comes from. Under a golden run it is
/// `--time=1399672130` × 1000, exactly.
pub(crate) fn now_ms(context: &Context) -> f64 {
    #[allow(clippy::cast_precision_loss)]
    {
        context.clock().system_time_millis() as f64
    }
}

// ---- assembling the realm ----

/// Installs every object this milestone binds.
pub(crate) fn install(context: &mut Context, host: Host) -> JsResult<()> {
    context.insert_data(host);
    install_app(context)?;
    install_console(context)?;
    install_util(context)?;
    super::af::install(context)?;
    super::doc::install(context)?;
    Ok(())
}

/// A native function from a plain function pointer, which is the only form
/// that can reach the host state (see the module documentation).
pub(crate) fn native(function: super::af::Bound) -> NativeFunction {
    NativeFunction::from_fn_ptr(function)
}

/// `app` — the alert transcript's source, and the properties `app_properties`
/// asserts.
fn install_app(context: &mut Context) -> JsResult<()> {
    let app = {
        let mut init = ObjectInitializer::new(context);
        init.function(native(app_alert), boa_engine::js_string!("alert"), 4)
            .function(native(app_beep), boa_engine::js_string!("beep"), 1)
            .function(native(app_response), boa_engine::js_string!("response"), 5)
            .function(
                native(app_set_timer),
                boa_engine::js_string!("setTimeOut"),
                2,
            )
            .function(
                native(app_set_timer),
                boa_engine::js_string!("setInterval"),
                2,
            );
        // The eight that are no-ops returning success upstream. Reproducing
        // "does nothing, succeeds" is correct behaviour, not a shortcut — a
        // script calling `app.browseForDoc` must not throw.
        init.function(
            native(super::doc::app_mail_msg),
            boa_engine::js_string!("mailMsg"),
            6,
        );
        for name in [
            "browseForDoc",
            "execDialog",
            "findComponent",
            "goBack",
            "goForward",
            "launchURL",
            "newFDF",
            "openFDF",
            "clearInterval",
            "clearTimeOut",
        ] {
            init.function(native(app_noop), boa_engine::js_string!(name), 0);
        }
        // The five that already error upstream with the same message — and
        // each carries its own qualified name, because `JSFormatErrorString`
        // prefixes it and the goldens quote the prefixed form.
        for (name, function) in [
            ("execMenuItem", app_exec_menu_item as super::af::Bound),
            ("newDoc", app_new_doc),
            ("openDoc", app_open_doc),
            ("popUpMenu", app_popup_menu),
            ("popUpMenuEx", app_popup_menu_ex),
        ] {
            init.function(native(function), boa_engine::js_string!(name), 0);
        }
        init.build()
    };
    context.register_global_property(
        boa_engine::js_string!("app"),
        app.clone(),
        Attribute::all(),
    )?;
    install_app_properties(&app, context)
}

/// `app`'s twelve properties, which fall into three shapes.
///
/// The goldens assert every line of all three:
///
/// - **five constants** whose *setter throws* `Operation not supported.` —
///   `formsVersion`, `language`, `platform`, `viewerType`, `viewerVariation`,
///   `viewerVersion`;
/// - **three that throw on read as well** — `fs`, `fullscreen`, `media`;
/// - **two real booleans**, `calculate` and `runtimeHighlight`, whose setters
///   coerce: `app.calculate = 3` yields 3 to the assignment expression and
///   reads back `true`, because the slot is a `bool`.
///
/// `activeDocs` is the odd one: it **reads** an array-like whose sole entry is
/// the document — which stringifies as `[object global]`, since `Doc` is the
/// global — and refuses a write.
fn install_app_properties(app: &boa_engine::JsObject, context: &mut Context) -> JsResult<()> {
    // The six constants. Each needs a **throwing setter**, which a
    // non-writable data property cannot give — an assignment to one of those
    // is a silent no-op in sloppy mode, and the golden asserts a thrown
    // `Operation not supported.` So each is an accessor over a getter that
    // answers its one value, and there is a two-line getter per name rather
    // than a closure, because `NativeFunction` needs `Copy` and a closure
    // capturing a `JsValue` is not.
    let properties: [(&str, super::af::Bound, super::af::Bound); 12] = [
        ("formsVersion", app_forms_version, app_no_forms_version),
        ("language", app_language, app_no_language),
        ("platform", app_platform, app_no_platform),
        ("viewerType", app_viewer_type, app_no_viewer_type),
        (
            "viewerVariation",
            app_viewer_variation,
            app_no_viewer_variation,
        ),
        ("viewerVersion", app_viewer_version, app_no_viewer_version),
        ("activeDocs", app_active_docs, app_no_active_docs),
        ("calculate", app_get_calculate, app_set_calculate),
        (
            "runtimeHighlight",
            app_get_runtime_highlight,
            app_set_runtime_highlight,
        ),
        // The three that throw on read as well as on write.
        ("fs", app_no_fs, app_no_fs),
        ("fullscreen", app_no_fullscreen, app_no_fullscreen),
        ("media", app_no_media, app_no_media),
    ];
    for (name, get, set) in properties {
        define_accessor(app, context, name, get, set)?;
    }
    Ok(())
}

/// The six constant `app` getters, values and all.
macro_rules! app_constant {
    ($fn_name:ident, $value:expr) => {
        #[allow(clippy::unnecessary_wraps)]
        fn $fn_name(_t: &JsValue, _a: &[JsValue], _c: &mut Context) -> JsResult<JsValue> {
            Ok(JsValue::from($value))
        }
    };
}

app_constant!(app_forms_version, 7);
app_constant!(app_language, boa_engine::js_string!("ENU"));
app_constant!(app_platform, boa_engine::js_string!("WIN"));
app_constant!(app_viewer_type, boa_engine::js_string!("pdfium"));
app_constant!(app_viewer_variation, boa_engine::js_string!("Full"));
app_constant!(app_viewer_version, 8);

/// A `JsObject` wrapping a bound function, for an accessor half.
fn accessor_function(
    function: super::af::Bound,
    context: &mut Context,
) -> JsResult<boa_engine::JsObject> {
    let object = ObjectInitializer::new(context)
        .function(native(function), boa_engine::js_string!("f"), 1)
        .build();
    object
        .get(boa_engine::js_string!("f"), context)?
        .as_object()
        .ok_or_else(|| JsError::from_opaque(JsValue::undefined()))
}

/// Defines one accessor property on an object.
pub(crate) fn define_accessor(
    object: &boa_engine::JsObject,
    context: &mut Context,
    name: &str,
    get: super::af::Bound,
    set: super::af::Bound,
) -> JsResult<()> {
    let getter = accessor_function(get, context)?;
    let setter = accessor_function(set, context)?;
    object.define_property_or_throw(
        boa_engine::js_string!(name.to_string()),
        boa_engine::property::PropertyDescriptor::builder()
            .get(JsValue::from(getter))
            .set(JsValue::from(setter))
            .enumerable(true)
            .configurable(true),
        context,
    )?;
    Ok(())
}

/// `app.activeDocs` — an array whose one entry is the document.
///
/// The document **is** the global, so the golden reads
/// `app.activeDocs is object [object global]`: a one-element array
/// stringifies to its element.
#[allow(
    clippy::unnecessary_wraps,
    reason = "the bound-function signature, which every entry in the table shares"
)]
fn app_active_docs(_t: &JsValue, _a: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let global = JsValue::from(context.global_object());
    Ok(JsValue::from(
        boa_engine::object::builtins::JsArray::from_iter([global], context),
    ))
}

/// The `app` members that throw `Operation not supported.` — on read, on
/// write, or on both.
///
/// Each carries **its own qualified name**, because `JSFormatErrorString`
/// prefixes `class.property` and `app_properties_expected.txt` quotes the
/// prefixed form for all nine of them.
macro_rules! app_declined {
    ($fn_name:ident, $member:literal) => {
        fn $fn_name(_t: &JsValue, _a: &[JsValue], _c: &mut Context) -> JsResult<JsValue> {
            Err(unsupported(concat!("app.", $member)))
        }
    };
}

app_declined!(app_no_active_docs, "activeDocs");
app_declined!(app_no_forms_version, "formsVersion");
app_declined!(app_no_language, "language");
app_declined!(app_no_platform, "platform");
app_declined!(app_no_viewer_type, "viewerType");
app_declined!(app_no_viewer_variation, "viewerVariation");
app_declined!(app_no_viewer_version, "viewerVersion");
app_declined!(app_no_fs, "fs");
app_declined!(app_no_fullscreen, "fullscreen");
app_declined!(app_no_media, "media");

/// `app.calculate` and `app.runtimeHighlight` — two real booleans.
///
/// The setter **coerces**, which is why `app.calculate = 3` yields 3 to the
/// assignment expression and reads back `true`.
macro_rules! app_flag {
    ($get:ident, $set:ident, $slot:ident, $default:literal) => {
        fn $get(_t: &JsValue, _a: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
            Ok(JsValue::from(
                host(context).map_or($default, |host| host.borrow().$slot),
            ))
        }

        fn $set(_t: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
            let value = args.get_or_undefined(0).to_boolean();
            if let Some(host) = host(context) {
                host.borrow_mut().$slot = value;
            }
            Ok(JsValue::undefined())
        }
    };
}

app_flag!(app_get_calculate, app_set_calculate, app_calculate, true);
app_flag!(
    app_get_runtime_highlight,
    app_set_runtime_highlight,
    app_runtime_highlight,
    false
);

/// `console` — four methods, **all four no-ops upstream**, so this is four
/// empty functions and `console_methods.in` passes.
fn install_console(context: &mut Context) -> JsResult<()> {
    let console = {
        let mut init = ObjectInitializer::new(context);
        init.function(
            native(console_println),
            boa_engine::js_string!("println"),
            1,
        );
        for name in ["clear", "hide", "show"] {
            init.function(native(app_noop), boa_engine::js_string!(name), 0);
        }
        init.build()
    };
    context.register_global_property(boa_engine::js_string!("console"), console, Attribute::all())
}

/// `util` — five methods, four of them with an exhaustive fixture apiece, all
/// five bound to `pdfrum-script`'s pure functions rather than reimplemented.
fn install_util(context: &mut Context) -> JsResult<()> {
    let util = {
        let mut init = ObjectInitializer::new(context);
        init.function(native(util_printf), boa_engine::js_string!("printf"), 1)
            .function(native(util_printd), boa_engine::js_string!("printd"), 3)
            .function(native(util_printx), boa_engine::js_string!("printx"), 2)
            .function(native(util_scand), boa_engine::js_string!("scand"), 2)
            .function(
                native(util_byte_to_char),
                boa_engine::js_string!("byteToChar"),
                1,
            );
        init.build()
    };
    context.register_global_property(boa_engine::js_string!("util"), util, Attribute::all())
}

/// The host state a fresh realm starts with.
pub(crate) fn new_host() -> Host {
    std::rc::Rc::new(std::cell::RefCell::new(HostState::default()))
}
