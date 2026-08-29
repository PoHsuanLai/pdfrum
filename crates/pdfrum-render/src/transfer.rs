//! Applying a `/TR` transfer function to a resolved colour (ISO 32000-1
//! §10.4, `cpdf_transferfunc.cpp:34-38`).
//!
//! The three 256-entry byte tables are sampled once at parse time by
//! `pdfrum-page`; what belongs here is only the channel mapping, which is the
//! subject of the render brief's **Q1** and is settled by the oracle's own
//! unit test rather than by reading the code.
//!
//! # Q1, resolved
//!
//! `CPDF_DocRenderData::CreateTransferFunc` loads the array as
//! `pFuncs[2 - i] = Load(array[i])` and then fills `samples[i]` from
//! `pFuncs[i]`, where `samples` is `{samples_r, samples_g, samples_b}` — read
//! literally, that maps `array[2]` to red. But
//! `CPDFDocRenderDataTest.TransferFunctionArray` asserts, for the array
//! `[Type0, Type2, Type4]`, that `GetSamplesR() == Type0`, and its ten
//! `TranslateColor` expectations agree: `TranslateColor(0x00FFFFFF)` yields
//! `0x001A0D00`, i.e. `samples_r[255] == 0` (Type0's last entry),
//! `samples_g[255] == 13` (Type2's) and `samples_b[255] == 26` (Type4's).
//! **The observable is that array order maps directly to R, G, B.**
//! SPEC §8 rules for the asserted observable, and that is what this module
//! implements — `pdfrum-page`'s storage is reversed relative to it, so the
//! mapping is undone here rather than in the parse.

use crate::color::Argb;

/// A sampled transfer function, with the channel mapping the oracle's
/// observable requires.
#[derive(Debug, Clone)]
pub struct TransferFunc<'a> {
    inner: &'a pdfrum_page::TransferFunc,
}

impl<'a> TransferFunc<'a> {
    /// Wrap a parsed transfer function.
    #[must_use]
    pub fn new(inner: &'a pdfrum_page::TransferFunc) -> Self {
        Self { inner }
    }

    /// Whether every entry is its own index, in which case the whole function
    /// may be skipped — the same early-out the C++ takes on `GetIdentity()`.
    #[must_use]
    pub fn is_identity(&self) -> bool {
        self.inner.identity
    }

    /// Map one colour through the function.
    ///
    /// Alpha is untouched: the transfer function applies to the colour only,
    /// and is applied *before* any grayscale or forced-colour translation, so
    /// a `kGray` render grays the already-transferred colour.
    #[must_use]
    pub fn translate(&self, c: Argb) -> Argb {
        Argb {
            a: c.a,
            r: self.channel(0, c.r),
            g: self.channel(1, c.g),
            b: self.channel(2, c.b),
        }
    }

    /// Look up one byte on one channel, undoing the parse's array reversal.
    fn channel(&self, channel: usize, value: u8) -> u8 {
        // `pdfrum-page` stores `array[i]` at internal slot `2 - i`; the
        // oracle's observable is `array[i]` on channel `i`. Both statements
        // are about the same three tables, so reading slot `2 - channel`
        // here restores the asserted mapping.
        self.inner
            .samples
            .get(2 - channel.min(2))
            .and_then(|c| c.get(usize::from(value)))
            .copied()
            .unwrap_or(value)
    }
}

#[cfg(test)]
mod tests {
    use pdfrum_common::{Diagnostics, Limits};
    use pdfrum_object::{Array, Dict, Name, NoResolve, Object};
    use pdfrum_page::function::FunctionCache;

    use super::*;

    fn nums(values: &[f32]) -> Object {
        Object::Array(Array::of(values.iter().copied().map(Object::Real)))
    }

    /// A type 2 function mapping `t` to a constant.
    fn constant(v: f32) -> Object {
        Object::Dict(Dict::from_pairs([
            (Name::from("FunctionType"), Object::Int(2)),
            (Name::from("Domain"), nums(&[0.0, 1.0])),
            (Name::from("N"), Object::Int(1)),
            (Name::from("C0"), nums(&[v])),
            (Name::from("C1"), nums(&[v])),
        ]))
    }

    fn load(obj: &Object) -> Option<pdfrum_page::TransferFunc> {
        let mut cache = FunctionCache::new();
        let mut diags = Diagnostics::default();
        pdfrum_page::TransferFunc::load(obj, &NoResolve, &mut cache, &Limits::default(), &mut diags)
    }

    #[test]
    fn array_order_maps_directly_to_rgb() {
        // The Q1 authority: CPDFDocRenderDataTest.TransferFunctionArray with
        // [f0, f1, f2] asserts GetSamplesR() == f0. Three distinguishable
        // constants make the mapping unambiguous.
        let array = Object::Array(Array::of([
            constant(0.0),           // -> 0
            constant(100.0 / 255.0), // -> 100
            constant(200.0 / 255.0), // -> 200
        ]));
        let parsed = load(&array).expect("should load");
        let tf = TransferFunc::new(&parsed);
        let out = tf.translate(Argb::opaque(10, 20, 30));
        assert_eq!(out.r, 0, "array[0] must drive red");
        assert_eq!(out.g, 100, "array[1] must drive green");
        assert_eq!(out.b, 200, "array[2] must drive blue");
    }

    #[test]
    fn alpha_survives_the_transfer() {
        let parsed = load(&constant(0.0)).expect("should load");
        let tf = TransferFunc::new(&parsed);
        let out = tf.translate(Argb {
            a: 77,
            r: 255,
            g: 255,
            b: 255,
        });
        assert_eq!(out.a, 77);
        assert_eq!((out.r, out.g, out.b), (0, 0, 0));
    }

    #[test]
    fn identity_is_detected() {
        let identity = Object::Dict(Dict::from_pairs([
            (Name::from("FunctionType"), Object::Int(2)),
            (Name::from("Domain"), nums(&[0.0, 1.0])),
            (Name::from("N"), Object::Int(1)),
            (Name::from("C0"), nums(&[0.0])),
            (Name::from("C1"), nums(&[1.0])),
        ]));
        let parsed = load(&identity).expect("should load");
        assert!(TransferFunc::new(&parsed).is_identity());
    }

    #[test]
    fn bad_transfer_functions_drop_the_whole_thing() {
        // Ports CPDFDocRenderDataTest.BadTransferFunctions: an array shorter
        // than three, and an array with an unloadable element, both yield
        // nothing rather than a partial function.
        assert!(load(&Object::Array(Array::of([constant(0.0), constant(0.0)]))).is_none());
        assert!(
            load(&Object::Array(Array::of([
                constant(0.0),
                Object::Int(7),
                constant(0.0)
            ])))
            .is_none()
        );
        assert!(load(&Object::Name(Name::from("Identity"))).is_none());
    }
}
