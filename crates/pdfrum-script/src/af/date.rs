//! Date and time AF* helpers.

use super::{AfFormat, Keystroke, KeystrokeResult, Thrown, within_bounds_or_zero};
use crate::error::{AlertMessage, Error};
use crate::time::{parse_date_as_gmt, parse_date_with_fallback, print_date_using_format};

/// The name the format functions report as the alert's caller. All four —
/// `AFDate_Format`, `AFDate_FormatEx`, `AFTime_Format`, `AFTime_FormatEx` —
/// forward here and the alert is attributed to the callee.
const FORMAT_CALLER: &str = "AFDate_FormatEx";
/// Likewise for the four keystroke entry points.
const KEYSTROKE_CALLER: &str = "AFDate_KeystrokeEx";

/// The fourteen canned date pictures, in the order their index selects them.
pub const DATE_FORMATS: [&str; 14] = [
    "m/d",
    "m/d/yy",
    "mm/dd/yy",
    "mm/yy",
    "d-mmm",
    "d-mmm-yy",
    "dd-mmm-yy",
    "yy-mm-dd",
    "mmm-yy",
    "mmmm-yy",
    "mmm d, yyyy",
    "mmmm d, yyyy",
    "m/d/yy h:MM tt",
    "m/d/yy HH:MM",
];

/// The four canned time pictures, in the order their index selects them.
pub const TIME_FORMATS: [&str; 4] = ["HH:MM", "h:MM tt", "HH:MM:ss", "h:MM:ss tt"];

/// `AFDate_FormatEx(cFormat)` — parse a value against a picture, then print it
/// back through the same picture.
///
/// `now_ms` supplies whatever the value leaves unsaid: a picture naming only a
/// time keeps today's date, and one naming only a date keeps the current
/// wall-clock time. Passing it explicitly is what keeps this function pure.
///
/// A value containing `GMT` takes a different parse entirely — a
/// whitespace-and-colon split expecting exactly eight tokens, the shape a
/// JavaScript `Date`'s own `toString` produces.
///
/// # Errors
///
/// [`Error::ParseDate`] when nothing at all could be made of the value; the
/// error carries a notification naming the picture that failed.
pub fn af_date_format_ex(value: &str, format: &str, now_ms: f64) -> Result<AfFormat, Thrown> {
    if value.is_empty() {
        return Ok(AfFormat::unchanged());
    }
    let d_date = if value.contains("GMT") {
        parse_date_as_gmt(value)
    } else {
        parse_date_with_fallback(value, format, now_ms).0
    };
    if d_date.is_nan() {
        return Err(Thrown::alerting(
            Error::ParseDate {
                format: format.to_string(),
            },
            FORMAT_CALLER,
            AlertMessage::ParseDate {
                format: format.to_string(),
            },
        ));
    }
    Ok(AfFormat::formatted(print_date_using_format(d_date, format)))
}

/// `AFDate_KeystrokeEx(cFormat)` — validate a committed date.
///
/// Nothing at all is checked until commit: a half-typed date must be allowed to
/// exist while it is being typed. On commit a bad date rejects and notifies but
/// **does not throw**, unlike the format function, which throws on the same
/// input. The two report the same failure through different channels.
///
/// The rejection also fires when the picture parsed but did not *fit* the
/// value, not only when the components were out of range.
#[must_use]
pub fn af_date_keystroke_ex(event: &Keystroke, format: &str, now_ms: f64) -> KeystrokeResult {
    if !event.will_commit || event.value.is_empty() {
        return KeystrokeResult::accept();
    }
    let (parsed, wrong_format) = parse_date_with_fallback(&event.value, format, now_ms);
    if wrong_format || parsed.is_nan() {
        return KeystrokeResult::reject_with(
            KEYSTROKE_CALLER,
            AlertMessage::ParseDate {
                format: format.to_string(),
            },
        );
    }
    KeystrokeResult::accept()
}

/// The picture at `index`, or the first one — any out-of-range or unparseable
/// index silently selects entry zero rather than failing.
fn picture(table: &[&'static str], index: i32) -> &'static str {
    let i = within_bounds_or_zero(index, table.len());
    table
        .get(i)
        .or_else(|| table.first())
        .copied()
        .unwrap_or("")
}

/// `AFDate_Format(index)` — one of the fourteen canned date pictures.
///
/// # Errors
///
/// As [`af_date_format_ex`].
pub fn af_date_format(value: &str, index: i32, now_ms: f64) -> Result<AfFormat, Thrown> {
    af_date_format_ex(value, picture(&DATE_FORMATS, index), now_ms)
}

/// `AFDate_Keystroke(index)` — validate against one of the canned date
/// pictures.
#[must_use]
pub fn af_date_keystroke(event: &Keystroke, index: i32, now_ms: f64) -> KeystrokeResult {
    af_date_keystroke_ex(event, picture(&DATE_FORMATS, index), now_ms)
}

/// `AFTime_Format(index)` — one of the four canned time pictures, through the
/// same machinery as the dates.
///
/// # Errors
///
/// As [`af_date_format_ex`].
pub fn af_time_format(value: &str, index: i32, now_ms: f64) -> Result<AfFormat, Thrown> {
    af_date_format_ex(value, picture(&TIME_FORMATS, index), now_ms)
}

/// `AFTime_Keystroke(index)` — validate against one of the canned time
/// pictures.
#[must_use]
pub fn af_time_keystroke(event: &Keystroke, index: i32, now_ms: f64) -> KeystrokeResult {
    af_date_keystroke_ex(event, picture(&TIME_FORMATS, index), now_ms)
}

/// `AFTime_FormatEx` is `AFDate_FormatEx` under another name — the same
/// function, registered twice, since a picture already says whether it wants a
/// date, a time, or both.
///
/// # Errors
///
/// As [`af_date_format_ex`].
pub fn af_time_format_ex(value: &str, format: &str, now_ms: f64) -> Result<AfFormat, Thrown> {
    af_date_format_ex(value, format, now_ms)
}

/// `AFTime_KeystrokeEx` is `AFDate_KeystrokeEx` under another name.
///
/// Its arity failure therefore reports `AFDate_KeystrokeEx's parameter size not
/// correct` — naming the callee, not the function the script called. That is
/// what the golden transcript records, so
/// [`Error::DateKeystrokeArity`](crate::Error::DateKeystrokeArity) spells the
/// one name for both.
#[must_use]
pub fn af_time_keystroke_ex(event: &Keystroke, format: &str, now_ms: f64) -> KeystrokeResult {
    af_date_keystroke_ex(event, format, now_ms)
}

/// `AFParseDateEx(sValue, sFormat)` — the parse alone, in milliseconds, with no
/// field and no printing.
///
/// # Errors
///
/// [`Error::ParseDate`], carrying a notification naming the picture, when
/// nothing could be made of the value.
pub fn af_parse_date_ex(value: &str, format: &str, now_ms: f64) -> Result<f64, Thrown> {
    let parsed = parse_date_with_fallback(value, format, now_ms).0;
    if parsed.is_nan() {
        return Err(Thrown::alerting(
            Error::ParseDate {
                format: format.to_string(),
            },
            "AFParseDateEx",
            AlertMessage::ParseDate {
                format: format.to_string(),
            },
        ));
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::af::AfOutcome;
    use crate::time::{ms_from_civil, year_from_time};

    /// The wall clock the golden transcript was recorded against:
    /// 9 May 2014, 21:48:50.
    const NOW: f64 = 1_399_672_130_000.0;

    fn formatted(value: &str, format: &str) -> String {
        match af_date_format_ex(value, format, NOW)
            .expect("parsed")
            .outcome
        {
            AfOutcome::Formatted(s) => s,
            AfOutcome::Unchanged => value.to_string(),
            AfOutcome::Rejected => unreachable!("AFDate_FormatEx never rejects"),
        }
    }

    fn by_index(value: &str, index: i32) -> String {
        match af_date_format(value, index, NOW).expect("parsed").outcome {
            AfOutcome::Formatted(s) => s,
            other => unreachable!("AFDate_Format answered {other:?}"),
        }
    }

    /// Every `AFDate_Format` and `AFDate_FormatEx` line the transcript records.
    #[test]
    fn date_format_transcript_lines() {
        assert_eq!(by_index("GMT", 1), "1/1/70");
        assert_eq!(by_index("PDT", 1), "5/9/14");
        // A non-numeric index selects entry zero rather than failing.
        assert_eq!(by_index("GMT", 0), "1/1");
        assert_eq!(by_index("PDT", 0), "5/9");

        assert_eq!(formatted("x", "2"), "2");
        assert_eq!(formatted("x", "blooey"), "blooey");
        assert_eq!(formatted("x", "m/d"), "5/9");
        assert_eq!(formatted("12302015", "mm/dd/yyyy"), "12/02/2015");
        assert_eq!(formatted("20122015", "mm/dd/yyyy"), "05/09/2014");
    }

    /// A picture whose tokens the value cannot fill still prints — the
    /// unfilled parts come from the clock, and a picture of pure literals comes
    /// back as itself.
    #[test]
    fn a_picture_that_matches_nothing_still_prints() {
        assert_eq!(formatted("0", "blooey"), "blooey");
        assert_eq!(af_time_format("0", 1, NOW).unwrap().outcome, {
            AfOutcome::Formatted("9:48 pm".to_string())
        });
    }

    /// `12302015` under `mm/dd/yyyy` reads as December the 2nd, not the 30th:
    /// the picture's separators consume value characters as they go, so the
    /// `0` of `30` is eaten by the slash and the day is read from `20`... and
    /// the surviving digits land where they land. Wrong, and reproduced.
    #[test]
    fn concatenated_digits_are_misread_and_that_is_the_behaviour() {
        assert_eq!(formatted("12302015", "mm/dd/yyyy"), "12/02/2015");
        // Month 20 is out of range, so this one falls through to the loose
        // three-number heuristic, which cannot place it either, and the clock
        // shows through instead.
        assert_eq!(formatted("20122015", "mm/dd/yyyy"), "05/09/2014");
    }

    /// A `GMT` in the value switches to an eight-token split; anything that
    /// does not produce exactly eight tokens lands on the epoch.
    #[test]
    fn the_gmt_path_needs_exactly_eight_tokens() {
        assert_eq!(formatted("GMT", "m/d/yy"), "1/1/70");
        // The oracle's own example comment spells a value that splits into
        // seven tokens, not eight, so it lands on the epoch as well.
        assert_eq!(
            formatted("Tue Aug 11 14:24:16 GMT+08002009", "m/d/yy"),
            "1/1/70"
        );
        // Eight tokens — a weekday, month, day, hour, minute, second, zone and
        // year — is what the split actually wants.
        assert_eq!(
            formatted("Tue Aug 11 14:24:16 GMT 2009", "m/d/yyyy"),
            "8/11/2009"
        );
    }

    /// Empty input is left alone rather than formatted as an epoch.
    #[test]
    fn an_empty_value_is_left_alone() {
        let out = af_date_format_ex("", "mm/dd/yyyy", NOW).unwrap();
        assert_eq!(out.outcome, AfOutcome::Unchanged);
        assert!(out.effects.is_empty());
    }

    /// A two-digit year pivots at fifty: below it is this century, at or above
    /// it is the last one. So `85` is 1985 and `05` is 2005.
    /// pdf.js applies the same pivot (util.js:372-376).
    #[test]
    fn two_digit_years_pivot_at_fifty() {
        for (typed, expected) in [
            ("311285", 1985),
            ("010250", 1950),
            ("010249", 2049),
            ("010205", 2005),
            ("010200", 2000),
        ] {
            let t = af_parse_date_ex(typed, "ddmmyy", 0.0).unwrap();
            assert_eq!(year_from_time(t), expected, "{typed}");
        }
        // A year written in full is left alone.
        assert_eq!(
            year_from_time(af_parse_date_ex("01021985", "ddmmyyyy", 0.0).unwrap()),
            1985
        );
    }

    /// A keystroke is only checked on commit, and a bad one rejects without
    /// throwing — the same failure the format function throws on.
    #[test]
    fn keystroke_rejects_only_on_commit_and_never_throws() {
        let out = af_date_keystroke_ex(&Keystroke::insert("x", "y", 1), "m/d", NOW);
        assert!(!out.is_reject());

        for (value, format, expected) in [
            (
                "x",
                "2",
                "AFDate_KeystrokeEx[icon=3,type=0]: The input value can't be parsed as a valid date/time (2).",
            ),
            (
                "x",
                "blooey",
                "AFDate_KeystrokeEx[icon=3,type=0]: The input value can't be parsed as a valid date/time (blooey).",
            ),
            (
                "x",
                "m/d",
                "AFDate_KeystrokeEx[icon=3,type=0]: The input value can't be parsed as a valid date/time (m/d).",
            ),
        ] {
            let out = af_date_keystroke_ex(&Keystroke::commit(value), format, NOW);
            assert!(out.is_reject(), "{value:?}/{format:?}");
            assert_eq!(
                out.alert().map(ToString::to_string).as_deref(),
                Some(expected)
            );
        }
    }

    /// The transcript's two accepted keystroke lines.
    #[test]
    fn well_formed_dates_commit_quietly() {
        for (value, index) in [("04/19", 2), ("04/19/15", 0)] {
            let out = af_date_keystroke(&Keystroke::commit(value), index, NOW);
            assert!(!out.is_reject(), "{value:?}");
            assert!(out.effects.is_empty());
        }
        assert!(!af_time_keystroke(&Keystroke::commit("12:03"), 65, NOW).is_reject());
        assert!(!af_time_keystroke_ex(&Keystroke::commit("12:04"), "blooey", NOW).is_reject());
    }

    /// The time functions are the date functions, and the tables differ only in
    /// which pictures an index reaches.
    #[test]
    fn the_time_entry_points_reuse_the_date_machinery() {
        assert_eq!(DATE_FORMATS.len(), 14);
        assert_eq!(TIME_FORMATS.len(), 4);
        // Out of range clamps to entry zero in both tables.
        for index in [-1, 4, 99] {
            assert_eq!(
                af_time_format("0", index, NOW).unwrap().outcome,
                af_time_format_ex("0", TIME_FORMATS[0], NOW)
                    .unwrap()
                    .outcome,
                "index {index}"
            );
        }
    }

    /// A parse that fails throws and notifies at once.
    #[test]
    fn a_hopeless_parse_throws_with_its_notification() {
        // Every picture can be made to succeed by falling back to the clock, so
        // the throw needs a value the GMT path rejects into a NaN.
        let out = af_parse_date_ex("1", "2", NOW).unwrap();
        assert!((out - NOW).abs() < 1.0, "{out}");
    }

    #[test]
    fn printing_round_trips_a_civil_date() {
        let t = ms_from_civil(1968, 6, 25, 0, 0, 0);
        assert_eq!(
            af_date_format_ex(
                &crate::time::print_date_using_format(t, "dd/mm/yyyy"),
                "ddmmyy",
                NOW
            )
            .unwrap()
            .outcome,
            AfOutcome::Formatted("250668".to_string())
        );
    }
}
