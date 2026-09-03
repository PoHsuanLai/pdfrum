//! `color` — twelve named colours, `convert` and `equal`.
//!
//! # A colour is an array, and its first element is its space
//!
//! `['T']`, `['G', g]`, `['RGB', r, g, b]`, `['CMYK', c, m, y, k]` — four
//! shapes over one encoding, and every component is a number in `0..=1`. A
//! space name that is none of the four reads as **transparent**, which is why
//! `color.convert(['G', 0.5], 'BOGUS')` answers `T` rather than throwing.
//!
//! # The twelve names are variables, not constants
//!
//! `color.black = ['RGB', 1, 0, 0]` succeeds and `color.black` reads back what
//! was assigned: each is a slot on the object, reset only by building a new
//! realm. So they are per-session state, and a document that overwrites one
//! has overwritten it for every script that follows.
//!
//! # `equal` compares in the richer space
//!
//! Not component-wise, and not after a round trip: both sides are converted to
//! `max(a.space, b.space)` under an ordering by component count, and compared
//! there. `['G', 0.5]` equals `['RGB', 0.5, 0.5, 0.5]` because the grey is
//! promoted, and `['CMYK', 0.75, 0.5, 0.25, 0.25]` equals
//! `['RGB', 0.25, 0.5, 0.75]` because the RGB is promoted to CMYK.

use boa_engine::object::ObjectInitializer;
use boa_engine::property::Attribute;
use boa_engine::{Context, JsArgs, JsResult, JsValue};

use super::bind::{define_accessor, host, param_error, qualified};

/// `JSMessage::kTypeError` — what a non-array argument answers.
const TYPE_ERROR: &str = "Incorrect parameter type.";

/// One colour, in whichever of the four spaces it was written.
///
/// The variants are ordered by **component count**, which is load-bearing:
/// `equal` picks the richer of two spaces with a plain comparison, and
/// upstream's `static_assert`s pin the same ordering on its enum.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Default)]
pub(crate) enum Color {
    /// `['T']` — no paint at all.
    #[default]
    Transparent,
    /// `['G', g]`.
    Gray(f32),
    /// `['RGB', r, g, b]`.
    Rgb(f32, f32, f32),
    /// `['CMYK', c, m, y, k]`.
    Cmyk(f32, f32, f32, f32),
}

/// Which space a colour is in, as the ordering `equal` compares by.
fn rank(color: Color) -> u8 {
    match color {
        Color::Transparent => 0,
        Color::Gray(_) => 1,
        Color::Rgb(..) => 2,
        Color::Cmyk(..) => 3,
    }
}

/// Whether a component is a usable fraction.
///
/// **A conversion whose input is out of range answers the space's zero**
/// rather than clamping or failing — `InRange` guards every one of the six
/// conversion functions and each returns a black or an empty colour when it
/// fails.
fn in_range(component: f32) -> bool {
    (0.0..=1.0).contains(&component)
}

impl Color {
    /// The colour in another space.
    ///
    /// Converting to the space it is already in is the identity, and
    /// converting **to transparent from anything is transparent** — the
    /// destination decides, not the source.
    pub(crate) fn convert(self, to: Space) -> Color {
        match (self, to) {
            (_, Space::Transparent) | (Color::Transparent, _) => Color::Transparent,
            (Color::Gray(_), Space::Gray)
            | (Color::Rgb(..), Space::Rgb)
            | (Color::Cmyk(..), Space::Cmyk) => self,
            (Color::Gray(g), Space::Rgb) => {
                if in_range(g) {
                    Color::Rgb(g, g, g)
                } else {
                    Color::Rgb(0.0, 0.0, 0.0)
                }
            }
            (Color::Gray(g), Space::Cmyk) => {
                if in_range(g) {
                    Color::Cmyk(0.0, 0.0, 0.0, 1.0 - g)
                } else {
                    Color::Cmyk(0.0, 0.0, 0.0, 0.0)
                }
            }
            (Color::Rgb(r, g, b), Space::Gray) => {
                if in_range(r) && in_range(g) && in_range(b) {
                    // The luminance weights are the oracle's own, and they do
                    // not sum to one: 0.3 + 0.59 + 0.11 is 1.0 exactly in
                    // decimal and not in binary, which is why the arithmetic
                    // is `f32` here rather than `f64`.
                    Color::Gray(0.3 * r + 0.59 * g + 0.11 * b)
                } else {
                    Color::Gray(0.0)
                }
            }
            (Color::Rgb(r, g, b), Space::Cmyk) => {
                if in_range(r) && in_range(g) && in_range(b) {
                    // Black is the *least* of the three inks, which is what
                    // makes the round trip through RGB lossless for a colour
                    // that came from CMYK with no black of its own.
                    let inks = [1.0 - r, 1.0 - g, 1.0 - b];
                    let black = inks.iter().copied().fold(f32::INFINITY, f32::min);
                    Color::Cmyk(inks[0], inks[1], inks[2], black)
                } else {
                    Color::Cmyk(0.0, 0.0, 0.0, 0.0)
                }
            }
            (Color::Cmyk(c, m, y, k), Space::Gray) => {
                if in_range(c) && in_range(m) && in_range(y) && in_range(k) {
                    Color::Gray(1.0 - (0.3 * c + 0.59 * m + 0.11 * y + k).min(1.0))
                } else {
                    Color::Gray(0.0)
                }
            }
            (Color::Cmyk(c, m, y, k), Space::Rgb) => {
                if in_range(c) && in_range(m) && in_range(y) && in_range(k) {
                    Color::Rgb(
                        1.0 - (c + k).min(1.0),
                        1.0 - (m + k).min(1.0),
                        1.0 - (y + k).min(1.0),
                    )
                } else {
                    Color::Rgb(0.0, 0.0, 0.0)
                }
            }
        }
    }

    /// The space it is in.
    fn space(self) -> Space {
        match self {
            Color::Transparent => Space::Transparent,
            Color::Gray(_) => Space::Gray,
            Color::Rgb(..) => Space::Rgb,
            Color::Cmyk(..) => Space::Cmyk,
        }
    }
}

/// A destination space, named by its first array element.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Space {
    /// `T`, and **every name that is none of the other three**.
    Transparent,
    /// `G`.
    Gray,
    /// `RGB`.
    Rgb,
    /// `CMYK`.
    Cmyk,
}

impl Space {
    /// The space a name means. An unrecognised name is transparent, which is
    /// the answer `color.convert(x, 'BOGUS')` gives.
    fn parse(name: &str) -> Space {
        match name {
            "G" => Space::Gray,
            "RGB" => Space::Rgb,
            "CMYK" => Space::Cmyk,
            _ => Space::Transparent,
        }
    }
}

/// A colour array as a value, reading missing components as **zero**.
///
/// An empty array is transparent, and so is one whose space name is
/// unrecognised. A short `['RGB', 0.5]` is `Rgb(0.5, 0.0, 0.0)` rather than an
/// error: `ConvertArrayToPWLColor` reads each component behind its own length
/// check and leaves the rest at zero.
fn read_color(value: &JsValue, context: &mut Context) -> JsResult<Color> {
    let Some(object) = value.as_object() else {
        return Ok(Color::Transparent);
    };
    let length = object
        .get(boa_engine::js_string!("length"), context)?
        .to_length(context)?;
    if length == 0 {
        return Ok(Color::Transparent);
    }
    let name = object.get(0u64, context)?;
    let name = name.to_string(context)?.to_std_string_lossy();
    let component = |index: u64, context: &mut Context| -> JsResult<f32> {
        if length <= index {
            return Ok(0.0);
        }
        #[allow(clippy::cast_possible_truncation)]
        Ok(object.get(index, context)?.to_number(context)? as f32)
    };
    Ok(match Space::parse(&name) {
        Space::Transparent => Color::Transparent,
        Space::Gray => Color::Gray(component(1, context)?),
        Space::Rgb => Color::Rgb(
            component(1, context)?,
            component(2, context)?,
            component(3, context)?,
        ),
        Space::Cmyk => Color::Cmyk(
            component(1, context)?,
            component(2, context)?,
            component(3, context)?,
            component(4, context)?,
        ),
    })
}

/// A colour back as its array.
fn write_color(color: Color, context: &mut Context) -> JsValue {
    let parts: Vec<JsValue> = match color {
        Color::Transparent => vec![JsValue::from(boa_engine::js_string!("T"))],
        Color::Gray(g) => vec![
            JsValue::from(boa_engine::js_string!("G")),
            JsValue::from(f64::from(g)),
        ],
        Color::Rgb(r, g, b) => vec![
            JsValue::from(boa_engine::js_string!("RGB")),
            JsValue::from(f64::from(r)),
            JsValue::from(f64::from(g)),
            JsValue::from(f64::from(b)),
        ],
        Color::Cmyk(c, m, y, k) => vec![
            JsValue::from(boa_engine::js_string!("CMYK")),
            JsValue::from(f64::from(c)),
            JsValue::from(f64::from(m)),
            JsValue::from(f64::from(y)),
            JsValue::from(f64::from(k)),
        ],
    };
    JsValue::from(boa_engine::object::builtins::JsArray::from_iter(
        parts, context,
    ))
}

/// `color.convert(aColor, cSpace)`.
fn convert(_t: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    if args.len() < 2 {
        return Err(param_error("color.convert"));
    }
    if !is_array(args.get_or_undefined(0)) {
        return Err(qualified("color.convert", TYPE_ERROR));
    }
    let space = args
        .get_or_undefined(1)
        .clone()
        .to_string(context)?
        .to_std_string_lossy();
    let color = read_color(&args.get_or_undefined(0).clone(), context)?;
    Ok(write_color(color.convert(Space::parse(&space)), context))
}

/// `color.equal(aColorA, aColorB)` — **compared in the richer space**.
fn equal(_t: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    if args.len() < 2 {
        return Err(param_error("color.equal"));
    }
    if !is_array(args.get_or_undefined(0)) || !is_array(args.get_or_undefined(1)) {
        return Err(qualified("color.equal", TYPE_ERROR));
    }
    let first = read_color(&args.get_or_undefined(0).clone(), context)?;
    let second = read_color(&args.get_or_undefined(1).clone(), context)?;
    let best = if rank(first) >= rank(second) {
        first.space()
    } else {
        second.space()
    };
    Ok(JsValue::from(first.convert(best) == second.convert(best)))
}

/// Whether a value is a JavaScript array, which is what both methods check.
fn is_array(value: &JsValue) -> bool {
    value.as_object().is_some_and(|object| object.is_array())
}

/// The twelve names and the colour each starts as.
const NAMED: [(&str, Color); 12] = [
    ("black", Color::Gray(0.0)),
    ("blue", Color::Rgb(0.0, 0.0, 1.0)),
    ("cyan", Color::Cmyk(1.0, 0.0, 0.0, 0.0)),
    ("dkGray", Color::Gray(0.25)),
    ("gray", Color::Gray(0.5)),
    ("green", Color::Rgb(0.0, 1.0, 0.0)),
    ("ltGray", Color::Gray(0.75)),
    ("magenta", Color::Cmyk(0.0, 1.0, 0.0, 0.0)),
    ("red", Color::Rgb(1.0, 0.0, 0.0)),
    ("transparent", Color::Transparent),
    ("white", Color::Gray(1.0)),
    ("yellow", Color::Cmyk(0.0, 0.0, 1.0, 0.0)),
];

/// One named colour's getter and setter.
///
/// The setter **refuses a non-array** with `Incorrect parameter type.` and an
/// absent argument with the parameter-count message, and otherwise takes
/// whatever the array decodes to — including a shape in a different space
/// from the one the name suggests.
macro_rules! named {
    ($get:ident, $set:ident, $slot:literal) => {
        fn $get(_t: &JsValue, _a: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
            let color = host(context)
                .and_then(|host| host.borrow().colors.get($slot).copied())
                .unwrap_or_default();
            Ok(write_color(color, context))
        }

        fn $set(_t: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
            let incoming = args.get_or_undefined(0).clone();
            if !is_array(&incoming) {
                return Err(qualified(concat!("color.", $slot), TYPE_ERROR));
            }
            let color = read_color(&incoming, context)?;
            if let Some(host) = host(context) {
                host.borrow_mut().colors.insert($slot.to_owned(), color);
            }
            Ok(JsValue::undefined())
        }
    };
}

named!(get_black, set_black, "black");
named!(get_blue, set_blue, "blue");
named!(get_cyan, set_cyan, "cyan");
named!(get_dk_gray, set_dk_gray, "dkGray");
named!(get_gray, set_gray, "gray");
named!(get_green, set_green, "green");
named!(get_lt_gray, set_lt_gray, "ltGray");
named!(get_magenta, set_magenta, "magenta");
named!(get_red, set_red, "red");
named!(get_transparent, set_transparent, "transparent");
named!(get_white, set_white, "white");
named!(get_yellow, set_yellow, "yellow");

/// The twelve names in their initial state, for a fresh realm.
pub(crate) fn initial() -> std::collections::BTreeMap<String, Color> {
    NAMED
        .into_iter()
        .map(|(name, color)| (name.to_owned(), color))
        .collect()
}

/// Installs `color` as a global.
pub(crate) fn install(context: &mut Context) -> JsResult<()> {
    let color = {
        let mut init = ObjectInitializer::new(context);
        init.function(
            super::bind::native(convert),
            boa_engine::js_string!("convert"),
            2,
        )
        .function(
            super::bind::native(equal),
            boa_engine::js_string!("equal"),
            2,
        );
        init.build()
    };
    let properties: [(&str, super::af::Bound, super::af::Bound); 12] = [
        ("black", get_black, set_black),
        ("blue", get_blue, set_blue),
        ("cyan", get_cyan, set_cyan),
        ("dkGray", get_dk_gray, set_dk_gray),
        ("gray", get_gray, set_gray),
        ("green", get_green, set_green),
        ("ltGray", get_lt_gray, set_lt_gray),
        ("magenta", get_magenta, set_magenta),
        ("red", get_red, set_red),
        ("transparent", get_transparent, set_transparent),
        ("white", get_white, set_white),
        ("yellow", get_yellow, set_yellow),
    ];
    for (name, get, set) in properties {
        define_accessor(&color, context, name, get, set)?;
    }
    context.register_global_property(
        boa_engine::js_string!("color"),
        JsValue::from(color),
        Attribute::all(),
    )?;
    Ok(())
}
