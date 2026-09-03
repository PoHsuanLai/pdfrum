//! Applying a `/TR` transfer function to a resolved colour (ISO 32000-1
//! §10.4).
//!
//! The three 256-entry byte tables are sampled once at parse time by
//! `pdfrum-page`, already indexed by channel — 0 red, 1 green, 2 blue. What
//! belongs here is only the packing into an [`Argb`] and the identity
//! early-out; there is no channel remapping to do.
//!
//! # Q1, resolved: the `/TR` array reversal is real
//!
//! Q1 asked whether `pFuncs[2 - i] = Load(array[i])` is observable. It is:
//! `array[2]` drives red and `array[0]` drives blue. The oracle's own unit
//! test looks like it says otherwise only because
//! `kExpectedType0FunctionSamples` and `kExpectedType4FunctionSamples` are
//! misnamed — the former is the type 4 program's sine ramp and the latter the
//! type 0 function's flat one. `CPDFDocRenderDataTest.TransferFunctionArray`'s
//! ten `TranslateColor` pairs settle it without naming a function at all, and
//! `pdfrum-page::transfer` records the arithmetic.
//!
//! An earlier reading of that test concluded the opposite and this module
//! compensated by reading slot `2 - channel`, which cancelled a reversal the
//! parse had applied correctly and left rendered `/TR` arrays with their red
//! and blue channels swapped. Both halves are gone.

use crate::color::Argb;

/// A sampled transfer function, viewed as a colour translation.
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

    /// The three channels' sample tables, in channel order.
    ///
    /// Exposed for [`crate::imagecache::PixmapRequest`], which must put the
    /// function's *identity* in a cache key: a rendered image is a function of
    /// these bytes, and two draws under different `/TR` are different pixels.
    /// A borrow of the tables, digested by the caller — the key owns a `u64`,
    /// not three quarters of a kilobyte.
    #[must_use]
    pub fn samples(&self) -> &[[u8; pdfrum_page::CHANNEL_SAMPLES]; 3] {
        &self.inner.samples
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
            r: self.inner.apply(0, c.r),
            g: self.inner.apply(1, c.g),
            b: self.inner.apply(2, c.b),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use pdfrum_common::{Diagnostics, Limits};
    use pdfrum_object::{Array, ByteSpan, Dict, Name, NoResolve, Object, Stream};
    use pdfrum_page::FunctionCache;

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

    /// Wrap `dict` and `data` as a stream object.
    fn stream(dict: Dict, data: &[u8]) -> Object {
        let file: Arc<[u8]> = Arc::from(data);
        let span = ByteSpan::new(Arc::clone(&file), 0..file.len()).expect("in range");
        Object::Stream(Stream::new(dict, span))
    }

    /// The oracle fixture's type 0 function: a four-entry eight-bit ramp
    /// over `/Range [0 0.5]`, whose samples are the bytes of "1234\0".
    fn oracle_type0() -> Object {
        let dict = Dict::from_pairs([
            (Name::from("FunctionType"), Object::Int(0)),
            (Name::from("BitsPerSample"), Object::Int(8)),
            (Name::from("Domain"), nums(&[0.0, 1.0])),
            (Name::from("Range"), nums(&[0.0, 0.5])),
            (
                Name::from("Size"),
                Object::Array(Array::of([Object::Int(4)])),
            ),
        ]);
        stream(dict, b"1234\0")
    }

    /// The oracle fixture's type 2 function.
    fn oracle_type2() -> Object {
        Object::Dict(Dict::from_pairs([
            (Name::from("FunctionType"), Object::Int(2)),
            (Name::from("N"), Object::Int(1)),
            (Name::from("Domain"), nums(&[0.0, 1.0])),
            (Name::from("C0"), nums(&[0.1, 0.2, 0.8])),
            (Name::from("C1"), nums(&[0.05, 0.01, 0.4])),
        ]))
    }

    /// The oracle fixture's type 4 program: one sine period, halved.
    fn oracle_type4() -> Object {
        let dict = Dict::from_pairs([
            (Name::from("FunctionType"), Object::Int(4)),
            (Name::from("Domain"), nums(&[0.0, 1.0])),
            (Name::from("Range"), nums(&[-1.0, 1.0])),
        ]);
        stream(dict, b"{ 360 mul sin 2 div }")
    }

    /// Audit item **A9**, at the consuming end. This asserted the oracle's
    /// reversal; table 58 gives the array as `[red green blue gray]`, so
    /// element `i` drives channel `i`, and nothing here undoes it.
    #[test]
    fn the_first_array_element_drives_red() {
        let array = Object::Array(Array::of([
            constant(10.0 / 255.0),
            constant(100.0 / 255.0),
            constant(200.0 / 255.0),
        ]));
        let parsed = load(&array).expect("should load");
        let out = TransferFunc::new(&parsed).translate(Argb::opaque(10, 20, 30));
        assert_eq!(out.r, 10, "array[0] must drive red");
        assert_eq!(out.g, 100, "array[1] must drive green");
        assert_eq!(out.b, 200, "array[2] must drive blue");
    }

    #[test]
    fn the_oracle_fixtures_ten_translate_colour_pairs() {
        // CPDFDocRenderDataTest.TransferFunctionArray, verbatim: the same
        // three functions in the same order, and the same ten expectations.
        // FX_COLORREF packs as (b << 16) | (g << 8) | r, so each pair is
        // written here as (in_r, in_g, in_b) -> (out_r, out_g, out_b).
        let array = Object::Array(Array::of([oracle_type0(), oracle_type2(), oracle_type4()]));
        let parsed = load(&array).expect("the oracle's array should load");
        let tf = TransferFunc::new(&parsed);
        assert!(!tf.is_identity());

        // Audit items **A9** and **A10**, and this table is where both are
        // visible at once. The upstream expectations are on the left; ours,
        // on the right, differ in exactly two ways and no others:
        //
        // * **A9** swaps red and blue. Upstream `0x00ffffff -> 0x001a0d00`
        //   makes red `0x00` (the sine, `array[2]`) and blue `0x1a` (the type
        //   0 function, `array[0]`); table 58 puts the type 0 function in red.
        // * **A10** saturates instead of wrapping. Upstream's `0xcccccc` row
        //   pins `-121.26` arriving as `0x87` — `size_t o =
        //   FXSYS_roundf(output[0] * 255)` stored into a `uint8_t` with no
        //   clamp, which is also undefined behaviour for a negative float.
        //   Clipped to `/Range` per §7.10.1 and saturated, it is `0x00`.
        //
        // | input      | upstream   | ours       |
        // |------------|------------|------------|
        // | `ff ff ff` | `00 0d 1a` | `1a 0d 00` |
        // | `00 00 ff` | `00 1a 1a` | `19 1a 00` |
        // | `00 ff 00` | `00 0d 19` | `19 0d 00` |
        // | `ff 00 00` | `00 1a 19` | `1a 1a 00` |
        // | `cc cc cc` | `87 0f 1a` | `1a 0f 00` |
        // | `56 34 12` | `6d 17 19` | `19 17 37` |
        for (input, expected) in [
            ((0xff, 0xff, 0xff), (0x1a, 0x0d, 0x00)),
            ((0x00, 0x00, 0xff), (0x19, 0x1a, 0x00)),
            ((0x00, 0xff, 0x00), (0x19, 0x0d, 0x00)),
            ((0xff, 0x00, 0x00), (0x1a, 0x1a, 0x00)),
            ((0xcc, 0xcc, 0xcc), (0x1a, 0x0f, 0x00)),
            ((0x56, 0x34, 0x12), (0x19, 0x17, 0x37)),
        ] {
            let (r, g, b) = input;
            let out = TransferFunc::new(&parsed).translate(Argb::opaque(r, g, b));
            assert_eq!(
                (out.r, out.g, out.b),
                expected,
                "translating ({r:#04x}, {g:#04x}, {b:#04x})"
            );
        }
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
