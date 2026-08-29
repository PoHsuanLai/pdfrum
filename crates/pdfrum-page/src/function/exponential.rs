//! Type 2: exponential interpolation (ISO 32000-1 §7.10.3).
//!
//! `C0 + x^N * (C1 - C0)`, evaluated with an unguarded `powf`, which is where
//! the interesting behaviour lives:
//!
//! - a **negative `N` with input 0** gives `+inf`; a `/Range` clamps it back
//!   to something finite, and without one the infinity escapes to the caller;
//! - a **non-integer `N` with a negative input** gives NaN, which passes
//!   through the range clamp unchanged;
//! - `N == 0` gives 1.0 for every input, including 0.
//!
//! Inputs are already domain-clamped when they arrive, and the domain is
//! **not** normalized onto `[0, 1]` first — a `/Domain [0 10]` feeds `10`
//! straight into `powf`.
//!
//! # The multi-input degradation
//!
//! This type is only meaningful with one input. With more, PDFium multiplies
//! its output count by the input count and lets `/Range` be zero-extended,
//! so every output past the original `/Range` entries clamps to `[0, 0]` and
//! reads as zero. That is the documented failure mode, and it is reproduced.

use super::Common;
use crate::names;
use pdfrum_object::{Dict, Resolve};

/// A type 2 function.
#[derive(Debug, Clone, PartialEq)]
pub struct Exponential {
    /// `/Domain`.
    pub domain: Box<[f32]>,
    /// `/Range`, zero-extended when the multi-input degradation applies.
    pub range: Box<[f32]>,
    /// `/C0`, defaulting to a single 0.0.
    pub c0: Box<[f32]>,
    /// `/C1`, defaulting to a single 1.0.
    pub c1: Box<[f32]>,
    /// `/N`, unconstrained.
    pub exponent: f32,
    /// Outputs per input, before the multi-input multiplication.
    pub orig_outputs: usize,
    /// The reported output count, `orig_outputs * inputs`.
    pub outputs: usize,
}

impl Exponential {
    /// Load from a dictionary or a stream's dictionary — this type accepts
    /// either.
    pub(super) fn load(dict: &Dict, common: &Common, r: &impl Resolve) -> Option<Self> {
        // `/N` is required and must be a number.
        let exponent = dict.number(names::N, r)?;
        let c0_array = dict.array(names::C0, r);
        let c1_array = dict.array(names::C1, r);

        // `/Range` wins over `/C0`; with neither, one output.
        let mut orig_outputs = common.outputs();
        if orig_outputs == 0 {
            orig_outputs = c0_array.as_ref().map_or(0, pdfrum_object::Array::len);
        }
        if orig_outputs == 0 {
            orig_outputs = 1;
        }
        if orig_outputs > super::MAX_OUTPUTS {
            return None;
        }

        // Short arrays yield the missing entries' defaults, not a failure.
        let c0 = (0..orig_outputs)
            .map(|i| c0_array.as_ref().map_or(0.0, |a| a.number_at_or_zero(i)))
            .collect();
        let c1 = (0..orig_outputs)
            .map(|i| c1_array.as_ref().map_or(1.0, |a| a.number_at_or_zero(i)))
            .collect();

        let inputs = common.inputs();
        let outputs = orig_outputs.checked_mul(inputs)?;
        if outputs > super::MAX_OUTPUTS {
            return None;
        }

        // The zero-extension that degrades a multi-input type 2.
        let mut range = common.range.to_vec();
        if !range.is_empty() && range.len() < outputs * 2 {
            range.resize(outputs * 2, 0.0);
        }

        Some(Self {
            domain: common.domain.clone(),
            range: range.into(),
            c0,
            c1,
            exponent,
            orig_outputs,
            outputs,
        })
    }

    /// Evaluate.
    pub(super) fn eval(&self, input: &[f32], out: &mut [f32]) -> bool {
        for (i, x) in input.iter().enumerate() {
            let powered = x.powf(self.exponent);
            for j in 0..self.orig_outputs {
                let c0 = self.c0.get(j).copied().unwrap_or(0.0);
                let c1 = self.c1.get(j).copied().unwrap_or(1.0);
                if let Some(slot) = out.get_mut(i * self.orig_outputs + j) {
                    *slot = c0 + powered * (c1 - c0);
                }
            }
        }
        true
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

    fn base(n: f32) -> Vec<(Name, Object)> {
        vec![
            (Name::from("FunctionType"), Object::Int(2)),
            (Name::from("Domain"), nums(&[0.0, 1.0])),
            (Name::from("N"), Object::Real(n)),
        ]
    }

    #[test]
    fn a_linear_ramp_between_c0_and_c1() {
        let mut pairs = base(1.0);
        pairs.push((Name::from("C0"), nums(&[0.0, 0.0, 1.0])));
        pairs.push((Name::from("C1"), nums(&[1.0, 0.5, 0.0])));
        let f = load(pairs).expect("a type 2 function");
        assert_eq!(f.output_count(), 3);
        let mut out = [0.0f32; 3];
        f.eval(&[0.5], &mut out).expect("evaluates");
        assert!((out[0] - 0.5).abs() < 1e-6);
        assert!((out[1] - 0.25).abs() < 1e-6);
        assert!((out[2] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn n_is_required_and_unconstrained() {
        // Missing `/N` fails.
        let pairs = vec![
            (Name::from("FunctionType"), Object::Int(2)),
            (Name::from("Domain"), nums(&[0.0, 1.0])),
        ];
        assert!(load(pairs).is_none());
        // Zero, negative and fractional all load.
        for n in [0.0f32, -1.0, 0.5] {
            assert!(load(base(n)).is_some(), "N = {n} should load");
        }
    }

    #[test]
    fn n_zero_yields_one_everywhere() {
        let f = load(base(0.0)).expect("a type 2 function");
        let mut out = [0.0f32];
        for x in [0.0f32, 0.5, 1.0] {
            f.eval(&[x], &mut out).expect("evaluates");
            assert!((out[0] - 1.0).abs() < 1e-6, "at {x} got {}", out[0]);
        }
    }

    #[test]
    fn a_negative_exponent_at_zero_is_infinite_unless_a_range_clamps_it() {
        let f = load(base(-1.0)).expect("a type 2 function");
        let mut out = [0.0f32];
        f.eval(&[0.0], &mut out).expect("evaluates");
        assert!(out[0].is_infinite(), "got {}", out[0]);

        // With a `/Range` the infinity is clamped finite.
        let mut pairs = base(-1.0);
        pairs.push((Name::from("Range"), nums(&[0.0, 1.0])));
        let f = load(pairs).expect("a type 2 function");
        f.eval(&[0.0], &mut out).expect("evaluates");
        assert!((out[0] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn extra_outputs_of_a_multi_input_function_clamp_to_zero() {
        let pairs = vec![
            (Name::from("FunctionType"), Object::Int(2)),
            (Name::from("Domain"), nums(&[0.0, 1.0, 0.0, 1.0])),
            (Name::from("Range"), nums(&[0.0, 1.0])),
            (Name::from("N"), Object::Int(1).clone()),
        ];
        let f = load(pairs).expect("a type 2 function");
        // One `/Range` pair times two inputs: two reported outputs.
        assert_eq!(f.output_count(), 2);
        let mut out = [9.0f32; 2];
        f.eval(&[0.5, 0.5], &mut out).expect("evaluates");
        assert!((out[0] - 0.5).abs() < 1e-6);
        // The zero-extended range clamps the second output to exactly zero.
        assert!(out[1].abs() < 1e-6, "got {}", out[1]);
    }

    #[test]
    fn c0_and_c1_default_to_zero_and_one() {
        let f = load(base(1.0)).expect("a type 2 function");
        assert_eq!(f.output_count(), 1);
        let mut out = [0.0f32];
        f.eval(&[0.0], &mut out).expect("evaluates");
        assert!(out[0].abs() < 1e-6);
        f.eval(&[1.0], &mut out).expect("evaluates");
        assert!((out[0] - 1.0).abs() < 1e-6);
    }
}
