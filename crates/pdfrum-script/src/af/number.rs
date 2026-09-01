//! `AFNumber_Format` / `AFNumber_Keystroke`.

use super::{
    AfFormat, Keystroke, KeystrokeResult, Thrown, decimal_mark_for_style,
    digit_separator_for_style, is_style_with_comma_decimal_mark, is_style_with_digit_separator,
    merge_change, valid_style_or_zero,
};
use crate::error::{AfColor, AlertMessage, Error};
use crate::parse::{c_atof, is_decimal_digit, is_number, normalize_decimal_mark, trim_spaces};

/// Nudge added when `nDec > 0` (`kDoubleCorrect` in `cjs_publicmethods.cpp`).
const DOUBLE_CORRECT: f64 = 0.000_000_000_000_001;
/// `std::numeric_limits<double>::digits10`.
const DOUBLE_DIGITS10: i32 = 15;

/// Fixed-point rendering to `prec` places.
///
/// Upstream guards against this coming back empty and retries with zero; it
/// cannot come back empty, because a finite magnitude always renders at least
/// one digit and a non-finite one is answered with `"0"` here rather than with
/// an infinity or NaN spelling that no downstream step knows how to separate.
pub(crate) fn format_fixed(value: f64, prec: usize) -> String {
    if !value.is_finite() {
        return "0".to_string();
    }
    format!("{value:.prec$}")
}

/// Render a magnitude, and report both its sign and where its integer part
/// ends — the index the thousands separators are laid out from.
///
/// Precision is capped at the most decimal digits a `f64` carries, so asking
/// for more places than that quietly gets that many.
pub(crate) fn calculate_string(d_value: f64, i_dec: i32) -> (String, bool, usize) {
    let negative = d_value < 0.0;
    let mag = if negative { -d_value } else { d_value };
    let prec = usize::try_from(i_dec.clamp(0, DOUBLE_DIGITS10)).unwrap_or(0);
    let formatted = format_fixed(mag, prec);
    let int_end = formatted.find('.').unwrap_or(formatted.len());
    (formatted, negative, int_end)
}

/// Walk backwards from the end of the integer part in steps of three, planting
/// `sep` at each stop above `stop`.
///
/// `s` at this point is the fixed-point rendering of a magnitude — ASCII digits
/// and at most one mark — so every index is a character boundary and inserting
/// at one cannot split anything.
pub(crate) fn insert_thousands(s: &mut String, int_end: usize, stop: usize, sep: char) {
    let mut at = int_end;
    while at > stop.saturating_add(3) {
        at = at.saturating_sub(3);
        if at > s.len() {
            break;
        }
        s.insert(at, sep);
    }
}

/// `AFNumber_Format(nDec, sepStyle, negStyle, currStyle, strCurrency,
/// bCurrencyPrepend)`.
///
/// Acrobat takes six arguments, but the fourth — the currency *style* — is read
/// and discarded, so it is not a parameter here; only the arity check, which
/// belongs to the caller, has to count six.
///
/// # Negative styles carry the sign in two different places
///
/// Style 0 prepends a minus and styles 2 and 3 wrap in parentheses, all of
/// which land in the returned string. Styles 1 and 3 additionally ask for red
/// text, and **style 1 prints no minus sign at all** — its entire indication
/// that the number is negative is the colour. That colour comes back in
/// [`AfFormat::effects`], so no part of the answer is lost even though this
/// function cannot reach a field.
///
/// The two red styles also ask for black on a *non-negative* value, to undo an
/// earlier red. Acrobat only writes that back when it differs from the field's
/// current colour; the comparison needs the field, so black is always reported
/// and the caller decides whether writing it is a no-op.
#[must_use]
pub fn af_number_format(
    value: &str,
    n_dec: i32,
    sep_style: i32,
    neg_style: i32,
    currency: &str,
    currency_prepend: bool,
) -> AfFormat {
    let trimmed = trim_spaces(value);
    if trimmed.is_empty() {
        return AfFormat::unchanged();
    }
    let i_dec = i32::try_from(n_dec.unsigned_abs()).unwrap_or(i32::MAX);
    let i_sep = valid_style_or_zero(sep_style);
    let i_neg = valid_style_or_zero(neg_style);

    let normalized = normalize_decimal_mark(trimmed);
    let mut d_value = c_atof(&normalized);
    if i_dec > 0 {
        d_value += DOUBLE_CORRECT;
    }

    let i_dec_clamped = i_dec.clamp(0, DOUBLE_DIGITS10);
    let (mut str_value, b_negative, i_dec2) = calculate_string(d_value, i_dec_clamped);

    if i_dec2 < str_value.len() {
        if is_style_with_comma_decimal_mark(i_sep) {
            str_value = str_value.replace('.', ",");
        }
        // A leading zero is planted in front of a bare `.5`, and `i_dec2` is
        // deliberately *not* advanced past it: the separator pass below reuses
        // the pre-insertion index. Fixed-point rendering always emits that zero
        // itself, so this branch only fires if it ever stopped doing so — and
        // if it did, keeping the stale index is the behaviour to keep.
        if i_dec2 == 0 {
            str_value.insert(0, '0');
        }
    }
    if is_style_with_digit_separator(i_sep) {
        let sep = digit_separator_for_style(i_sep);
        insert_thousands(&mut str_value, i_dec2, 0, sep);
    }

    let mut out = if currency_prepend {
        let mut s = String::from(currency);
        s.push_str(&str_value);
        s
    } else {
        let mut s = str_value;
        s.push_str(currency);
        s
    };

    let wants_red_style = i_neg == 1 || i_neg == 3;
    if b_negative {
        if i_neg == 0 {
            out.insert(0, '-');
        } else if i_neg == 2 || i_neg == 3 {
            out.insert(0, '(');
            out.push(')');
        }
    }

    let formatted = AfFormat::formatted(out);
    if wants_red_style {
        let color = if b_negative {
            AfColor::RED
        } else {
            AfColor::BLACK
        };
        formatted.with_color(color)
    } else {
        formatted
    }
}

/// `AFNumber_Keystroke(nDec, sepStyle, …)`.
///
/// `n_dec` is part of Acrobat's signature and is never read; it is accepted so
/// the engine binding can pass its arguments through unshuffled.
///
/// # Two different jobs behind one name
///
/// On commit, the *whole* value is checked and a non-number both notifies the
/// user and throws — the one path in this crate that does both, which is why
/// the error carries the alert with it.
///
/// On an ordinary keystroke, only the proposed insertion is checked, character
/// by character, and a rejection is silent: `event.rc` goes false and nothing
/// is shown. At most one decimal mark and at most one minus may exist across
/// value and change, and the minus is legal only as the first character of a
/// change that lands at the very start of the field.
///
/// # Errors
///
/// [`Error::InvalidInput`], on commit, when the trimmed value is not a number.
pub fn af_number_keystroke(
    event: &Keystroke,
    _n_dec: i32,
    sep_style: i32,
) -> Result<KeystrokeResult, Thrown> {
    const CALLER: &str = "AFNumber_Keystroke";

    if event.will_commit {
        let sw_temp = trim_spaces(&event.value);
        if sw_temp.is_empty() {
            return Ok(KeystrokeResult::accept());
        }
        let normalized = normalize_decimal_mark(sw_temp);
        if !is_number(&normalized) {
            return Err(Thrown::alerting(
                Error::InvalidInput,
                CALLER,
                AlertMessage::InvalidInput,
            ));
        }
        return Ok(KeystrokeResult::accept());
    }

    let selected: String = match usize::try_from(event.sel_start) {
        Ok(start) => {
            let end = usize::try_from(event.sel_end.max(event.sel_start)).unwrap_or(start);
            event
                .value
                .chars()
                .skip(start)
                .take(end.saturating_sub(start))
                .collect()
        }
        Err(_) => String::new(),
    };

    // A minus already in the field, and not about to be replaced, blocks any
    // insertion in front of it.
    let mut has_sign = event.value.contains('-') && !selected.contains('-');
    if has_sign && !selected.is_empty() && event.sel_start == 0 {
        return Ok(KeystrokeResult::reject());
    }

    let i_sep = valid_style_or_zero(sep_style);
    let c_sep = decimal_mark_for_style(i_sep);
    let mut has_sep = event.value.contains(c_sep);

    for (index, ch) in event.change.chars().enumerate() {
        if ch == c_sep {
            if has_sep {
                return Ok(KeystrokeResult::reject());
            }
            has_sep = true;
            continue;
        }
        if ch == '-' {
            // A second minus anywhere, a minus that is not the first character
            // of the change, or a change that does not land at the field's very
            // start: each of the three rejects.
            if has_sign || index != 0 || event.sel_start != 0 {
                return Ok(KeystrokeResult::reject());
            }
            has_sign = true;
            continue;
        }
        if !is_decimal_digit(ch) {
            return Ok(KeystrokeResult::reject());
        }
    }

    // Every character passed, so the insertion is spliced in and handed back as
    // the field's new value.
    let merged = merge_change(&event.value, &event.change, event.sel_start, event.sel_end);
    Ok(KeystrokeResult::accept_value(merged))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::af::AfOutcome;

    fn fmt(value: &str, n_dec: i32, sep: i32, neg: i32, currency: &str, prepend: bool) -> String {
        match af_number_format(value, n_dec, sep, neg, currency, prepend).outcome {
            AfOutcome::Formatted(s) => s,
            AfOutcome::Unchanged => value.to_string(),
            AfOutcome::Rejected => unreachable!("AFNumber_Format never rejects"),
        }
    }

    #[test]
    fn empty_after_trim_leaves_the_value_alone() {
        for value in ["", "   "] {
            let out = af_number_format(value, 2, 0, 0, "", false);
            assert_eq!(out.outcome, AfOutcome::Unchanged, "{value:?}");
            assert!(out.effects.is_empty());
        }
    }

    /// The two `AFNumber_Format` lines the golden transcript records.
    #[test]
    fn transcript_lines() {
        assert_eq!(fmt("blooey", 0, 1, 0, "", false), "0");
        assert_eq!(fmt("12", 0, 1, 0, "", false), "12");
    }

    #[test]
    fn separator_styles() {
        assert_eq!(fmt("1234.5", 2, 0, 0, "", false), "1,234.50");
        assert_eq!(fmt("1234.5", 2, 1, 0, "", false), "1234.50");
        assert_eq!(fmt("1234.5", 2, 2, 0, "", false), "1.234,50");
        assert_eq!(fmt("1234.5", 2, 3, 0, "", false), "1234,50");
        assert_eq!(fmt("1234567.5", 2, 0, 0, "", false), "1,234,567.50");
    }

    /// Style 4 is reachable through the percent entry point and not this one,
    /// where anything outside 0..=3 clamps to 0.
    #[test]
    fn out_of_range_separator_style_clamps_to_zero() {
        for style in [4, 5, -1, i32::MIN, i32::MAX] {
            assert_eq!(
                fmt("1234", 0, style, 0, "", false),
                "1,234",
                "style {style}"
            );
        }
    }

    /// Style 1 has no minus sign at all — the colour is the only indication.
    #[test]
    fn negative_styles_split_the_sign_between_text_and_colour() {
        assert_eq!(fmt("-12.5", 2, 0, 0, "", false), "-12.50");
        assert_eq!(fmt("-12.5", 2, 0, 1, "", false), "12.50");
        assert_eq!(fmt("-12.5", 2, 0, 2, "", false), "(12.50)");
        assert_eq!(fmt("-12.5", 2, 0, 3, "", false), "(12.50)");

        assert_eq!(
            af_number_format("-12.5", 2, 0, 1, "", false)
                .effects
                .text_color,
            Some(AfColor::RED)
        );
        assert_eq!(
            af_number_format("-12.5", 2, 0, 3, "", false)
                .effects
                .text_color,
            Some(AfColor::RED)
        );
        // The plain styles never touch the colour.
        for style in [0, 2] {
            assert_eq!(
                af_number_format("-12.5", 2, 0, style, "", false)
                    .effects
                    .text_color,
                None,
                "style {style}"
            );
        }
    }

    /// The red styles ask for black on a non-negative value, to undo an earlier
    /// red; the caller decides whether writing it changes anything.
    #[test]
    fn red_styles_ask_for_black_when_the_value_is_not_negative() {
        for style in [1, 3] {
            assert_eq!(
                af_number_format("12.5", 2, 0, style, "", false)
                    .effects
                    .text_color,
                Some(AfColor::BLACK),
                "style {style}"
            );
        }
    }

    /// Currency attaches before the sign, so a parenthesised negative reads
    /// `($12.50)` and never `$(12.50)`.
    #[test]
    fn currency_is_inside_the_parentheses() {
        assert_eq!(fmt("12", 2, 0, 0, "$", true), "$12.00");
        assert_eq!(fmt("12", 2, 0, 0, " EUR", false), "12.00 EUR");
        assert_eq!(fmt("-12", 0, 0, 0, "$", true), "-$12");
        assert_eq!(fmt("-12.5", 2, 0, 2, "$", true), "($12.50)");
        assert_eq!(fmt("-12.5", 2, 0, 3, "$", true), "($12.50)");
    }

    /// A comma is rewritten to a period before parsing, so a value written with
    /// a comma decimal mark reads as a decimal — but a value written with comma
    /// *thousands* separators becomes several periods, and reading stops at the
    /// second one, so `1,234.5` comes back as `1.23`.
    #[test]
    fn commas_become_periods_before_parsing() {
        assert_eq!(fmt("1,5", 1, 0, 0, "", false), "1.5");
        assert_eq!(fmt("1,234.5", 2, 0, 0, "", false), "1.23");
    }

    /// A magnitude below the requested precision still renders its leading zero.
    #[test]
    fn sub_unit_values_keep_a_leading_zero() {
        assert_eq!(fmt(".5", 1, 0, 0, "", false), "0.5");
        assert_eq!(fmt(".5", 2, 0, 0, "", false), "0.50");
        assert_eq!(fmt(".123456", 4, 0, 0, "", false), "0.1235");
        assert_eq!(fmt(".5", 1, 2, 0, "", false), "0,5");
    }

    /// On commit the whole value is checked, and the failure both notifies and
    /// throws — the transcript records the notification line and the thrown
    /// text for the same call.
    #[test]
    fn commit_of_a_non_number_notifies_and_throws() {
        let thrown = af_number_keystroke(&Keystroke::commit("abc"), 1, 2).unwrap_err();
        assert_eq!(thrown.error, Error::InvalidInput);
        assert_eq!(thrown.error.to_string(), "The input value is invalid.");
        assert_eq!(
            thrown.effects.alerts.first().map(ToString::to_string),
            Some("AFNumber_Keystroke[icon=3,type=0]: The input value is invalid.".to_string())
        );
    }

    #[test]
    fn commit_of_a_number_or_of_nothing_is_accepted() {
        for value in ["123", "", "  ", "560,024", "-1.23e+23"] {
            let out = af_number_keystroke(&Keystroke::commit(value), 1, 2);
            assert!(out.is_ok(), "{value:?}");
        }
    }

    /// An ordinary keystroke rejects silently: `rc` goes false, nothing shown.
    #[test]
    fn keystroke_rejections_are_silent() {
        let cases: &[(&str, &str, i32)] = &[
            (".2", ".", 2),  // a second decimal mark
            ("12", "a", 2),  // not a digit
            ("12", "-", 1),  // a minus that is not at the field's start
            ("-12", "-", 0), // a second minus
            ("12", "1-", 2), // a minus that is not first in the change
        ];
        for &(value, change, caret) in cases {
            let out = af_number_keystroke(&Keystroke::insert(value, change, caret), 1, 0).unwrap();
            assert!(out.is_reject(), "{value:?} + {change:?}");
            assert!(out.effects.is_empty(), "{value:?} + {change:?}");
        }
    }

    #[test]
    fn accepted_keystrokes_hand_back_the_merged_value() {
        for &(value, change, caret, expected) in &[
            ("12", "3", 2, "123"),
            ("12", "-", 0, "-12"),
            ("1", ".", 1, "1."),
        ] {
            let out = af_number_keystroke(&Keystroke::insert(value, change, caret), 1, 0).unwrap();
            assert_eq!(out.value(), Some(expected), "{value:?} + {change:?}");
        }
    }

    /// Style 2 and 3 make the comma the decimal mark, so a comma is then the
    /// character that may appear only once and a period is plain junk.
    #[test]
    fn the_separator_style_decides_which_mark_is_the_decimal_one() {
        let out = af_number_keystroke(&Keystroke::insert("1,2", ",", 3), 1, 2).unwrap();
        assert!(out.is_reject());
        let out = af_number_keystroke(&Keystroke::insert("1,2", ".", 3), 1, 2).unwrap();
        assert!(out.is_reject());
    }

    /// Replacing the minus with the selection frees the sign slot again.
    #[test]
    fn a_selected_minus_no_longer_blocks_the_front() {
        let out = af_number_keystroke(&Keystroke::replace("-12", "9", 0, 1), 1, 0).unwrap();
        assert_eq!(out.value(), Some("912"));
    }
}
