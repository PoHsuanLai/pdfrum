//! `AFSimple`, `AFSimple_Calculate`, `AFMakeNumber`.

use super::Thrown;
use crate::error::Error;
use crate::parse::{js_to_number, normalize_decimal_mark, string_to_double, trim_spaces};

/// The five operations `AFSimple` and `AFSimple_Calculate` name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimpleOp {
    /// The mean of the contributions.
    Avg,
    /// Their sum.
    Sum,
    /// Their product.
    Prd,
    /// The smallest.
    Min,
    /// The largest.
    Max,
}

impl SimpleOp {
    /// Recognise an operation name, ignoring case.
    ///
    // [oracle-bug] cjs_publicmethods.cpp:215-232 matches the name with
    // `EqualsASCIINoCase`, but the two operations needing a second step check
    // it again with the case-*sensitive* `EqualsASCII`: the average's divide at
    // :1341 and :1456, and the extremum's seed at :1442. A lowercase `"avg"`
    // therefore reaches the combining step, adds, and then never divides —
    // answering a sum under an average's name — and a lowercase `"min"` starts
    // from zero instead of from the first value. pdf.js matches all five names
    // consistently (aform.js:350-363), as does Adobe's documented behaviour.
    // One name, one answer: recognition here is case-insensitive throughout.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            n if n.eq_ignore_ascii_case("AVG") => Self::Avg,
            n if n.eq_ignore_ascii_case("SUM") => Self::Sum,
            n if n.eq_ignore_ascii_case("PRD") => Self::Prd,
            n if n.eq_ignore_ascii_case("MIN") => Self::Min,
            n if n.eq_ignore_ascii_case("MAX") => Self::Max,
            _ => return None,
        })
    }

    /// Fold one more contribution into the running total.
    #[must_use]
    pub fn combine(self, running: f64, next: f64) -> f64 {
        match self {
            // An average is a sum until the count divides it.
            Self::Avg | Self::Sum => running + next,
            Self::Prd => running * next,
            Self::Min => running.min(next),
            Self::Max => running.max(next),
        }
    }

    /// What the running total starts at before any contribution arrives.
    #[must_use]
    pub fn identity(self) -> f64 {
        match self {
            Self::Prd => 1.0,
            Self::Avg | Self::Sum | Self::Min | Self::Max => 0.0,
        }
    }

    /// Whether this operation takes its starting point from the first
    /// contribution rather than from [`Self::identity`].
    #[must_use]
    pub fn seeds_from_first(self) -> bool {
        matches!(self, Self::Min | Self::Max)
    }
}

/// `AFSimple(cFunction, nValue1, nValue2)` — one named operation over two
/// numbers.
///
/// The name is matched without regard to case, consistently: `"avg"`, `"Avg"`
/// and `"AVG"` all answer the mean.
///
/// # Errors
///
/// [`Error::Value`] if either argument is not a number, or the name is not one
/// of the five operations.
pub fn af_simple(op: &str, a: f64, b: f64) -> Result<f64, Thrown> {
    if a.is_nan() || b.is_nan() {
        return Err(Thrown::bare(Error::Value));
    }
    let op = SimpleOp::parse(op).ok_or_else(|| Thrown::bare(Error::Value))?;
    let combined = op.combine(a, b);
    Ok(if op == SimpleOp::Avg {
        combined / 2.0
    } else {
        combined
    })
}

/// Round to six places by scaling, nudging by just under a half, and flooring.
///
/// The nudge is `0.49`, not `0.5`, so a value sitting exactly on a half rounds
/// *down* — and the scale is computed in single precision before being widened,
/// which is what makes the sixth place land where it does. Both are load-bearing
/// and neither is what a careful implementation would choose.
fn round_to_six_places(value: f64) -> f64 {
    let scale = f64::from(10f32.powi(6));
    (value * scale + 0.49).floor() / scale
}

/// The arithmetic half of `AFSimple_Calculate`, over values the caller has
/// already resolved.
///
/// Finding the fields, reading each one according to its kind, and deciding
/// which of them count is the caller's job — it needs a document, and this
/// crate has none. What arrives here is one number per *contributing* field, in
/// order, and that ordering carries a rule: **a field that exists but yields
/// nothing still contributes**, as a zero, and so still divides an average,
/// while a name matching no field at all contributes nothing and does not.
/// A caller that collapses those two cases will get the wrong average.
///
/// The name is matched without regard to case, consistently — see
/// [`SimpleOp::parse`].
///
/// # Errors
///
/// [`Error::Value`] if the operation is not one of the five — but only once
/// there is at least one value, since with none the operation is never
/// consulted and the answer is zero.
pub fn af_simple_calculate(op: &str, values: &[f64]) -> Result<f64, Thrown> {
    let Some(op) = SimpleOp::parse(op) else {
        // An unrecognised name only matters once there is something to combine:
        // with no contributions the operation is never reached, and the answer
        // is the empty total rather than a failure.
        return if values.is_empty() {
            Ok(round_to_six_places(0.0))
        } else {
            Err(Thrown::bare(Error::Value))
        };
    };

    let mut total = op.identity();
    let mut counted = 0usize;
    for (index, value) in values.iter().copied().enumerate() {
        if index == 0 && op.seeds_from_first() {
            total = value;
        }
        total = op.combine(total, value);
        counted = counted.saturating_add(1);
    }
    // One contribution per form field, so a count that overflows a `u32` is not
    // a form; `f64` holds every `u32` exactly, and a zero count never divides.
    if op == SimpleOp::Avg
        && let Ok(divisor) = u32::try_from(counted)
        && divisor > 0
    {
        total /= f64::from(divisor);
    }
    Ok(round_to_six_places(total))
}

/// [`af_simple_calculate`] over field *text*, trimmed and read as a number the
/// permissive way — leading junk signs are skipped and trailing junk is
/// ignored, so `"--100abc"` reads as `-100`.
///
/// # Errors
///
/// As [`af_simple_calculate`].
pub fn af_simple_calculate_texts(op: &str, texts: &[&str]) -> Result<f64, Thrown> {
    let nums: Vec<f64> = texts
        .iter()
        .map(|t| string_to_double(trim_spaces(t)))
        .collect();
    af_simple_calculate(op, &nums)
}

/// Split the comma-separated form of a field-name list, trimming each name.
///
/// An empty string yields no names at all, and a **trailing comma yields no
/// empty final name** — the walk stops as soon as nothing is left, rather than
/// emitting the empty tail a plain split would. `"a,"` is one name, not two.
#[must_use]
pub fn af_split_field_list(s: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut rest = s;
    while !rest.is_empty() {
        if let Some((head, tail)) = rest.split_once(',') {
            names.push(trim_spaces(head).to_string());
            rest = tail;
        } else {
            names.push(trim_spaces(rest).to_string());
            break;
        }
    }
    names
}

/// `AFMakeNumber` — read a string as a number, treating a comma as a decimal
/// mark.
///
/// Unlike the loose reading the calculation path uses, this one is
/// all-or-nothing: trailing junk makes the whole string a non-number and the
/// answer is `0`, so `"2blooey"` is `0` rather than `2`.
#[must_use]
pub fn af_make_number(s: &str) -> f64 {
    let normalized = normalize_decimal_mark(s);
    match js_to_number(&normalized) {
        Some(n) if n.is_nan() => f64::NAN, // the string "NaN"
        Some(n) => n,
        None => 0.0,
    }
}
