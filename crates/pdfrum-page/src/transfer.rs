//! Transfer functions: `/TR` and `/TR2` (ISO 32000-1 §10.4).
//!
//! Sampled once at load into three 256-entry byte tables, one per channel.
//! Three rules govern which functions get sampled:
//!
//! - **`/TR` is skipped entirely when `/TR2` is also present** in the same
//!   dictionary; it is not merged or overridden, it is simply not read.
//! - **A `/TR2` that is a Name stores nothing** — so `/TR2 /Identity` and
//!   `/TR2 /Default` alike disable any transfer function rather than
//!   installing one.
//! - The array form needs **at least three elements**, and **element `i`
//!   drives channel `2 - i`**: the last array element is the red channel and
//!   the first is blue. This is not a storage convenience — it is the
//!   observable, and it survives to the rendered pixel. Any element failing
//!   to load makes the whole transfer function null.
//!
//! # The array reversal is real
//!
//! It is easy to conclude the opposite from the oracle's own unit test.
//! `CPDFDocRenderDataTest.TransferFunctionArray` builds the array
//! `[Type0, Type2, Type4]` and asserts `GetSamplesR() ==
//! kExpectedType0FunctionSamples` — which reads as "element 0 drives red".
//! **The constants are misnamed.** `kExpectedType0FunctionSamples` begins
//! `0, 3, 6, 9, 13, 16, …` and ends `…, 250, 253, 0`, which is the *type 4*
//! program `{ 360 mul sin 2 div }` sampled at `v / 255` — a full sine period,
//! verified to match all 256 entries exactly. `kExpectedType4FunctionSamples`
//! is flat at 25/26, which is the *type 0* sampled function's ramp over its
//! `/Range [0 0.5]`. Only `kExpectedType2FunctionSamples` is named for the
//! function it actually holds.
//!
//! The test's ten `TranslateColor` pairs settle it independently, without
//! having to name any function: `TranslateColor(0x00FFFFFF)` yields
//! `0x001A0D00`, and `FX_COLORREF` packs as `(b << 16) | (g << 8) | r`, so
//! `samples_r[255] == 0`, `samples_g[255] == 13`, `samples_b[255] == 26`.
//! Zero is the type 4 program's last sample, 13 the type 2 function's, and 26
//! the type 0 function's — i.e. **`array[2]` is red and `array[0]` is blue**,
//! exactly what `pFuncs[2 - i] = Load(array[i])` reads like literally.

use crate::function::{Function, FunctionCache};
use pdfrum_common::{Diagnostics, Limits};
use pdfrum_object::{Object, Resolve};

/// Entries per channel.
pub const CHANNEL_SAMPLES: usize = 256;

/// The most outputs a function feeding a transfer function may have.
///
/// A function above this is **skipped and the identity used for that
/// channel**. PDFium instead reuses the previous iteration's stale output,
/// producing a garbage ramp; that is a plain bug and is not ported
/// (design brief D7).
pub const MAX_OUTPUTS: usize = 16;

/// Three 256-entry byte tables, one per channel.
#[derive(Debug, Clone, PartialEq)]
pub struct TransferFunc {
    /// The samples, indexed `[channel][input]` with channel 0 red, 1 green
    /// and 2 blue.
    ///
    /// A consumer indexes by channel and nothing else: the array form's
    /// reversal is already applied here, so channel 0 holds `/TR`'s *last*
    /// element.
    pub samples: Box<[[u8; CHANNEL_SAMPLES]; 3]>,
    /// Whether every entry is its own index, in which case the function is a
    /// no-op and a renderer may skip it entirely.
    pub identity: bool,
}

impl TransferFunc {
    /// Sample a `/TR` or `/TR2` object.
    ///
    /// Returns `None` for every shape PDFium refuses: a name, a
    /// three-or-more-element array with a bad entry, or an object that is
    /// neither a function nor an array of them.
    #[must_use]
    pub fn load<R: Resolve>(
        obj: &Object,
        r: &R,
        cache: &mut FunctionCache,
        limits: &Limits,
        diags: &mut Diagnostics,
    ) -> Option<Self> {
        let resolved = obj.resolve(r).ok()?;
        // A name — `/Identity`, `/Default`, anything — stores nothing.
        if resolved.as_name().is_some() {
            return None;
        }
        let mut samples = Box::new([[0u8; CHANNEL_SAMPLES]; 3]);
        if let Some(array) = resolved.as_array() {
            // The array form needs three elements, and is reversed: element
            // 0 drives blue, element 2 drives red.
            if array.len() < 3 {
                return None;
            }
            for i in 0..3 {
                let element = array.raw_at(i)?;
                let func = cache.load(element, r, limits, diags)?;
                let channel = samples.get_mut(2 - i)?;
                sample_channel(&func, channel);
            }
        } else {
            let func = cache.load(&resolved, r, limits, diags)?;
            let mut one = [0u8; CHANNEL_SAMPLES];
            sample_channel(&func, &mut one);
            for channel in samples.iter_mut() {
                *channel = one;
            }
        }
        let identity = samples.iter().all(|channel| {
            channel
                .iter()
                .enumerate()
                .all(|(i, v)| usize::from(*v) == i)
        });
        Some(Self { samples, identity })
    }

    /// Apply the function to one colour byte on `channel` — 0 red, 1 green,
    /// 2 blue.
    #[must_use]
    pub fn apply(&self, channel: usize, value: u8) -> u8 {
        self.samples
            .get(channel.min(2))
            .and_then(|c| c.get(usize::from(value)))
            .copied()
            .unwrap_or(value)
    }
}

/// Sample one channel: 256 inputs from `i / 255`, rounded and **wrapped**
/// into a byte.
///
/// The wrap is deliberate and observable. PDFium rounds `output[0] * 255`
/// into a `size_t` and stores it into a `uint8_t` with no clamp, so a
/// function whose `/Range` admits negatives — the oracle's own type 4
/// fixture, `{ 360 mul sin 2 div }` over `[-1 1]`, is one — folds its
/// negative half back to the top of the byte range rather than to zero.
/// `CPDFDocRenderDataTest.TransferFunctionArray` pins two such samples:
/// `-121.26` becomes `0x87`, not `0x00`. Clamping instead would quietly
/// flatten every signed transfer function's lower half to black.
///
/// A function with too many outputs is skipped and the identity used, which
/// is where we deliberately diverge from the C++'s stale-output bug.
fn sample_channel(func: &Function, out: &mut [u8; CHANNEL_SAMPLES]) {
    if func.output_count() > MAX_OUTPUTS {
        for (i, slot) in out.iter_mut().enumerate() {
            *slot = u8::try_from(i).unwrap_or(u8::MAX);
        }
        return;
    }
    let mut results = vec![0.0f32; func.output_count().max(1)];
    for (i, slot) in out.iter_mut().enumerate() {
        #[expect(
            clippy::cast_precision_loss,
            reason = "an index below 256 is exact in f32"
        )]
        let input = (i as f32) / 255.0;
        let identity = u8::try_from(i).unwrap_or(u8::MAX);
        if func.eval_into(&[input], &mut results) == 0 {
            *slot = identity;
            continue;
        }
        let value = results.first().copied().unwrap_or(0.0);
        *slot = wrap_to_byte(value * 255.0);
    }
}

/// Round to the nearest integer and keep the low eight bits.
///
/// This is the arithmetic PDFium's `size_t o = FXSYS_roundf(x); u8 = o`
/// performs — a wrap, not a clamp, so a sample of `-121.26` lands on `135`.
/// Rust's `as u8` saturates instead, so the two-step cast is written out.
/// A non-finite sample has no defined wrap and becomes zero.
fn wrap_to_byte(value: f32) -> u8 {
    let rounded = value.round();
    if !rounded.is_finite() {
        return 0;
    }
    // `rem_euclid` folds any magnitude into `0.0..256.0` the way the two
    // C++ casts do for the values a function can actually produce.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "rem_euclid bounds the value to 0.0..256.0"
    )]
    let wrapped = rounded.rem_euclid(256.0) as u8;
    wrapped
}

/// Which of `/TR` and `/TR2` a dictionary's transfer function comes from.
///
/// `/TR2` wins outright: when it is present, `/TR` is never read.
#[must_use]
pub fn transfer_key(has_tr2: bool) -> &'static pdfrum_object::Name {
    if has_tr2 {
        crate::names::TR2
    } else {
        crate::names::TR
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

    use super::{CHANNEL_SAMPLES, TransferFunc, transfer_key};
    use crate::function::FunctionCache;
    use pdfrum_common::{Diagnostics, Limits};
    use pdfrum_object::{Array, Dict, Name, NoResolve, Object};

    fn nums(values: &[f32]) -> Object {
        Object::Array(Array::of(values.iter().copied().map(Object::Real)))
    }

    /// A type 2 function mapping `t` to `1 - t`.
    fn invert() -> Object {
        Object::Dict(Dict::from_pairs([
            (Name::from("FunctionType"), Object::Int(2)),
            (Name::from("Domain"), nums(&[0.0, 1.0])),
            (Name::from("N"), Object::Int(1)),
            (Name::from("C0"), nums(&[1.0])),
            (Name::from("C1"), nums(&[0.0])),
        ]))
    }

    fn load(obj: &Object) -> Option<TransferFunc> {
        let mut cache = FunctionCache::new();
        let mut diags = Diagnostics::default();
        TransferFunc::load(obj, &NoResolve, &mut cache, &Limits::default(), &mut diags)
    }

    #[test]
    fn a_single_function_applies_to_every_channel() {
        let tr = load(&invert()).expect("should load");
        assert_eq!(tr.apply(0, 0), 255);
        assert_eq!(tr.apply(1, 255), 0);
        assert_eq!(tr.apply(2, 128), 127);
        assert!(!tr.identity);
    }

    #[test]
    fn a_name_stores_nothing() {
        for name in ["Identity", "Default", "Anything"] {
            assert!(
                load(&Object::Name(Name::from(name))).is_none(),
                "/{name} should disable the transfer function"
            );
        }
    }

    #[test]
    fn tr2_wins_over_tr_outright() {
        assert_eq!(transfer_key(true).as_bytes(), b"TR2");
        assert_eq!(transfer_key(false).as_bytes(), b"TR");
    }

    #[test]
    fn the_array_form_needs_three_elements() {
        let two = Object::Array(Array::of([invert(), invert()]));
        assert!(load(&two).is_none());
        let three = Object::Array(Array::of([invert(), invert(), invert()]));
        assert!(load(&three).is_some());
    }

    #[test]
    fn a_bad_element_makes_the_whole_function_null() {
        let bad = Object::Array(Array::of([invert(), Object::Int(7), invert()]));
        assert!(load(&bad).is_none());
    }

    /// A type 2 function that is constant at `v` for every input.
    fn constant(v: f32) -> Object {
        Object::Dict(Dict::from_pairs([
            (Name::from("FunctionType"), Object::Int(2)),
            (Name::from("Domain"), nums(&[0.0, 1.0])),
            (Name::from("N"), Object::Int(1)),
            (Name::from("C0"), nums(&[v])),
            (Name::from("C1"), nums(&[v])),
        ]))
    }

    #[test]
    fn the_last_array_element_drives_red() {
        // Three constants no two of which collide, so the mapping of array
        // position to channel reads straight off the output bytes.
        let array = Object::Array(Array::of([
            constant(10.0 / 255.0),
            constant(100.0 / 255.0),
            constant(200.0 / 255.0),
        ]));
        let tr = load(&array).expect("should load");
        // Read through the public accessor, not the raw slots: this is the
        // observable the render crate consumes.
        assert_eq!(tr.apply(0, 0), 200, "array[2] must drive red");
        assert_eq!(tr.apply(1, 0), 100, "array[1] must drive green");
        assert_eq!(tr.apply(2, 0), 10, "array[0] must drive blue");
    }

    #[test]
    fn a_reversed_array_survives_to_the_output_bytes() {
        // Red inverts, green and blue are the identity — but the inverting
        // function is in the *last* array slot, because that is the one the
        // oracle reads as red.
        let identity = constant_ramp();
        let array = Object::Array(Array::of([identity.clone(), identity, invert()]));
        let tr = load(&array).expect("should load");
        assert!(!tr.identity);
        assert_eq!(tr.apply(0, 0), 255, "red inverts");
        assert_eq!(tr.apply(1, 0), 0, "green is the identity");
        assert_eq!(tr.apply(2, 0), 0, "blue is the identity");
    }

    /// A type 2 function mapping `t` to `t`.
    fn constant_ramp() -> Object {
        Object::Dict(Dict::from_pairs([
            (Name::from("FunctionType"), Object::Int(2)),
            (Name::from("Domain"), nums(&[0.0, 1.0])),
            (Name::from("N"), Object::Int(1)),
            (Name::from("C0"), nums(&[0.0])),
            (Name::from("C1"), nums(&[1.0])),
        ]))
    }

    #[test]
    fn an_identity_function_is_recognised_as_a_no_op() {
        let tr = load(&constant_ramp()).expect("should load");
        assert!(tr.identity);
        assert_eq!(tr.samples[0].len(), CHANNEL_SAMPLES);
    }
}
