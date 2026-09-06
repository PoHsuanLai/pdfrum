//! Colour spaces and the values that live in them (ISO 32000-1 §8.6).
//!
//! One enum for all eleven families, with the per-family maths in a module
//! each. Two things about this design are load-bearing:
//!
//! - **There are two conversion paths, and they disagree.** [`to_rgb`] is the
//!   scalar path a vector fill takes; [`translate_image_line`] is the bulk
//!   path an image takes. For most families they agree modulo byte order, but
//!   `CalRGB` ignores its gamma, matrix and white point in bulk, and `Lab`
//!   scales its inputs differently. Both halves are ported because the oracle
//!   renders both.
//! - **`Pattern` has no `to_rgb`.** The C++ makes it an unreachable
//!   assertion; here the case is simply not representable, and a pattern's
//!   colour comes from [`PatternValue`] instead (design brief D10).
//!
//! [`to_rgb`]: ColorSpace::to_rgb
//! [`translate_image_line`]: ColorSpace::translate_image_line

mod cie;
mod cmyk_table;
mod device;
mod icc;
mod indexed;
mod load;
mod special;
mod srgb_table;
mod value;

pub(crate) use cie::{CalGray, CalRgb, Lab};
/// The Adobe CMYK -> sRGB table lookup, byte in and byte out.
///
/// Re-exported because the image path needs it without the float wrapper
/// around it: `Pixels::sample_bytes` reaches it directly, and the two spellings
/// are proved equal exhaustively rather than assumed.
pub use device::adobe_cmyk_to_srgb;
pub(crate) use icc::{IccBased, IccProfile, is_valid_icc_components};
pub use icc::{cmyk_profile_bytes, srgb_profile_bytes};
pub(crate) use indexed::Indexed;
pub(crate) use load::ColorSpaceCache;
pub use load::load_colorspace;
pub(crate) use special::{DeviceN, MAX_PATTERN_COMPONENTS};
pub use special::{PatternSpace, Separation};
pub use value::{ColorValue, PatternValue, SetComponentsError};

/// A colour in the device's RGB space, each channel nominally in `0..=1`.
///
/// Not clamped on construction: several conversion paths deliberately return
/// out-of-range values (`CalGray` passes its input through untouched) and the
/// consumer clamps where PDFium clamps.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rgb {
    /// Red.
    pub r: f32,
    /// Green.
    pub g: f32,
    /// Blue.
    pub b: f32,
}

impl Rgb {
    /// Opaque black, the colour a broken space paints.
    pub const BLACK: Self = Self {
        r: 0.0,
        g: 0.0,
        b: 0.0,
    };

    /// The eight-bit encoding used for colour *references*: clamp, then round
    /// to nearest.
    ///
    /// Contrast [`Self::to_bytes_truncating`], which the bulk image path
    /// uses. The two differ by one on roughly half of all inputs, so calling
    /// the wrong one is a visible bug.
    #[must_use]
    pub fn to_bytes(self) -> [u8; 3] {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "the clamp bounds the product to 0..=255"
        )]
        let enc = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        [enc(self.r), enc(self.g), enc(self.b)]
    }

    /// The eight-bit encoding the image scanline path uses: clamp, then
    /// truncate.
    #[must_use]
    pub fn to_bytes_truncating(self) -> [u8; 3] {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "the clamp bounds the product to 0..=255"
        )]
        let enc = |v: f32| (v.clamp(0.0, 1.0) * 255.0) as u8;
        [enc(self.r), enc(self.g), enc(self.b)]
    }
}

/// The eleven colour space families, with the integer tags PDFium exposes
/// through its public API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Family {
    /// A space that failed to load.
    Unknown = 0,
    /// `DeviceGray`.
    DeviceGray = 1,
    /// `DeviceRGB`.
    DeviceRgb = 2,
    /// `DeviceCMYK`.
    DeviceCmyk = 3,
    /// `CalGray`.
    CalGray = 4,
    /// `CalRGB`.
    CalRgb = 5,
    /// `Lab`.
    Lab = 6,
    /// `ICCBased`.
    IccBased = 7,
    /// `Separation`.
    Separation = 8,
    /// `DeviceN`.
    DeviceN = 9,
    /// `Indexed`.
    Indexed = 10,
    /// `Pattern`.
    Pattern = 11,
}

/// A loaded colour space.
///
/// Cheap to clone: the heavy members — ICC profiles, tint transforms — are
/// behind `Arc`, and palettes are boxed slices.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum ColorSpace {
    /// One component: a grey level.
    DeviceGray,
    /// Three components: red, green, blue.
    DeviceRgb,
    /// Four components: cyan, magenta, yellow, black.
    DeviceCmyk,
    /// A calibrated grey space whose calibration is parsed and then ignored.
    CalGray(Box<CalGray>),
    /// A calibrated RGB space.
    CalRgb(Box<CalRgb>),
    /// A CIE 1976 L\*a\*b\* space.
    Lab(Box<Lab>),
    /// A space defined by an embedded ICC profile.
    IccBased(Box<IccBased>),
    /// A palette over some base space.
    Indexed(Box<Indexed>),
    /// One colorant driving an alternate space through a tint transform.
    Separation(Box<Separation>),
    /// Several colorants driving an alternate space.
    DeviceN(Box<DeviceN>),
    /// Colour supplied by a pattern rather than by components.
    Pattern(Box<PatternSpace>),
}

impl ColorSpace {
    /// Which family this is.
    #[must_use]
    pub fn family(&self) -> Family {
        match self {
            Self::DeviceGray => Family::DeviceGray,
            Self::DeviceRgb => Family::DeviceRgb,
            Self::DeviceCmyk => Family::DeviceCmyk,
            Self::CalGray(_) => Family::CalGray,
            Self::CalRgb(_) => Family::CalRgb,
            Self::Lab(_) => Family::Lab,
            Self::IccBased(_) => Family::IccBased,
            Self::Indexed(_) => Family::Indexed,
            Self::Separation(_) => Family::Separation,
            Self::DeviceN(_) => Family::DeviceN,
            Self::Pattern(_) => Family::Pattern,
        }
    }

    /// How many components a colour in this space carries.
    ///
    /// `Separation` and `Indexed` always report **1**; `ICCBased` reports its
    /// `/N` rather than whatever the profile says; `DeviceN` reports its
    /// colorant count with no cap.
    #[must_use]
    pub fn n_components(&self) -> usize {
        match self {
            Self::DeviceGray | Self::CalGray(_) | Self::Indexed(_) | Self::Separation(_) => 1,
            Self::DeviceRgb | Self::CalRgb(_) | Self::Lab(_) => 3,
            Self::DeviceCmyk => 4,
            Self::IccBased(icc) => usize::from(icc.n),
            Self::DeviceN(cs) => cs.names.len(),
            Self::Pattern(cs) => cs.n_components(),
        }
    }

    /// Whether the space's components are indices or tints rather than
    /// colour: `Separation`, `DeviceN`, `Indexed`, `Pattern`.
    ///
    /// Several validation paths refuse a special space where a colour is
    /// required — a shading's alternate, an `ICCBased`'s `/Alternate`.
    #[must_use]
    pub fn is_special(&self) -> bool {
        matches!(
            self,
            Self::Separation(_) | Self::DeviceN(_) | Self::Indexed(_) | Self::Pattern(_)
        )
    }

    /// Whether an image's samples in this space have to be **run through the
    /// space** before they are colour.
    ///
    /// Every family could reach a colour through the scalar conversion, but
    /// most have a bulk shortcut that makes the trip unnecessary — and taking
    /// the scalar conversion anyway would be a divergence rather than a fix,
    /// because two of those shortcuts are *not* the scalar conversion.
    /// `CalGray` copies the grey byte into all three channels; `CalRGB` is a
    /// channel reversal that drops gamma and matrix entirely; `ICCBased` runs
    /// the profile over bytes; and `Lab` rescales `L*a*b*` out of the **byte**
    /// domain rather than out of the decoded component range. `Indexed` is
    /// handled before this by its palette, and `Pattern` never carries image
    /// samples at all.
    ///
    /// What is left is `Separation` and `DeviceN`, whose samples are *tints*
    /// driving a tint transform and have no device reading whatsoever. Those
    /// take the generic per-pixel path, and the palette precomputation over
    /// `1 << bits` entries when the sample depth allows it. ISO 32000-1
    /// §8.6.6.4 and §8.6.6.5 say the same: the components are colorant tints,
    /// and the tint transform is what turns them into colour.
    ///
    /// Crate-internal: this is `unpack`'s dispatch rule, not a fact about the
    /// space a caller outside the image build has any use for.
    #[must_use]
    pub(crate) fn needs_image_conversion(&self) -> bool {
        matches!(self, Self::Separation(_) | Self::DeviceN(_))
    }

    /// Whether the space is a plain additive or subtractive colour space,
    /// which decides whether a soft mask may take its backdrop from it.
    #[must_use]
    pub fn is_normal(&self) -> bool {
        match self {
            Self::DeviceGray
            | Self::DeviceRgb
            | Self::DeviceCmyk
            | Self::CalGray(_)
            | Self::CalRgb(_) => true,
            Self::IccBased(icc) => icc.is_normal(),
            _ => false,
        }
    }

    /// Convert components to RGB, taking the vector-fill path.
    ///
    /// A space that cannot produce a colour for these components — an out of
    /// range `Indexed` entry, a `/None` `Separation` — paints **black**,
    /// which is what every caller of the C++'s `GetRGBOrZerosOnError` sees.
    /// Use [`Self::try_to_rgb`] when the distinction matters.
    #[must_use]
    pub fn to_rgb(&self, comps: &[f32]) -> Rgb {
        self.try_to_rgb(comps).unwrap_or(Rgb::BLACK)
    }

    /// Convert components, distinguishing "black" from "no colour at all".
    #[must_use]
    pub fn try_to_rgb(&self, comps: &[f32]) -> Option<Rgb> {
        match self {
            Self::DeviceGray => Some(device::gray_to_rgb(comps)),
            Self::DeviceRgb => Some(device::rgb_to_rgb(comps)),
            Self::DeviceCmyk => Some(device::cmyk_to_rgb(comps)),
            Self::CalGray(_) => Some(cie::cal_gray_to_rgb(comps)),
            Self::CalRgb(cs) => Some(cie::cal_rgb_to_rgb(cs, comps)),
            Self::Lab(_) => Some(cie::lab_to_rgb(comps)),
            Self::IccBased(icc) => Some(icc.to_rgb(comps)),
            Self::Indexed(cs) => cs.to_rgb(comps),
            Self::Separation(cs) => cs.to_rgb(comps),
            Self::DeviceN(cs) => cs.to_rgb(comps),
            // A pattern's colour is not a function of its components; see
            // `PatternValue`.
            Self::Pattern(_) => None,
        }
    }

    /// The initial value and legal interval of component `index`.
    ///
    /// The initial *colour* of a space is these values for each component:
    /// zero everywhere except `Separation` and `DeviceN`, which start at full
    /// colorant.
    #[must_use]
    pub fn default_value(&self, index: usize) -> (f32, f32, f32) {
        match self {
            Self::Lab(lab) => lab.default_value(index),
            Self::Indexed(cs) => (0.0, 0.0, f32::from(cs.max_index)),
            // Full colorant, unlike every other family's zero.
            Self::Separation(_) | Self::DeviceN(_) => (1.0, 0.0, 1.0),
            _ => (0.0, 0.0, 1.0),
        }
    }

    /// The space's initial colour: one [`Self::default_value`] per component.
    #[must_use]
    pub fn default_color(&self) -> Vec<f32> {
        (0..self.n_components())
            .map(|i| self.default_value(i).0)
            .collect()
    }

    /// Convert a run of image samples to **B, G, R** triples.
    ///
    /// This is the bulk path, and it is *not* `to_rgb` in a loop for every
    /// family — see the module docs. `samples` holds `pixels *
    /// n_components()` bytes; `dest` receives `pixels * 3`.
    ///
    /// `trans_mask` selects `DeviceCMYK`'s second formula, the one that is
    /// unreachable from the scalar path; every other family ignores it.
    pub fn translate_image_line(
        &self,
        dest: &mut [u8],
        samples: &[u8],
        pixels: usize,
        trans_mask: bool,
    ) {
        match self {
            // The byte replicated, written R,G,B — the one family that does
            // not swap.
            Self::DeviceGray | Self::CalGray(_) => {
                for i in 0..pixels {
                    let v = samples.get(i).copied().unwrap_or(0);
                    if let Some(px) = dest.get_mut(i * 3..i * 3 + 3) {
                        px.fill(v);
                    }
                }
            }
            // A plain red-blue swap. `CalRGB` lands here too, which is where
            // its gamma, matrix and white point are silently dropped.
            Self::DeviceRgb | Self::CalRgb(_) => reverse_rgb(dest, samples, pixels),
            Self::DeviceCmyk => {
                Self::translate_cmyk_line(dest, samples, pixels, trans_mask);
            }
            // The same maths as the scalar path but on a different input
            // encoding: L* spans the byte range, a* and b* are offset by 128.
            Self::Lab(_) => {
                for i in 0..pixels {
                    let Some(&[l, a, b]) = triple(samples, i) else {
                        continue;
                    };
                    let comps = [
                        f32::from(l) * 100.0 / 255.0,
                        f32::from(a) - 128.0,
                        f32::from(b) - 128.0,
                    ];
                    write_bgr(dest, i, cie::lab_to_rgb(&comps));
                }
            }
            Self::IccBased(icc) => {
                if icc.profile.is_srgb() {
                    reverse_rgb(dest, samples, pixels);
                } else if icc.profile.is_supported() {
                    self.translate_generic_line(dest, samples, pixels);
                } else if let Some(base) = &icc.base {
                    base.translate_image_line(dest, samples, pixels, false);
                } else {
                    for i in 0..pixels {
                        write_bgr(dest, i, Rgb::BLACK);
                    }
                }
            }
            _ => self.translate_generic_line(dest, samples, pixels),
        }
    }

    /// `DeviceCMYK`'s two bulk formulas.
    #[expect(
        clippy::many_single_char_names,
        reason = "cyan, magenta, yellow and black are single-letter by convention"
    )]
    fn translate_cmyk_line(dest: &mut [u8], samples: &[u8], pixels: usize, trans_mask: bool) {
        for i in 0..pixels {
            let Some(&[c8, m8, y8, k8]) = samples
                .get(i * 4..i * 4 + 4)
                .and_then(|s| <&[u8; 4]>::try_from(s).ok())
            else {
                continue;
            };
            let (c, m, y, k) = (u32::from(c8), u32::from(m8), u32::from(y8), u32::from(k8));
            #[expect(
                clippy::cast_possible_truncation,
                reason = "every arm's arithmetic stays within a byte"
            )]
            let bgr = if trans_mask {
                // The naive un-inversion, reachable only from an image whose
                // group colorspace is also CMYK. Note this arm alone leaves
                // cyan driving the *first* byte, where the other two swap it
                // with yellow.
                let kk = 255 - k;
                [
                    (((255 - c) * kk) / 255) as u8,
                    (((255 - m) * kk) / 255) as u8,
                    (((255 - y) * kk) / 255) as u8,
                ]
            } else {
                let [r, g, b] = device::adobe_cmyk_to_srgb(c8, m8, y8, k8);
                [b, g, r]
            };
            write_bytes(dest, i, bgr);
        }
    }

    /// The generic per-pixel path: normalize, convert, write BGR truncated.
    ///
    /// `Indexed` divides by **1** rather than 255, so its samples reach
    /// `to_rgb` as raw indices; every other family normalizes.
    fn translate_generic_line(&self, dest: &mut [u8], samples: &[u8], pixels: usize) {
        let n = self.n_components();
        let divisor = if matches!(self, Self::Indexed(_)) {
            1.0
        } else {
            255.0
        };
        let mut comps = vec![0.0f32; n.max(1)];
        for i in 0..pixels {
            for (j, slot) in comps.iter_mut().enumerate() {
                *slot = f32::from(samples.get(i * n + j).copied().unwrap_or(0)) / divisor;
            }
            // A failed conversion renders black rather than skipping.
            write_bgr(dest, i, self.to_rgb(&comps));
        }
    }
}

/// Write one pixel as B, G, R with the truncating encoding.
fn write_bgr(dest: &mut [u8], index: usize, rgb: Rgb) {
    let [r, g, b] = rgb.to_bytes_truncating();
    write_bytes(dest, index, [b, g, r]);
}

/// Write three already-encoded bytes at pixel `index`.
fn write_bytes(dest: &mut [u8], index: usize, bytes: [u8; 3]) {
    if let Some(px) = dest.get_mut(index * 3..index * 3 + 3) {
        px.copy_from_slice(&bytes);
    }
}

/// The three bytes of pixel `index`, when the slice holds them.
fn triple(samples: &[u8], index: usize) -> Option<&[u8; 3]> {
    samples
        .get(index * 3..index * 3 + 3)
        .and_then(|s| <&[u8; 3]>::try_from(s).ok())
}

/// A red-blue swap, which several families' bulk path reduces to.
fn reverse_rgb(dest: &mut [u8], samples: &[u8], pixels: usize) {
    for i in 0..pixels {
        let Some(&[r, g, b]) = triple(samples, i) else {
            continue;
        };
        write_bytes(dest, i, [b, g, r]);
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

    use super::{ColorSpace, Family, Rgb};

    /// The bulk `DeviceCMYK` arm this crate runs, and the record of the one
    /// it does not.
    ///
    /// Pins the reachable bulk `DeviceCMYK` arm, the Adobe table. The
    /// *other* arm — the oracle's standard-conversion formula — is recomputed
    /// here rather than called, because it is not implemented:
    /// `device::cmyk_to_rgb` carries the proof that it can never run in the
    /// oracle. It is asserted to disagree, so the record stays falsifiable.
    #[test]
    fn the_bulk_cmyk_arm_is_the_adobe_table() {
        let cs = ColorSpace::DeviceCmyk;
        // One saturated pixel: c=128, m=64, y=0, k=64.
        let src = [128u8, 64, 0, 64];

        let mut dest = [0u8; 3];
        cs.translate_image_line(&mut dest, &src, 1, false);
        // The Adobe table's own answer, as bytes in B, G, R order.
        let [r, g, b] = super::device::adobe_cmyk_to_srgb(128, 64, 0, 64);
        assert_eq!(dest, [b, g, r]);

        // What the inert std arm would have written. The oracle assigns
        // `blue = 255 - min(255, cyan + k)` into an `FX_RGB_STRUCT`, whose
        // fields are laid out **red, green, blue** (`fx_dib.h:53`) — so cyan
        // lands in the *last* byte and yellow in the first. Byte 0 =
        // 255-min(255,0+64) = 191, byte 1 = 255-min(255,64+64) = 127, byte 2 =
        // 255-min(255,128+64) = 63.
        let would_have_been = [191u8, 127, 63];
        assert_ne!(
            dest, would_have_been,
            "the Adobe table must not agree with the naive formula here"
        );
    }

    /// `trans_mask` takes the second formula, the one arm where cyan drives
    /// the *first* byte rather than being swapped with yellow.
    #[test]
    fn trans_mask_takes_the_naive_un_inversion() {
        let cs = ColorSpace::DeviceCmyk;
        let src = [128u8, 64, 0, 64];
        let mut a = [0u8; 3];
        cs.translate_image_line(&mut a, &src, 1, true);
        // k' = 255-64 = 191; ((255-128)*191)/255 = 95, ((255-64)*191)/255 = 143,
        // ((255-0)*191)/255 = 191.
        assert_eq!(a, [95, 143, 191]);
    }

    #[test]
    fn component_counts_match_the_families() {
        assert_eq!(ColorSpace::DeviceGray.n_components(), 1);
        assert_eq!(ColorSpace::DeviceRgb.n_components(), 3);
        assert_eq!(ColorSpace::DeviceCmyk.n_components(), 4);
        assert_eq!(ColorSpace::DeviceGray.family(), Family::DeviceGray);
        assert!(!ColorSpace::DeviceRgb.is_special());
        assert!(ColorSpace::DeviceRgb.is_normal());
    }

    #[test]
    fn cal_gray_translates_one_byte_per_pixel_replicated() {
        // The oracle's `CPDFCalRGBTest`/`CalGray` vector.
        let cs = ColorSpace::CalGray(Box::new(super::CalGray {
            white_point: [0.9505, 1.0, 1.089],
            black_point: [0.0; 3],
            gamma: 1.0,
        }));
        let src = [255u8, 0, 0, 0, 255, 0, 0, 0, 255, 128, 128, 128];
        let mut dest = [0u8; 12];
        cs.translate_image_line(&mut dest, &src, 4, false);
        assert_eq!(dest, [255, 255, 255, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn cal_rgb_bulk_is_only_a_red_blue_swap() {
        // The test that pins the scalar/bulk disagreement: gamma, matrix and
        // white point are all ignored here.
        let cs = ColorSpace::CalRgb(Box::new(super::CalRgb {
            white_point: [0.9505, 1.0, 1.089],
            black_point: [0.0; 3],
            gamma: Some([2.2, 2.2, 2.2]),
            matrix: Some([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]),
        }));
        let src = [255u8, 0, 0, 0, 255, 0, 0, 0, 255, 128, 128, 128];
        let mut dest = [0u8; 12];
        cs.translate_image_line(&mut dest, &src, 4, false);
        assert_eq!(dest, [0, 0, 255, 0, 255, 0, 255, 0, 0, 128, 128, 128]);
    }

    #[test]
    fn rgb_encodings_round_and_truncate_differently() {
        let c = Rgb {
            r: 0.5,
            g: 0.5,
            b: 0.5,
        };
        assert_eq!(c.to_bytes(), [128, 128, 128]);
        assert_eq!(c.to_bytes_truncating(), [127, 127, 127]);
        // Both clamp.
        let c = Rgb {
            r: -1.0,
            g: 2.0,
            b: 0.0,
        };
        assert_eq!(c.to_bytes(), [0, 255, 0]);
        assert_eq!(c.to_bytes_truncating(), [0, 255, 0]);
    }

    #[test]
    fn separation_and_device_n_start_at_full_colorant() {
        let sep = ColorSpace::Separation(Box::new(super::Separation {
            none: false,
            alternate: Some(Box::new(ColorSpace::DeviceGray)),
            tint: None,
        }));
        assert_eq!(sep.default_color(), vec![1.0]);
        assert_eq!(ColorSpace::DeviceRgb.default_color(), vec![0.0, 0.0, 0.0]);
        assert_eq!(ColorSpace::DeviceCmyk.default_color(), vec![0.0; 4]);
    }

    #[test]
    fn pattern_has_no_scalar_colour() {
        let cs = ColorSpace::Pattern(Box::default());
        assert!(cs.try_to_rgb(&[0.5]).is_none());
        // …and the fallible-free wrapper paints black rather than panicking.
        assert_eq!(cs.to_rgb(&[0.5]), Rgb::BLACK);
    }
}
