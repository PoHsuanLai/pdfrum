//! The nine constant namespaces, and the bare `IDS_*` and `RE_*` globals.
//!
//! Pure tables: `border`, `display`, `font`, `highlight`, `position`,
//! `scaleHow`, `scaleWhen`, `style` and `zoomtype` are objects with nothing
//! but named constants on them, and there is no behaviour to reproduce beyond
//! the values. Every one is transcribed from its `JSConstSpec` table.
//!
//! # A name a namespace does not carry reads `undefined`
//!
//! Not an error — the objects are ordinary and the goldens assert
//! `border.nonesuch is undefined` for each of the nine.
//!
//! # The `IDS_*` strings are the messages `AF*` builds its alerts from
//!
//! They are *globals*, not members of a namespace, and a script may read them
//! to build its own message. Two carry `% s` with a space, which is not a
//! typo to fix: it is what the string literally contains, and a script
//! splitting on it would break if it were corrected.

use boa_engine::object::ObjectInitializer;
use boa_engine::property::Attribute;
use boa_engine::{Context, JsResult, JsValue};

/// `border` — the five border styles, by their one-letter names.
const BORDER: [(&str, &str); 5] = [
    ("s", "solid"),
    ("b", "beveled"),
    ("d", "dashed"),
    ("i", "inset"),
    ("u", "underline"),
];

/// `font` — the fourteen base-14 names, as `/BaseFont` spells them.
const FONT: [(&str, &str); 14] = [
    ("Times", "Times-Roman"),
    ("TimesB", "Times-Bold"),
    ("TimesI", "Times-Italic"),
    ("TimesBI", "Times-BoldItalic"),
    ("Helv", "Helvetica"),
    ("HelvB", "Helvetica-Bold"),
    ("HelvI", "Helvetica-Oblique"),
    ("HelvBI", "Helvetica-BoldOblique"),
    ("Cour", "Courier"),
    ("CourB", "Courier-Bold"),
    ("CourI", "Courier-Oblique"),
    ("CourBI", "Courier-BoldOblique"),
    ("Symbol", "Symbol"),
    ("ZapfD", "ZapfDingbats"),
];

/// `highlight` — a push button's highlight mode, which is `/MK /H`'s value.
const HIGHLIGHT: [(&str, &str); 4] = [
    ("n", "none"),
    ("i", "invert"),
    ("p", "push"),
    ("o", "outline"),
];

/// `style` — a check box's glyph, which is `/MK /CA`'s.
const STYLE: [(&str, &str); 6] = [
    ("ch", "check"),
    ("cr", "cross"),
    ("di", "diamond"),
    ("ci", "circle"),
    ("st", "star"),
    ("sq", "square"),
];

/// `zoomtype` — a destination's zoom mode.
const ZOOMTYPE: [(&str, &str); 7] = [
    ("none", "NoVary"),
    ("fitP", "FitPage"),
    ("fitW", "FitWidth"),
    ("fitH", "FitHeight"),
    ("fitV", "FitVisibleWidth"),
    ("pref", "Preferred"),
    ("refW", "ReflowWidth"),
];

/// `display` — the four states `Field.display` takes.
const DISPLAY: [(&str, i32); 4] = [("visible", 0), ("hidden", 1), ("noPrint", 2), ("noView", 3)];

/// `position` — where a push button's icon sits relative to its caption.
const POSITION: [(&str, i32); 7] = [
    ("textOnly", 0),
    ("iconOnly", 1),
    ("iconTextV", 2),
    ("textIconV", 3),
    ("iconTextH", 4),
    ("textIconH", 5),
    ("overlay", 6),
];

/// `scaleHow` — whether an icon keeps its aspect ratio.
const SCALE_HOW: [(&str, i32); 2] = [("proportional", 0), ("anamorphic", 1)];

/// `scaleWhen` — when an icon is scaled at all.
const SCALE_WHEN: [(&str, i32); 4] = [("always", 0), ("never", 1), ("tooBig", 2), ("tooSmall", 3)];

/// The `IDS_*` message strings, verbatim.
///
/// **The `% s` spacing is theirs**, and so is `exists.Field` running two
/// sentences together — both are in the C++ literals and a script reading one
/// sees exactly these bytes.
const MESSAGES: [(&str, &str); 10] = [
    (
        "IDS_GREATER_THAN",
        "Invalid value: must be greater than or equal to % s.",
    ),
    (
        "IDS_GT_AND_LT",
        "Invalid value: must be greater than or equal to % s and less than or equal to % s.",
    ),
    (
        "IDS_LESS_THAN",
        "Invalid value: must be less than or equal to % s.",
    ),
    ("IDS_INVALID_MONTH", "**Invalid**"),
    (
        "IDS_INVALID_DATE",
        "Invalid date / time: please ensure that the date / time exists.Field",
    ),
    (
        "IDS_INVALID_VALUE",
        "The value entered does not match the format of the field",
    ),
    ("IDS_AM", "am"),
    ("IDS_PM", "pm"),
    (
        "IDS_MONTH_INFO",
        "January[1] February[2] March[3] April[4] May[5] June[6] July[7] \
         August[8] September[9] October[10] November[11] December[12] \
         Sept[9] Jan[1] Feb[2] Mar[3] Apr[4] Jun[6] Jul[7] Aug[8] Sep[9] \
         Oct[10] Nov[11] Dec[12]",
    ),
    ("IDS_STARTUP_CONSOLE_MSG", "** ^ _ ^ **"),
];

/// The `RE_*` arrays — the regular expressions `AFSpecial_*` matches against,
/// as *arrays of alternatives* rather than one pattern.
///
/// A script may read one and add its own alternative, which is why they are
/// arrays and why a single joined string would be the wrong shape.
const PATTERNS: [(&str, &[&str]); 12] = [
    ("RE_NUMBER_ENTRY_DOT_SEP", &[r"[+-]?\d*\.?\d*"]),
    (
        "RE_NUMBER_COMMIT_DOT_SEP",
        &[r"[+-]?\d+(\.\d+)?", r"[+-]?\.\d+", r"[+-]?\d+\."],
    ),
    ("RE_NUMBER_ENTRY_COMMA_SEP", &[r"[+-]?\d*,?\d*"]),
    (
        "RE_NUMBER_COMMIT_COMMA_SEP",
        &[r"[+-]?\d+([.,]\d+)?", r"[+-]?[.,]\d+", r"[+-]?\d+[.,]"],
    ),
    ("RE_ZIP_ENTRY", &[r"\d{0,5}"]),
    ("RE_ZIP_COMMIT", &[r"\d{5}"]),
    ("RE_ZIP4_ENTRY", &[r"\d{0,5}(\.|[- ])?\d{0,4}"]),
    ("RE_ZIP4_COMMIT", &[r"\d{5}(\.|[- ])?\d{4}"]),
    (
        "RE_PHONE_ENTRY",
        &[
            r"\d{0,3}(\.|[- ])?\d{0,3}(\.|[- ])?\d{0,4}",
            r"\(\d{0,3}",
            r"\(\d{0,3}\)(\.|[- ])?\d{0,3}(\.|[- ])?\d{0,4}",
            r"\(\d{0,3}(\.|[- ])?\d{0,3}(\.|[- ])?\d{0,4}",
            r"\d{0,3}\)(\.|[- ])?\d{0,3}(\.|[- ])?\d{0,4}",
            r"011(\.|[- \d])*",
        ],
    ),
    (
        "RE_PHONE_COMMIT",
        &[
            r"\d{3}(\.|[- ])?\d{4}",
            r"\d{3}(\.|[- ])?\d{3}(\.|[- ])?\d{4}",
            r"\(\d{3}\)(\.|[- ])?\d{3}(\.|[- ])?\d{4}",
            r"011(\.|[- \d])*",
        ],
    ),
    (
        "RE_SSN_ENTRY",
        &[r"\d{0,3}(\.|[- ])?\d{0,2}(\.|[- ])?\d{0,4}"],
    ),
    ("RE_SSN_COMMIT", &[r"\d{3}(\.|[- ])?\d{2}(\.|[- ])?\d{4}"]),
];

/// Registers one namespace of string constants as a global object.
fn install_strings(context: &mut Context, name: &str, entries: &[(&str, &str)]) -> JsResult<()> {
    let object = {
        let mut init = ObjectInitializer::new(context);
        for (member, value) in entries {
            init.property(
                boa_engine::js_string!((*member).to_string()),
                JsValue::from(boa_engine::js_string!((*value).to_string())),
                Attribute::all(),
            );
        }
        init.build()
    };
    context.register_global_property(
        boa_engine::js_string!(name.to_string()),
        JsValue::from(object),
        Attribute::all(),
    )
}

/// The same for a namespace of integers.
fn install_numbers(context: &mut Context, name: &str, entries: &[(&str, i32)]) -> JsResult<()> {
    let object = {
        let mut init = ObjectInitializer::new(context);
        for (member, value) in entries {
            init.property(
                boa_engine::js_string!((*member).to_string()),
                JsValue::from(*value),
                Attribute::all(),
            );
        }
        init.build()
    };
    context.register_global_property(
        boa_engine::js_string!(name.to_string()),
        JsValue::from(object),
        Attribute::all(),
    )
}

/// Installs all nine namespaces and the twenty-two bare globals.
pub(crate) fn install(context: &mut Context) -> JsResult<()> {
    install_strings(context, "border", &BORDER)?;
    install_strings(context, "font", &FONT)?;
    install_strings(context, "highlight", &HIGHLIGHT)?;
    install_strings(context, "style", &STYLE)?;
    install_strings(context, "zoomtype", &ZOOMTYPE)?;
    install_numbers(context, "display", &DISPLAY)?;
    install_numbers(context, "position", &POSITION)?;
    install_numbers(context, "scaleHow", &SCALE_HOW)?;
    install_numbers(context, "scaleWhen", &SCALE_WHEN)?;

    for (name, value) in MESSAGES {
        context.register_global_property(
            boa_engine::js_string!(name),
            JsValue::from(boa_engine::js_string!(value)),
            Attribute::all(),
        )?;
    }
    for (name, alternatives) in PATTERNS {
        let array = boa_engine::object::builtins::JsArray::from_iter(
            alternatives
                .iter()
                .map(|pattern| JsValue::from(boa_engine::js_string!((*pattern).to_string()))),
            context,
        );
        context.register_global_property(
            boa_engine::js_string!(name),
            JsValue::from(array),
            Attribute::all(),
        )?;
    }
    Ok(())
}
