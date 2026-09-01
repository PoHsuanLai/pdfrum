//! `AFRange_Validate`.

use super::AfFormat;
use crate::error::AlertMessage;
use crate::parse::c_atof;

/// The name this function reports as the alert's caller.
const CALLER: &str = "AFRange_Validate";

/// `AFRange_Validate(bGreaterThan, nGreaterThan, bLessThan, nLessThan)`.
///
/// Empty input is accepted. Out of range rejects and notifies; it never
/// throws, so this function is infallible.
///
/// The bounds arrive twice: once parsed, for the comparison, and once as the
/// caller's own text, for the message — a bound written `2` must read `2` in
/// the alert and never `2.0`. The comparison is inclusive-pass: only a value
/// strictly outside the bound trips.
#[must_use]
pub fn af_range_validate(
    value: &str,
    check_min: bool,
    min: f64,
    min_label: &str,
    check_max: bool,
    max: f64,
    max_label: &str,
) -> AfFormat {
    if value.is_empty() {
        return AfFormat::unchanged();
    }
    let d = c_atof(value);
    let breach = if check_min && check_max {
        (d < min || d > max).then(|| AlertMessage::RangeBetween {
            min: min_label.to_string(),
            max: max_label.to_string(),
        })
    } else if check_min {
        (d < min).then(|| AlertMessage::RangeGreater {
            min: min_label.to_string(),
        })
    } else if check_max {
        (d > max).then(|| AlertMessage::RangeLess {
            max: max_label.to_string(),
        })
    } else {
        None
    };
    match breach {
        Some(message) => AfFormat::rejected(CALLER, message),
        None => AfFormat::unchanged(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::af::AfOutcome;

    /// The notification line, or `None` when nothing was raised — which is
    /// exactly what the transcript shows or omits.
    fn notification(
        value: &str,
        check_min: bool,
        min: f64,
        min_label: &str,
        check_max: bool,
        max: f64,
        max_label: &str,
    ) -> Option<String> {
        let out = af_range_validate(value, check_min, min, min_label, check_max, max, max_label);
        out.alert().map(ToString::to_string)
    }

    /// Every `AFRange_Validate` line the golden transcript records, message
    /// text and all.
    #[test]
    fn range_notifications_match_the_transcript() {
        assert_eq!(
            notification("1", true, 2.0, "2", true, 4.0, "4").as_deref(),
            Some(
                "AFRange_Validate[icon=3,type=0]: The input value must be greater than or equal to 2 and less than or equal to 4."
            )
        );
        assert_eq!(
            notification("5", true, 2.0, "2", true, 4.0, "4").as_deref(),
            Some(
                "AFRange_Validate[icon=3,type=0]: The input value must be greater than or equal to 2 and less than or equal to 4."
            )
        );
        assert_eq!(
            notification("1", true, 2.0, "2", false, 4.0, "4").as_deref(),
            Some(
                "AFRange_Validate[icon=3,type=0]: The input value must be greater than or equal to 2."
            )
        );
        assert_eq!(
            notification("5", false, 2.0, "2", true, 4.0, "4").as_deref(),
            Some(
                "AFRange_Validate[icon=3,type=0]: The input value must be less than or equal to 4."
            )
        );
    }

    /// The four lines the transcript marks "No notification", plus empty input.
    #[test]
    fn in_range_and_unchecked_bounds_stay_quiet() {
        for (value, check_min, check_max) in [
            ("3", true, true),
            ("1", false, true),
            ("5", true, false),
            ("", true, true),
        ] {
            let out = af_range_validate(value, check_min, 2.0, "2", check_max, 4.0, "4");
            assert_eq!(out.outcome, AfOutcome::Unchanged, "{value}");
            assert!(out.effects.is_empty(), "{value}");
        }
    }

    /// A value equal to a bound passes: only strict `<` and `>` trip.
    #[test]
    fn bounds_are_inclusive_pass() {
        for value in ["2", "4"] {
            assert_eq!(notification(value, true, 2.0, "2", true, 4.0, "4"), None);
        }
    }

    /// The bound text is the caller's, not a re-rendered double.
    #[test]
    fn bound_text_is_reproduced_not_reformatted() {
        let msg = notification("1", true, 2.0, "2.00", false, 0.0, "").unwrap_or_default();
        assert!(msg.ends_with("greater than or equal to 2.00."), "{msg}");
    }
}
