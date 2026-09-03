//! `global` — a property bag whose deletions are **tombstones**, not removals.
//!
//! # Nothing is ever persisted, and that is upstream's behaviour
//!
//! The name promises a store that survives between sessions, and the C++ has
//! the machinery for one: an RC4-encrypted file, `LoadGlobalPersistentVariables`
//! and `SaveGlobalPersistentVariables`. **None of it runs.** `CJS_Global`'s
//! constructor passes a hard-coded `nullptr` delegate
//! (`fxjs/cjs_global.cpp:178-181`), and both the load and the save return
//! `false` on their first line when the delegate is null
//! (`fxjs/cfx_globaldata.cpp:263-266`). So `setPersistent` sets a flag nothing
//! reads, and reproducing the flag without the file is *exact* rather than a
//! shortfall — implementing the store would give this library its only
//! file-writing path, for a feature the oracle does not have.
//!
//! # A deleted property is a tombstone
//!
//! `DelProperty` sets `bDeleted = true` and leaves the entry in the map
//! (`:191-199`). Three consequences the golden asserts:
//!
//! - the name reads `undefined` and is not enumerated;
//! - `setPersistent` on it fails with `Global value not found.`, the same
//!   message a name that was never set gets;
//! - assigning to it again **revives** it, because the entry was never gone.
//!
//! Assigning `undefined` is a *delete*, not a store: `SetProperty`'s last
//! branch calls `DelProperty` (`:255-258`). That is why `undefined_var` is
//! never enumerable however often it is set.
//!
//! # Enumeration is sorted, and `setPersistent` comes first
//!
//! The bag is a `std::map<ByteString, …>`, so `for (var name in global)`
//! walks the names in byte order. `setPersistent` is a *method* on the object
//! rather than an entry in the bag, so it is enumerated ahead of all of them.

use std::collections::BTreeMap;

use boa_engine::object::ObjectInitializer;
use boa_engine::property::Attribute;
use boa_engine::{Context, JsArgs, JsResult, JsValue};

use super::bind::{host, param_error, qualified};

/// `JSMessage::kGlobalNotFoundError`.
const NOT_FOUND: &str = "Global value not found.";

/// One entry in the bag.
#[derive(Debug, Clone)]
pub(crate) struct Entry {
    /// What it holds. `None` while it is a tombstone.
    pub(crate) value: Option<JsValue>,
    /// Whether `setPersistent` was called with `true` for it.
    ///
    /// **Nothing reads this.** It is the flag upstream sets and never
    /// consults, kept so the property's presence is answerable and its
    /// absence is not mistaken for a store this crate refuses to build.
    pub(crate) persistent: bool,
}

/// The whole bag, by name.
///
/// A `BTreeMap` rather than a hash map because **enumeration order is
/// observable**: upstream's `std::map` walks its names in byte order and the
/// golden pins that order.
pub(crate) type Bag = BTreeMap<String, Entry>;

/// `global.setPersistent(cVariable, bPersist)`.
///
/// Two arguments exactly, and the name must be **live**: a tombstone and a
/// name that was never set both answer `Global value not found.`
fn set_persistent(_t: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    if args.len() != 2 {
        return Err(param_error("global.setPersistent"));
    }
    let name = args
        .get_or_undefined(0)
        .clone()
        .to_string(context)?
        .to_std_string_lossy();
    let persist = args.get_or_undefined(1).to_boolean();
    let Some(host) = host(context) else {
        return Err(qualified("global.setPersistent", NOT_FOUND));
    };
    let mut state = host.borrow_mut();
    let Some(entry) = state.globals.get_mut(&name) else {
        return Err(qualified("global.setPersistent", NOT_FOUND));
    };
    if entry.value.is_none() {
        // A tombstone. Same message as a name that never existed, which is
        // what makes `delete` then `setPersistent` indistinguishable from
        // never having set it.
        return Err(qualified("global.setPersistent", NOT_FOUND));
    }
    entry.persistent = persist;
    Ok(JsValue::undefined())
}

/// The proxy's `get` trap.
///
/// # A name the bag does not carry falls through to the target
///
/// Not to `undefined`: the object is still an ordinary JavaScript object with
/// `Object.prototype` behind it, and the fixture calls
/// `global.hasOwnProperty(name)` inside its own enumeration loop. Answering
/// `undefined` for every unbagged name makes that call a `TypeError` and
/// swallows the whole dump. `setPersistent` reaches the target the same way.
fn trap_get(_t: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let key = args.get_or_undefined(1).clone();
    let name = key.clone().to_string(context)?.to_std_string_lossy();
    if let Some(entry) = host(context)
        .and_then(|host| host.borrow().globals.get(&name).cloned())
        .and_then(|entry| entry.value)
    {
        return Ok(entry);
    }
    let Some(target) = args.get_or_undefined(0).as_object() else {
        return Ok(JsValue::undefined());
    };
    target.get(key.to_property_key(context)?, context)
}

/// The proxy's `set` trap.
///
/// **Assigning `undefined` deletes**, and every other type is stored — there
/// is no type this refuses, because upstream's five branches cover number,
/// boolean, string, object and null, and `undefined` is the delete.
fn trap_set(_t: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let name = args
        .get_or_undefined(1)
        .clone()
        .to_string(context)?
        .to_std_string_lossy();
    let value = args.get_or_undefined(2).clone();
    if let Some(host) = host(context) {
        let mut state = host.borrow_mut();
        if value.is_undefined() {
            // A tombstone: the entry stays, so a later assignment revives it.
            if let Some(entry) = state.globals.get_mut(&name) {
                entry.value = None;
            }
        } else {
            state.globals.insert(
                name,
                Entry {
                    value: Some(value),
                    persistent: false,
                },
            );
        }
    }
    // A `set` trap answering false throws in strict mode; true is "accepted",
    // which every assignment here is.
    Ok(JsValue::from(true))
}

/// The proxy's `deleteProperty` trap — the tombstone.
fn trap_delete(_t: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let name = args
        .get_or_undefined(1)
        .clone()
        .to_string(context)?
        .to_std_string_lossy();
    if let Some(host) = host(context)
        && let Some(entry) = host.borrow_mut().globals.get_mut(&name)
    {
        entry.value = None;
    }
    Ok(JsValue::from(true))
}

/// The proxy's `has` trap, which is what `in` and `hasOwnProperty` reach.
fn trap_has(_t: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let name = args
        .get_or_undefined(1)
        .clone()
        .to_string(context)?
        .to_std_string_lossy();
    if name == "setPersistent" {
        return Ok(JsValue::from(true));
    }
    Ok(JsValue::from(host(context).is_some_and(|host| {
        host.borrow()
            .globals
            .get(&name)
            .is_some_and(|entry| entry.value.is_some())
    })))
}

/// The proxy's `ownKeys` trap — the live names, **in byte order**, with
/// `setPersistent` first.
#[allow(
    clippy::unnecessary_wraps,
    reason = "the bound-function signature every trap shares"
)]
fn trap_own_keys(_t: &JsValue, _a: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let mut keys = vec![JsValue::from(boa_engine::js_string!("setPersistent"))];
    if let Some(host) = host(context) {
        for (name, entry) in &host.borrow().globals {
            if entry.value.is_some() {
                keys.push(JsValue::from(boa_engine::js_string!(name.clone())));
            }
        }
    }
    Ok(JsValue::from(
        boa_engine::object::builtins::JsArray::from_iter(keys, context),
    ))
}

/// The proxy's `getOwnPropertyDescriptor` trap.
///
/// Needed as well as `ownKeys`: `for..in` filters the key list by each key's
/// *enumerable* flag, and a proxy with no descriptor trap reports the
/// target's — which has none of these names, so every key would be dropped
/// and the loop would print nothing.
fn trap_descriptor(_t: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let name = args
        .get_or_undefined(1)
        .clone()
        .to_string(context)?
        .to_std_string_lossy();
    let value = if name == "setPersistent" {
        args.get_or_undefined(0)
            .as_object()
            .map(|target| target.get(boa_engine::js_string!("setPersistent"), context))
            .transpose()?
    } else {
        host(context).and_then(|host| {
            host.borrow()
                .globals
                .get(&name)
                .and_then(|entry| entry.value.clone())
        })
    };
    let Some(value) = value else {
        return Ok(JsValue::undefined());
    };
    let descriptor = ObjectInitializer::new(context)
        .property(boa_engine::js_string!("value"), value, Attribute::all())
        .property(
            boa_engine::js_string!("writable"),
            JsValue::from(true),
            Attribute::all(),
        )
        .property(
            boa_engine::js_string!("enumerable"),
            JsValue::from(true),
            Attribute::all(),
        )
        .property(
            boa_engine::js_string!("configurable"),
            JsValue::from(true),
            Attribute::all(),
        )
        .build();
    Ok(JsValue::from(descriptor))
}

/// Installs `global` as a proxy over an object carrying `setPersistent`.
///
/// A proxy rather than accessors, because the shape is an **interceptor**:
/// upstream's `DefineObjAllProperties` installs query/get/put/delete/enumerate
/// callbacks for *every* name, including ones no script has mentioned yet, and
/// there is no fixed property list to define accessors from.
pub(crate) fn install(context: &mut Context) -> JsResult<()> {
    let target = ObjectInitializer::new(context)
        .function(
            super::bind::native(set_persistent),
            boa_engine::js_string!("setPersistent"),
            2,
        )
        .build();
    let proxy = boa_engine::object::builtins::JsProxy::builder(target)
        .get(trap_get)
        .set(trap_set)
        .delete_property(trap_delete)
        .has(trap_has)
        .own_keys(trap_own_keys)
        .get_own_property_descriptor(trap_descriptor)
        .build(context)?;
    context.register_global_property(
        boa_engine::js_string!("global"),
        JsValue::from(proxy),
        Attribute::all(),
    )?;
    Ok(())
}
