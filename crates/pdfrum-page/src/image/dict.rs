//! Validating an image dictionary before any decoding
//! (ISO 32000-1 §8.9.5).
//!
//! # There is no "repair bad BPC to 8"
//!
//! This is the single most commonly assumed behaviour that PDFium does *not*
//! have. Only two coercions exist, and both are driven by the filter, not by
//! the value being wrong:
//!
//! | Last filter | Effect |
//! |---|---|
//! | `JPXDecode` | the bit-depth check is **skipped entirely**; even 0 is fine |
//! | `CCITTFaxDecode`, `JBIG2Decode` | forced to 1 bit, 1 component |
//! | `DCTDecode` | forced to 8 bits |
//! | anything else, including `RunLengthDecode` | untouched |
//!
//! After that, a bit depth outside `{1, 2, 4, 8, 16}` is a **hard failure**.
//! So `/BitsPerComponent 3` on a Flate image rejects the image; it does not
//! silently become 8. `RunLengthDecode` is deliberately excluded from the
//! coercion, with the C++ comment that too many documents do not conform.
//!
//! # `/ImageMask` swallows `/ColorSpace`
//!
//! A stencil mask, or an image with no `/ColorSpace` at all, is forced to one
//! bit and one component **regardless of `/BitsPerComponent`**, and any
//! `/ColorSpace` present is ignored outright. The one exception is a
//! `JPXDecode` image with no `/ColorSpace`, which is not a mask — its
//! colour space comes from the codestream.

use crate::error::Error;
use crate::names;
use pdfrum_common::{DiagKind, Diagnostics, Severity};
use pdfrum_filters::Filter;
use pdfrum_object::{Array, Dict, Object, Resolve, Resolved};

/// The largest `/Width` or `/Height` PDFium accepts, `0x01FFFF`.
pub const MAX_DIMENSION: i64 = 131_071;

/// Bit depths the sample unpacker can handle.
const ALLOWED_BPC: [i64; 5] = [1, 2, 4, 8, 16];

/// The facts a decoder needs, already validated, so no decoder re-reads the
/// dictionary.
#[derive(Debug, Clone, PartialEq)]
pub struct ImageDict {
    /// `/Width`, at least one and within [`MAX_DIMENSION`].
    pub width: u32,
    /// `/Height`, likewise.
    pub height: u32,
    /// Bits per component after the filter-driven coercions. Zero only for a
    /// `JPXDecode` image, whose depth the codestream supplies.
    pub bpc: u32,
    /// Components per sample after the coercions.
    pub components: u32,
    /// Whether this is a stencil mask painted with the fill colour.
    pub image_mask: bool,
    /// Whether `/Decode` was absent or states the identity mapping — which is
    /// what decides whether a one-bit mask is **inverted**.
    pub default_decode: bool,
    /// The `/Decode` array, verbatim.
    pub decode: Option<Array>,
    /// The last filter in the chain, which is the one that drives everything.
    pub last_filter: Option<Filter>,
    /// The name of the last filter, even when it is one we have no codec for.
    pub last_filter_name: Option<pdfrum_object::Name>,
    /// That filter's `/DecodeParms`.
    pub params: Dict,
}

impl ImageDict {
    /// Read and validate.
    ///
    /// # Errors
    ///
    /// [`Error::ImageBadDict`] when a dimension or the bit depth is outside
    /// what PDFium accepts.
    pub fn load<R: Resolve>(dict: &Dict, r: &R, diags: &mut Diagnostics) -> Result<Self, Error> {
        let width = dict.int(names::WIDTH, r).unwrap_or(0);
        let height = dict.int(names::HEIGHT, r).unwrap_or(0);
        if !is_valid_dimension(width) || !is_valid_dimension(height) {
            diags.record(Severity::Suspicious, DiagKind::ImageBadDimensions, None);
            return Err(Error::ImageBadDict {
                what: "width or height",
            });
        }
        let (last_filter, last_filter_name, params) = last_filter(dict, r);

        let bpc_orig = dict.int(names::BITS_PER_COMPONENT, r).unwrap_or(0);
        // The `[0, 16]` gate comes before any coercion and is a hard failure.
        if !(0..=16).contains(&bpc_orig) {
            diags.record(Severity::Suspicious, DiagKind::ImageBadBitDepth, None);
            return Err(Error::ImageBadDict {
                what: "bits per component",
            });
        }

        let declared_mask = dict.bool(names::IMAGE_MASK).unwrap_or(false);
        let has_colorspace = dict.raw(names::COLOR_SPACE).is_some();
        let decode = dict.array(names::DECODE, r);

        // The mask forcing, and its one JPX exception.
        if declared_mask || !has_colorspace {
            if !declared_mask && last_filter == Some(Filter::Jpx) {
                // Not a mask: the codestream carries the colour space, and
                // the bit depth check is skipped.
                return Ok(Self {
                    width: to_u32(width),
                    height: to_u32(height),
                    bpc: 0,
                    components: 0,
                    image_mask: false,
                    default_decode: default_decode(decode.as_ref()),
                    decode,
                    last_filter,
                    last_filter_name,
                    params,
                });
            }
            return Ok(Self {
                width: to_u32(width),
                height: to_u32(height),
                // One bit and one component regardless of what the dictionary
                // said, and any `/ColorSpace` is ignored entirely.
                bpc: 1,
                components: 1,
                image_mask: true,
                default_decode: default_decode(decode.as_ref()),
                decode,
                last_filter,
                last_filter_name,
                params,
            });
        }

        // JPX skips the bit-depth *check* — but not the assignment before it.
        // `ValidateDictParam` opens with `bpc_ = bpc_orig_;` and only then
        // returns early for `JPXDecode`, so the dictionary's declared depth
        // survives on this path and the `/Indexed` downshift below reads it.
        // (The no-colour-space JPX path above returns from `LoadColorInfo`
        // before `ValidateDictParam` ever runs, which is why *it* leaves the
        // depth at zero.)
        if last_filter == Some(Filter::Jpx) {
            return Ok(Self {
                width: to_u32(width),
                height: to_u32(height),
                bpc: to_u32(bpc_orig),
                components: 0,
                image_mask: false,
                default_decode: default_decode(decode.as_ref()),
                decode,
                last_filter,
                last_filter_name,
                params,
            });
        }

        // The two filter-driven coercions. Note `RunLengthDecode` is not
        // here: the C++ comment says too many documents do not conform.
        let (bpc, forced_components) = match last_filter {
            Some(Filter::CcittFax | Filter::Jbig2) => (1, Some(1)),
            Some(Filter::Dct) => (8, None),
            _ => (bpc_orig, None),
        };
        if !ALLOWED_BPC.contains(&bpc) {
            diags.record(Severity::Suspicious, DiagKind::ImageBadBitDepth, None);
            return Err(Error::ImageBadDict {
                what: "bits per component",
            });
        }

        Ok(Self {
            width: to_u32(width),
            height: to_u32(height),
            bpc: to_u32(bpc),
            // The real component count comes from the colorspace unless a
            // filter forced it; the caller fills it in.
            components: forced_components.unwrap_or(0),
            image_mask: false,
            default_decode: default_decode(decode.as_ref()),
            decode,
            last_filter,
            last_filter_name,
            params,
        })
    }

    /// Bytes per scanline, or `None` when the product overflows.
    #[must_use]
    pub fn pitch(&self) -> Option<usize> {
        let bits = u64::from(self.bpc)
            .checked_mul(u64::from(self.components))?
            .checked_mul(u64::from(self.width))?;
        usize::try_from(bits.checked_add(7)? / 8).ok()
    }

    /// Total sample bytes, or `None` when the product overflows a `u32` —
    /// which is PDFium's effective whole-image cap of just under four
    /// gibibytes.
    #[must_use]
    pub fn total_bytes(&self) -> Option<usize> {
        let total = u64::try_from(self.pitch()?)
            .ok()?
            .checked_mul(u64::from(self.height))?;
        if total > u64::from(u32::MAX) {
            return None;
        }
        usize::try_from(total).ok()
    }
}

/// `IsValidDimension`: strictly positive and within the cap.
#[must_use]
pub fn is_valid_dimension(v: i64) -> bool {
    v > 0 && v <= MAX_DIMENSION
}

/// Whether a bit depth is one the sample unpacker handles.
#[must_use]
pub fn is_allowed_bits_per_component(v: i64) -> bool {
    ALLOWED_BPC.contains(&v)
}

fn to_u32(v: i64) -> u32 {
    u32::try_from(v).unwrap_or(0)
}

/// Whether `/Decode` is absent or states the identity, which is what a
/// one-bit mask's inversion turns on.
fn default_decode(decode: Option<&Array>) -> bool {
    match decode {
        None => true,
        Some(a) => a.int_at(0) == Some(0),
    }
}

/// The last filter in the chain, its canonical name, and its parameters.
///
/// **Every level here resolves.** `GetDecoderArray` reaches `/Filter` through
/// `GetDirectObjectFor` and each element through `GetByteStringAt`, both of
/// which follow one level of indirection — so a `/Filter` array may name its
/// filters by reference, and `bug_1986` does: `[6 0 R /LZWDecode 7 0 R]`,
/// where object 7 is `/JPXDecode`. Reading those elements raw leaves the last
/// filter unrecognised, and an image whose colour space lives in its JPX
/// codestream is then forced to a one-bit stencil instead.
fn last_filter<R: Resolve>(
    dict: &Dict,
    r: &R,
) -> (Option<Filter>, Option<pdfrum_object::Name>, Dict) {
    let params_obj = dict.get(names::DECODE_PARMS, r);
    let params_obj = params_obj.as_ref().and_then(Resolved::as_direct);
    let filter = dict.get(names::FILTER, r);
    match filter.as_ref().and_then(Resolved::as_direct) {
        Some(Object::Name(n)) => (
            Filter::from_name(n),
            Some(n.clone()),
            match params_obj {
                Some(Object::Dict(d)) => d.clone(),
                _ => Dict::new(),
            },
        ),
        Some(Object::Array(a)) => {
            let last = a.len().checked_sub(1);
            let name = last
                .and_then(|i| a.get(i, r))
                .as_ref()
                .and_then(Resolved::as_direct)
                .and_then(Object::as_name)
                .cloned();
            let params = last
                .and_then(|i| {
                    params_obj
                        .and_then(Object::as_array)
                        .and_then(|p| p.dict_at(i, r))
                })
                .unwrap_or_default();
            (name.as_ref().and_then(Filter::from_name), name, params)
        }
        _ => (None, None, Dict::new()),
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

    use super::{ImageDict, MAX_DIMENSION, is_allowed_bits_per_component, is_valid_dimension};
    use pdfrum_common::Diagnostics;
    use pdfrum_object::{Array, Dict, Name, NoResolve, Object};

    fn image(pairs: Vec<(Name, Object)>) -> Result<ImageDict, crate::Error> {
        let mut diags = Diagnostics::default();
        ImageDict::load(&Dict::from_pairs(pairs), &NoResolve, &mut diags)
    }

    fn base(bpc: i64) -> Vec<(Name, Object)> {
        vec![
            (Name::from("Width"), Object::Int(4)),
            (Name::from("Height"), Object::Int(4)),
            (Name::from("BitsPerComponent"), Object::Int(bpc)),
            (
                Name::from("ColorSpace"),
                Object::Name(Name::from("DeviceGray")),
            ),
        ]
    }

    #[test]
    fn dimensions_are_bounded_at_both_ends() {
        assert!(is_valid_dimension(1));
        assert!(is_valid_dimension(MAX_DIMENSION));
        assert!(!is_valid_dimension(0));
        assert!(!is_valid_dimension(-1));
        assert!(!is_valid_dimension(MAX_DIMENSION + 1));

        let mut pairs = base(8);
        pairs[0] = (Name::from("Width"), Object::Int(0));
        assert!(image(pairs).is_err());
    }

    #[test]
    fn odd_bit_depths_are_rejected_not_repaired_to_eight() {
        // The claim this test exists to refute: there is no "corrupt BPC → 8".
        for bpc in [3i64, 5, 6, 7, 9, 15] {
            let got = image(base(bpc));
            assert!(got.is_err(), "bpc {bpc} should be rejected");
        }
        for bpc in [1i64, 2, 4, 8, 16] {
            assert!(is_allowed_bits_per_component(bpc));
            assert!(image(base(bpc)).is_ok(), "bpc {bpc} should load");
        }
    }

    #[test]
    fn a_bit_depth_outside_zero_to_sixteen_fails_the_gate() {
        assert!(image(base(17)).is_err());
        assert!(image(base(-1)).is_err());
        // Zero passes the gate but fails the allowed set, still an error.
        assert!(image(base(0)).is_err());
    }

    #[test]
    fn ccitt_and_jbig2_force_one_bit_one_component() {
        for filter in ["CCITTFaxDecode", "JBIG2Decode"] {
            let mut pairs = base(8);
            pairs.push((Name::from("Filter"), Object::Name(Name::from(filter))));
            let got = image(pairs).expect("should load");
            assert_eq!(got.bpc, 1, "{filter}");
            assert_eq!(got.components, 1, "{filter}");
        }
    }

    #[test]
    fn dct_forces_eight_bits() {
        let mut pairs = base(4);
        pairs.push((Name::from("Filter"), Object::Name(Name::from("DCTDecode"))));
        assert_eq!(image(pairs).expect("should load").bpc, 8);
    }

    #[test]
    fn run_length_is_deliberately_not_coerced() {
        let mut pairs = base(3);
        pairs.push((
            Name::from("Filter"),
            Object::Name(Name::from("RunLengthDecode")),
        ));
        // Still rejected: no coercion saves it.
        assert!(image(pairs).is_err());
    }

    /// A `/Filter` array may name its filters by **indirect reference**, and
    /// every level of the lookup resolves — `GetDecoderArray` reaches the
    /// array through `GetDirectObjectFor` and each element through
    /// `GetByteStringAt`.
    ///
    /// `bug_1986` is the file: `/Filter [6 0 R /LZWDecode 7 0 R]` with object
    /// 7 = `/JPXDecode`, and no `/ColorSpace` because the codestream carries
    /// it. Reading the last element raw leaves the filter unrecognised, and
    /// the missing colour space then forces the image to a one-bit stencil —
    /// a red page rendered as a black-on-white mask.
    #[test]
    fn a_filter_array_may_name_its_last_filter_by_reference() {
        /// Object 7 is `/JPXDecode`; nothing else resolves.
        struct FilterStore;
        impl pdfrum_object::Resolve for FilterStore {
            fn fetch(
                &self,
                r: pdfrum_object::ObjRef,
            ) -> Result<std::sync::Arc<Object>, pdfrum_object::Error> {
                match r.num {
                    6 => Ok(std::sync::Arc::new(Object::Name(Name::from(
                        "ASCIIHexDecode",
                    )))),
                    7 => Ok(std::sync::Arc::new(Object::Name(Name::from("JPXDecode")))),
                    _ => Err(pdfrum_object::Error::UnresolvedRef(r)),
                }
            }
        }

        let dict = Dict::from_pairs(vec![
            (Name::from("Width"), Object::Int(612)),
            (Name::from("Height"), Object::Int(792)),
            (
                Name::from("Filter"),
                Object::Array(Array::of([
                    Object::Ref(pdfrum_object::ObjRef::new(6, 0)),
                    Object::Name(Name::from("LZWDecode")),
                    Object::Ref(pdfrum_object::ObjRef::new(7, 0)),
                ])),
            ),
        ]);
        let mut diags = Diagnostics::default();
        let got = ImageDict::load(&dict, &FilterStore, &mut diags).expect("should load");
        assert_eq!(got.last_filter, Some(pdfrum_filters::Filter::Jpx));
        assert!(
            !got.image_mask,
            "a JPX image with no /ColorSpace is not a stencil: the codestream \
             carries the space"
        );
        assert_eq!(got.bpc, 0, "the JPX no-colour-space path leaves it at zero");
    }

    #[test]
    fn jpx_skips_the_bit_depth_check_entirely() {
        let mut pairs = base(0);
        pairs.push((Name::from("Filter"), Object::Name(Name::from("JPXDecode"))));
        let got = image(pairs).expect("a JPX image with bpc 0 should load");
        assert_eq!(got.bpc, 0);
        assert!(!got.image_mask);
    }

    #[test]
    fn jpx_skips_the_check_but_keeps_the_declared_bit_depth() {
        // `ValidateDictParam` assigns `bpc_ = bpc_orig_` *before* it returns
        // early for `JPXDecode`, so a depth the check would have rejected is
        // still the depth the `/Indexed` downshift reads. Two and four are
        // both legal and both below eight, which is the case that matters.
        for depth in [1i64, 2, 4, 8, 16] {
            let mut pairs = base(depth);
            pairs.push((Name::from("Filter"), Object::Name(Name::from("JPXDecode"))));
            let got = image(pairs).expect("a JPX image should load at any depth");
            assert_eq!(got.bpc, u32::try_from(depth).expect("small"), "bpc {depth}");
        }
        // A depth the *check* would reject still survives, because the check
        // never runs on this path.
        let mut pairs = base(7);
        pairs.push((Name::from("Filter"), Object::Name(Name::from("JPXDecode"))));
        assert_eq!(image(pairs).expect("should load").bpc, 7);
    }

    #[test]
    fn a_jpx_image_with_no_colorspace_keeps_a_zero_depth() {
        // This path returns from `LoadColorInfo` before `ValidateDictParam`
        // ever runs, so the assignment above never happens here.
        let pairs = vec![
            (Name::from("Width"), Object::Int(4)),
            (Name::from("Height"), Object::Int(4)),
            (Name::from("BitsPerComponent"), Object::Int(8)),
            (Name::from("Filter"), Object::Name(Name::from("JPXDecode"))),
        ];
        let got = image(pairs).expect("should load");
        assert_eq!(got.bpc, 0);
        assert!(!got.image_mask);
    }

    #[test]
    fn an_image_mask_ignores_its_colorspace_and_bit_depth() {
        let mut pairs = base(8);
        pairs.push((Name::from("ImageMask"), Object::Bool(true)));
        let got = image(pairs).expect("should load");
        assert!(got.image_mask);
        assert_eq!(got.bpc, 1);
        assert_eq!(got.components, 1);
    }

    #[test]
    fn a_missing_colorspace_makes_a_mask_unless_the_filter_is_jpx() {
        let pairs = vec![
            (Name::from("Width"), Object::Int(4)),
            (Name::from("Height"), Object::Int(4)),
            (Name::from("BitsPerComponent"), Object::Int(8)),
        ];
        assert!(image(pairs).expect("should load").image_mask);

        let pairs = vec![
            (Name::from("Width"), Object::Int(4)),
            (Name::from("Height"), Object::Int(4)),
            (Name::from("Filter"), Object::Name(Name::from("JPXDecode"))),
        ];
        assert!(!image(pairs).expect("should load").image_mask);
    }

    #[test]
    fn default_decode_tracks_whether_the_array_states_the_identity() {
        let mut pairs = base(1);
        pairs.push((Name::from("ImageMask"), Object::Bool(true)));
        assert!(image(pairs.clone()).expect("should load").default_decode);

        let mut with_identity = pairs.clone();
        with_identity.push((
            Name::from("Decode"),
            Object::Array(Array::of([Object::Int(0), Object::Int(1)])),
        ));
        assert!(image(with_identity).expect("should load").default_decode);

        let mut inverted = pairs;
        inverted.push((
            Name::from("Decode"),
            Object::Array(Array::of([Object::Int(1), Object::Int(0)])),
        ));
        assert!(!image(inverted).expect("should load").default_decode);
    }

    #[test]
    fn the_last_filter_of_a_chain_is_the_one_that_counts() {
        let mut pairs = base(4);
        pairs.push((
            Name::from("Filter"),
            Object::Array(Array::of([
                Object::Name(Name::from("ASCII85Decode")),
                Object::Name(Name::from("DCTDecode")),
            ])),
        ));
        // The DCT at the end forces eight bits.
        assert_eq!(image(pairs).expect("should load").bpc, 8);
    }

    #[test]
    fn the_whole_image_size_cap_is_just_under_four_gibibytes() {
        let d = ImageDict {
            width: 131_071,
            height: 131_071,
            bpc: 8,
            components: 4,
            image_mask: false,
            default_decode: true,
            decode: None,
            last_filter: None,
            last_filter_name: None,
            params: Dict::new(),
        };
        // 131071 * 4 * 131071 is well past `u32::MAX`.
        assert!(d.total_bytes().is_none());
        let small = ImageDict {
            width: 100,
            height: 100,
            ..d
        };
        assert_eq!(small.total_bytes(), Some(40_000));
        assert_eq!(small.pitch(), Some(400));
    }
}
