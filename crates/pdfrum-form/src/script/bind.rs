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

/// The message every declined method answers with, verbatim from
/// `fxjs/js_resources.cpp` — `JSMessage::kNotSupported`.
pub(crate) const NOT_SUPPORTED: &str = "Operation not supported.";

/// `JSMessage::kParamError`, the message sixteen of the goldens assert.
pub(crate) const PARAM_ERROR: &str = "Incorrect number of parameters passed to function.";

/// **Every error a bound function throws carries its own name.**
///
/// `JSFormatErrorString` (`fxjs/js_resources.cpp:97-108`) is
/// `class_name` + optional `"." + property_name` + `": "` + the message, and
/// every call site in `fxjs/` routes through it — the `AF*` wrapper at
/// `cjs_publicmethods.cpp:159-161`, and `JSMethod`/`JSPropGetter` for the
/// object methods. So the goldens read
/// `util.printd: Incorrect number of parameters passed to function.` and
/// `AFDate_Format: Incorrect number of parameters passed to function.`, not
/// the bare message.
///
/// This is a scoring requirement rather than a nicety: roughly seventy of the
/// golden assertions are arity checks, and every one of them quotes the
/// qualified form. A bare message fails all of them.
/// # And it is thrown as a **bare string**, not an `Error`
///
/// `fxv8::ThrowExceptionHelper` is
/// `pIsolate->ThrowException(NewStringHelper(pIsolate, str))`
/// (`fxjs/fxv8.cpp:350-356`) — a string primitive, not an `Error` object. So
/// `'' + e` is the message alone, with **no `TypeError: ` prefix**, and
/// `expect.js`'s `'PASS: ' + expression + ' threw ' + e` produces
/// `threw app.alert: Incorrect number of parameters passed to function.`
///
/// The goldens show the difference directly: PDFium's own errors carry no
/// class name, while the two genuine V8 exceptions in `immutable_proto` read
/// `threw TypeError: Immutable prototype object …`. A `JsNativeError` here
/// would prefix every one of ours, failing every `expectError` assertion in
/// the seven fixtures that include `expect.js` — so the throw is
/// `JsError::from_opaque` over a string.
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
fn host(context: &Context) -> Option<Host> {
    context.get_data::<Host>().cloned()
}

/// Pushes one line onto the transcript.
pub(crate) fn say(context: &Context, line: TranscriptLine) {
    if let Some(host) = host(context) {
        host.borrow_mut().transcript.push(line);
    }
}

/// An argument as a string, the way `ToWideStringReentrant` would take it.
fn string_of(value: &JsValue, context: &mut Context) -> JsResult<String> {
    Ok(value.to_string(context)?.to_std_string_lossy())
}

// ---- app ----

/// `app.alert` — 42 of the 47 fixtures, and the function the milestone is
/// scored on.
///
/// # Argument handling is two shapes, and the second is easy to miss
///
/// `ExpandKeywordParams(params, 4, "cMsg", "nIcon", "nType", "cTitle")`
/// (`fxjs/js_define.cpp:65-98`) reads the four positionally — **unless** there
/// is exactly one argument, it is an object, and it is *not* an array, in
/// which case the four are read as named properties off it and the positional
/// reading is discarded entirely. A property that is `undefined` stays
/// "unknown" and takes its default rather than becoming the string
/// `"undefined"`.
///
/// # And three details the goldens pin
///
/// - **An array `cMsg` is joined**, not stringified:
///   `"[" + join(", ") + "]"` (`cjs_app.cpp:239-250`). A plain object is not,
///   which is why `consts_expected.txt` carries `[object Object]`.
/// - A missing `cMsg` throws [`PARAM_ERROR`], and the message is asserted.
/// - With no form-fill environment the call **returns 0 without erroring**
///   (`:233-236`). Here there is always a transcript, so the value returned is
///   the host's answer — 0, because nothing is prompted and no button is
///   pressed.
fn app_alert(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let expanded = expand_keywords(args, &["cMsg", "nIcon", "nType", "cTitle"], context)?;
    let Some(message_value) = expanded.first().filter(|v| !v.is_undefined()) else {
        return Err(param_error("app.alert"));
    };
    let message_value = message_value.clone();

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

    let known = |index: usize| -> Option<JsValue> {
        expanded
            .get(index)
            .filter(|value| !value.is_undefined())
            .cloned()
    };
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
/// than supplementing it — `result[0]` is explicitly cleared first
/// (`js_define.cpp:83`), so `alert({nIcon: 1})` has no message at all and
/// throws.
fn expand_keywords(
    args: &[JsValue],
    keywords: &[&str],
    context: &mut Context,
) -> JsResult<Vec<JsValue>> {
    let mut out = vec![JsValue::undefined(); keywords.len()];
    for (slot, value) in out.iter_mut().zip(args.iter()) {
        *slot = value.clone();
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
        *first = JsValue::undefined();
    }
    for (index, keyword) in keywords.iter().enumerate() {
        let value = object.get(boa_engine::js_string!(*keyword), context)?;
        if !value.is_undefined()
            && let Some(slot) = out.get_mut(index)
        {
            *slot = value;
        }
    }
    Ok(out)
}

/// `app.beep(nType)`. One argument, and its absence is an error
/// (`cjs_app.cpp:283-286`).
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
    if expanded.first().is_none_or(JsValue::is_undefined) {
        return Err(param_error("app.response"));
    }
    let text = |index: usize, context: &mut Context| -> JsResult<String> {
        match expanded.get(index).filter(|v| !v.is_undefined()) {
            Some(value) => string_of(&value.clone(), context),
            None => Ok(String::new()),
        }
    };
    let question = text(0, context)?;
    let title = match expanded.get(1).filter(|v| !v.is_undefined()) {
        Some(value) => string_of(&value.clone(), context)?,
        // `JSMessage::kAlert` is reused as the response dialog's default
        // title, which is why the goldens read `PDF: question` for one call
        // and `title: question` for the other.
        None => "PDF".to_string(),
    };
    let default_value = text(2, context)?;
    let password = expanded
        .get(3)
        .is_some_and(|value| !value.is_undefined() && value.to_boolean());
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
    Ok(JsValue::from(boa_engine::js_string!("")))
}

/// One of the eight `app` methods that are **no-ops returning success**
/// upstream (`cjs_app.cpp:207, 219, 296, 443, 449, 502, 532, 608`).
///
/// `app.launchURL` is the interesting one and the reason this is a shared
/// body rather than a decline: its C++ body is literally a comment and
/// `return Success()` — it does not even parse its arguments. There is
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
/// machinery upstream is a **process-wide** `map<int32_t, GlobalTimer*>`
/// (`fxjs/global_timer.cpp:18-19`), which STYLE §1 forbids outright ("No
/// global state. None."). `RunJsScript` bails entirely while the runtime is
/// blocking (`cjs_app.cpp:433-441`), so a timer inside an alert never fires
/// anyway. And a one-shot with `ms == 0` **never runs its script at all**,
/// because `TimerProc` gates on `!IsOneShot() || GetTimeOut() > 0`
/// (`:418-423`).
///
/// No fixture calls `app.setInterval`, and the one that touches `setTimeOut`
/// (`constructor.in`) only asks whether the returned object's constructor is
/// callable. M14's D14 reserved `advance_time` as the step function where a
/// later milestone fires these; the registry is per-session, never global.
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

/// `console.println`. **Upstream discards its argument**
/// (`fxjs/cjs_console.cpp` — all four methods are empty bodies returning
/// success), so the line is recorded but renders to nothing, and
/// `console_methods.in` passes precisely because nothing is printed.
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
    if args.len() < 2 {
        return Err(param_error("util.printd"));
    }
    let format = args.get_or_undefined(0).clone();
    let date = args.get_or_undefined(1).clone();
    let millis = date_millis(&date, context)?;

    // A numeric first argument selects one of the built-in styles rather than
    // being a picture string (`cjs_util.cpp:117-140`).
    let result = if let Some(style) = format.as_number() {
        #[allow(clippy::cast_possible_truncation)]
        let style = style as i32;
        pdfrum_script::util_printd_style(style, millis)
    } else {
        let format = string_of(&format, context)?;
        pdfrum_script::util_printd(&format, millis)
    };
    match result {
        Ok(text) => Ok(JsValue::from(boa_engine::js_string!(text))),
        Err(error) => Err(thrown("util.printd", &error)),
    }
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
/// Answers a real `Date`, because that is what the fixture reads properties
/// off. A string the picture cannot parse answers `undefined` rather than
/// throwing (`cjs_util.cpp:296-311`).
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
/// anything else is a value error (`cjs_util.cpp:334-345`).
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
/// `Display` carries `fxjs/js_resources.cpp`'s table word for word and this
/// function does nothing but pass it through.
pub(crate) fn thrown(name: &str, error: &pdfrum_script::Error) -> JsError {
    qualified(name, &error.to_string())
}

/// Milliseconds since the epoch from a value the script offered as a date.
///
/// A `Date` gives its own time; anything else is coerced to a number, which
/// is what `ToDateReentrant` does.
fn date_millis(value: &JsValue, context: &mut Context) -> JsResult<f64> {
    if let Some(object) = value.as_object() {
        let time = object.get(boa_engine::js_string!("getTime"), context)?;
        if let Some(callable) = time.as_callable() {
            return callable.call(value, &[], context)?.to_number(context);
        }
    }
    value.to_number(context)
}

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
    Ok(())
}

/// A native function from a plain function pointer, which is the only form
/// that can reach the host state (see the module documentation).
fn native(function: super::af::Bound) -> NativeFunction {
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
        // `cjs_app.cpp:38-52`. The values are PDFium's own, and
        // `app_properties.in` asserts each.
        init.property(
            boa_engine::js_string!("viewerType"),
            boa_engine::js_string!("pdfium"),
            Attribute::all(),
        )
        .property(
            boa_engine::js_string!("viewerVariation"),
            boa_engine::js_string!("Full"),
            Attribute::all(),
        )
        .property(
            boa_engine::js_string!("platform"),
            boa_engine::js_string!("WIN"),
            Attribute::all(),
        )
        .property(
            boa_engine::js_string!("language"),
            boa_engine::js_string!("ENU"),
            Attribute::all(),
        )
        .property(
            boa_engine::js_string!("viewerVersion"),
            JsValue::from(8),
            Attribute::all(),
        )
        .property(
            boa_engine::js_string!("formsVersion"),
            JsValue::from(7),
            Attribute::all(),
        )
        .property(
            boa_engine::js_string!("calculate"),
            JsValue::from(true),
            Attribute::all(),
        )
        .property(
            boa_engine::js_string!("runtimeHighlight"),
            JsValue::from(false),
            Attribute::all(),
        );
        init.build()
    };
    context.register_global_property(boa_engine::js_string!("app"), app, Attribute::all())
}

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
