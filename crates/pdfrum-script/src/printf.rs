//! `util.printf`, and the format-string classification it needs first.

use crate::error::Error;
use crate::parse::is_decimal_digit;

/// Narrow a double to an integer for an integer conversion, discarding its
/// fraction and clamping rather than wrapping at the extremes.
fn narrow_to_i32(v: f64) -> i32 {
    if v.is_nan() {
        return 0;
    }
    // `f64` represents every `i32` exactly, so the clamp is not itself an
    // approximation and the truncation cannot overflow once it holds.
    let clamped = v.clamp(f64::from(i32::MIN), f64::from(i32::MAX));
    #[expect(
        clippy::cast_possible_truncation,
        reason = "clamped into i32's range on the line above"
    )]
    {
        clamped as i32
    }
}

/// The kind of argument `ParseDataType` says a printf fragment expects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrintfDataType {
    /// Not a single well-understood conversion — copy the fragment literally.
    Invalid,
    /// `c C d i o u x X`
    Int,
    /// `e E f g G`
    Double,
    /// `s S` (the C++ rewrites `s` to `S`).
    String,
}

/// One argument to [`util_printf`].
#[derive(Debug, Clone, PartialEq)]
pub enum PrintfArg {
    /// Integer conversion (`ToInt32` in the C++).
    Int(i32),
    /// Floating conversion (`ToDouble` in the C++).
    Double(f64),
    /// String conversion.
    String(String),
}

/// Which single conversion a `util.printf` format asks for, if any.
///
/// Mutates the format in place, upper-casing a `%s` to `%S`.
///
/// Integer conversions with more than two precision digits are `Invalid`
/// (<https://crbug.com/740166>).
pub fn parse_data_type(format: &mut String) -> PrintfDataType {
    #[derive(Clone, Copy)]
    enum State {
        Before,
        Flags,
        Width,
        Precision,
        Specifier,
        After,
    }
    let chars: Vec<char> = format.chars().collect();
    let mut result = PrintfDataType::Invalid;
    let mut state = State::Before;
    let mut precision_digits = 0usize;
    let mut i = 0;
    let mut rewrite_s_at: Option<usize> = None;
    while i < chars.len() {
        let Some(c) = chars.get(i).copied() else {
            break;
        };
        let mut reprocess = false;
        match state {
            State::Before => {
                if c == '%' {
                    state = State::Flags;
                }
            }
            State::Flags => {
                if c == '+' || c == '-' || c == '#' || c == ' ' {
                    // stay
                } else {
                    state = State::Width;
                    reprocess = true;
                }
            }
            State::Width => {
                if c == '*' {
                    return PrintfDataType::Invalid;
                }
                if is_decimal_digit(c) {
                    // stay
                } else if c == '.' {
                    state = State::Precision;
                } else {
                    state = State::Specifier;
                    reprocess = true;
                }
            }
            State::Precision => {
                if c == '*' {
                    return PrintfDataType::Invalid;
                }
                if is_decimal_digit(c) {
                    precision_digits = precision_digits.saturating_add(1);
                } else {
                    state = State::Specifier;
                    reprocess = true;
                }
            }
            State::Specifier => {
                result = match c {
                    'c' | 'C' | 'd' | 'i' | 'o' | 'u' | 'x' | 'X' => PrintfDataType::Int,
                    'e' | 'E' | 'f' | 'g' | 'G' => PrintfDataType::Double,
                    's' | 'S' => {
                        if c == 's' {
                            rewrite_s_at = Some(i);
                        }
                        PrintfDataType::String
                    }
                    _ => return PrintfDataType::Invalid,
                };
                state = State::After;
            }
            State::After => {
                if c == '%' {
                    return PrintfDataType::Invalid;
                }
            }
        }
        if !reprocess {
            i = i.saturating_add(1);
        }
    }
    if let Some(idx) = rewrite_s_at {
        let mut out = String::new();
        for (k, ch) in chars.iter().enumerate() {
            if k == idx {
                out.push('S');
            } else {
                out.push(*ch);
            }
        }
        *format = out;
    }
    if result == PrintfDataType::Int && precision_digits > 2 {
        return PrintfDataType::Invalid;
    }
    result
}

fn split_percent_fragments(fmt: &str) -> Vec<String> {
    // Sentinel 'S' so there is always text before the first '%'.
    let unsafe_fmt: String = {
        let mut s = String::from("S");
        s.push_str(fmt);
        s
    };
    let mut fragments = Vec::new();
    let mut offset: usize = 0;
    loop {
        let Some(rest) = unsafe_fmt.get(offset.saturating_add(1)..) else {
            if let Some(tail) = unsafe_fmt.get(offset..) {
                fragments.push(tail.to_string());
            }
            break;
        };
        if let Some(rel) = rest.find('%') {
            let end = offset.saturating_add(1).saturating_add(rel);
            if let Some(piece) = unsafe_fmt.get(offset..end) {
                fragments.push(piece.to_string());
            }
            offset = end;
        } else {
            if let Some(tail) = unsafe_fmt.get(offset..) {
                fragments.push(tail.to_string());
            }
            break;
        }
    }
    fragments
}

/// Format a single C-like conversion plus any trailing literal in `fmt`.
fn format_one(fmt: &str, arg: &PrintfArg) -> String {
    let mut spec = String::new();
    let chars: Vec<char> = fmt.chars().collect();
    // Skip leading '%'
    let mut i = 1;
    let mut left = false;
    let mut sign = false;
    let mut space = false;
    let mut alt = false;
    let mut zero = false;
    while i < chars.len() {
        match chars.get(i).copied() {
            Some('+') => sign = true,
            Some('-') => left = true,
            Some('#') => alt = true,
            Some(' ') => space = true,
            Some('0') => zero = true,
            _ => break,
        }
        i = i.saturating_add(1);
    }
    let mut width: usize = 0;
    while i < chars.len() {
        match chars.get(i).copied() {
            Some(c) if is_decimal_digit(c) => {
                width = width
                    .saturating_mul(10)
                    .saturating_add((c as usize) - ('0' as usize));
                i = i.saturating_add(1);
            }
            _ => break,
        }
    }
    let mut precision: Option<usize> = None;
    if i < chars.len() && chars.get(i).copied() == Some('.') {
        i = i.saturating_add(1);
        let mut p = 0usize;
        let mut saw = false;
        while i < chars.len() {
            match chars.get(i).copied() {
                Some(c) if is_decimal_digit(c) => {
                    saw = true;
                    p = p
                        .saturating_mul(10)
                        .saturating_add((c as usize) - ('0' as usize));
                    i = i.saturating_add(1);
                }
                _ => break,
            }
        }
        precision = Some(if saw { p } else { 0 });
    }
    let spec_char = chars.get(i).copied().unwrap_or('\0');
    i = i.saturating_add(1);
    let rest: String = chars.iter().skip(i).collect();

    // A conversion reads its argument as whatever it needs, coercing across the
    // three argument kinds the way the C library's varargs would.
    let signed = |arg: &PrintfArg| match arg {
        PrintfArg::Int(v) => *v,
        PrintfArg::Double(v) => narrow_to_i32(*v),
        PrintfArg::String(_) => 0,
    };
    // An unsigned conversion reinterprets the same bits, as the C library does.
    let unsigned = |arg: &PrintfArg| signed(arg).cast_unsigned();

    let body = match spec_char {
        'd' | 'i' => {
            let n = signed(arg);
            format_int(
                n.into(),
                10,
                false,
                sign,
                space,
                false,
                zero,
                left,
                width,
                precision,
            )
        }
        'u' => {
            let n = unsigned(arg);
            format_uint(u64::from(n), 10, false, alt, zero, left, width, precision)
        }
        'o' => {
            let n = unsigned(arg);
            format_uint(u64::from(n), 8, false, alt, zero, left, width, precision)
        }
        'x' | 'X' => {
            let n = unsigned(arg);
            format_uint(
                u64::from(n),
                16,
                spec_char == 'X',
                alt,
                zero,
                left,
                width,
                precision,
            )
        }
        'c' | 'C' => {
            let n = match arg {
                PrintfArg::String(s) => s.chars().next().map_or(0, |c| c as u32),
                other => unsigned(other),
            };
            // A character conversion keeps only the low byte, as the C library
            // does when a wider value reaches it.
            let ch = char::from_u32(n & 0xff).unwrap_or('\0');
            pad_str(&ch.to_string(), width, left, false)
        }
        'f' | 'F' => {
            let n = match arg {
                PrintfArg::Double(v) => *v,
                PrintfArg::Int(v) => f64::from(*v),
                PrintfArg::String(_) => 0.0,
            };
            let prec = precision.unwrap_or(6);
            format_float(n, prec, sign, space, left, zero, width)
        }
        'e' | 'E' => {
            let n = match arg {
                PrintfArg::Double(v) => *v,
                PrintfArg::Int(v) => f64::from(*v),
                PrintfArg::String(_) => 0.0,
            };
            let prec = precision.unwrap_or(6);
            let mut s = format!("{n:.prec$e}");
            if spec_char == 'E' {
                s = s.to_ascii_uppercase();
            }
            apply_sign_pad(&s, n, sign, space, left, zero, width)
        }
        'g' | 'G' => {
            let n = match arg {
                PrintfArg::Double(v) => *v,
                PrintfArg::Int(v) => f64::from(*v),
                PrintfArg::String(_) => 0.0,
            };
            let prec = precision.unwrap_or(6).max(1);
            let mut s = format!("{n:.prec$}");
            if spec_char == 'G' {
                s = s.to_ascii_uppercase();
            }
            apply_sign_pad(&s, n, sign, space, left, zero, width)
        }
        's' | 'S' => {
            let s = match arg {
                PrintfArg::String(v) => v.clone(),
                PrintfArg::Int(v) => format!("{v}"),
                PrintfArg::Double(v) => format!("{v}"),
            };
            let sliced = match precision {
                Some(p) => s.chars().take(p).collect(),
                None => s,
            };
            pad_str(&sliced, width, left, false)
        }
        _ => {
            spec.push('%');
            spec.push_str(fmt);
            return spec;
        }
    };
    let mut out = body;
    out.push_str(&rest);
    let _ = (alt, spec);
    out
}

#[allow(clippy::too_many_arguments)]
fn format_int(
    n: i64,
    base: u32,
    upper: bool,
    sign: bool,
    space: bool,
    alt: bool,
    zero: bool,
    left: bool,
    width: usize,
    precision: Option<usize>,
) -> String {
    let neg = n < 0;
    let mag = n.unsigned_abs();
    let mut digits = to_base(mag, base, upper);
    if let Some(p) = precision {
        while digits.len() < p {
            digits.insert(0, '0');
        }
        if p == 0 && mag == 0 {
            digits.clear();
        }
    }
    if alt && base == 16 && mag != 0 {
        digits.insert(0, if upper { 'X' } else { 'x' });
        digits.insert(0, '0');
    }
    let mut prefix = String::new();
    if neg {
        prefix.push('-');
    } else if sign {
        prefix.push('+');
    } else if space {
        prefix.push(' ');
    }
    pad_with_prefix(&prefix, &digits, width, left, zero && precision.is_none())
}

#[allow(clippy::too_many_arguments)]
fn format_uint(
    n: u64,
    base: u32,
    upper: bool,
    alt: bool,
    zero: bool,
    left: bool,
    width: usize,
    precision: Option<usize>,
) -> String {
    let mut digits = to_base(n, base, upper);
    if let Some(p) = precision {
        while digits.len() < p {
            digits.insert(0, '0');
        }
        if p == 0 && n == 0 {
            digits.clear();
        }
    }
    if alt && base == 16 && n != 0 {
        digits.insert(0, if upper { 'X' } else { 'x' });
        digits.insert(0, '0');
    }
    if alt && base == 8 && !digits.starts_with('0') {
        digits.insert(0, '0');
    }
    pad_with_prefix("", &digits, width, left, zero && precision.is_none())
}

fn to_base(mut n: u64, base: u32, upper: bool) -> String {
    if n == 0 {
        return "0".to_string();
    }
    let digits = if upper {
        b"0123456789ABCDEF"
    } else {
        b"0123456789abcdef"
    };
    let mut out = Vec::new();
    while n > 0 {
        // The remainder is below the base, which is at most sixteen, so it
        // always fits an index.
        let d = usize::try_from(n % u64::from(base)).unwrap_or(0);
        if let Some(ch) = digits.get(d).copied() {
            out.push(ch);
        }
        n /= u64::from(base);
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_else(|_| "0".to_string())
}

fn pad_str(s: &str, width: usize, left: bool, zero: bool) -> String {
    if s.len() >= width {
        return s.to_string();
    }
    let pad = width.saturating_sub(s.len());
    let fill = if zero { "0" } else { " " };
    let extra = fill.repeat(pad);
    if left {
        let mut o = s.to_string();
        o.push_str(&extra);
        o
    } else {
        let mut o = extra;
        o.push_str(s);
        o
    }
}

fn pad_with_prefix(prefix: &str, digits: &str, width: usize, left: bool, zero: bool) -> String {
    let body_len = prefix.len().saturating_add(digits.len());
    if body_len >= width {
        let mut o = String::from(prefix);
        o.push_str(digits);
        return o;
    }
    let pad = width.saturating_sub(body_len);
    if left {
        let mut o = String::from(prefix);
        o.push_str(digits);
        o.push_str(&" ".repeat(pad));
        o
    } else if zero {
        let mut o = String::from(prefix);
        o.push_str(&"0".repeat(pad));
        o.push_str(digits);
        o
    } else {
        let mut o = " ".repeat(pad);
        o.push_str(prefix);
        o.push_str(digits);
        o
    }
}

fn format_float(
    n: f64,
    prec: usize,
    sign: bool,
    space: bool,
    left: bool,
    zero: bool,
    width: usize,
) -> String {
    let s = format!("{n:.prec$}");
    apply_sign_pad(&s, n, sign, space, left, zero, width)
}

fn apply_sign_pad(
    s: &str,
    n: f64,
    sign: bool,
    space: bool,
    left: bool,
    zero: bool,
    width: usize,
) -> String {
    let already_signed = s.starts_with('-') || s.starts_with('+');
    let mut prefix = String::new();
    let body = if already_signed {
        s.to_string()
    } else if n.is_sign_negative() && n != 0.0 {
        let mut o = String::from("-");
        o.push_str(s);
        o
    } else if sign {
        prefix.push('+');
        s.to_string()
    } else if space {
        prefix.push(' ');
        s.to_string()
    } else {
        s.to_string()
    };
    if prefix.is_empty() {
        pad_str(&body, width, left, zero && !body.starts_with('-'))
    } else {
        pad_with_prefix(&prefix, &body, width, left, zero)
    }
}

/// `util.printf` — expand a format string against a list of arguments.
///
/// A fragment whose conversion is not one this understands is copied out
/// literally rather than rejected, and a fragment with no argument left to
/// consume is copied out unexpanded. Neither is an error.
///
/// # Errors
///
/// Currently never; the result type is kept so a future conversion that must
/// refuse has somewhere to say so.
pub fn util_printf(fmt: &str, args: &[PrintfArg]) -> Result<String, Error> {
    let fragments = split_percent_fragments(fmt);
    let mut result = fragments.first().cloned().unwrap_or_default();
    for (i, frag) in fragments.iter().enumerate().skip(1) {
        if i > args.len() {
            result.push_str(frag);
            continue;
        }
        let Some(arg) = args.get(i.saturating_sub(1)) else {
            result.push_str(frag);
            continue;
        };
        let mut spec = frag.clone();
        match parse_data_type(&mut spec) {
            PrintfDataType::Invalid => result.push_str(frag),
            _ => result.push_str(&format_one(&spec, arg)),
        }
    }
    // Strip the 'S' sentinel.
    if result.starts_with('S') {
        result.remove(0);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_data_type_matches_cjs_util_unittest() {
        // fxjs/cjs_util_unittest.cpp TEST(CJSUtilTest, ParseDataType)
        let cases: &[(&str, PrintfDataType)] = &[
            ("", PrintfDataType::Invalid),
            ("d", PrintfDataType::Invalid),
            ("%d", PrintfDataType::Int),
            ("%x", PrintfDataType::Int),
            ("%f", PrintfDataType::Double),
            ("%s", PrintfDataType::String),
            ("%+d", PrintfDataType::Int),
            ("%+x", PrintfDataType::Int),
            ("%+f", PrintfDataType::Double),
            ("% d", PrintfDataType::Int),
            ("%0d", PrintfDataType::Int),
            ("%#d", PrintfDataType::Int),
            ("%5d", PrintfDataType::Int),
            ("%05d", PrintfDataType::Int),
            ("%5s", PrintfDataType::String),
            ("%.5f", PrintfDataType::Double),
            ("%.14f", PrintfDataType::Double),
            ("%.1d", PrintfDataType::Int),
            ("%.10d", PrintfDataType::Int),
            ("%.100d", PrintfDataType::Invalid),
            ("%ad", PrintfDataType::Invalid),
            ("%bx", PrintfDataType::Invalid),
            ("%hx", PrintfDataType::Invalid),
            ("%js", PrintfDataType::Invalid),
            ("%+6d", PrintfDataType::Int),
            ("% 7x", PrintfDataType::Int),
            ("%#9.3f", PrintfDataType::Double),
            ("%10s", PrintfDataType::String),
        ];
        for &(input, expected) in cases {
            let mut s = input.to_string();
            assert_eq!(parse_data_type(&mut s), expected, "{input}");
        }
    }

    #[test]
    fn printf_bug_740166_precision_cap() {
        // testing/resources/javascript/bug_740166.in
        // %.1x and %.10x are valid ints; %.100x and %.1000x are copied literally.
        let a = util_printf(
            "Values = %0.1x .9999 %x",
            &[PrintfArg::Int(1), PrintfArg::Int(2)],
        )
        .unwrap();
        assert_eq!(a, "Values = 1 .9999 2");
        let b = util_printf(
            "Values = %0.10x .9999 %x",
            &[PrintfArg::Int(1), PrintfArg::Int(2)],
        )
        .unwrap();
        assert_eq!(b, "Values = 0000000001 .9999 2");
        let c = util_printf(
            "Values = %0.100x .9999 %x",
            &[PrintfArg::Int(1), PrintfArg::Int(2)],
        )
        .unwrap();
        assert_eq!(c, "Values = %0.100x .9999 2");
        let d = util_printf(
            "Values = %0.1000x .9999 %x",
            &[PrintfArg::Int(1), PrintfArg::Int(2)],
        )
        .unwrap();
        assert_eq!(d, "Values = %0.1000x .9999 2");
    }
}
