//! Type 3: stitching functions (ISO 32000-1 §7.10.4).
//!
//! One input, `k` sub-functions, `k - 1` boundaries. The strictness is
//! asymmetric in a way worth naming:
//!
//! - `/Functions`, `/Bounds` and `/Encode` are **all three required**, and an
//!   *empty* `/Bounds` is legal — indeed required — when there is one
//!   sub-function;
//! - size checks reject arrays that are too **short** but accept ones that
//!   are too long, so a `/Encode` with spare entries loads fine;
//! - every sub-function must take one input, produce a non-zero number of
//!   outputs, and **agree with its siblings** on that number, which then
//!   overrides whatever `/Range` implied.
//!
//! There is **no monotonicity check on `/Bounds` and no check that they lie
//! inside `/Domain`.** Out-of-order bounds simply make the interval scan stop
//! early, and the interpolation then runs on an inverted or degenerate
//! interval — extrapolating outside the encode range, or collapsing onto its
//! low end. No error is raised; the selected sub-function's own domain clamp
//! absorbs the result.

use super::{Common, Function, FunctionCache, interpolate};
use crate::names;
use pdfrum_common::{Diagnostics, Limits};
use pdfrum_object::{Dict, Resolve};
use std::sync::Arc;

/// A type 3 function.
#[derive(Debug, Clone, PartialEq)]
pub struct Stitching {
    /// `/Domain`, always exactly one input.
    pub domain: Box<[f32]>,
    /// `/Range`, resized to the children's agreed output count.
    pub range: Box<[f32]>,
    /// The agreed output count, taken from the children.
    pub outputs: usize,
    /// The sub-functions, in order.
    pub functions: Box<[Arc<Function>]>,
    /// The interval boundaries, `[domain_lo, /Bounds…, domain_hi]` — one
    /// longer than the sub-function count.
    pub bounds: Box<[f32]>,
    /// `/Encode`, two per sub-function.
    pub encode: Box<[f32]>,
}

impl Stitching {
    /// Load from a dictionary or a stream's dictionary.
    pub(super) fn load<R: Resolve>(
        dict: &Dict,
        common: &Common,
        r: &R,
        cache: &mut FunctionCache,
        limits: &Limits,
        diags: &mut Diagnostics,
        depth: u32,
    ) -> Option<Self> {
        // Exactly one input, always.
        if common.inputs() != 1 {
            return None;
        }
        let functions_array = dict.array(names::FUNCTIONS, r)?;
        let bounds_array = dict.array(names::BOUNDS, r)?;
        let encode_array = dict.array(names::ENCODE, r)?;

        let count = functions_array.len();
        if count == 0 {
            return None;
        }
        // Too short fails; too long is fine.
        if bounds_array.len() < count - 1 || encode_array.len() < count * 2 {
            return None;
        }

        let mut functions = Vec::with_capacity(count);
        let mut outputs = 0usize;
        for i in 0..count {
            let obj = functions_array.raw_at(i)?;
            let sub = cache.load_at(obj, r, limits, diags, depth + 1)?;
            if sub.input_count() != 1 {
                return None;
            }
            let n = sub.output_count();
            if n == 0 {
                return None;
            }
            // Every child must agree on the output count.
            if i == 0 {
                outputs = n;
            } else if n != outputs {
                return None;
            }
            functions.push(sub);
        }
        if outputs > super::MAX_OUTPUTS {
            return None;
        }

        // `[domain_lo, /Bounds…, domain_hi]`.
        let domain_lo = common.domain.first().copied().unwrap_or(0.0);
        let domain_hi = common.domain.get(1).copied().unwrap_or(0.0);
        let mut bounds = Vec::with_capacity(count + 1);
        bounds.push(domain_lo);
        for i in 0..count.saturating_sub(1) {
            bounds.push(bounds_array.number_at_or_zero(i));
        }
        bounds.push(domain_hi);

        let encode: Box<[f32]> = (0..count * 2)
            .map(|i| encode_array.number_at_or_zero(i))
            .collect();

        // The children's output count overrides whatever `/Range` implied.
        let mut range = common.range.to_vec();
        if !range.is_empty() && range.len() < outputs * 2 {
            range.resize(outputs * 2, 0.0);
        }

        Some(Self {
            domain: common.domain.clone(),
            range: range.into(),
            outputs,
            functions: functions.into(),
            bounds: bounds.into(),
            encode,
        })
    }

    /// Evaluate: pick the interval, re-encode into the sub-function's domain,
    /// call it.
    pub(super) fn eval(&self, input: &[f32], out: &mut [f32]) -> bool {
        let x = input.first().copied().unwrap_or(0.0);
        // Half-open `[bounds[i], bounds[i+1])`, with the last interval closed
        // on the right because the scan is bounded.
        let mut i = 0usize;
        let last = self.functions.len().saturating_sub(1);
        while i < last {
            if x < self.bounds.get(i + 1).copied().unwrap_or(0.0) {
                break;
            }
            i += 1;
        }
        let encoded = interpolate(
            x,
            self.bounds.get(i).copied().unwrap_or(0.0),
            self.bounds.get(i + 1).copied().unwrap_or(0.0),
            self.encode.get(i * 2).copied().unwrap_or(0.0),
            self.encode.get(i * 2 + 1).copied().unwrap_or(0.0),
        );
        let Some(sub) = self.functions.get(i) else {
            return false;
        };
        sub.eval(&[encoded], out).is_ok()
    }
}

#[cfg(test)]
mod tests {
    // Test fixtures quote the oracle's own vectors, compare floats exactly
    // where the behaviour being pinned is exact, and index arrays whose
    // length the fixture itself fixes.
    #![allow(
        clippy::unreadable_literal,
        clippy::float_cmp,
        clippy::indexing_slicing,
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        reason = "test fixtures quote oracle vectors verbatim and compare exactly"
    )]

    use crate::function::{Function, FunctionCache};
    use pdfrum_common::{Diagnostics, Limits};
    use pdfrum_object::{Array, Dict, Name, NoResolve, Object};

    fn nums(values: &[f32]) -> Object {
        Object::Array(Array::of(values.iter().copied().map(Object::Real)))
    }

    /// A type 2 sub-function ramping `lo` to `hi` over `[0, 1]`.
    fn ramp(lo: f32, hi: f32) -> Object {
        Object::Dict(Dict::from_pairs([
            (Name::from("FunctionType"), Object::Int(2)),
            (Name::from("Domain"), nums(&[0.0, 1.0])),
            (Name::from("N"), Object::Int(1)),
            (Name::from("C0"), nums(&[lo])),
            (Name::from("C1"), nums(&[hi])),
        ]))
    }

    fn load(pairs: Vec<(Name, Object)>) -> Option<Function> {
        let mut cache = FunctionCache::new();
        let mut diags = Diagnostics::default();
        cache
            .load(
                &Object::Dict(Dict::from_pairs(pairs)),
                &NoResolve,
                &Limits::default(),
                &mut diags,
            )
            .map(|f| (*f).clone())
    }

    fn two_part(bounds: &[f32], encode: &[f32]) -> Vec<(Name, Object)> {
        vec![
            (Name::from("FunctionType"), Object::Int(3)),
            (Name::from("Domain"), nums(&[0.0, 1.0])),
            (
                Name::from("Functions"),
                Object::Array(Array::of([ramp(0.0, 1.0), ramp(1.0, 0.0)])),
            ),
            (Name::from("Bounds"), nums(bounds)),
            (Name::from("Encode"), nums(encode)),
        ]
    }

    #[test]
    fn intervals_are_half_open_and_re_encoded() {
        let f = load(two_part(&[0.5], &[0.0, 1.0, 0.0, 1.0])).expect("a type 3 function");
        let mut out = [0.0f32];
        // Just below the boundary: the first ramp, at its top.
        f.eval(&[0.499], &mut out).expect("evaluates");
        assert!(out[0] > 0.99, "got {}", out[0]);
        // At the boundary: the second ramp, at its bottom (which is 1.0).
        f.eval(&[0.5], &mut out).expect("evaluates");
        assert!((out[0] - 1.0).abs() < 1e-3, "got {}", out[0]);
        // At the top: the second ramp's top, which is 0.0.
        f.eval(&[1.0], &mut out).expect("evaluates");
        assert!(out[0].abs() < 1e-3, "got {}", out[0]);
    }

    #[test]
    fn all_three_arrays_are_required() {
        let base = two_part(&[0.5], &[0.0, 1.0, 0.0, 1.0]);
        for drop in ["Functions", "Bounds", "Encode"] {
            let pairs: Vec<_> = base
                .iter()
                .filter(|(k, _)| k.as_bytes() != drop.as_bytes())
                .cloned()
                .collect();
            assert!(load(pairs).is_none(), "dropping /{drop} should fail");
        }
    }

    #[test]
    fn an_empty_bounds_is_legal_with_one_sub_function() {
        let pairs = vec![
            (Name::from("FunctionType"), Object::Int(3)),
            (Name::from("Domain"), nums(&[0.0, 1.0])),
            (
                Name::from("Functions"),
                Object::Array(Array::of([ramp(0.0, 1.0)])),
            ),
            (Name::from("Bounds"), nums(&[])),
            (Name::from("Encode"), nums(&[0.0, 1.0])),
        ];
        let f = load(pairs).expect("a type 3 function");
        let mut out = [0.0f32];
        f.eval(&[0.25], &mut out).expect("evaluates");
        assert!((out[0] - 0.25).abs() < 1e-6);
    }

    #[test]
    fn a_longer_than_needed_encode_is_accepted() {
        let f = load(two_part(&[0.5], &[0.0, 1.0, 0.0, 1.0, 9.0, 9.0]))
            .expect("a longer /Encode should still load");
        assert_eq!(f.output_count(), 1);
    }

    #[test]
    fn a_too_short_encode_or_bounds_fails() {
        assert!(load(two_part(&[0.5], &[0.0, 1.0])).is_none());
        assert!(load(two_part(&[], &[0.0, 1.0, 0.0, 1.0])).is_none());
    }

    #[test]
    fn out_of_order_bounds_are_accepted_and_stop_the_scan_early() {
        // A boundary below the domain's start: every input lands in the
        // second interval.
        let f = load(two_part(&[-1.0], &[0.0, 1.0, 0.0, 1.0])).expect("a type 3 function");
        let mut out = [0.0f32];
        f.eval(&[0.0], &mut out).expect("evaluates");
        // The second ramp is 1.0 at its bottom, and the inverted interval
        // `[0, -1]` extrapolates — the point is that it does not error.
        assert!(out[0].is_finite(), "got {}", out[0]);
    }

    #[test]
    fn sub_functions_must_agree_on_output_count() {
        let two_out = Object::Dict(Dict::from_pairs([
            (Name::from("FunctionType"), Object::Int(2)),
            (Name::from("Domain"), nums(&[0.0, 1.0])),
            (Name::from("N"), Object::Int(1)),
            (Name::from("C0"), nums(&[0.0, 0.0])),
            (Name::from("C1"), nums(&[1.0, 1.0])),
        ]));
        let pairs = vec![
            (Name::from("FunctionType"), Object::Int(3)),
            (Name::from("Domain"), nums(&[0.0, 1.0])),
            (
                Name::from("Functions"),
                Object::Array(Array::of([ramp(0.0, 1.0), two_out])),
            ),
            (Name::from("Bounds"), nums(&[0.5])),
            (Name::from("Encode"), nums(&[0.0, 1.0, 0.0, 1.0])),
        ];
        assert!(load(pairs).is_none());
    }

    #[test]
    fn an_empty_functions_array_fails() {
        let pairs = vec![
            (Name::from("FunctionType"), Object::Int(3)),
            (Name::from("Domain"), nums(&[0.0, 1.0])),
            (Name::from("Functions"), Object::Array(Array::new())),
            (Name::from("Bounds"), nums(&[])),
            (Name::from("Encode"), nums(&[])),
        ];
        assert!(load(pairs).is_none());
    }
}
