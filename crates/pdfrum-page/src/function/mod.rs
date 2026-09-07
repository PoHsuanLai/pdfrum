//! PDF functions, types 0, 2, 3 and 4 (ISO 32000-1 §7.10).
//!
//! **Type 1 does not exist here**, because it does not exist in PDFium
//! either: the type dispatcher accepts exactly 0, 2, 3 and 4 and rejects
//! everything else, so a `/FunctionType 1` sampled-spline function loads as
//! nothing at all.
//!
//! # `/Domain` is the one universally required key
//!
//! Every type needs it, and `inputs = len(Domain) / 2` — integer division, so
//! an odd-length array truncates and a one-element array yields **zero**
//! inputs, which fails the load. `/Range` is required only for types 0 and 4.
//!
//! # Nothing is capped in the C++
//!
//! There is no limit on input count, output count, or type-3 nesting depth;
//! recursion is bounded only by a cycle set and, in practice, by the native
//! stack. A `Vec` sized from untrusted data is a denial-of-service vector
//! Rust must not accept, so [`FunctionCache::load`] enforces
//! [`MAX_DEPTH`] and [`MAX_OUTPUTS`]. Exceeding either fails the load, which
//! is the same observable outcome as the C++'s stack overflow but survivable.

mod exponential;
mod postscript;
mod sampled;
mod stitching;

pub(crate) use sampled::BitReader;

pub(crate) use exponential::Exponential;
pub use postscript::{PostScript, parse_program};
pub(crate) use sampled::Sampled;
pub(crate) use stitching::Stitching;

use crate::error::Error;
use crate::names;
use pdfrum_common::{DiagKind, Diagnostics, Limits, Severity};
use pdfrum_object::{Dict, ObjRef, Object, Resolve};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// How deep type-3 stitching may nest before the load fails.
pub const MAX_DEPTH: u32 = 32;

/// The largest output count a function may declare.
pub const MAX_OUTPUTS: usize = 1024;

/// A loaded PDF function.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Function {
    /// Type 0: samples on a regular grid, interpolated.
    Sampled(Sampled),
    /// Type 2: `C0 + x^N * (C1 - C0)`.
    Exponential(Exponential),
    /// Type 3: sub-functions stitched over sub-intervals of the domain.
    Stitching(Stitching),
    /// Type 4: a small PostScript calculator program.
    PostScript(PostScript),
}

impl Function {
    /// How many inputs the function takes.
    #[must_use]
    pub fn input_count(&self) -> usize {
        self.domain().len() / 2
    }

    /// How many outputs the function produces.
    #[must_use]
    pub fn output_count(&self) -> usize {
        match self {
            Self::Sampled(f) => f.outputs,
            Self::Exponential(f) => f.outputs,
            Self::Stitching(f) => f.outputs,
            Self::PostScript(f) => f.outputs,
        }
    }

    /// The input intervals, `[lo0, hi0, lo1, hi1, …]`.
    #[must_use]
    pub fn domain(&self) -> &[f32] {
        match self {
            Self::Sampled(f) => &f.domain,
            Self::Exponential(f) => &f.domain,
            Self::Stitching(f) => &f.domain,
            Self::PostScript(f) => &f.domain,
        }
    }

    /// The output intervals, empty when `/Range` was absent.
    ///
    /// An empty range **skips output clamping entirely**, which is how a
    /// type 2 function with a negative exponent can return an infinity.
    #[must_use]
    pub fn range(&self) -> &[f32] {
        match self {
            Self::Sampled(f) => &f.range,
            Self::Exponential(f) => &f.range,
            Self::Stitching(f) => &f.range,
            Self::PostScript(f) => &f.range,
        }
    }

    /// Evaluate the function.
    ///
    /// # Errors
    ///
    /// [`Error::FunctionArity`] when `input.len()` is not the declared input
    /// count or `out` is shorter than the declared output count, and
    /// [`Error::FunctionInterval`] when a `/Domain` or `/Range` interval has
    /// its bounds the wrong way round — both of which PDFium reports by
    /// returning "no result" and painting nothing.
    ///
    /// ```
    /// # use pdfrum_common::{Diagnostics, Limits};
    /// # use pdfrum_object::{Array, Dict, Name, NoResolve, Object};
    /// # use pdfrum_page::FunctionCache;
    /// let dict = Dict::from_pairs([
    ///     (Name::from("FunctionType"), Object::Int(2)),
    ///     (Name::from("Domain"), Object::Array(Array::of([Object::Int(0), Object::Int(1)]))),
    ///     (Name::from("N"), Object::Int(1)),
    /// ]);
    /// let mut cache = FunctionCache::new();
    /// let mut diags = Diagnostics::default();
    /// let f = cache
    ///     .load(&Object::Dict(dict), &NoResolve, &Limits::default(), &mut diags)
    ///     .expect("a type 2 function");
    ///
    /// let mut out = [0.0f32];
    /// f.eval(&[0.25], &mut out).expect("evaluates");
    /// assert!((out[0] - 0.25).abs() < 1e-6);
    /// ```
    pub fn eval(&self, input: &[f32], out: &mut [f32]) -> Result<(), Error> {
        let inputs = self.input_count();
        let outputs = self.output_count();
        if input.len() != inputs || out.len() < outputs {
            return Err(Error::FunctionArity {
                expected: inputs,
                got: input.len(),
                outputs,
                got_outputs: out.len(),
            });
        }
        // Clamp each input into its domain interval, refusing an inverted one.
        let mut clamped = Vec::with_capacity(inputs);
        let domain = self.domain();
        for (i, v) in input.iter().enumerate() {
            let lo = domain.get(i * 2).copied().unwrap_or(0.0);
            let hi = domain.get(i * 2 + 1).copied().unwrap_or(0.0);
            if lo > hi {
                return Err(Error::FunctionInterval);
            }
            clamped.push(v.clamp(lo, hi));
        }

        if !self.eval_raw(&clamped, out) {
            return Err(Error::FunctionInterval);
        }

        // An empty `/Range` skips clamping entirely.
        let range = self.range();
        if range.is_empty() {
            return Ok(());
        }
        for i in 0..outputs {
            let lo = range.get(i * 2).copied().unwrap_or(0.0);
            let hi = range.get(i * 2 + 1).copied().unwrap_or(0.0);
            if lo > hi {
                return Err(Error::FunctionInterval);
            }
            if let Some(slot) = out.get_mut(i) {
                *slot = slot.clamp(lo, hi);
            }
        }
        Ok(())
    }

    /// Evaluate, returning the number of outputs written and **0** on any
    /// failure.
    ///
    /// This is the shape every colorspace and shading call site wants: the
    /// C++ returns an optional count, and zero means "paint nothing".
    #[must_use]
    pub fn eval_into(&self, input: &[f32], out: &mut [f32]) -> usize {
        match self.eval(input, out) {
            Ok(()) => self.output_count(),
            Err(_) => 0,
        }
    }

    /// The per-type evaluation, after domain clamping. `false` means the
    /// function refused.
    fn eval_raw(&self, input: &[f32], out: &mut [f32]) -> bool {
        match self {
            Self::Sampled(f) => f.eval(input, out),
            Self::Exponential(f) => f.eval(input, out),
            Self::Stitching(f) => f.eval(input, out),
            Self::PostScript(f) => f.eval(input, out),
        }
    }
}

/// Linear interpolation with PDFium's degenerate-interval rule: a zero-width
/// input interval yields `ymin` rather than a division by zero.
#[must_use]
pub fn interpolate(x: f32, xmin: f32, xmax: f32, ymin: f32, ymax: f32) -> f32 {
    let divisor = xmax - xmin;
    if divisor == 0.0 {
        return ymin;
    }
    ymin + (x - xmin) * (ymax - ymin) / divisor
}

/// Session-scoped function memoization, keyed on the reference that named the
/// function.
#[derive(Debug, Default)]
pub struct FunctionCache {
    entries: HashMap<ObjRef, Option<Arc<Function>>>,
    /// References on the current load path, which is the cycle guard.
    in_flight: HashSet<ObjRef>,
}

impl FunctionCache {
    /// An empty cache.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// How many functions have been loaded through this cache.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether nothing has been loaded yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Load the function `obj` names, or `None` for every failure.
    #[must_use]
    pub fn load<R: Resolve>(
        &mut self,
        obj: &Object,
        r: &R,
        limits: &Limits,
        diags: &mut Diagnostics,
    ) -> Option<Arc<Function>> {
        let loaded = self.load_at(obj, r, limits, diags, 0);
        if loaded.is_none() {
            diags.record(Severity::Suspicious, DiagKind::FunctionUnsupported, None);
        }
        loaded
    }

    fn load_at<R: Resolve>(
        &mut self,
        obj: &Object,
        r: &R,
        limits: &Limits,
        diags: &mut Diagnostics,
        depth: u32,
    ) -> Option<Arc<Function>> {
        if depth > MAX_DEPTH {
            return None;
        }
        let reference = obj.as_ref_id();
        if let Some(id) = reference {
            if let Some(hit) = self.entries.get(&id) {
                return hit.clone();
            }
            // A cycle: the same function reachable through disjoint branches
            // is fine, but a true loop is not.
            if !self.in_flight.insert(id) {
                return None;
            }
        }
        let resolved = obj.resolve(r).ok();
        let built = resolved
            .as_deref()
            .and_then(|direct| self.build(direct, r, limits, diags, depth))
            .map(Arc::new);
        if let Some(id) = reference {
            self.in_flight.remove(&id);
            self.entries.insert(id, built.clone());
        }
        built
    }

    fn build<R: Resolve>(
        &mut self,
        obj: &Object,
        r: &R,
        limits: &Limits,
        diags: &mut Diagnostics,
        depth: u32,
    ) -> Option<Function> {
        let dict = match obj {
            Object::Dict(d) => d,
            Object::Stream(s) => &s.dict,
            _ => return None,
        };
        let kind = dict.int(names::FUNCTION_TYPE, r)?;
        let common = Common::load(dict, r)?;
        match kind {
            0 => obj
                .as_stream()
                .and_then(|s| Sampled::load(s, &common, r, limits, diags))
                .map(Function::Sampled),
            2 => Exponential::load(dict, &common, r).map(Function::Exponential),
            3 => Stitching::load(dict, &common, r, self, limits, diags, depth)
                .map(Function::Stitching),
            4 => obj
                .as_stream()
                .and_then(|s| PostScript::load(s, &common, r, limits, diags))
                .map(Function::PostScript),
            // 1 is not a typo: PDFium implements no type 1.
            _ => None,
        }
    }
}

/// The `/Domain` and `/Range` every type shares.
pub(crate) struct Common {
    pub(crate) domain: Box<[f32]>,
    pub(crate) range: Box<[f32]>,
}

impl Common {
    /// `/Domain` is required and must describe at least one input;
    /// `/Range` is optional here and checked per type.
    fn load(dict: &Dict, r: &impl Resolve) -> Option<Self> {
        let domain_array = dict.array(names::DOMAIN, r)?;
        let inputs = domain_array.len() / 2;
        if inputs == 0 {
            return None;
        }
        let domain = (0..inputs * 2)
            .map(|i| domain_array.number_at_or_zero(i))
            .collect();
        let range = match dict.array(names::RANGE, r) {
            Some(a) => {
                let outputs = a.len() / 2;
                if outputs > MAX_OUTPUTS {
                    return None;
                }
                (0..outputs * 2).map(|i| a.number_at_or_zero(i)).collect()
            }
            None => Box::default(),
        };
        Some(Self { domain, range })
    }

    pub(crate) fn inputs(&self) -> usize {
        self.domain.len() / 2
    }

    pub(crate) fn outputs(&self) -> usize {
        self.range.len() / 2
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

    use super::{Function, FunctionCache, interpolate};
    use pdfrum_common::{Diagnostics, Limits};
    use pdfrum_object::{Array, Dict, Name, NoResolve, Object};

    pub(super) fn nums(values: &[f32]) -> Object {
        Object::Array(Array::of(values.iter().copied().map(Object::Real)))
    }

    fn load(dict: Dict) -> Option<Function> {
        let mut cache = FunctionCache::new();
        let mut diags = Diagnostics::default();
        cache
            .load(
                &Object::Dict(dict),
                &NoResolve,
                &Limits::default(),
                &mut diags,
            )
            .map(|f| (*f).clone())
    }

    #[test]
    fn interpolation_survives_a_degenerate_interval() {
        assert!((interpolate(5.0, 0.0, 10.0, 0.0, 1.0) - 0.5).abs() < 1e-6);
        // A zero-width input interval yields ymin, not NaN.
        assert!((interpolate(5.0, 3.0, 3.0, 7.0, 9.0) - 7.0).abs() < 1e-6);
    }

    #[test]
    fn only_types_zero_two_three_and_four_load() {
        for kind in [-2i64, 1, 5, 100] {
            let dict = Dict::from_pairs([
                (Name::from("FunctionType"), Object::Int(kind)),
                (Name::from("Domain"), nums(&[0.0, 1.0])),
                (Name::from("N"), Object::Int(1)),
            ]);
            assert!(load(dict).is_none(), "type {kind} should not load");
        }
    }

    #[test]
    fn domain_is_required_and_must_describe_an_input() {
        // Missing entirely.
        let dict = Dict::from_pairs([
            (Name::from("FunctionType"), Object::Int(2)),
            (Name::from("N"), Object::Int(1)),
        ]);
        assert!(load(dict).is_none());

        // Empty, so zero inputs.
        let dict = Dict::from_pairs([
            (Name::from("FunctionType"), Object::Int(2)),
            (Name::from("Domain"), nums(&[])),
            (Name::from("N"), Object::Int(1)),
        ]);
        assert!(load(dict).is_none());

        // One element also truncates to zero inputs.
        let dict = Dict::from_pairs([
            (Name::from("FunctionType"), Object::Int(2)),
            (Name::from("Domain"), nums(&[0.0])),
            (Name::from("N"), Object::Int(1)),
        ]);
        assert!(load(dict).is_none());
    }

    #[test]
    fn arity_mismatches_are_errors_not_panics() {
        let dict = Dict::from_pairs([
            (Name::from("FunctionType"), Object::Int(2)),
            (Name::from("Domain"), nums(&[0.0, 1.0])),
            (Name::from("N"), Object::Int(1)),
        ]);
        let f = load(dict).expect("a type 2 function");
        let mut out = [0.0f32; 4];
        assert!(f.eval(&[0.5, 0.5], &mut out).is_err());
        assert_eq!(f.eval_into(&[0.5, 0.5], &mut out), 0);
        assert!(f.eval(&[0.5], &mut []).is_err());
    }
}
