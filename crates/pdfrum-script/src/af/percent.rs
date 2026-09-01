//! `AFPercent_Format` / `AFPercent_Keystroke`.

use super::number::{af_number_keystroke, insert_thousands};
use super::{
    AfFormat, Keystroke, KeystrokeResult, Thrown, decimal_mark_for_style,
    digit_separator_for_style, is_style_with_apostrophe_separator, is_style_with_digit_separator,
};
use crate::error::Error;
use crate::parse::{c_atof, trim_spaces};

const MAX_SEP_STYLE: i32 = 49;
const DEC_LIMIT: i32 = 512;

/// `AFPercent_Format(nDec, sepStyle, bPercentPrepend)`.
///
/// # Where this differs from `AFNumber_Format`
///
/// The separator style is validated rather than clamped — anything outside
/// `0..=49` throws instead of silently becoming style 0 — and style 4, the
/// apostrophe thousands separator, is therefore *reachable here and nowhere
/// else*. The decimal count is likewise validated against zero, but a count
/// above 512 is answered with a bare `"%"` rather than an error, because
/// Acrobat ran out of buffer at that point and this reproduces what it printed.
///
/// The value is also **not** comma-normalised before parsing, unlike
/// `AFNumber_Format`, so a comma here simply ends the number.
///
/// # Errors
///
/// [`Error::Value`] when `n_dec` is negative or `sep_style` is outside
/// `0..=49`.
pub fn af_percent_format(
    value: &str,
    n_dec: i32,
    sep_style: i32,
    percent_prepend: bool,
) -> Result<AfFormat, Thrown> {
    if n_dec < 0 || !(0..=MAX_SEP_STYLE).contains(&sep_style) {
        return Err(Thrown::bare(Error::Value));
    }
    if n_dec > DEC_LIMIT {
        return Ok(AfFormat::formatted("%"));
    }

    let mut str_value = trim_spaces(value).to_string();
    if str_value.is_empty() {
        str_value = "0".to_string();
    }

    // No NormalizeDecimalMark — C++ atof's the UTF-8 directly.
    let mut d_value = c_atof(&str_value);
    d_value *= 100.0;

    let prec = usize::try_from(n_dec).unwrap_or(0);
    str_value = format!("{d_value:.prec$}");

    let mark_pos = str_value.find('.');
    if let Some(pos) = mark_pos {
        let mark = decimal_mark_for_style(sep_style);
        if mark != '.' {
            let mut chars: Vec<char> = str_value.chars().collect();
            if let Some(slot) = chars.get_mut(pos) {
                *slot = mark;
            }
            str_value = chars.into_iter().collect();
        }
    }

    let use_digit_sep = is_style_with_digit_separator(sep_style);
    if use_digit_sep || is_style_with_apostrophe_separator(sep_style) {
        let sep = if use_digit_sep {
            digit_separator_for_style(sep_style)
        } else {
            '\''
        };
        let int_end = mark_pos.unwrap_or(str_value.len());
        // A negative value keeps its minus in front of the first group, so the
        // walk stops one character later than it would for a positive one.
        let stop = usize::from(d_value < 0.0);
        insert_thousands(&mut str_value, int_end, stop, sep);
    }

    if percent_prepend {
        str_value.insert(0, '%');
    } else {
        str_value.push('%');
    }
    Ok(AfFormat::formatted(str_value))
}

/// `AFPercent_Keystroke` is `AFNumber_Keystroke` under another name — the same
/// function, registered twice.
///
/// The alert it raises therefore names `AFNumber_Keystroke`, not this
/// function; that attribution is what the golden transcript records.
///
/// # Errors
///
/// As [`af_number_keystroke`].
pub fn af_percent_keystroke(
    event: &Keystroke,
    n_dec: i32,
    sep_style: i32,
) -> Result<KeystrokeResult, Thrown> {
    af_number_keystroke(event, n_dec, sep_style)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::af::AfOutcome;

    fn fmt(value: &str, n_dec: i32, sep: i32, prepend: bool) -> String {
        match af_percent_format(value, n_dec, sep, prepend)
            .expect("valid arguments")
            .outcome
        {
            AfOutcome::Formatted(s) => s,
            other => unreachable!("AFPercent_Format answered {other:?}"),
        }
    }

    /// The value is multiplied by a hundred, so a fifth of a percent arrives as
    /// `0.002`, and the requested precision then rounds what is left.
    #[test]
    fn scaling_then_rounding_matches_the_transcript() {
        assert_eq!(fmt("0.009876", 0, 0, false), "1%");
        assert_eq!(fmt("0.009876", 1, 0, false), "1.0%");
        assert_eq!(fmt("0.009876", 2, 0, false), "0.99%");
        assert_eq!(fmt("0.009876", 3, 0, false), "0.988%");
        assert_eq!(fmt("0.01", 1, 0, false), "1.0%");
        assert_eq!(fmt("0.001", 1, 0, false), "0.1%");
        assert_eq!(fmt("0.0001", 1, 0, false), "0.0%");
        assert_eq!(fmt("0.00001", 1, 0, false), "0.0%");
        assert_eq!(fmt("0.000001", 1, 0, false), "0.0%");
        assert_eq!(fmt("0.000001", 10, 2, false), "0,0001000000%");
        assert_eq!(fmt("12.3456", 1, 0, false), "1,234.6%");
        assert_eq!(fmt("12.3456", 4, 1, false), "1234.5600%");
    }

    /// Empty input formats as zero rather than being left alone — the opposite
    /// of `AFNumber_Format`, which returns the field untouched.
    #[test]
    fn empty_input_formats_as_zero() {
        for (n_dec, sep, expected) in [
            (0, 0, "0%"),
            (1, 0, "0.0%"),
            (1, 2, "0,0%"),
            (1, 3, "0,0%"),
            (1, 4, "0.0%"),
            (2, 2, "0,00%"),
            (10, 0, "0.0000000000%"),
            (10, 2, "0,0000000000%"),
        ] {
            assert_eq!(fmt("", n_dec, sep, false), expected, "{n_dec}/{sep}");
            assert_eq!(fmt("0", n_dec, sep, false), expected, "{n_dec}/{sep}");
        }
    }

    /// Style 4's apostrophe separator is reachable from here and from nowhere
    /// else — `AFNumber_Format` clamps 4 to 0.
    #[test]
    fn every_separator_style_including_the_apostrophe() {
        let v = "987654321.001234";
        assert_eq!(fmt(v, 0, 0, false), "98,765,432,100%");
        assert_eq!(fmt(v, 0, 1, false), "98765432100%");
        assert_eq!(fmt(v, 0, 2, false), "98.765.432.100%");
        assert_eq!(fmt(v, 0, 3, false), "98765432100%");
        assert_eq!(fmt(v, 0, 4, false), "98'765'432'100%");
        assert_eq!(fmt(v, 1, 2, false), "98.765.432.100,1%");
        assert_eq!(fmt(v, 1, 4, false), "98'765'432'100.1%");
        assert_eq!(fmt(v, 3, 4, false), "98'765'432'100.123%");
        assert_eq!(fmt(v, 10, 0, false), "98,765,432,100.1234130859%");
        assert_eq!(fmt(v, 10, 4, false), "98'765'432'100.1234130859%");
    }

    /// A negative keeps its minus outside the first group, so the separator
    /// walk stops one position later.
    #[test]
    fn a_negative_value_keeps_its_minus_out_of_the_groups() {
        for sep in 0..=4 {
            assert_eq!(fmt("-5.1234", 0, sep, false), "-512%", "style {sep}");
        }
        assert_eq!(fmt("-5.1234", 1, 0, false), "-512.3%");
        assert_eq!(fmt("-5.1234", 1, 2, false), "-512,3%");
        assert_eq!(fmt("-5.1234", 10, 4, false), "-512.3400000000%");
        // Three digits ahead of the mark and a minus: still no separator.
        assert_eq!(fmt("-1.23", 0, 0, false), "-123%");
        assert_eq!(fmt("1.23", 0, 0, false), "123%");
    }

    #[test]
    fn the_percent_sign_goes_where_asked() {
        assert_eq!(fmt("-5.1234", 1, 0, false), "-512.3%");
        assert_eq!(fmt("-5.1234", 1, 0, true), "%-512.3");
        assert_eq!(fmt("", 10, 0, true), "%0.0000000000");
    }

    /// The style is validated, not clamped: unlike `AFNumber_Format`, an
    /// out-of-range one throws.
    #[test]
    fn out_of_range_arguments_throw_rather_than_clamp() {
        for (n_dec, sep) in [
            (-3, 0),
            (-3, 1),
            (-1, 3),
            (0, -3),
            (0, -1),
            (0, 50),
            (0, 51),
        ] {
            let thrown = af_percent_format("0", n_dec, sep, false).unwrap_err();
            assert_eq!(thrown.error, Error::Value, "{n_dec}/{sep}");
            assert_eq!(thrown.error.to_string(), "Incorrect parameter value.");
        }
    }

    /// Past 512 places Acrobat ran out of buffer and printed the sign alone.
    #[test]
    fn more_than_512_places_is_just_the_percent_sign() {
        assert_eq!(fmt("0", 513, 0, false), "%");
        // 512 itself still formats, and 308 is what the transcript pins.
        assert_eq!(fmt("0", 20, 0, false), "0.00000000000000000000%");
        assert!(fmt("0", 308, 0, false).len() > 300);
    }

    /// A comma is not normalised away first, so it simply ends the number.
    #[test]
    fn a_comma_is_not_a_decimal_mark_here() {
        assert_eq!(fmt("1,5", 0, 1, false), "100%");
    }
}
