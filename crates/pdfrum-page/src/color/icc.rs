//! `ICCBased` spaces and the fallback ladder around a profile the colour
//! engine will not accept (ISO 32000-1 §8.6.5.5).
//!
//! `/N` is authoritative and strict: it must be 1, 3 or 4, and a missing key
//! reads as 0 and is rejected. PDFium's comment is explicit that this
//! matches Acrobat rather than the lenient viewers. Everything downstream
//! follows one ladder:
//!
//! ```text
//! sRGB special case  → the components pass straight through, unclamped
//! profile accepted   → the colour engine transforms
//! /Alternate loaded  → that space converts
//! otherwise          → BLACK, an engaged colour rather than "no colour"
//! ```
//!
//! That last rung matters: a wholly broken ICC space paints black, it does
//! not skip the object.

use super::{ColorSpace, Rgb};
use std::sync::Arc;

/// The exact byte length of the sRGB profile PDFium special-cases.
const SRGB_PROFILE_LEN: usize = 3144;

/// Where its identifying description sits.
const SRGB_TAG_OFFSET: usize = 400;

/// The description itself, seventeen bytes with no terminator.
const SRGB_TAG: &[u8; 17] = b"sRGB IEC61966-2.1";

/// Component counts a colour engine will accept for an ICC profile.
#[must_use]
pub fn is_valid_icc_components(n: i64) -> bool {
    matches!(n, 1 | 3 | 4)
}

/// A loaded ICC profile, or the reason it is not usable.
///
/// `moxcms` replaces the C++'s lcms here. The *ladder* around a rejection is
/// pinned exactly; which malformed profiles get rejected is the colour
/// engine's judgement and is measured against the oracle rather than
/// specified (brief Q2).
pub struct IccProfile {
    /// The prepared transform to sRGB, when the profile is usable.
    transform: Option<Arc<moxcms::TransformF32Executor>>,
    /// Whether the profile is byte-for-byte the sRGB profile PDFium
    /// recognises without a transform.
    srgb: bool,
    /// Components the *profile* declares, which must agree with `/N`.
    components: usize,
    /// Whether the source space is one of Gray, RGB or CMYK.
    normal: bool,
}

impl std::fmt::Debug for IccProfile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IccProfile")
            .field("supported", &self.transform.is_some())
            .field("srgb", &self.srgb)
            .field("components", &self.components)
            .field("normal", &self.normal)
            .finish()
    }
}

impl PartialEq for IccProfile {
    /// Profiles compare by their observable behaviour, not by transform
    /// identity — two loads of the same bytes must be equal.
    fn eq(&self, other: &Self) -> bool {
        self.srgb == other.srgb
            && self.components == other.components
            && self.normal == other.normal
            && self.transform.is_some() == other.transform.is_some()
    }
}

impl IccProfile {
    /// Load a profile that is expected to carry `expected` components.
    ///
    /// The sRGB special case is checked **only** for three components, and
    /// when it hits the profile reports itself unsupported — so the
    /// `/Alternate` search still runs, even though `to_rgb` never reaches it.
    #[must_use]
    pub fn load(data: &[u8], expected: usize) -> Self {
        if expected == 3 && is_srgb_profile(data) {
            return Self {
                transform: None,
                srgb: true,
                components: 3,
                normal: true,
            };
        }
        let Ok(profile) = moxcms::ColorProfile::new_from_slice(data) else {
            return Self::rejected(expected);
        };
        let components = channels_of(profile.color_space);
        // A profile whose own channel count disagrees with `/N` is rejected,
        // exactly as the C++'s CMS wrapper does.
        if components != expected || !is_valid_icc_components(components.try_into().unwrap_or(0)) {
            return Self::rejected(expected);
        }
        let srgb = moxcms::ColorProfile::new_srgb();
        let options = moxcms::TransformOptions {
            rendering_intent: moxcms::RenderingIntent::Perceptual,
            ..Default::default()
        };
        let layout = match components {
            1 => moxcms::Layout::Gray,
            4 => moxcms::Layout::Rgba,
            _ => moxcms::Layout::Rgb,
        };
        let transform = profile
            .create_transform_f32(layout, &srgb, moxcms::Layout::Rgb, options)
            .ok();
        Self {
            normal: matches!(
                profile.color_space,
                moxcms::DataColorSpace::Gray
                    | moxcms::DataColorSpace::Rgb
                    | moxcms::DataColorSpace::Cmyk
            ),
            transform,
            srgb: false,
            components,
        }
    }

    fn rejected(expected: usize) -> Self {
        Self {
            transform: None,
            srgb: false,
            components: expected,
            normal: false,
        }
    }

    /// Whether the profile can transform colours itself.
    #[must_use]
    pub fn is_supported(&self) -> bool {
        self.transform.is_some()
    }

    /// Whether this is the recognised sRGB profile.
    #[must_use]
    pub fn is_srgb(&self) -> bool {
        self.srgb
    }

    /// Whether the source space is Gray, RGB or CMYK, which decides whether
    /// this space may back a soft mask's backdrop.
    #[must_use]
    pub fn is_normal(&self) -> bool {
        self.srgb || self.normal
    }

    /// Transform one colour, clamping inputs the way the C++ wrapper does.
    #[must_use]
    pub fn to_rgb(&self, comps: &[f32]) -> Option<Rgb> {
        let transform = self.transform.as_ref()?;
        let mut src = [0f32; 4];
        for (slot, v) in src.iter_mut().zip(comps.iter()) {
            *slot = v.clamp(0.0, 1.0);
        }
        let mut dst = [0f32; 3];
        transform
            .transform(src.get(..self.components)?, &mut dst)
            .ok()?;
        Some(Rgb {
            r: dst[0],
            g: dst[1],
            b: dst[2],
        })
    }
}

/// How many channels a colour space carries, for the `/N` agreement check.
fn channels_of(space: moxcms::DataColorSpace) -> usize {
    match space {
        moxcms::DataColorSpace::Gray => 1,
        moxcms::DataColorSpace::Cmyk | moxcms::DataColorSpace::Color4 => 4,
        moxcms::DataColorSpace::Color2 => 2,
        moxcms::DataColorSpace::Color5 => 5,
        moxcms::DataColorSpace::Color6 => 6,
        moxcms::DataColorSpace::Color7 => 7,
        moxcms::DataColorSpace::Color8 => 8,
        _ => 3,
    }
}

/// PDFium's sRGB detection: an exact byte length and an exact description at
/// a fixed offset.
#[must_use]
pub fn is_srgb_profile(data: &[u8]) -> bool {
    data.len() == SRGB_PROFILE_LEN
        && data.get(SRGB_TAG_OFFSET..SRGB_TAG_OFFSET + SRGB_TAG.len()) == Some(&SRGB_TAG[..])
}

/// An `ICCBased` colorspace: the profile, the declared component count, and
/// whatever the fallback ladder settled on.
#[derive(Debug, Clone, PartialEq)]
pub struct IccBased {
    /// The profile, shared because several spaces may name one stream.
    pub profile: Arc<IccProfile>,
    /// The `/N` value, which is what [`ColorSpace::n_components`] reports —
    /// **not** whatever the profile says.
    pub n: u8,
    /// The space colours fall back to: the `/Alternate` when it loaded and
    /// agreed on component count, otherwise the stock device space for `/N`.
    pub base: Option<Box<ColorSpace>>,
    /// `/Range`, kept because the dictionary carries it. Nothing reads it,
    /// matching the C++'s standing TODO.
    pub ranges: Box<[f32]>,
}

impl IccBased {
    /// The ladder from the module docs, in order.
    #[must_use]
    pub fn to_rgb(&self, comps: &[f32]) -> Rgb {
        if self.profile.is_srgb() {
            // A pass-through, unclamped — the sRGB profile is the identity by
            // definition, so the components *are* the colour.
            return Rgb {
                r: comps.first().copied().unwrap_or(0.0),
                g: comps.get(1).copied().unwrap_or(0.0),
                b: comps.get(2).copied().unwrap_or(0.0),
            };
        }
        if let Some(rgb) = self.profile.to_rgb(comps) {
            return rgb;
        }
        if let Some(base) = &self.base {
            return base.to_rgb(comps);
        }
        // Black, not "no colour": a broken ICC space still paints.
        Rgb {
            r: 0.0,
            g: 0.0,
            b: 0.0,
        }
    }

    /// Whether this space may serve as a soft mask's group colorspace.
    #[must_use]
    pub fn is_normal(&self) -> bool {
        if self.profile.is_srgb() || self.profile.is_supported() {
            return self.profile.is_normal();
        }
        self.base.as_ref().is_some_and(|b| b.is_normal())
    }

    /// The stock space `/N` implies when nothing better is available.
    #[must_use]
    pub fn stock_alternate(n: u8) -> Option<ColorSpace> {
        Some(match n {
            1 => ColorSpace::DeviceGray,
            3 => ColorSpace::DeviceRgb,
            4 => ColorSpace::DeviceCmyk,
            _ => return None,
        })
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

    use super::{IccProfile, is_srgb_profile, is_valid_icc_components};

    #[test]
    fn only_one_three_and_four_components_are_valid() {
        assert!(is_valid_icc_components(1));
        assert!(is_valid_icc_components(3));
        assert!(is_valid_icc_components(4));
        for n in [0, 2, 5, -1, 100] {
            assert!(!is_valid_icc_components(n), "{n} should be rejected");
        }
    }

    #[test]
    fn srgb_detection_needs_the_exact_length_and_tag() {
        let mut data = vec![0u8; 3144];
        assert!(!is_srgb_profile(&data));
        data[400..417].copy_from_slice(b"sRGB IEC61966-2.1");
        assert!(is_srgb_profile(&data));
        // One byte longer and it is a different profile.
        data.push(0);
        assert!(!is_srgb_profile(&data));
        // The tag one byte earlier does not count.
        let mut data = vec![0u8; 3144];
        data[399..416].copy_from_slice(b"sRGB IEC61966-2.1");
        assert!(!is_srgb_profile(&data));
    }

    #[test]
    fn the_srgb_special_case_only_applies_to_three_components() {
        let mut data = vec![0u8; 3144];
        data[400..417].copy_from_slice(b"sRGB IEC61966-2.1");
        assert!(IccProfile::load(&data, 3).is_srgb());
        // With `/N 1` the same bytes go down the ordinary load path, which
        // rejects them as not a parsable profile.
        let one = IccProfile::load(&data, 1);
        assert!(!one.is_srgb());
        assert!(!one.is_supported());
    }

    #[test]
    fn garbage_profiles_are_rejected_not_panicked_on() {
        for data in [&b""[..], b"\x00", b"not an icc profile at all"] {
            let p = IccProfile::load(data, 3);
            assert!(!p.is_supported());
            assert!(!p.is_srgb());
            assert!(p.to_rgb(&[0.5, 0.5, 0.5]).is_none());
        }
    }
}

/// An sRGB ICC profile, encoded, for a writer that must embed one.
///
/// PDF/A requires an `/OutputIntent` whose `/DestOutputProfile` is an embedded
/// ICC profile, and the conversion has to produce those bytes from somewhere.
/// It comes from here rather than from a vendored binary blob because
/// `moxcms` is already the colour engine in this crate and already models a
/// profile — `docs/design/pdfa.md` §6 records the alternative that was
/// declined, which was to check a 3 KB `.icc` file into the tree.
///
/// Returns nothing if the encoder refuses, which it does not for the built-in
/// sRGB profile; the caller reports it rather than writing an intent whose
/// profile is empty, since that is a worse PDF/A failure than having none.
#[must_use]
pub fn srgb_profile_bytes() -> Option<Vec<u8>> {
    moxcms::ColorProfile::new_srgb().encode().ok()
}

#[cfg(test)]
mod srgb_profile_tests {
    use super::srgb_profile_bytes;

    // The conversion embeds these bytes as a PDF/A output intent's
    // `/DestOutputProfile`, so they must be a profile a validator accepts: an
    // ICC header whose declared size matches, an `RGB ` data space and a
    // `mntr` device class.
    #[test]
    fn the_srgb_profile_encodes_as_a_well_formed_icc_profile() {
        let bytes = srgb_profile_bytes().expect("the built-in sRGB profile encodes");
        assert!(bytes.len() > 128, "an ICC profile is at least a header");
        let field = |at: usize| bytes.get(at..at + 4).expect("an ICC header is 128 bytes");
        let declared =
            u32::from_be_bytes(field(0).try_into().expect("four bytes are four bytes")) as usize;
        assert_eq!(
            declared,
            bytes.len(),
            "the header's size field is the truth"
        );
        assert_eq!(field(12), b"mntr", "a display device class");
        assert_eq!(field(16), b"RGB ", "an RGB data colour space");
        assert_eq!(field(36), b"acsp", "the ICC signature");
        // And the engine reads back what it wrote.
        assert!(moxcms::ColorProfile::new_from_slice(&bytes).is_ok());
    }
}
