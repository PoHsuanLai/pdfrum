//! The two float spellings a generated appearance stream uses.
//!
//! `cpdf_generateap.cpp` writes floats two ways and the choice is **per call
//! site**, not per function: the newer emitters (form fields, free text,
//! borders, colours, the sticky-note symbol) go through a shortest-round-trip
//! writer, while the older per-subtype markup generators stream the float
//! into a C++ `ostream` and get `%g` with six significant digits. Six of the
//! ten `GenerateAnnotAP` cases are the second kind. Getting one site wrong
//! costs byte parity on every file that uses it, so both live here, named
//! apart, and every emitter names the one it wants.
//!
//! The two spellings side by side, which `the_two_writers_disagree_where_it
//! _matters` below asserts: the shortest writer drops a leading zero and
//! never goes scientific (`0.5` is `.5`, `1e7` is `10000000`); the ostream
//! writer keeps the zero and switches to exponent form (`0.5`, `1e+07`).

/// The buffer cap the shortest writer works within; a value whose decimal
/// expansion would exceed it loses its tail.
const MAX_LENGTH: usize = 49;

/// Formatter **A**: shortest round-trip, fixed notation, no leading zero.
///
/// The rules, in the order they apply:
///
/// - `NaN` and either zero write as `0`.
/// - An infinity is replaced by the finite extreme of the same sign and then
///   formatted normally, so `f32::INFINITY` writes as `f32::MAX`'s digits.
/// - Scientific notation never appears: a small value gets leading zeros
///   after the point, a large one trailing zeros before it.
/// - A magnitude below one writes **no leading zero** — `0.5` is `.5`. This
///   is the most visible difference from Rust's own `{}`.
#[must_use]
pub fn shortest(value: f32) -> String {
    if value.is_nan() || value == 0.0 {
        return "0".to_owned();
    }
    let value = if value.is_infinite() {
        if value > 0.0 { f32::MAX } else { -f32::MAX }
    } else {
        value
    };

    let mut out = String::new();
    if value < 0.0 {
        out.push('-');
    }

    // `ryu` gives the shortest round-tripping decimal; splitting it back into
    // digits and an exponent is the same `(significand, exponent)` pair the
    // C++'s dragonbox call produces, with trailing zeros already removed.
    let mut buffer = ryu::Buffer::new();
    let (digits, exponent) = decompose(buffer.format_finite(value.abs()));

    if exponent >= 0 {
        out.push_str(&digits);
        for _ in 0..exponent {
            if out.len() + 1 >= MAX_LENGTH {
                break;
            }
            out.push('0');
        }
        return out;
    }

    let places_before = i32::try_from(digits.len()).unwrap_or(i32::MAX) + exponent;
    if places_before > 0 {
        let split = usize::try_from(places_before).unwrap_or(0);
        out.push_str(digits.get(..split).unwrap_or_default());
        out.push('.');
        out.push_str(digits.get(split..).unwrap_or_default());
        return out;
    }

    out.push('.');
    for _ in 0..-places_before {
        if out.len() + 1 >= MAX_LENGTH {
            return out;
        }
        out.push('0');
    }
    out.push_str(&digits);
    out
}

/// Splits `ryu`'s output into a bare digit string and a base-ten exponent,
/// such that the value is `digits * 10^exponent`.
fn decompose(formatted: &str) -> (String, i32) {
    let (mantissa, exponent) = match formatted.split_once(['e', 'E']) {
        Some((m, e)) => (m, e.parse::<i32>().unwrap_or(0)),
        None => (formatted, 0),
    };
    let (integer, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));

    let mut digits: String = integer.chars().chain(fraction.chars()).collect();
    let mut exponent = exponent - i32::try_from(fraction.len()).unwrap_or(0);

    // `ryu` writes a lone `0.0` for zero and a trailing `.0` for integers;
    // both leave zeros the C++'s `trailing_zero::remove` would not have.
    let trimmed = digits.trim_end_matches('0').len();
    if trimmed > 0 {
        exponent += i32::try_from(digits.len() - trimmed).unwrap_or(0);
        digits.truncate(trimmed);
    }
    let leading = digits.len() - digits.trim_start_matches('0').len();
    if leading > 0 && leading < digits.len() {
        digits.replace_range(..leading, "");
    }
    (digits, exponent)
}

/// Formatter **B**: C++'s default `ostream` spelling of a float — `%g` with
/// six significant digits.
///
/// Scientific notation kicks in below `1e-5` and at or above `1e6`, the
/// exponent carries a sign and at least two digits, and trailing zeros in the
/// significand are removed. Negative zero keeps its sign here, unlike
/// [`shortest`].
#[must_use]
pub fn g6(value: f32) -> String {
    let wide = f64::from(value);
    if wide.is_nan() {
        return if wide.is_sign_negative() {
            "-nan"
        } else {
            "nan"
        }
        .to_owned();
    }
    if wide.is_infinite() {
        return if wide < 0.0 { "-inf" } else { "inf" }.to_owned();
    }
    if wide == 0.0 {
        return if wide.is_sign_negative() { "-0" } else { "0" }.to_owned();
    }

    // %g picks the exponent from the value rounded to six significant digits,
    // so `999999.6` reports exponent 6 and prints as `1e+06`, not `1000000`.
    let exponent = {
        // A first estimate, only used if the exact spelling below fails.
        let decade = wide.abs().log10().floor();
        let raw = if decade.is_finite() {
            #[allow(clippy::cast_possible_truncation)]
            {
                decade.clamp(-999.0, 999.0) as i32
            }
        } else {
            0
        };
        let rounded = format!("{:.*e}", 5, wide.abs());
        rounded
            .split_once('e')
            .and_then(|(_, e)| e.parse::<i32>().ok())
            .unwrap_or(raw)
    };

    if !(-4..6).contains(&exponent) {
        let mantissa = trim_zeros(&format!("{:.*}", 5, wide / 10f64.powi(exponent)));
        let sign = if exponent < 0 { '-' } else { '+' };
        return format!("{mantissa}e{sign}{:02}", exponent.abs());
    }
    let places = usize::try_from((5 - exponent).max(0)).unwrap_or(0);
    trim_zeros(&format!("{wide:.places$}"))
}

/// Removes a fractional part's trailing zeros, and the point with them.
fn trim_zeros(text: &str) -> String {
    if !text.contains('.') {
        return text.to_owned();
    }
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

#[cfg(test)]
mod tests {
    use super::{g6, shortest};

    // The C++ reference column, produced by the two writers this file
    // reimplements: `WriteFloat` (dragonbox, no leading zero, never
    // scientific) and `std::ostream::operator<<(float)` (%g, six significant
    // digits).
    const CASES: &[(f32, &str, &str)] = &[
        (0.0, "0", "0"),
        (-0.0, "0", "-0"),
        (0.5, ".5", "0.5"),
        (-0.5, "-.5", "-0.5"),
        (0.25, ".25", "0.25"),
        (1.0, "1", "1"),
        (12.0, "12", "12"),
        (1e-7, ".0000001", "1e-07"),
        (1e7, "10000000", "1e+07"),
        // Shortest round-trip, not the exact value: the nearest `f32` to
        // 123456789 is 123456792, and 123456790 is the shortest decimal that
        // still selects it.
        (123_456_789.0, "123456790", "1.23457e+08"),
        (0.1, ".1", "0.1"),
        (1.0 / 3.0, ".33333334", "0.333333"),
        (100.0, "100", "100"),
        (0.062_5, ".0625", "0.0625"),
        (-3.75, "-3.75", "-3.75"),
        (1e-5, ".00001", "1e-05"),
        (1e-4, ".0001", "0.0001"),
        (999_999.0, "999999", "999999"),
    ];

    #[test]
    fn both_formatters_match_their_c_plus_plus_reference() {
        for &(value, a, b) in CASES {
            assert_eq!(shortest(value), a, "shortest({value})");
            assert_eq!(g6(value), b, "g6({value})");
        }
    }

    #[test]
    fn infinities_and_nan_diverge_between_the_two() {
        // The shortest writer substitutes the finite extreme; %g does not.
        assert_eq!(shortest(f32::INFINITY), shortest(f32::MAX));
        assert_eq!(shortest(f32::NEG_INFINITY), shortest(-f32::MAX));
        assert_eq!(shortest(f32::NAN), "0");
        assert_eq!(g6(f32::INFINITY), "inf");
        assert_eq!(g6(f32::NEG_INFINITY), "-inf");
        assert_eq!(g6(f32::NAN), "nan");
    }

    #[test]
    fn the_shortest_writer_stays_in_fixed_notation_at_both_extremes() {
        let big = shortest(f32::MAX);
        assert!(!big.contains('e'), "{big}");
        assert!(big.starts_with("340282350000000000000000000000000000000"));
        let small = shortest(f32::MIN_POSITIVE);
        assert!(small.starts_with(".000000000000000000000000000000000000011754944"));
        assert!(small.len() < 49);
    }

    #[test]
    fn every_shortest_output_round_trips() {
        for &(value, _, _) in CASES {
            if value.is_nan() {
                continue;
            }
            let text = shortest(value);
            let back: f32 = if let Some(rest) = text.strip_prefix('.') {
                format!("0.{rest}").parse()
            } else if let Some(rest) = text.strip_prefix("-.") {
                format!("-0.{rest}").parse()
            } else {
                text.parse()
            }
            .unwrap_or(f32::NAN);
            assert!(
                (back - value).abs() < f32::EPSILON * value.abs().max(1.0),
                "{text} did not round-trip"
            );
        }
    }

    /// Was the module doctest until `ap::fmt` went private with the rest of
    /// `ap`'s internals (§WP8's recurring cost). It is the one assertion that
    /// shows both writers on the same two inputs, so it is kept rather than
    /// folded into the table-driven tests above.
    #[test]
    fn the_two_writers_disagree_where_it_matters() {
        assert_eq!(shortest(0.5), ".5");
        assert_eq!(shortest(1e7), "10000000");
        assert_eq!(g6(0.5), "0.5");
        assert_eq!(g6(1e7), "1e+07");
    }
}
