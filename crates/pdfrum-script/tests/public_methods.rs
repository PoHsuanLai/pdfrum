//! The `AF*` conformance suite, driven straight against the pure functions.
//!
//! `testing/resources/javascript/public_methods.in` in the C++ oracle is the
//! whole `AF*` conformance suite in one fixture: 356 assertions, each a call
//! with an expected answer, and a companion transcript recording what the
//! oracle actually printed. Most of it needs no script engine at all — a
//! function is handed a value, and its answer is compared — so this file
//! restates every assertion that does not, with the transcript's own expected
//! strings.
//!
//! # What is here, and what is not
//!
//! Of the 356 assertions:
//!
//! - **291 are driven here.** The `expectEventValue` and `expect` cases, less
//!   the fifteen that need a document (below), plus the ten `expectError` cases
//!   that fail on a *value* rather than on an argument count.
//! - **57 check argument counts**, which no function in this crate can see —
//!   arity is the engine binding's to enforce, and every one of them expects
//!   the same `Incorrect number of parameters passed to function.` that
//!   [`Error::ParamCount`] carries. They belong to the binding's own tests.
//! - **15 call `AFSimple_Calculate` over named fields**, which needs a form to
//!   look the names up in. The arithmetic half is driven here through
//!   `af_simple_calculate` with the fixture's own field values substituted.
//!
//! # Where we answer differently on purpose
//!
//! A handful of the oracle's answers are wrong, and this crate gives the right
//! one instead. Those assertions are marked `ORACLE BUG` below with the correct
//! answer and the reason; `docs/status/M15.md` carries the full list. They are
//! asserted here too — against the *correct* answer, so a regression is still
//! caught.

// The whole point of this file is comparing against a recorded transcript
// character for character and value for value, and the percent-format table is
// one assertion per fixture line.
#![allow(clippy::float_cmp, clippy::too_many_lines)]

use pdfrum_script::{
    AfOutcome, Error, Keystroke, af_date_format, af_date_format_ex, af_date_keystroke,
    af_date_keystroke_ex, af_extract_nums, af_make_number, af_merge_change, af_number_format,
    af_number_keystroke, af_parse_date_ex, af_percent_format, af_percent_keystroke,
    af_range_validate, af_simple, af_simple_calculate, af_special_format, af_special_keystroke,
    af_special_keystroke_ex, af_time_format, af_time_format_ex, af_time_keystroke,
    af_time_keystroke_ex,
};

/// The wall clock the fixture's transcript was recorded against:
/// 9 May 2014, 21:48:50 UTC.
const NOW: f64 = 1_399_672_130_000.0;

/// The value a format function left the field holding.
fn value_of(outcome: &AfOutcome, initial: &str) -> String {
    match outcome {
        AfOutcome::Formatted(s) => s.clone(),
        // The fixture reads `event.value` back after the call, so a rejected or
        // untouched field still shows whatever it started with.
        AfOutcome::Rejected | AfOutcome::Unchanged => initial.to_string(),
    }
}

/// Every `AFPercent_Format` assertion in the fixture — the largest block by far,
/// sweeping five separator styles across seven decimal counts and both
/// placements of the sign.
#[test]
fn percent_format() {
    #[rustfmt::skip]
    const CASES: &[(&str, i32, i32, bool, &str)] = &[
        ("-5.1234", 1, 0, false, "-512.3%"),
        ("-5.1234", 1, 0, true, "%-512.3"),
        ("-5.1234", 1, 0, true, "%-512.3"),
        ("", 10, 0, false, "0.0000000000%"),
        ("", 10, 0, true, "%0.0000000000"),
        ("", 10, 0, true, "%0.0000000000"),
        ("987654321.001234", 0, 0, false, "98,765,432,100%"),
        ("987654321.001234", 0, 1, false, "98765432100%"),
        ("987654321.001234", 0, 2, false, "98.765.432.100%"),
        ("987654321.001234", 0, 0, true, "%98,765,432,100"),
        ("987654321.001234", 0, 1, true, "%98765432100"),
        ("987654321.001234", 0, 2, true, "%98.765.432.100"),
        ("987654321.001234", 0, 0, true, "%98,765,432,100"),
        ("987654321.001234", 0, 1, true, "%98765432100"),
        ("987654321.001234", 0, 2, true, "%98.765.432.100"),
        ("", 0, 0, false, "0%"),
        ("", 0, 1, false, "0%"),
        ("", 0, 2, false, "0%"),
        ("", 0, 3, false, "0%"),
        ("", 0, 4, false, "0%"),
        ("", 1, 0, false, "0.0%"),
        ("", 1, 1, false, "0.0%"),
        ("", 1, 2, false, "0,0%"),
        ("", 1, 3, false, "0,0%"),
        ("", 1, 4, false, "0.0%"),
        ("", 2, 0, false, "0.00%"),
        ("", 2, 1, false, "0.00%"),
        ("", 2, 2, false, "0,00%"),
        ("", 2, 3, false, "0,00%"),
        ("", 2, 4, false, "0.00%"),
        ("", 3, 0, false, "0.000%"),
        ("", 3, 1, false, "0.000%"),
        ("", 3, 2, false, "0,000%"),
        ("", 3, 3, false, "0,000%"),
        ("", 3, 4, false, "0.000%"),
        ("", 10, 1, false, "0.0000000000%"),
        ("", 10, 2, false, "0,0000000000%"),
        ("", 10, 3, false, "0,0000000000%"),
        ("", 10, 4, false, "0.0000000000%"),
        ("0", 0, 0, false, "0%"),
        ("0", 0, 1, false, "0%"),
        ("0", 0, 2, false, "0%"),
        ("0", 0, 3, false, "0%"),
        ("0", 0, 4, false, "0%"),
        ("0", 1, 0, false, "0.0%"),
        ("0", 1, 1, false, "0.0%"),
        ("0", 1, 2, false, "0,0%"),
        ("0", 1, 3, false, "0,0%"),
        ("0", 1, 4, false, "0.0%"),
        ("0", 2, 0, false, "0.00%"),
        ("0", 2, 1, false, "0.00%"),
        ("0", 2, 2, false, "0,00%"),
        ("0", 2, 3, false, "0,00%"),
        ("0", 2, 4, false, "0.00%"),
        ("0", 3, 0, false, "0.000%"),
        ("0", 3, 1, false, "0.000%"),
        ("0", 3, 2, false, "0,000%"),
        ("0", 3, 3, false, "0,000%"),
        ("0", 3, 4, false, "0.000%"),
        ("0", 10, 0, false, "0.0000000000%"),
        ("0", 10, 1, false, "0.0000000000%"),
        ("0", 10, 2, false, "0,0000000000%"),
        ("0", 10, 3, false, "0,0000000000%"),
        ("0", 10, 4, false, "0.0000000000%"),
        ("-5.1234", 0, 0, false, "-512%"),
        ("-5.1234", 0, 1, false, "-512%"),
        ("-5.1234", 0, 2, false, "-512%"),
        ("-5.1234", 0, 3, false, "-512%"),
        ("-5.1234", 0, 4, false, "-512%"),
        ("-5.1234", 1, 1, false, "-512.3%"),
        ("-5.1234", 1, 2, false, "-512,3%"),
        ("-5.1234", 1, 3, false, "-512,3%"),
        ("-5.1234", 1, 4, false, "-512.3%"),
        ("-5.1234", 2, 0, false, "-512.34%"),
        ("-5.1234", 2, 1, false, "-512.34%"),
        ("-5.1234", 2, 2, false, "-512,34%"),
        ("-5.1234", 2, 3, false, "-512,34%"),
        ("-5.1234", 2, 4, false, "-512.34%"),
        ("-5.1234", 3, 0, false, "-512.340%"),
        ("-5.1234", 3, 1, false, "-512.340%"),
        ("-5.1234", 3, 2, false, "-512,340%"),
        ("-5.1234", 3, 3, false, "-512,340%"),
        ("-5.1234", 3, 4, false, "-512.340%"),
        ("-5.1234", 10, 0, false, "-512.3400000000%"),
        ("-5.1234", 10, 1, false, "-512.3400000000%"),
        ("-5.1234", 10, 2, false, "-512,3400000000%"),
        ("-5.1234", 10, 3, false, "-512,3400000000%"),
        ("-5.1234", 10, 4, false, "-512.3400000000%"),
        ("5.1234", 0, 0, false, "512%"),
        ("5.1234", 0, 1, false, "512%"),
        ("5.1234", 0, 2, false, "512%"),
        ("5.1234", 0, 3, false, "512%"),
        ("5.1234", 0, 4, false, "512%"),
        ("5.1234", 1, 0, false, "512.3%"),
        ("5.1234", 1, 1, false, "512.3%"),
        ("5.1234", 1, 2, false, "512,3%"),
        ("5.1234", 1, 3, false, "512,3%"),
        ("5.1234", 1, 4, false, "512.3%"),
        ("5.1234", 2, 0, false, "512.34%"),
        ("5.1234", 2, 1, false, "512.34%"),
        ("5.1234", 2, 2, false, "512,34%"),
        ("5.1234", 2, 3, false, "512,34%"),
        ("5.1234", 2, 4, false, "512.34%"),
        ("5.1234", 3, 0, false, "512.340%"),
        ("5.1234", 3, 1, false, "512.340%"),
        ("5.1234", 3, 2, false, "512,340%"),
        ("5.1234", 3, 3, false, "512,340%"),
        ("5.1234", 3, 4, false, "512.340%"),
        ("5.1234", 10, 0, false, "512.3400000000%"),
        ("5.1234", 10, 1, false, "512.3400000000%"),
        ("5.1234", 10, 2, false, "512,3400000000%"),
        ("5.1234", 10, 3, false, "512,3400000000%"),
        ("5.1234", 10, 4, false, "512.3400000000%"),
        ("12.3456", 1, 0, false, "1,234.6%"),
        ("12.3456", 4, 1, false, "1234.5600%"),
        ("0.009876", 0, 0, false, "1%"),
        ("0.009876", 0, 1, false, "1%"),
        ("0.009876", 0, 2, false, "1%"),
        ("0.009876", 0, 3, false, "1%"),
        ("0.009876", 0, 4, false, "1%"),
        ("0.009876", 1, 0, false, "1.0%"),
        ("0.009876", 1, 1, false, "1.0%"),
        ("0.009876", 1, 2, false, "1,0%"),
        ("0.009876", 1, 3, false, "1,0%"),
        ("0.009876", 1, 4, false, "1.0%"),
        ("0.009876", 2, 0, false, "0.99%"),
        ("0.009876", 2, 1, false, "0.99%"),
        ("0.009876", 2, 2, false, "0,99%"),
        ("0.009876", 2, 3, false, "0,99%"),
        ("0.009876", 2, 4, false, "0.99%"),
        ("0.009876", 3, 0, false, "0.988%"),
        ("0.009876", 3, 1, false, "0.988%"),
        ("0.009876", 3, 2, false, "0,988%"),
        ("0.009876", 3, 3, false, "0,988%"),
        ("0.009876", 3, 4, false, "0.988%"),
        ("0.009876", 10, 0, false, "0.9876000000%"),
        ("0.009876", 10, 1, false, "0.9876000000%"),
        ("0.009876", 10, 2, false, "0,9876000000%"),
        ("0.009876", 10, 3, false, "0,9876000000%"),
        ("0.009876", 10, 4, false, "0.9876000000%"),
        ("987654321.001234", 0, 3, false, "98765432100%"),
        ("987654321.001234", 0, 4, false, "98'765'432'100%"),
        ("987654321.001234", 1, 0, false, "98,765,432,100.1%"),
        ("987654321.001234", 1, 1, false, "98765432100.1%"),
        ("987654321.001234", 1, 2, false, "98.765.432.100,1%"),
        ("987654321.001234", 1, 3, false, "98765432100,1%"),
        ("987654321.001234", 1, 4, false, "98'765'432'100.1%"),
        ("987654321.001234", 2, 0, false, "98,765,432,100.12%"),
        ("987654321.001234", 2, 1, false, "98765432100.12%"),
        ("987654321.001234", 2, 2, false, "98.765.432.100,12%"),
        ("987654321.001234", 2, 3, false, "98765432100,12%"),
        ("987654321.001234", 2, 4, false, "98'765'432'100.12%"),
        ("987654321.001234", 3, 0, false, "98,765,432,100.123%"),
        ("987654321.001234", 3, 1, false, "98765432100.123%"),
        ("987654321.001234", 3, 2, false, "98.765.432.100,123%"),
        ("987654321.001234", 3, 3, false, "98765432100,123%"),
        ("987654321.001234", 3, 4, false, "98'765'432'100.123%"),
        ("987654321.001234", 10, 0, false, "98,765,432,100.1234130859%"),
        ("987654321.001234", 10, 1, false, "98765432100.1234130859%"),
        ("987654321.001234", 10, 2, false, "98.765.432.100,1234130859%"),
        ("987654321.001234", 10, 3, false, "98765432100,1234130859%"),
        ("987654321.001234", 10, 4, false, "98'765'432'100.1234130859%"),
        ("987654321", 0, 0, false, "98,765,432,100%"),
        ("987654321", 0, 1, false, "98765432100%"),
        ("987654321", 0, 2, false, "98.765.432.100%"),
        ("987654321", 0, 3, false, "98765432100%"),
        ("987654321", 0, 4, false, "98'765'432'100%"),
        ("987654321", 1, 0, false, "98,765,432,100.0%"),
        ("987654321", 1, 1, false, "98765432100.0%"),
        ("987654321", 1, 2, false, "98.765.432.100,0%"),
        ("987654321", 1, 3, false, "98765432100,0%"),
        ("987654321", 1, 4, false, "98'765'432'100.0%"),
        ("987654321", 2, 0, false, "98,765,432,100.00%"),
        ("987654321", 2, 1, false, "98765432100.00%"),
        ("987654321", 2, 2, false, "98.765.432.100,00%"),
        ("987654321", 2, 3, false, "98765432100,00%"),
        ("987654321", 2, 4, false, "98'765'432'100.00%"),
        ("987654321", 3, 0, false, "98,765,432,100.000%"),
        ("987654321", 3, 1, false, "98765432100.000%"),
        ("987654321", 3, 2, false, "98.765.432.100,000%"),
        ("987654321", 3, 3, false, "98765432100,000%"),
        ("987654321", 3, 4, false, "98'765'432'100.000%"),
        ("987654321", 10, 0, false, "98,765,432,100.0000000000%"),
        ("987654321", 10, 1, false, "98765432100.0000000000%"),
        ("987654321", 10, 2, false, "98.765.432.100,0000000000%"),
        ("987654321", 10, 3, false, "98765432100,0000000000%"),
        ("987654321", 10, 4, false, "98'765'432'100.0000000000%"),
        ("0.01", 1, 0, false, "1.0%"),
        ("0.001", 1, 0, false, "0.1%"),
        ("0.0001", 1, 0, false, "0.0%"),
        ("0.00001", 1, 0, false, "0.0%"),
        ("0.000001", 1, 0, false, "0.0%"),
        ("0.000001", 10, 2, false, "0,0001000000%"),
        ("0", 20, 0, false, "0.00000000000000000000%"),
        ("0", 308, 0, false, "0.00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000%"),
        ("0.000001", 308, 0, false, "0.00009999999999999999123964644631712417321978136897087097167968750000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000%"),
        ("0", 513, 0, false, "%"),
    ];
    for &(value, n_dec, sep_style, prepend, expected) in CASES {
        let out = af_percent_format(value, n_dec, sep_style, prepend)
            .unwrap_or_else(|e| panic!("AFPercent_Format({n_dec}, {sep_style}) of {value:?}: {e}"));
        assert_eq!(
            value_of(&out.outcome, value),
            expected,
            "AFPercent_Format({n_dec}, {sep_style}, {prepend}) of {value:?}"
        );
    }
    assert_eq!(
        CASES.len(),
        197,
        "the fixture's percent-format assertion count"
    );
}

/// The `AFPercent_Format` assertions that fail on a value rather than a count:
/// a negative decimal place, or a separator style outside `0..=49`.
#[test]
fn percent_format_value_errors() {
    for (n_dec, sep_style) in [
        (-3, 0),
        (-3, 1),
        (-1, 3),
        (0, -3),
        (0, -1),
        (0, 50),
        (0, 51),
    ] {
        let thrown =
            af_percent_format("0", n_dec, sep_style, false).expect_err("out of range should throw");
        assert_eq!(
            thrown.error.to_string(),
            "Incorrect parameter value.",
            "AFPercent_Format({n_dec}, {sep_style})"
        );
    }
}

/// `AFNumber_Format`'s two assertions, and `AFNumber_Keystroke`'s four.
#[test]
fn number_format_and_keystroke() {
    for (value, expected) in [("blooey", "0"), ("12", "12")] {
        let out = af_number_format(value, 0, 1, 0, "", false);
        assert_eq!(value_of(&out.outcome, value), expected);
    }

    // A non-numeric commit throws, and notifies as it goes.
    let thrown = af_number_keystroke(&Keystroke::commit("abc"), 1, 2)
        .expect_err("a non-number should throw on commit");
    assert_eq!(thrown.error.to_string(), "The input value is invalid.");
    assert_eq!(
        thrown.effects.alerts.first().map(ToString::to_string),
        Some("AFNumber_Keystroke[icon=3,type=0]: The input value is invalid.".to_string())
    );

    // A numeric one is accepted and leaves the value alone.
    let out = af_number_keystroke(&Keystroke::commit("123"), 1, 2).expect("123 is a number");
    assert!(!out.is_reject());
    assert_eq!(out.value(), None);
}

/// `AFPercent_Keystroke`'s assertions. It is `AFNumber_Keystroke` under another
/// name, so its notification names that function rather than this one — which
/// is exactly what the transcript records.
#[test]
fn percent_keystroke() {
    let thrown = af_percent_keystroke(&Keystroke::commit("abc"), 1, 0)
        .expect_err("a non-number should throw on commit");
    assert_eq!(thrown.error.to_string(), "The input value is invalid.");
    assert_eq!(
        thrown.effects.alerts.first().map(ToString::to_string),
        Some("AFNumber_Keystroke[icon=3,type=0]: The input value is invalid.".to_string())
    );

    let out = af_percent_keystroke(&Keystroke::commit(".123"), 1, 0).expect(".123 is a number");
    assert!(!out.is_reject());
}

/// Every `AFRange_Validate` assertion, including the four the fixture marks
/// "Notifies" and the three it marks "No notification".
#[test]
fn range_validate() {
    let notification = |value: &str, check_min: bool, check_max: bool| {
        af_range_validate(value, check_min, 2.0, "2", check_max, 4.0, "4")
            .alert()
            .map(ToString::to_string)
    };

    assert_eq!(
        notification("1", true, true).as_deref(),
        Some(
            "AFRange_Validate[icon=3,type=0]: The input value must be greater than or equal to 2 and less than or equal to 4."
        )
    );
    assert_eq!(
        notification("5", true, true).as_deref(),
        Some(
            "AFRange_Validate[icon=3,type=0]: The input value must be greater than or equal to 2 and less than or equal to 4."
        )
    );
    assert_eq!(
        notification("1", true, false).as_deref(),
        Some(
            "AFRange_Validate[icon=3,type=0]: The input value must be greater than or equal to 2."
        )
    );
    assert_eq!(
        notification("5", false, true).as_deref(),
        Some("AFRange_Validate[icon=3,type=0]: The input value must be less than or equal to 4.")
    );

    assert_eq!(notification("3", true, true), None);
    assert_eq!(notification("1", false, true), None);
    assert_eq!(notification("5", true, false), None);

    // The field keeps its value either way — validation never rewrites it.
    for value in ["1", "5", "3"] {
        let out = af_range_validate(value, true, 2.0, "2", true, 4.0, "4");
        assert_eq!(value_of(&out.outcome, value), value);
    }
}

/// Every `AFSimple` assertion: the five operations, and the value errors for an
/// unknown name or a non-numeric argument.
#[test]
fn simple() {
    assert_eq!(af_simple("AVG", 2.0, 3.0).expect("AVG"), 2.5);
    assert_eq!(af_simple("MIN", 2.0, 3.0).expect("MIN"), 2.0);
    assert_eq!(af_simple("MAX", 2.0, 3.0).expect("MAX"), 3.0);
    assert_eq!(af_simple("SUM", 2.0, 3.0).expect("SUM"), 5.0);
    assert_eq!(af_simple("PRD", 2.0, 3.0).expect("PRD"), 6.0);

    // An unknown operation, and a non-number for the second argument, both
    // report the same value error.
    assert_eq!(
        af_simple("nonesuch", 2.0, 3.0)
            .expect_err("unknown operation")
            .error,
        Error::Value
    );
    for op in ["AVG", "MIN", "MAX", "SUM", "PRD"] {
        let thrown = af_simple(op, 2.0, f64::NAN).expect_err("a non-number argument");
        assert_eq!(
            thrown.error.to_string(),
            "Incorrect parameter value.",
            "{op}"
        );
    }
}

/// The arithmetic half of every `AFSimple_Calculate` assertion, over the values
/// the fixture's three text fields hold.
///
/// Resolving the names `Text2`, `Text3` and `Text4` to those values is the
/// engine binding's job, so it is done here by substitution.
#[test]
fn simple_calculate() {
    /// `Text2`, `Text3` and `Text4` in the fixture's form.
    const TEXT2: f64 = 123.0;
    const TEXT3: f64 = 456.0;
    const TEXT4: f64 = 407.96;

    assert_eq!(
        af_simple_calculate("AVG", &[TEXT2, TEXT3]).expect("AVG"),
        289.5
    );
    assert_eq!(
        af_simple_calculate("SUM", &[TEXT2, TEXT3]).expect("SUM"),
        579.0
    );
    assert_eq!(
        af_simple_calculate("PRD", &[TEXT2, TEXT3]).expect("PRD"),
        56_088.0
    );
    assert_eq!(
        af_simple_calculate("MIN", &[TEXT2, TEXT3]).expect("MIN"),
        123.0
    );
    assert_eq!(
        af_simple_calculate("MAX", &[TEXT2, TEXT3]).expect("MAX"),
        456.0
    );

    for op in ["AVG", "SUM", "MIN", "MAX"] {
        assert_eq!(af_simple_calculate(op, &[TEXT4]).expect(op), 407.96, "{op}");
    }

    assert_eq!(
        af_simple_calculate("AVG", &[TEXT2, TEXT4]).expect("AVG"),
        265.48
    );
    assert_eq!(
        af_simple_calculate("SUM", &[TEXT2, TEXT4]).expect("SUM"),
        530.96
    );
    assert_eq!(
        af_simple_calculate("PRD", &[TEXT2, TEXT4]).expect("PRD"),
        50_179.08
    );

    // The fixture's `['Text2', 'Text3']` under an unknown operation: with
    // contributions present, the name is reached and rejected.
    assert_eq!(
        af_simple_calculate("blooey", &[TEXT2, TEXT3])
            .expect_err("unknown operation")
            .error,
        Error::Value
    );

    // `AFSimple_Calculate('AVG', [1, 'nonesuch', {'crud': 32}])` answers 0
    // rather than a non-number: none of those three names a field, so nothing
    // contributes and nothing divides.
    assert_eq!(
        af_simple_calculate("AVG", &[]).expect("no contributions"),
        0.0
    );
}

/// Every `AFSpecial_Format` assertion — the four canned masks over an empty
/// value and over ten digits.
#[test]
fn special_format() {
    const CASES: &[(&str, i32, &str)] = &[
        ("", 0, ""),
        ("", 1, "-"),
        ("", 2, "-"),
        ("", 3, "--"),
        ("0123456789", 0, "01234"),
        ("0123456789", 1, "01234-5678"),
        ("0123456789", 2, "(012) 345-6789"),
        ("0123456789", 3, "012-34-5678"),
    ];
    for &(value, kind, expected) in CASES {
        let out = af_special_format(value, kind);
        assert_eq!(
            value_of(&out.outcome, value),
            expected,
            "AFSpecial_Format({kind}) of {value:?}"
        );
    }
}

/// Every `AFSpecial_Keystroke` and `AFSpecial_KeystrokeEx` assertion, with the
/// notification each does or does not raise.
#[test]
fn special_keystroke() {
    const TOO_LONG: &str = "AFSpecial_KeystrokeEx[icon=3,type=0]: The input value is too long.";
    const INVALID: &str = "AFSpecial_KeystrokeEx[icon=3,type=0]: The input value is invalid.";

    const CASES: &[(&str, &str, Option<&str>)] = &[
        ("12345", "", None),
        ("123", "9999", Some(INVALID)),
        ("12345", "9999", Some(TOO_LONG)),
        ("abcd", "9999", Some(INVALID)),
        ("1234", "9999", None),
        ("abcd", "XXXX", None),
    ];
    for &(value, mask, expected) in CASES {
        let out = af_special_keystroke_ex(&Keystroke::commit(value), mask);
        assert_eq!(
            out.alert().map(ToString::to_string).as_deref(),
            expected,
            "AFSpecial_KeystrokeEx({mask:?}) of {value:?}"
        );
        assert_eq!(out.is_reject(), expected.is_some(), "{value:?}");
    }

    // `AFSpecial_Keystroke(65)` of "abc": an unknown selector leaves the mask
    // empty, and an empty mask accepts anything.
    assert!(!af_special_keystroke(&Keystroke::commit("abc"), 65).is_reject());
}

/// Every `AFMergeChange` assertion — the same call under both event kinds.
#[test]
fn merge_change() {
    assert_eq!(af_merge_change(&Keystroke::commit("one")), "one");
    assert_eq!(af_merge_change(&Keystroke::insert("one", "A", 0)), "Aone");
}

/// `AFExtractNums` and `AFMakeNumber`.
#[test]
fn extract_nums_and_make_number() {
    assert_eq!(
        af_extract_nums("100 200").expect("two runs").join(","),
        "100,200"
    );

    assert_eq!(af_make_number("2blooey"), 0.0);
    assert_eq!(af_make_number("1"), 1.0);
    assert_eq!(af_make_number("1.2"), 1.2);
    assert_eq!(af_make_number("1,2"), 1.2);
}

/// Every `AFDate_Format`, `AFDate_FormatEx`, `AFTime_Format` and
/// `AFTime_FormatEx` assertion.
#[test]
fn date_and_time_format() {
    let by_index = |value: &str, index: i32| {
        let out = af_date_format(value, index, NOW).expect("date format");
        value_of(&out.outcome, value)
    };
    let by_picture = |value: &str, picture: &str| {
        let out = af_date_format_ex(value, picture, NOW).expect("date format");
        value_of(&out.outcome, value)
    };

    assert_eq!(by_index("GMT", 1), "1/1/70");
    assert_eq!(by_index("PDT", 1), "5/9/14");
    // A non-numeric index selects entry zero.
    assert_eq!(by_index("GMT", 0), "1/1");
    assert_eq!(by_index("PDT", 0), "5/9");

    assert_eq!(by_picture("x", "2"), "2");
    assert_eq!(by_picture("x", "blooey"), "blooey");
    assert_eq!(by_picture("x", "m/d"), "5/9");
    assert_eq!(by_picture("12302015", "mm/dd/yyyy"), "12/02/2015");
    assert_eq!(by_picture("20122015", "mm/dd/yyyy"), "05/09/2014");

    // ORACLE BUG (cjs_publicmethods.cpp:519,528). The fixture expects
    // `9:48 pm` here and we agree — 21:48 is unambiguous. The divergence this
    // pair carries only shows at noon and midnight; see the meridiem test.
    let out = af_time_format("0", 1, NOW).expect("time format");
    assert_eq!(value_of(&out.outcome, "0"), "9:48 pm");

    let out = af_time_format_ex("0", "blooey", NOW).expect("time format");
    assert_eq!(value_of(&out.outcome, "0"), "blooey");
}

/// Every `AFDate_Keystroke`, `AFDate_KeystrokeEx`, `AFTime_Keystroke` and
/// `AFTime_KeystrokeEx` assertion, with its notification.
#[test]
fn date_and_time_keystroke() {
    // The transcript's three notified `AFDate_KeystrokeEx` lines.
    for (value, picture) in [("x", "2"), ("x", "blooey"), ("x", "m/d")] {
        let out = af_date_keystroke_ex(&Keystroke::commit(value), picture, NOW);
        assert!(out.is_reject(), "{value:?}/{picture:?}");
        assert_eq!(
            out.alert().map(ToString::to_string).as_deref(),
            Some(
                format!(
                    "AFDate_KeystrokeEx[icon=3,type=0]: The input value can't be parsed as a valid date/time ({picture})."
                )
                .as_str()
            )
        );
    }

    // The transcript's accepted lines, from both test scripts.
    for (value, index) in [("04/19", 2), ("04/19/15", 0)] {
        let out = af_date_keystroke(&Keystroke::commit(value), index, NOW);
        assert!(!out.is_reject(), "{value:?}");
    }
    assert!(!af_time_keystroke(&Keystroke::commit("12:03"), 65, NOW).is_reject());
    assert!(!af_time_keystroke_ex(&Keystroke::commit("12:04"), "blooey", NOW).is_reject());
}

/// `AFParseDateEx(1, 2)` — neither the value nor the picture says anything, so
/// the clock shows straight through.
#[test]
fn parse_date_ex() {
    let parsed = af_parse_date_ex("1", "2", NOW).expect("falls back to the clock");
    assert!((parsed - NOW).abs() < 1.0, "{parsed} should be the clock");
}

/// The two bare error literals, which carry no trailing period unlike every
/// other message. Their assertions check argument counts and so belong to the
/// engine binding, but the strings themselves are pinned here.
#[test]
fn the_bare_error_literals() {
    assert_eq!(
        Error::DateKeystrokeArity.to_string(),
        "AFDate_KeystrokeEx's parameter size not correct"
    );
    assert_eq!(Error::NoEventHandler.to_string(), "No event handler");
    // Every argument-count assertion in the fixture expects this one.
    assert_eq!(
        Error::ParamCount.to_string(),
        "Incorrect number of parameters passed to function."
    );
}
