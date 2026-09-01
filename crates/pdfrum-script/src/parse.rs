//! Number parsing matching `core/fxcrt/fx_string.cpp` and C `atof`.

/// Trim only ASCII spaces, matching `StrTrim(str)` / `Trim(' ')` in
/// `cjs_publicmethods.cpp`.
#[must_use]
pub fn trim_spaces(s: &str) -> &str {
    s.trim_matches(' ')
}

/// C `isspace` on an ASCII byte: space, tab, newline, vertical tab, form feed, CR.
fn is_c_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

/// ASCII decimal digit, matching `FXSYS_IsDecimalDigit`.
#[must_use]
pub fn is_decimal_digit(c: char) -> bool {
    c.is_ascii_digit()
}

/// ASCII letter, matching `isascii && isalpha`.
#[must_use]
pub fn is_ascii_alpha(c: char) -> bool {
    c.is_ascii_alphabetic()
}

/// ASCII letter or digit, matching `isascii && isalnum`.
#[must_use]
pub fn is_ascii_alnum(c: char) -> bool {
    c.is_ascii_alphanumeric()
}

/// Skip leading spaces and extra `+`/`-`, leaving at most the minus immediately
/// before the first digit — `ParseLeadingChars` in `fx_string.cpp`.
fn parse_leading_chars(s: &str) -> &str {
    let bytes = s.as_bytes();
    let mut start = 0;
    while start < bytes.len() {
        let Some(b) = bytes.get(start).copied() else {
            break;
        };
        if b == b' ' || b == b'+' || b == b'-' {
            start = start.saturating_add(1);
        } else {
            break;
        }
    }
    if start > 0
        && let Some(b) = bytes.get(start.saturating_sub(1)).copied()
        && b == b'-'
    {
        start = start.saturating_sub(1);
    }
    s.get(start..).unwrap_or_default()
}

/// Scan a C `strtod` numeric prefix starting at `start` in `bytes`. Returns the
/// end index (exclusive) of the prefix, or `start` if none.
fn scan_float_prefix(bytes: &[u8], start: usize) -> usize {
    let mut i = start;
    if i < bytes.len()
        && let Some(b) = bytes.get(i).copied()
        && (b == b'+' || b == b'-')
    {
        i = i.saturating_add(1);
    }
    let mut saw_digit = false;
    while i < bytes.len() {
        match bytes.get(i).copied() {
            Some(b) if b.is_ascii_digit() => {
                saw_digit = true;
                i = i.saturating_add(1);
            }
            _ => break,
        }
    }
    if i < bytes.len()
        && let Some(b) = bytes.get(i).copied()
        && b == b'.'
    {
        i = i.saturating_add(1);
        while i < bytes.len() {
            match bytes.get(i).copied() {
                Some(b) if b.is_ascii_digit() => {
                    saw_digit = true;
                    i = i.saturating_add(1);
                }
                _ => break,
            }
        }
    }
    if saw_digit
        && i < bytes.len()
        && let Some(b) = bytes.get(i).copied()
        && (b == b'e' || b == b'E')
    {
        let mut j = i.saturating_add(1);
        if j < bytes.len()
            && let Some(b) = bytes.get(j).copied()
            && (b == b'+' || b == b'-')
        {
            j = j.saturating_add(1);
        }
        if j < bytes.len()
            && let Some(b) = bytes.get(j).copied()
            && b.is_ascii_digit()
        {
            j = j.saturating_add(1);
            while j < bytes.len() {
                match bytes.get(j).copied() {
                    Some(b) if b.is_ascii_digit() => j = j.saturating_add(1),
                    _ => break,
                }
            }
            i = j;
        }
    }
    if saw_digit { i } else { start }
}

fn parse_scanned(bytes: &[u8], start: usize, end: usize) -> f64 {
    if end <= start {
        return 0.0;
    }
    let Some(slice) = bytes.get(start..end) else {
        return 0.0;
    };
    let Ok(text) = std::str::from_utf8(slice) else {
        return 0.0;
    };
    text.parse().unwrap_or(0.0)
}

/// C `atof`: skip `isspace`, then parse a floating prefix. Invalid input is 0.
///
/// Used by `AFNumber_Format` / `AFPercent_Format` / `AFRange_Validate`.
#[must_use]
pub fn c_atof(input: &str) -> f64 {
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes.get(i).copied() {
            Some(b) if is_c_space(b) => i = i.saturating_add(1),
            _ => break,
        }
    }
    let end = scan_float_prefix(bytes, i);
    parse_scanned(bytes, i, end)
}

/// `StringToDouble` / `StringToFloatImpl` in `fx_string.cpp`: skip extra signs,
/// then parse a floating prefix. Does not require consuming the whole string.
#[must_use]
pub fn string_to_double(input: &str) -> f64 {
    let rest = parse_leading_chars(input);
    let bytes = rest.as_bytes();
    let end = scan_float_prefix(bytes, 0);
    parse_scanned(bytes, 0, end)
}

/// Replace every comma with a period, matching `NormalizeDecimalMarkW`.
#[must_use]
pub fn normalize_decimal_mark(s: &str) -> String {
    s.replace(',', ".")
}

/// ECMAScript `ToNumber` of a string after comma-normalisation, as used by
/// `AFMakeNumber` via `MaybeCoerceToNumber`.
///
/// Returns `None` when the C++ would leave the value as a non-number (then
/// `AFMakeNumber` yields 0), `Some(n)` when it is a number — including `NaN`
/// for the string `"NaN"`.
#[must_use]
pub fn js_to_number(input: &str) -> Option<f64> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed == "NaN" {
        return Some(f64::NAN);
    }
    if trimmed == "Infinity" || trimmed == "+Infinity" {
        return Some(f64::INFINITY);
    }
    if trimmed == "-Infinity" {
        return Some(f64::NEG_INFINITY);
    }
    // Hex, as ES ToNumber accepts 0x…; the C++ `IsNumber` does not, but
    // `MaybeCoerceToNumber` uses V8 ToNumber.
    if let Some(hex) = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
    {
        return i32::from_str_radix(hex, 16).ok().map(f64::from);
    }
    // Whole-string decimal: unlike `atof`, leftover junk is not a number.
    match trimmed.parse::<f64>() {
        Ok(n) if n.is_finite() => Some(n),
        Ok(n) if n.is_infinite() => Some(n),
        _ => None,
    }
}

/// Whether a committed field value reads as a number, by the grammar Acrobat
/// applies to keystroke validation.
///
/// It is a loose grammar and an idiosyncratic one, and three of its rules are
/// worth stating because they surprise:
///
/// - **A comma and a period are the same token**, and at most one of either may
///   appear anywhere. So `560,024` is a number and `1,000,000` is not.
/// - **A sign is legal only at the very start.** `+123` and `-98765` are
///   numbers; `1-2` is not.
/// - **An exponent must carry an explicit sign.** `1e-9` and `-1.23e+23` are
///   numbers; `-1e5` is not, and `e-5` — an exponent with nothing in front of
///   it — is.
///
/// Leading and trailing spaces are trimmed first, so `  345 ` is a number.
///
// [oracle-bug] cjs_publicmethods.cpp:272-309 walks a NUL-terminated string and
// returns true when it reaches the terminator, so the empty string and a
// string of nothing but spaces both come back as numbers. The oracle's own
// unit test flags this: `// TODO(weili): Check whether results from case 0, 1,
// 10, 15 are intended.` names exactly those two cases (and two more). Nothing
// is a number, so an empty string is answered `false` here.
//
// This divergence is unobservable through the `AF*` functions: the only caller,
// `af_number_keystroke`, tests for an empty value and accepts it *before*
// consulting this grammar, so both answers accept an empty commit. No golden
// assertion reaches it.
#[must_use]
pub fn is_number(str: &str) -> bool {
    let trimmed = trim_spaces(str);
    if trimmed.is_empty() {
        return false;
    }
    let chars: Vec<char> = trimmed.chars().collect();
    let mut seen_mark = false;
    let mut seen_exponent = false;
    let mut i = 0;
    while let Some(c) = chars.get(i).copied() {
        match c {
            // One mark, of either spelling, anywhere in the string.
            '.' | ',' => {
                if seen_mark {
                    return false;
                }
                seen_mark = true;
            }
            // A sign is only ever the first character.
            '-' | '+' if i == 0 => {}
            '-' | '+' => return false,
            'e' | 'E' => {
                if seen_exponent {
                    return false;
                }
                // The exponent's own sign is mandatory, not optional.
                i = i.saturating_add(1);
                if !matches!(chars.get(i), Some('+' | '-')) {
                    return false;
                }
                seen_exponent = true;
            }
            _ if is_decimal_digit(c) => {}
            _ => return false,
        }
        i = i.saturating_add(1);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The oracle's own unit-test table, restated — with the two cases its
    /// `TODO(weili)` comment questions answered the other way.
    #[test]
    fn is_number_grammar() {
        let cases: &[(&str, bool)] = &[
            // Nothing is not a number, whatever it is padded with.
            // The oracle answers `true` here; see the note on `is_number`.
            ("", false),
            ("  ", false),
            // Plain junk, and junk attached to a number.
            ("xyz00", false),
            ("1%", false),
            ("0x234", false),
            // A sign, but only at the front.
            ("+123", true),
            ("-98765", true),
            // Spaces are trimmed before the grammar runs.
            ("  345 ", true),
            // An exponent needs an explicit sign, and needs nothing in front.
            ("-1e5", false),
            ("-2e", false),
            ("e-5", true),
            ("1e-9", true),
            ("-1.23e+23", true),
            // A comma is a decimal mark, so only one may appear.
            ("1,000,000", false),
            ("560,024", true),
            // Ordinary numbers.
            ("0.023", true),
            (".356089", true),
            ("0", true),
            ("0123", true),
            ("9876123", true),
        ];
        for &(input, expected) in cases {
            assert_eq!(is_number(input), expected, "is_number({input:?})");
        }
    }

    /// Two marks reject however they are spelled and wherever they sit.
    #[test]
    fn a_period_and_a_comma_are_the_same_token() {
        for input in ["1.2.3", "1,2,3", "1.2,3", "1,2.3"] {
            assert!(!is_number(input), "{input:?}");
        }
        for input in ["1.23", "1,23"] {
            assert!(is_number(input), "{input:?}");
        }
    }

    /// A number is read up to the first character that cannot continue it, and
    /// anything unreadable is zero. A comma stops it — this reading does not
    /// treat one as a decimal mark.
    #[test]
    fn c_atof_reads_a_prefix() {
        assert_eq!(c_atof("1,2"), 1.0);
        assert_eq!(c_atof("blooey"), 0.0);
        assert_eq!(c_atof(""), 0.0);
        assert_eq!(c_atof("  12.5abc"), 12.5);
        assert!((c_atof("-5.1234") - -5.1234).abs() < 1e-12);
        assert_eq!(c_atof("1e+3"), 1000.0);
        // An exponent that goes nowhere is not consumed.
        assert_eq!(c_atof("1e"), 1.0);
    }

    /// The looser reading skips a run of leading signs, keeping the last one,
    /// before reading the number.
    #[test]
    fn string_to_double_skips_repeated_signs() {
        assert_eq!(string_to_double("--100.0"), -100.0);
        assert_eq!(string_to_double("+-100.0"), -100.0);
        assert_eq!(string_to_double("++100.0"), 100.0);
        assert_eq!(string_to_double("-+-100.0"), -100.0);
        assert_eq!(string_to_double("invalid"), 0.0);
        assert_eq!(string_to_double("    100.0"), 100.0);
    }

    /// The strict reading is all-or-nothing: trailing junk means no number.
    #[test]
    fn js_to_number_rejects_trailing_junk() {
        assert_eq!(js_to_number("2blooey"), None);
        assert_eq!(js_to_number("1.2"), Some(1.2));
        assert_eq!(js_to_number(""), None);
        assert_eq!(js_to_number("  7  "), Some(7.0));
    }

    /// Only ASCII spaces are trimmed — a tab stays and makes the value junk.
    #[test]
    fn trimming_is_spaces_only() {
        assert_eq!(trim_spaces("  a  "), "a");
        assert_eq!(trim_spaces("\ta\t"), "\ta\t");
        assert_eq!(trim_spaces("   "), "");
    }
}
