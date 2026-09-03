//! `Doc` — the document object, which **is** the global.
//!
//! # `this` and the global are one object
//!
//! `Doc` **is** the global, so `this.getField(…)` and a bare `getField(…)` are
//! the same call and `this === globalThis`. Everything here is defined on the
//! global object rather than on a `Doc` binding a script would have to name —
//! which is also why a variable a script assigned `this` to stringifies as
//! `[object global]`.
//!
//! # Three shapes, and only one of them is ours
//!
//! The 42 methods fall into three classes, and the counts are what make this
//! module a table rather than an implementation:
//!
//! | class | count | what we do |
//! |---|---|---|
//! | real | 17 | implement, from the model the caller installed |
//! | no-op returning success | 23 | **one shared body**, [`noop`] — this is upstream's own behaviour, not a stub of it |
//! | throws unconditionally | 2 | answer the oracle's own message |
//!
//! The 23 are not shortcuts: there is nothing to decline, because the oracle
//! already declines them and the goldens assert `= undefined` for each.
//!
//! # The properties split the same way, and two idioms are conflated
//!
//! Of the 32 properties, eight metadata setters **silently succeed** on
//! assignment while eight others throw `Cannot assign to readonly property.`.
//! That is two idioms in one file rather than a distinction with a rule behind
//! it, and both halves are reproduced exactly.

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
use boa_engine::property::{Attribute, PropertyDescriptor};
use boa_engine::{Context, JsArgs, JsError, JsObject, JsResult, JsValue};

use super::bind::{host, native, param_error, qualified, say, string_of};
use super::model::DocumentModel;
use super::transcript::TranscriptLine;

/// The class name every error this object throws is qualified by.
///
/// **`Document`, not `Doc`** — the JS entry point is `this` and the *class* is
/// `Document`, so every golden reads `Document.getField: …`.
pub(crate) const CLASS: &str = "Document";

/// `JSMessage::kReadOnlyError`.
const READ_ONLY: &str = "Cannot assign to readonly property.";
/// `JSMessage::kValueError`.
pub(crate) const VALUE_ERROR: &str = "Incorrect parameter value.";
/// `JSMessage::kTypeError`.
pub(crate) const TYPE_ERROR: &str = "Incorrect parameter type.";
/// `JSMessage::kBadObjectError` — thrown for a thing that is gone or was
/// never there, which upstream uses for a great deal more than it sounds like.
pub(crate) const BAD_OBJECT: &str = "Object no longer exists.";
/// `JSMessage::kUserGestureRequiredError`.
const USER_GESTURE: &str = "User gesture required.";
/// `JSMessage::kNotSupportedError`.
pub(crate) const NOT_SUPPORTED: &str = super::bind::NOT_SUPPORTED;

/// The error a `Document` member throws, qualified `Document.<member>: …`.
fn err(member: &str, message: &str) -> JsError {
    qualified(&format!("{CLASS}.{member}"), message)
}

/// The `Incorrect number of parameters` error, qualified.
fn params(member: &str) -> JsError {
    param_error(&format!("{CLASS}.{member}"))
}

/// Runs `body` against the installed model.
///
/// Every getter and every real method goes through here, so there is one
/// place that knows the model lives in the host slot.
fn with_model<T>(context: &Context, body: impl FnOnce(&DocumentModel) -> T) -> Option<T> {
    let host = host(context)?;
    let state = host.borrow();
    Some(body(&state.document))
}

// ---- the shared bodies ----

/// The **23** methods that are no-ops returning success upstream.
///
/// One body, because upstream is twenty-three functions that each return
/// success without doing anything. Neither arity nor argument types are
/// checked, so `this.addAnnot(1, 2, "clams", [1, 2, 3])` answers `undefined`
/// as readily as `this.addAnnot()` does.
#[allow(clippy::unnecessary_wraps)]
fn noop(_this: &JsValue, _args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::undefined())
}

/// A getter that always answers `undefined`, for the twelve properties whose
/// getters upstream have no body at all.
#[allow(clippy::unnecessary_wraps)]
fn undefined_getter(_this: &JsValue, _a: &[JsValue], _c: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::undefined())
}

/// A setter that accepts anything and does nothing.
///
/// The commonest shape in the file, and it covers **two** upstream idioms at
/// once: the twelve properties whose setters are a bare `Success()`, and the
/// eight metadata ones commented `// Read-only.` that nonetheless succeed.
/// Both are indistinguishable to a script, and a golden asserts
/// `this.author = true; yields true` for the second.
#[allow(clippy::unnecessary_wraps)]
fn ignoring_setter(_this: &JsValue, _a: &[JsValue], _c: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::undefined())
}

// ---- the metadata getters ----

/// One `/Info` entry, or the `Object no longer exists.` a document with no
/// `/Info` dictionary throws.
///
/// The distinction is not "the key is missing": `GetPropertyInternal` answers
/// `dict->GetUnicodeTextFor(name)`, which is `""` for a key that is not there,
/// and throws only when `GetInfoDict()` itself is null. So a document with an
/// empty `/Info` answers eight empty strings and one with none at all throws
/// eight times.
fn info_entry(context: &Context, member: &str, key: &str) -> JsResult<JsValue> {
    let Some(found) = with_model(context, |model| {
        model.has_info.then(|| {
            model
                .info
                .iter()
                .find(|(name, _)| name == key)
                .map_or_else(String::new, |(_, value)| value.clone())
        })
    }) else {
        return Ok(JsValue::undefined());
    };
    match found {
        Some(text) => Ok(JsValue::from(boa_engine::js_string!(text))),
        None => Err(err(member, BAD_OBJECT)),
    }
}

/// The eight `/Info` getters, one per key.
macro_rules! metadata {
    ($fn_name:ident, $member:literal, $key:literal) => {
        fn $fn_name(_t: &JsValue, _a: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
            info_entry(context, $member, $key)
        }
    };
}

metadata!(get_author, "author", "Author");
metadata!(get_title, "title", "Title");
metadata!(get_subject, "subject", "Subject");
metadata!(get_keywords, "keywords", "Keywords");
metadata!(get_creator, "creator", "Creator");
metadata!(get_producer, "producer", "Producer");
metadata!(get_creation_date, "creationDate", "CreationDate");
metadata!(get_mod_date, "modDate", "ModDate");

/// `Doc.info` — the nine fixed keys, then every other entry the dictionary
/// carries.
///
/// The nine are always present and `""` when the file does not name them; the
/// rest follow in the file's own order, which is why the model keeps `/Info`
/// as a `Vec` and not a map.
fn get_info(_t: &JsValue, _a: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    const FIXED: [&str; 9] = [
        "Author",
        "Title",
        "Subject",
        "Keywords",
        "Creator",
        "Producer",
        "CreationDate",
        "ModDate",
        "Trapped",
    ];
    let Some(entries) = with_model(context, |model| model.has_info.then(|| model.info.clone()))
    else {
        return Ok(JsValue::undefined());
    };
    let Some(entries) = entries else {
        return Err(err("info", BAD_OBJECT));
    };
    let mut init = ObjectInitializer::new(context);
    for key in FIXED {
        let value = entries
            .iter()
            .find(|(name, _)| name == key)
            .map_or_else(String::new, |(_, value)| value.clone());
        init.property(
            boa_engine::js_string!(key),
            boa_engine::js_string!(value),
            Attribute::all(),
        );
    }
    for (key, value) in &entries {
        if FIXED.contains(&key.as_str()) {
            continue;
        }
        init.property(
            boa_engine::js_string!(key.clone()),
            boa_engine::js_string!(value.clone()),
            Attribute::all(),
        );
    }
    Ok(JsValue::from(init.build()))
}

// ---- the counted and named getters ----

/// `Doc.numPages` — the page count the viewer knows.
fn get_num_pages(_t: &JsValue, _a: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::from(
        with_model(context, |model| model.page_count).unwrap_or(0),
    ))
}

/// `Doc.numFields` — `CountFields(WideString())`, the whole terminal-field
/// list.
fn get_num_fields(_t: &JsValue, _a: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let count = with_model(context, |model| model.fields.len()).unwrap_or(0);
    Ok(JsValue::from(i32::try_from(count).unwrap_or(i32::MAX)))
}

/// `Doc.path` — `SysPathToPDFPath`, which prefixes a `/`.
fn get_path(_t: &JsValue, _a: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let path = with_model(context, |model| model.path.clone()).unwrap_or_default();
    Ok(JsValue::from(boa_engine::js_string!(path)))
}

/// `Doc.URL` — `JS_docGetFilePath()`, the raw path.
fn get_url(_t: &JsValue, _a: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let url = with_model(context, |model| model.url.clone()).unwrap_or_default();
    Ok(JsValue::from(boa_engine::js_string!(url)))
}

/// `Doc.documentFileName` — the basename, taken by scanning back for a
/// separator.
///
/// `""` when there is none, and `""` when the separator is the last character
/// — reproduced rather than tidied, because the fixture's `/myfile.pdf` and
/// the golden's empty answer only agree under the oracle's own scan.
fn get_document_file_name(_t: &JsValue, _a: &[JsValue], c: &mut Context) -> JsResult<JsValue> {
    let path = with_model(c, |model| model.url.clone()).unwrap_or_default();
    let name = path
        .rfind(['/', '\\'])
        .map_or(String::new(), |at| path[at + 1..].to_string());
    Ok(JsValue::from(boa_engine::js_string!(name)))
}

/// `Doc.calculate` — whether a recalculation sweep runs at all.
fn get_calculate(_t: &JsValue, _a: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::from(
        with_model(context, |model| model.calculate).unwrap_or(true),
    ))
}

/// `Doc.calculate = b` — `EnableCalculate`, which really does stop the sweep.
fn set_calculate(_t: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let enabled = args.get_or_undefined(0).to_boolean();
    if let Some(host) = host(context) {
        host.borrow_mut().document.calculate = enabled;
    }
    Ok(JsValue::undefined())
}

/// `Doc.filesize` — **always zero**, never the real size.
#[allow(clippy::unnecessary_wraps)]
fn get_filesize(_t: &JsValue, _a: &[JsValue], _c: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::from(0))
}

/// `Doc.external` — **always true**, with the comment
/// `// In Chrome case, should always return true` to prove it.
#[allow(clippy::unnecessary_wraps)]
fn get_external(_t: &JsValue, _a: &[JsValue], _c: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::from(true))
}

/// `Doc.icons` — `undefined` until `addIcon` has been called, then an array
/// of `Icon` objects.
fn get_icons(_t: &JsValue, _a: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let Some(host) = host(context) else {
        return Ok(JsValue::undefined());
    };
    let names = host.borrow().icon_names.clone();
    if names.is_empty() {
        return Ok(JsValue::undefined());
    }
    let icons: Vec<JsValue> = names.into_iter().map(|name| icon(&name, context)).collect();
    Ok(JsValue::from(
        boa_engine::object::builtins::JsArray::from_iter(icons, context),
    ))
}

/// The key an `Icon` carries its name under.
const ICON_KEY: &str = "__pdfrum_icon_name";

/// `Icon.name` — the whole of the object.
///
/// `undefined` for an icon nothing named, which is what
/// `Field.buttonGetIcon` hands back: an icon object with no name set.
fn icon_get_name(this: &JsValue, _a: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let Some(object) = this.as_object() else {
        return Ok(JsValue::undefined());
    };
    object.get(boa_engine::js_string!(ICON_KEY), context)
}

/// `Icon.name = …` — `Cannot assign to readonly property.` on a **bound**
/// icon, and a **silent no-op** on one built from JavaScript.
///
/// `icons.in` makes the second kind with `new icon1.constructor()` and
/// comments the reason: *"No error setting the name because for an unbound
/// object, control doesn't get far enough to reach the readonly check in the
/// property handler."* So the refusal is not a property of the *name* — it is
/// a property of the object having a native binding at all, and an object
/// that has none accepts the assignment and forgets it.
fn icon_set_name(this: &JsValue, _a: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let bound = this.as_object().is_some_and(|object| {
        object
            .has_own_property(boa_engine::js_string!(ICON_KEY), context)
            .unwrap_or(false)
    });
    if bound {
        return Err(qualified("Icon.name", READ_ONLY));
    }
    Ok(JsValue::undefined())
}

/// The key the realm keeps the shared `Icon` prototype under.
const ICON_PROTOTYPE: &str = "__pdfrum_icon_prototype";

/// One `Icon`. `None` is the nameless kind `buttonGetIcon` produces.
///
/// # Why the accessor lives on a **prototype** rather than on the object
///
/// `icons.in` builds an icon from JavaScript with
/// `new icon1.constructor()` and asserts three things about the result: its
/// prototype is the same object (`Prototype comparison is true`), its `name`
/// reads `undefined`, and assigning to `name` is a **silent no-op**. A
/// per-object accessor gives none of those — the new object would share no
/// prototype, have no `name` at all, and accept the assignment as an own data
/// property, which is what `name is unchanged from pebble` caught.
///
/// So the accessor pair is defined once, on a prototype every `Icon` shares,
/// and the *binding* is the private key each real icon carries. An object
/// with the prototype and without the key is exactly upstream's "unbound"
/// icon: the getter finds nothing and the setter's readonly check is never
/// reached, which is the comment `icons.in` puts beside the assignment.
pub(crate) fn icon_object(name: Option<&str>, context: &mut Context) -> JsResult<JsObject> {
    let prototype = icon_prototype(context)?;
    let object = JsObject::with_object_proto(context.intrinsics());
    object.set_prototype(Some(prototype));
    let value = match name {
        Some(name) => JsValue::from(boa_engine::js_string!(name.to_string())),
        None => JsValue::undefined(),
    };
    object.create_data_property_or_throw(boa_engine::js_string!(ICON_KEY), value, context)?;
    Ok(object)
}

/// The realm's one `Icon` prototype, built on first use.
fn icon_prototype(context: &mut Context) -> JsResult<JsObject> {
    let global = context.global_object();
    let existing = global.get(boa_engine::js_string!(ICON_PROTOTYPE), context)?;
    if let Some(object) = existing.as_object() {
        return Ok(object);
    }
    let prototype = ObjectInitializer::new(context).build();
    super::bind::define_accessor(&prototype, context, "name", icon_get_name, icon_set_name)?;
    global.define_property_or_throw(
        boa_engine::js_string!(ICON_PROTOTYPE),
        PropertyDescriptor::builder()
            .value(prototype.clone())
            .writable(false)
            .enumerable(false)
            .configurable(false),
        context,
    )?;
    Ok(prototype)
}

/// One named `Icon`, for `Doc.icons` and `Doc.getIcon`.
fn icon(name: &str, context: &mut Context) -> JsValue {
    icon_object(Some(name), context).map_or_else(|_| JsValue::undefined(), JsValue::from)
}

/// A property that throws `Cannot assign to readonly property.` on
/// assignment.
macro_rules! readonly {
    ($fn_name:ident, $member:literal) => {
        fn $fn_name(_t: &JsValue, _a: &[JsValue], _c: &mut Context) -> JsResult<JsValue> {
            Err(err($member, READ_ONLY))
        }
    };
}

readonly!(set_document_file_name, "documentFileName");
readonly!(set_filesize, "filesize");
readonly!(set_icons, "icons");
readonly!(set_info, "info");
readonly!(set_num_fields, "numFields");
readonly!(set_num_pages, "numPages");
readonly!(set_path, "path");
readonly!(set_url, "URL");

// ---- the navigating and form methods ----

/// `Doc.pageNum` — the current page, which no headless run has.
///
/// `undefined` is the answer with no page view, and that is what the golden
/// reads; the *setter* still navigates.
#[allow(clippy::unnecessary_wraps)]
fn get_page_num(_t: &JsValue, _a: &[JsValue], _c: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::undefined())
}

/// `Doc.pageNum = n` — `JS_docgotoPage`, clamped into range.
///
/// The clamp is upstream's own and is not a guard: `n >= count` goes to
/// `count - 1` and `n < 0` goes to `0`, so a **zero-page** document navigates
/// to `-1`. Reproduced, because `document_properties_expected.txt` pins the
/// resulting `Goto Page:` lines for every value the fixture assigns —
/// `true` becoming 1, an array becoming 0, `"red"` becoming 0.
fn set_page_num(_t: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let wanted = args.get_or_undefined(0).to_i32(context)?;
    let count = i32::try_from(with_model(context, |model| model.page_count).unwrap_or(0))
        .unwrap_or(i32::MAX);
    let target = if wanted >= count {
        count - 1
    } else if wanted < 0 {
        0
    } else {
        wanted
    };
    say(context, TranscriptLine::GotoPage(target));
    Ok(JsValue::undefined())
}

/// `Doc.getNthFieldName(n)` — the `n`th terminal field's fully qualified
/// name.
///
/// Three failures, each with its own message and each asserted: no argument
/// is a parameter-count error, a negative index is a *value* error, and an
/// index past the end is `Object no longer exists.` — because upstream's
/// `GetField(n, "")` answers null and the null check throws
/// `kBadObjectError`.
fn get_nth_field_name(_t: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    if args.len() != 1 {
        return Err(params("getNthFieldName"));
    }
    let index = args.get_or_undefined(0).to_i32(context)?;
    if index < 0 {
        return Err(err("getNthFieldName", VALUE_ERROR));
    }
    let name = with_model(context, |model| {
        usize::try_from(index)
            .ok()
            .and_then(|index| model.field_at(index))
            .map(|field| field.name.clone())
    })
    .flatten();
    match name {
        Some(name) => Ok(JsValue::from(boa_engine::js_string!(name))),
        None => Err(err("getNthFieldName", BAD_OBJECT)),
    }
}

/// `Doc.getField(name)` — a `Field` object, or **`undefined`**.
///
/// Not `null` and not an error: a name that counts no fields short-circuits
/// to `undefined` before a `Field` is ever built, which is why
/// `MyField.nonesuch` is `undefined` and the empty string is an object.
///
/// The name is normalized **after** that count: `".."` collapses to `"."`,
/// repeatedly, which is what makes `MyField..MyPushButton` resolve and
/// `MyField...nonesuch` not.
fn get_field(_t: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    if args.is_empty() {
        return Err(params("getField"));
    }
    let asked = string_of(&args.get_or_undefined(0).clone(), context)?;
    // The gate is `CountFields(wideName)` on the **uncollapsed** name
    // (`cjs_document.cpp:267`) — the collapse happens afterwards, inside
    // `AttachField`, and the two disagreeing is the whole of why
    // `getField('MyField..nonesuch')` is an object and
    // `getField('MyField.nonesuch')` is `undefined`.
    let reachable = with_model(context, |model| model.count_fields(&asked)).unwrap_or(0);
    if reachable == 0 {
        return Ok(JsValue::undefined());
    }
    // `AttachField`: collapse once, and if *that* reaches nothing, fall back
    // to splitting a trailing widget-index suffix.
    let collapsed = collapse_dots_once(&asked);
    let reachable = with_model(context, |model| model.count_fields(&collapsed)).unwrap_or(0);
    let name = if reachable > 0 {
        collapsed
    } else {
        // `AttachField` answered `false`, and `getField` **ignores its
        // return value** (`cjs_document.cpp:283`) — so the object is still
        // handed back, and `field_name_` is left at its default, which is
        // empty. That is why `getField('MyField..nonesuch').name` reads as
        // the empty string rather than as anything the caller typed.
        parse_field_name(&collapsed).unwrap_or_default()
    };
    let index = with_model(context, |model| model.field_named(&name))
        .flatten()
        .unwrap_or(0);
    super::field::build(index, &name, context).map(JsValue::from)
}

/// `AttachField`'s `swFieldNameTemp.Replace(L"..", L".")`.
///
/// **One pass, not a fixed point.** `WideString::Replace` walks the string
/// once replacing non-overlapping occurrences, so `"..."` becomes `".."` and
/// stays there — which is exactly why
/// `getField('MyField...nonesuch').name` reads `MyField..nonesuch` and the
/// four-dot spelling reads the same thing.
fn collapse_dots_once(name: &str) -> String {
    name.replace("..", ".")
}

/// The field-name half of a `<name>.<widget index>` spelling.
///
/// Splits a trailing `.<n>` widget index off a name that reached no node:
/// `MyField.3` becomes `MyField`. The suffix must parse as an integer, and a
/// suffix that parses as **zero** is only accepted when it really is `"0"`
/// (after trailing spaces are trimmed) — which is what keeps `MyField.clams`
/// from silently becoming `MyField`.
fn parse_field_name(name: &str) -> Option<String> {
    let at = name.rfind('.')?;
    let (head, suffix) = name.split_at(at);
    let suffix = &suffix[1..];
    let index: i32 = suffix.trim_start().trim_end().parse().unwrap_or(0);
    if index == 0 && suffix.trim_end() != "0" {
        return None;
    }
    Some(head.to_string())
}

/// `Doc.calculateNow()` — runs the recalculation sweep.
///
/// The sweep itself is the `Cascade`'s, and what this can do from inside a
/// script is ask for it: the flag is read back by the caller after the script
/// returns, because re-entering the cascade from a native function is the
/// re-entry the oracle refuses too.
fn calculate_now(_t: &JsValue, _a: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    if let Some(host) = host(context) {
        host.borrow_mut().calculate_requested = true;
    }
    Ok(JsValue::undefined())
}

/// `Doc.resetForm([names])` — every named field back to its `/DV`, or all of
/// them.
///
/// A string argument is wrapped into a one-element list, which is upstream's
/// own coercion; anything that is not an array and not a string resets
/// nothing, and the call still succeeds.
fn reset_form(_t: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let wanted: Option<Vec<String>> = match args.first() {
        None => None,
        Some(value) if value.is_undefined() => None,
        Some(value) => {
            let value = value.clone();
            if let Some(array) = value.as_object().filter(|o| o.is_array()) {
                let length = array
                    .get(boa_engine::js_string!("length"), context)?
                    .to_length(context)?;
                let mut names = Vec::new();
                for index in 0..length {
                    let element = array.get(index, context)?;
                    names.push(string_of(&element, context)?);
                }
                Some(names)
            } else {
                Some(vec![string_of(&value, context)?])
            }
        }
    };

    if let Some(host) = host(context) {
        let mut state = host.borrow_mut();
        let targets: Vec<usize> = match &wanted {
            None => (0..state.document.fields.len()).collect(),
            Some(asked) => asked
                .iter()
                .filter_map(|name| state.document.field_named(name))
                .collect(),
        };
        for index in targets {
            let Some(field) = state.document.fields.get_mut(index) else {
                continue;
            };
            field.value.clone_from(&field.default_value);
            let value = field.value.clone();
            state
                .field_writes
                .push((u32::try_from(index).unwrap_or(u32::MAX), value));
        }
    }
    Ok(JsValue::undefined())
}

/// `Doc.getAnnot(nPage, name)` — one annotation by its `/NM`.
///
/// A linear scan, and not finding it is `Object no longer exists.` rather than
/// `undefined` — which is the asymmetry with `getField` that
/// `annot_properties_expected.txt`'s first line asserts.
fn get_annot(_t: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    if args.len() != 2 {
        return Err(params("getAnnot"));
    }
    let page = args.get_or_undefined(0).to_i32(context)?;
    let name = string_of(&args.get_or_undefined(1).clone(), context)?;
    let found = with_model(context, |model| {
        model
            .annotations
            .iter()
            .position(|annot| i64::from(annot.page) == i64::from(page) && annot.name == name)
    })
    .flatten();
    let Some(index) = found else {
        return Err(err("getAnnot", BAD_OBJECT));
    };
    annot_object(index, context).map(JsValue::from)
}

/// `Doc.getAnnots()` — every annotation on every page.
///
/// Pop-ups and widgets are already gone: the model excludes them, because
/// `getAnnots` skips both subtypes and `bug_421304870`'s whole assertion is
/// the resulting count.
fn get_annots(_t: &JsValue, _a: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let count = with_model(context, |model| model.annotations.len()).unwrap_or(0);
    let mut values = Vec::with_capacity(count);
    for index in 0..count {
        values.push(JsValue::from(annot_object(index, context)?));
    }
    Ok(JsValue::from(
        boa_engine::object::builtins::JsArray::from_iter(values, context),
    ))
}

/// The key an `Annot` object carries its position in the model under.
const ANNOT_KEY: &str = "__pdfrum_annot_index";

/// The position in the model an `Annot` object names.
fn annot_index(this: &JsValue, context: &mut Context) -> Option<usize> {
    let object = this.as_object()?;
    let value = object
        .get(boa_engine::js_string!(ANNOT_KEY), context)
        .ok()?;
    usize::try_from(value.to_i32(context).ok()?).ok()
}

/// `Annot.name` — `/NM`, and **writable**.
///
/// The write is not cosmetic: it is the key `Doc.getAnnot` matches on, so
/// renaming an annotation makes the old name stop resolving and the new one
/// start. `annot_properties.in` asserts both halves in sequence, which is why
/// this reaches the model rather than sitting on the object.
fn annot_get_name(this: &JsValue, _a: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let Some(index) = annot_index(this, context) else {
        return Ok(JsValue::undefined());
    };
    let name = with_model(context, |model| {
        model.annotations.get(index).map(|annot| annot.name.clone())
    })
    .flatten()
    .unwrap_or_default();
    Ok(JsValue::from(boa_engine::js_string!(name)))
}

/// `Annot.name = s`.
fn annot_set_name(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let Some(index) = annot_index(this, context) else {
        return Ok(JsValue::undefined());
    };
    let name = string_of(&args.get_or_undefined(0).clone(), context)?;
    if let Some(host) = host(context)
        && let Some(annot) = host.borrow_mut().document.annotations.get_mut(index)
    {
        annot.name = name;
    }
    Ok(JsValue::undefined())
}

/// `Annot.hidden` — the `/F` hidden bit, and writable.
fn annot_get_hidden(this: &JsValue, _a: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let Some(index) = annot_index(this, context) else {
        return Ok(JsValue::undefined());
    };
    let hidden = with_model(context, |model| {
        model.annotations.get(index).map(|annot| annot.hidden)
    })
    .flatten()
    .unwrap_or(false);
    Ok(JsValue::from(hidden))
}

/// `Annot.hidden = b`.
fn annot_set_hidden(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let Some(index) = annot_index(this, context) else {
        return Ok(JsValue::undefined());
    };
    let hidden = args.get_or_undefined(0).to_boolean();
    if let Some(host) = host(context)
        && let Some(annot) = host.borrow_mut().document.annotations.get_mut(index)
    {
        annot.hidden = hidden;
    }
    Ok(JsValue::undefined())
}

/// `Annot.type` — the subtype's own name, read-only.
fn annot_get_type(this: &JsValue, _a: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let Some(index) = annot_index(this, context) else {
        return Ok(JsValue::undefined());
    };
    let kind = with_model(context, |model| {
        model.annotations.get(index).map(|annot| annot.kind.clone())
    })
    .flatten()
    .unwrap_or_default();
    Ok(JsValue::from(boa_engine::js_string!(kind)))
}

/// `Annot.type = …` — `Cannot assign to readonly property.`
fn annot_set_type(_t: &JsValue, _a: &[JsValue], _c: &mut Context) -> JsResult<JsValue> {
    Err(qualified("Annot.type", READ_ONLY))
}

/// One `Annot` — three properties, and only `type` refuses a write.
fn annot_object(index: usize, context: &mut Context) -> JsResult<JsObject> {
    let object = ObjectInitializer::new(context).build();
    object.create_data_property_or_throw(
        boa_engine::js_string!(ANNOT_KEY),
        JsValue::from(u32::try_from(index).unwrap_or(u32::MAX)),
        context,
    )?;
    for (name, get, set) in [
        (
            "name",
            annot_get_name as super::af::Bound,
            annot_set_name as super::af::Bound,
        ),
        ("hidden", annot_get_hidden, annot_set_hidden),
        ("type", annot_get_type, annot_set_type),
    ] {
        super::bind::define_accessor(&object, context, name, get, set)?;
    }
    Ok(object)
}

/// `Doc.gotoNamedDest(name)` — navigate to a `/Dests` entry.
fn goto_named_dest(_t: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    if args.len() != 1 {
        return Err(params("gotoNamedDest"));
    }
    let name = string_of(&args.get_or_undefined(0).clone(), context)?;
    let page = with_model(context, |model| {
        model
            .named_destinations
            .iter()
            .find(|(dest, _)| *dest == name)
            .map(|(_, page)| *page)
    })
    .flatten();
    if page.is_none() {
        return Err(err("gotoNamedDest", BAD_OBJECT));
    }
    // **And then nothing is said.** `gotoNamedDest` navigates through
    // `FFI_DoGoToAction` (`fpdfsdk/cpdfsdk_formfillenvironment.cpp:466-473`),
    // which is an `FPDF_FORMFILLINFO` callback and **not** the
    // `IPDF_JSPLATFORM` `Doc_gotoPage` that `Doc.pageNum` uses — and
    // `pdfium_test` sets the second and not the first. So a successful
    // `gotoNamedDest` produces no transcript line at all, and only the
    // *failure* is observable. `bug_1358075` and `bug_1335681` are both that
    // shape: one alert, no `Goto Page:`.
    Ok(JsValue::undefined())
}

/// `Doc.addIcon(name, icon)` — records the **name** and discards the icon.
///
/// Upstream validates that the second argument is an `Icon` object and then
/// keeps only the name (`icon_names_` is a `list<WideString>`), duplicates
/// included. `removeIcon` is a no-op, so the list only ever grows.
fn add_icon(_t: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    if args.len() != 2 {
        return Err(params("addIcon"));
    }
    let name = string_of(&args.get_or_undefined(0).clone(), context)?;
    // The type test is `JSGetObject<CJS_Icon>`: an object that is not an Icon
    // fails it exactly as a number does.
    // `JSGetObject<CJS_Icon>`: an object that is not an `Icon` fails the test
    // exactly as a number does. Here the marker is the private key
    // `icon_object` writes, which is the same kind of test — an identity
    // check, not a duck-typed one on `name`, since an ordinary object with a
    // `name` is not an `Icon`.
    let is_icon = args.get_or_undefined(1).as_object().is_some_and(|object| {
        object
            .has_own_property(boa_engine::js_string!(ICON_KEY), context)
            .unwrap_or(false)
    });
    if !is_icon {
        return Err(err("addIcon", TYPE_ERROR));
    }
    if let Some(host) = host(context) {
        host.borrow_mut().icon_names.push(name);
    }
    Ok(JsValue::undefined())
}

/// `Doc.getIcon(name)` — a **new** `Icon` each call, or
/// `Object no longer exists.`
fn get_icon(_t: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    if args.len() != 1 {
        return Err(params("getIcon"));
    }
    let name = string_of(&args.get_or_undefined(0).clone(), context)?;
    let known = host(context).is_some_and(|host| host.borrow().icon_names.contains(&name));
    if !known {
        return Err(err("getIcon", BAD_OBJECT));
    }
    Ok(icon(&name, context))
}

/// `Doc.mailDoc(...)` and `Doc.mailForm(...)` — the message goes on the
/// transcript and **nothing is sent**.
///
/// No network, no MAPI, no process. `mailDoc` also sends an empty attachment
/// span upstream, so the two differ only in a permission check this run has
/// no way to fail.
fn mail_msg(_t: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    mail(args, context, None)
}

/// `app.mailMsg(bUI, …)` — the same message, and the **one** difference:
/// `bUI` is required rather than defaulted.
///
/// `app.mailMsg` requires `bUI` where `Doc.mailDoc` defaults it to `true`, so
/// `app.mailMsg()` throws and `this.mailDoc()` sends an empty message. The
/// goldens assert each half.
pub(crate) fn app_mail_msg(
    _t: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    mail(args, context, Some("app.mailMsg"))
}

/// The shared body. `required` names the member whose `bUI` is mandatory, or
/// `None` for the one that defaults it.
fn mail(args: &[JsValue], context: &mut Context, required: Option<&str>) -> JsResult<JsValue> {
    let expanded = super::bind::expand_keywords(
        args,
        &["bUI", "cTo", "cCc", "cBcc", "cSubject", "cMsg"],
        context,
    )?;
    let text = |index: usize, context: &mut Context| -> JsResult<String> {
        match expanded.get(index).cloned().flatten() {
            Some(value) => string_of(&value, context),
            None => Ok(String::new()),
        }
    };
    let known_ui = expanded.first().and_then(Clone::clone);
    if let Some(member) = required {
        // Two gates, and the second is the surprising one: `bUI` must be
        // given, **and `cTo` is required whenever `bUI` is false** — the
        // comment in the C++ is `// cTo parameter required when UI not
        // invoked.` (`fxjs/cjs_app.cpp:466-471`). So `app.mailMsg(false)`
        // throws where `app.mailMsg(true)` sends an empty message, and
        // `app_methods_expected.txt` asserts both.
        let Some(ui) = known_ui.clone() else {
            return Err(param_error(member));
        };
        if !ui.to_boolean() && expanded.get(1).and_then(Clone::clone).is_none() {
            return Err(param_error(member));
        }
    }
    let ui = known_ui.is_none_or(|value| value.to_boolean());
    let to = text(1, context)?;
    let cc = text(2, context)?;
    let bcc = text(3, context)?;
    let subject = text(4, context)?;
    let body = text(5, context)?;
    say(
        context,
        TranscriptLine::MailMsg {
            ui,
            to,
            cc,
            bcc,
            subject,
            body,
        },
    );
    Ok(JsValue::undefined())
}

/// `Doc.print(...)` and `Doc.submitForm(...)` — **refused, and it is the
/// oracle that refuses them.**
///
/// Both guard on a user gesture, which is false for a script no click
/// provoked. A headless run is exactly that case, so `User gesture required.`
/// is the answer rather than a decline of ours.
///
/// `submitForm` checks its arity **first**, which is why the golden has a
/// parameter-count line for the no-argument call and a gesture line for the
/// four-argument one.
fn submit_form(_t: &JsValue, args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    if args.is_empty() {
        return Err(params("submitForm"));
    }
    Err(err("submitForm", USER_GESTURE))
}

/// `Doc.print(...)` — see [`submit_form`]. No arity check precedes the gate.
fn print(_t: &JsValue, _a: &[JsValue], _c: &mut Context) -> JsResult<JsValue> {
    Err(err("print", USER_GESTURE))
}

/// `Doc.removeField(name)` — which **removes nothing**.
///
/// Upstream's body gets the field's widgets, inflates each rectangle by a
/// point and repaints; the field itself survives. So the observable behaviour
/// is a redraw and a change mark, and here it is neither — but the arity
/// check is real and is what the golden asserts.
fn remove_field(_t: &JsValue, args: &[JsValue], _c: &mut Context) -> JsResult<JsValue> {
    if args.is_empty() {
        return Err(params("removeField"));
    }
    Ok(JsValue::undefined())
}

/// `Doc.getPageNthWord` / `getPageNumWords` — **declined, with the oracle's
/// own range check first.**
///
/// The words themselves need a content-stream text extraction this object has
/// no model for, and inventing one would answer something false rather than
/// nothing. The range check runs anyway because it comes first upstream and
/// its message is what three of the golden's lines assert.
fn page_words(member: &str, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let page = match args.first() {
        Some(value) => value.clone().to_i32(context)?,
        None => 0,
    };
    let count = i32::try_from(with_model(context, |model| model.page_count).unwrap_or(0))
        .unwrap_or(i32::MAX);
    if page < 0 || page >= count {
        return Err(err(member, VALUE_ERROR));
    }
    Err(err(member, NOT_SUPPORTED))
}

/// `Doc.getPageNthWord` — see [`page_words`].
fn get_page_nth_word(_t: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    page_words("getPageNthWord", args, context)
}

/// `Doc.getPageNumWords` — see [`page_words`].
fn get_page_num_words(_t: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    page_words("getPageNumWords", args, context)
}

/// The two methods that throw `Operation not supported.` whatever they are
/// given.
macro_rules! declined {
    ($fn_name:ident, $member:literal) => {
        fn $fn_name(_t: &JsValue, _a: &[JsValue], _c: &mut Context) -> JsResult<JsValue> {
            Err(err($member, NOT_SUPPORTED))
        }
    };
}

declined!(get_print_params, "getPrintParams");
declined!(get_page_nth_word_quads, "getPageNthWordQuads");

// ---- assembling the object, which is the global ----

/// Defines one accessor property on the global object.
///
/// The global is an ordinary `JsObject` here, so this is
/// [`super::bind::define_accessor`] against it — one helper, three callers
/// (`app`, `Doc` and `Field`), rather than three spellings that could drift.
fn define(
    context: &mut Context,
    name: &str,
    get: super::af::Bound,
    set: super::af::Bound,
) -> JsResult<()> {
    let global = context.global_object();
    super::bind::define_accessor(&global, context, name, get, set)
}

/// Binds `Doc` — which is to say, binds it onto the global object.
///
/// Long on purpose: it is the **table**, and the whole value of this module is
/// that every name upstream binds appears once here beside the shape it takes.
/// Splitting it into `install_properties` / `install_methods` would hide the
/// one thing a reader checks it for.
#[allow(
    clippy::too_many_lines,
    reason = "the function is the table; see the doc comment"
)]
pub(crate) fn install(context: &mut Context) -> JsResult<()> {
    // **The global stringifies as `[object global]`**, which four golden
    // lines assert — `field_properties`'s `doc = [object global]`,
    // `icons`'s `doc is [object global]`, `app_properties`'s
    // `app.activeDocs is object [object global]`, and `immutable_proto`'s
    // error text. V8 gets it from the global object's own class name;
    // `Symbol.toStringTag` is the standard spelling of the same thing, and it
    // is what `Object.prototype.toString` reads before falling back to the
    // internal class.
    let global = context.global_object();
    global.define_property_or_throw(
        boa_engine::JsSymbol::to_string_tag(),
        PropertyDescriptor::builder()
            .value(boa_engine::js_string!("global"))
            .writable(false)
            .enumerable(false)
            .configurable(true),
        context,
    )?;

    // The twelve properties whose getter and setter are both bare
    // `Success()` upstream: `undefined`, and assignment is accepted and
    // discarded.
    for name in [
        "ADBE",
        "bookmarkRoot",
        "Collab",
        "layout",
        "media",
        "mouseX",
        "mouseY",
        "pageWindowRect",
        "zoom",
        "zoomType",
    ] {
        define(context, name, undefined_getter, ignoring_setter)?;
    }
    // `delay` and `dirty` are ordinary boolean flags: they read `false` to
    // start with, they take a write, and the write reads back. `delay` is
    // permission-gated upstream and `dirty` sets a change mark, neither of
    // which a headless run can observe — the *flag* is what the golden reads.
    define(context, "delay", get_delay, set_delay)?;
    define(context, "dirty", get_dirty, set_dirty)?;

    // The eight `/Info` getters, every one of which succeeds silently on
    // assignment despite its `// Read-only.` comment.
    let metadata: [(&str, super::af::Bound); 8] = [
        ("author", get_author),
        ("title", get_title),
        ("subject", get_subject),
        ("keywords", get_keywords),
        ("creator", get_creator),
        ("producer", get_producer),
        ("creationDate", get_creation_date),
        ("modDate", get_mod_date),
    ];
    for (name, getter) in metadata {
        define(context, name, getter, ignoring_setter)?;
    }

    // The eight that throw on assignment.
    let read_only: [(&str, super::af::Bound, super::af::Bound); 8] = [
        (
            "documentFileName",
            get_document_file_name,
            set_document_file_name,
        ),
        ("filesize", get_filesize, set_filesize),
        ("icons", get_icons, set_icons),
        ("info", get_info, set_info),
        ("numFields", get_num_fields, set_num_fields),
        ("numPages", get_num_pages, set_num_pages),
        ("path", get_path, set_path),
        ("URL", get_url, set_url),
    ];
    for (name, getter, setter) in read_only {
        define(context, name, getter, setter)?;
    }

    define(context, "baseURL", get_base_url, set_base_url)?;
    define(context, "calculate", get_calculate, set_calculate)?;
    define(context, "external", get_external, ignoring_setter)?;
    define(context, "pageNum", get_page_num, set_page_num)?;

    // The 23 no-ops returning success. One body.
    for name in [
        "addAnnot",
        "addField",
        "addLink",
        "closeDoc",
        "createDataObject",
        "deletePages",
        "exportAsFDF",
        "exportAsText",
        "exportAsXFDF",
        "extractPages",
        "getAnnot3D",
        "getAnnots3D",
        "getLinks",
        "getOCGs",
        "getPageBox",
        "getURL",
        "importAnFDF",
        "importAnXFDF",
        "importTextData",
        "insertPages",
        "removeIcon",
        "replacePages",
        "saveAs",
        "syncAnnotScan",
    ] {
        context.register_global_builtin_callable(boa_engine::js_string!(name), 0, native(noop))?;
    }

    let methods: [(&str, usize, super::af::Bound); 15] = [
        ("addIcon", 2, add_icon),
        ("calculateNow", 0, calculate_now),
        ("getAnnot", 2, get_annot),
        ("getAnnots", 0, get_annots),
        ("getField", 1, get_field),
        ("getIcon", 1, get_icon),
        ("getNthFieldName", 1, get_nth_field_name),
        ("getPrintParams", 0, get_print_params),
        ("getPageNthWordQuads", 0, get_page_nth_word_quads),
        ("gotoNamedDest", 1, goto_named_dest),
        ("mailDoc", 6, mail_msg),
        ("mailForm", 6, mail_msg),
        ("print", 8, print),
        ("removeField", 1, remove_field),
        ("resetForm", 1, reset_form),
    ];
    for (name, length, function) in methods {
        context.register_global_builtin_callable(
            boa_engine::js_string!(name),
            length,
            native(function),
        )?;
    }
    context.register_global_builtin_callable(
        boa_engine::js_string!("submitForm"),
        1,
        native(submit_form),
    )?;
    // The two word readers share a body; each has its own two-line wrapper so
    // its member name reaches the error, because a closure capturing the name
    // would not be `Copy` and `NativeFunction` requires that.
    context.register_global_builtin_callable(
        boa_engine::js_string!("getPageNthWord"),
        3,
        native(get_page_nth_word),
    )?;
    context.register_global_builtin_callable(
        boa_engine::js_string!("getPageNumWords"),
        1,
        native(get_page_num_words),
    )?;
    Ok(())
}

/// The two boolean flags, which read back what was written.
macro_rules! flag {
    ($get:ident, $set:ident, $slot:ident) => {
        fn $get(_t: &JsValue, _a: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
            Ok(JsValue::from(
                host(context).is_some_and(|host| host.borrow().$slot),
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

flag!(get_delay, set_delay, delay);
flag!(get_dirty, set_dirty, dirty);

/// `Doc.baseURL` — a real variable that reaches nothing else.
fn get_base_url(_t: &JsValue, _a: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let text = host(context).map(|host| host.borrow().base_url.clone());
    Ok(JsValue::from(boa_engine::js_string!(
        text.unwrap_or_default()
    )))
}

/// `Doc.baseURL = v` — stringified and kept.
fn set_base_url(_t: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let text = string_of(&args.get_or_undefined(0).clone(), context)?;
    if let Some(host) = host(context) {
        host.borrow_mut().base_url = text;
    }
    Ok(JsValue::undefined())
}
