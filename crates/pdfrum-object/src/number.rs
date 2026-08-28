//! Number semantics: the two views PDFium's `FX_Number` exposes, and the
//! float-to-decimal spelling its writer uses.
//!
//! # Why there are two integer views
//!
//! PDFium stores a parsed integer as *either* `uint32_t` or `int32_t`
//! depending on whether the token carried a sign, and its two accessors
//! disagree about large unsigned values: the integer accessor reinterprets
//! the `uint32_t` bit pattern as signed (so `4294967295` reads back as `-1`)
//! while the numeric accessor widens it (`4294967296.0`). Both are observable
//! in real files — `/P` in an encryption dictionary is written unsigned and
//! read signed.
//!
//! We keep one variant, [`Object::Int`](crate::Object::Int), holding the
//! *mathematical* value in an `i64`, and reproduce both observables in the
//! accessors: [`as_c_int`] for integer contexts, [`as_c_float`] for numeric
//! ones. Every integer a lexer can produce lies in `-2^31 ..= 2^32 - 1`
//! (see [`INT_RANGE`]) because PDFium's parse rules fold anything wider to 0.

use core::ops::RangeInclusive;

/// The range of integer values a conforming lexer may store in
/// [`Object::Int`](crate::Object::Int).
///
/// An unsigned token accumulates into a `u32` and yields `0` on overflow; a
/// signed token beyond `i32` range yields `0`. Nothing outside this range is
/// reachable from parsing, and the accessors are only faithful within it.
pub const INT_RANGE: RangeInclusive<i64> = -2_147_483_648..=4_294_967_295;

/// The integer view of a stored integer: PDFium's `FX_Number::GetSigned`.
///
/// Values above `i32::MAX` came from an unsigned token and are reinterpreted
/// as a signed 32-bit integer, exactly as C++ does when it hands its
/// `uint32_t` to an `int` accessor.
///
/// ```
/// use pdfrum_object::as_c_int;
///
/// assert_eq!(as_c_int(1245), 1245);
/// assert_eq!(as_c_int(-99), -99);
/// assert_eq!(as_c_int(4_294_967_295), -1);
/// assert_eq!(as_c_int(2_147_483_648), -2_147_483_648);
/// ```
#[must_use]
pub fn as_c_int(v: i64) -> i64 {
    debug_assert!(
        INT_RANGE.contains(&v),
        "Object::Int outside the reachable parse range"
    );
    // Narrow to 32 bits, then reinterpret those bits as signed — the two
    // steps C++ performs implicitly when a `uint32_t` reaches an `int`.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "reproducing the C++ narrowing exactly is the point"
    )]
    let bits = v as u32;
    i64::from(bits.cast_signed())
}

/// The numeric view of a stored integer: PDFium's `FX_Number::GetFloat`.
///
/// No wrapping here — `4294967295` widens to `4294967296.0` because that is
/// the nearest `f32`.
///
/// ```
/// use pdfrum_object::as_c_float;
///
/// assert_eq!(as_c_float(1245), 1245.0);
/// assert_eq!(as_c_float(4_294_967_295), 4_294_967_296.0);
/// ```
#[must_use]
#[expect(
    clippy::cast_precision_loss,
    reason = "matching C++ uint32->float / int32->float, which rounds the same way"
)]
pub fn as_c_float(v: i64) -> f32 {
    debug_assert!(
        INT_RANGE.contains(&v),
        "Object::Int outside the reachable parse range"
    );
    v as f32
}

/// The integer view of a real: a saturating cast, with NaN mapping to 0.
///
/// PDFium reaches this through `pdfium::saturated_cast<int32_t>`.
///
/// ```
/// use pdfrum_object::real_as_c_int;
///
/// assert_eq!(real_as_c_int(5.2), 5);
/// assert_eq!(real_as_c_int(-5.9), -5);
/// assert_eq!(real_as_c_int(f32::NAN), 0);
/// assert_eq!(real_as_c_int(f32::INFINITY), i64::from(i32::MAX));
/// ```
#[must_use]
pub fn real_as_c_int(v: f32) -> i64 {
    if v.is_nan() {
        return 0;
    }
    // Rust's `as` on floats already saturates at the target's bounds.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "saturating float->int is exactly the C++ behavior being matched"
    )]
    i64::from(v as i32)
}

/// The digits and base-10 exponent of the shortest decimal that round-trips
/// to `v`, with trailing zeros removed: `v == digits * 10^exponent`.
///
/// `v` must be finite, non-zero and positive. Derived from `ryu`'s shortest
/// representation, which produces the same digit string as the dragonbox
/// algorithm PDFium uses.
fn shortest_decimal(v: f32) -> (Vec<u8>, i32) {
    let mut buf = ryu::Buffer::new();
    let s = buf.format_finite(v);
    let s = s.strip_prefix('-').unwrap_or(s);

    let (mantissa, exp10) = match s.split_once(['e', 'E']) {
        Some((m, e)) => (m, e.parse::<i32>().unwrap_or(0)),
        None => (s, 0),
    };
    let (int_part, frac_part) = mantissa.split_once('.').unwrap_or((mantissa, ""));

    let mut digits: Vec<u8> = int_part
        .bytes()
        .chain(frac_part.bytes())
        .skip_while(|b| *b == b'0')
        .collect();
    let frac_len = i32::try_from(frac_part.len()).unwrap_or(i32::MAX);
    let mut exponent = exp10.saturating_sub(frac_len);

    while digits.len() > 1 && digits.last() == Some(&b'0') {
        digits.pop();
        exponent = exponent.saturating_add(1);
    }
    (digits, exponent)
}

/// Longest output PDFium's float writer will produce, in bytes.
/// C++ sizes its buffer at 49 including the terminating NUL, so 48 characters
/// of payload; the cap only ever binds for `-f32::MIN` denormals.
const MAX_FLOAT_DECIMAL_LEN: usize = 48;

/// Spell a real the way PDFium's content-stream and object writers do.
///
/// Never scientific notation, never a leading zero before the point, NaN and
/// both zeros render as `"0"`, and the infinities render as the finite
/// extremes (PDF has no syntax for either).
///
/// ```
/// use pdfrum_object::fmt_number;
///
/// assert_eq!(fmt_number(0.0), "0");
/// assert_eq!(fmt_number(-0.0), "0");
/// assert_eq!(fmt_number(-7.5), "-7.5");
/// assert_eq!(fmt_number(0.5), ".5");
/// assert_eq!(fmt_number(f32::NAN), "0");
/// assert_eq!(fmt_number(f32::MAX), fmt_number(f32::INFINITY));
/// ```
#[must_use]
pub fn fmt_number(value: f32) -> String {
    if value.is_nan() || value == 0.0 {
        return "0".to_owned();
    }

    let mut magnitude = if value == f32::INFINITY {
        f32::MAX
    } else if value == f32::NEG_INFINITY {
        f32::MIN
    } else {
        value
    };

    let mut out = String::new();
    if magnitude < 0.0 {
        out.push('-');
        magnitude = -magnitude;
    }

    let (digits, exponent) = shortest_decimal(magnitude);
    let digit_count = i32::try_from(digits.len()).unwrap_or(i32::MAX);

    if exponent >= 0 {
        out.push_str(&String::from_utf8_lossy(&digits));
        for _ in 0..exponent {
            out.push('0');
        }
        return out;
    }

    let places_before_point = digit_count + exponent;
    if places_before_point > 0 {
        let split = usize::try_from(places_before_point).unwrap_or(0);
        let (whole, fraction) = digits.split_at(split.min(digits.len()));
        out.push_str(&String::from_utf8_lossy(whole));
        out.push('.');
        out.push_str(&String::from_utf8_lossy(fraction));
        return out;
    }

    // Value below 1: no leading zero, then the zeros the exponent asks for,
    // then the digits — truncated at the writer's buffer size.
    out.push('.');
    for _ in 0..-places_before_point {
        out.push('0');
    }
    for d in digits {
        out.push(char::from(d));
        if out.len() >= MAX_FLOAT_DECIMAL_LEN {
            break;
        }
    }
    out
}

/// Spell an integer the way PDFium's writers do: through the C-int view, so a
/// stored `4294967295` writes as `-1`.
///
/// ```
/// use pdfrum_object::fmt_int;
///
/// assert_eq!(fmt_int(1234), "1234");
/// assert_eq!(fmt_int(-54321), "-54321");
/// assert_eq!(fmt_int(4_294_967_295), "-1");
/// ```
#[must_use]
pub fn fmt_int(value: i64) -> String {
    as_c_int(value).to_string()
}

#[cfg(test)]
#[expect(
    clippy::float_cmp,
    reason = "these assertions pin exact bit patterns the oracle produces"
)]
mod tests {
    use super::{as_c_float, as_c_int, fmt_int, fmt_number, real_as_c_int};

    // Restated from FX_Number's tri-state semantics (fx_number.cpp:87-115):
    // the same stored value reads differently through the two accessors.
    #[test]
    fn integer_view_wraps_the_unsigned_range() {
        assert_eq!(as_c_int(0), 0);
        assert_eq!(as_c_int(1245), 1245);
        assert_eq!(as_c_int(-2_147_483_648), -2_147_483_648);
        assert_eq!(as_c_int(2_147_483_647), 2_147_483_647);
        // Beyond i32::MAX the token was unsigned, so the bits reinterpret.
        assert_eq!(as_c_int(2_147_483_648), -2_147_483_648);
        assert_eq!(as_c_int(4_294_967_295), -1);
        assert_eq!(as_c_int(4_294_967_294), -2);
    }

    #[test]
    fn numeric_view_widens_instead_of_wrapping() {
        assert_eq!(as_c_float(0), 0.0);
        assert_eq!(as_c_float(1245), 1245.0);
        assert_eq!(as_c_float(-2_147_483_648), -2_147_483_648.0);
        assert_eq!(as_c_float(4_294_967_295), 4_294_967_296.0);
    }

    #[test]
    fn real_to_integer_saturates_and_zeroes_nan() {
        assert_eq!(real_as_c_int(5.2), 5);
        assert_eq!(real_as_c_int(9.003_45), 9);
        assert_eq!(real_as_c_int(-0.5), 0);
        assert_eq!(real_as_c_int(f32::NAN), 0);
        assert_eq!(real_as_c_int(f32::INFINITY), i64::from(i32::MAX));
        assert_eq!(real_as_c_int(f32::NEG_INFINITY), i64::from(i32::MIN));
        assert_eq!(real_as_c_int(1e30), i64::from(i32::MAX));
    }

    // Goldens from cpdf_contentstream_write_utils_unittest.cpp:26-50 and
    // cpdf_number_unittest.cpp:37-131 — the dragonbox-parity anchors.
    #[test]
    fn float_spelling_matches_the_oracle() {
        let cases: &[(f32, &str)] = &[
            (0.0, "0"),
            (-0.0, "0"),
            (1.0, "1"),
            (-1.0, "-1"),
            (0.5, ".5"),
            (-0.5, "-.5"),
            (0.001_25, ".00125"),
            (123.45, "123.45"),
            (-7.5, "-7.5"),
            (38.895_285, "38.895287"),
            (-77.037_23, "-77.03723"),
            (9.003_45, "9.00345"),
            (0.23, ".23"),
            (f32::MAX, "340282350000000000000000000000000000000"),
            (-f32::MAX, "-340282350000000000000000000000000000000"),
            (
                f32::MIN_POSITIVE,
                ".000000000000000000000000000000000000011754944",
            ),
            (
                -f32::MIN_POSITIVE,
                "-.000000000000000000000000000000000000011754944",
            ),
            (f32::INFINITY, "340282350000000000000000000000000000000"),
            (
                f32::NEG_INFINITY,
                "-340282350000000000000000000000000000000",
            ),
            (f32::NAN, "0"),
        ];
        for (value, want) in cases {
            assert_eq!(&fmt_number(*value), want, "spelling {value}");
        }
    }

    #[test]
    fn smallest_denormal_hits_the_writer_length_cap() {
        // The one input where C++'s 49-byte buffer truncates the digits.
        let smallest = f32::from_bits(1);
        assert_eq!(fmt_number(-smallest).len(), 47);
        assert!(fmt_number(smallest).len() <= 48);
    }

    // From cpdf_number_unittest.cpp:105-131.
    #[test]
    fn integer_spelling_matches_the_oracle() {
        assert_eq!(fmt_int(0), "0");
        assert_eq!(fmt_int(1), "1");
        assert_eq!(fmt_int(-99), "-99");
        assert_eq!(fmt_int(1234), "1234");
        assert_eq!(fmt_int(-54321), "-54321");
        assert_eq!(fmt_int(2_147_483_647), "2147483647");
        assert_eq!(fmt_int(-2_147_483_648), "-2147483648");
        assert_eq!(fmt_int(4_294_967_295), "-1");
    }
}
