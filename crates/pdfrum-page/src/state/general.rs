//! Alphas, blend mode, soft mask, and the `/ExtGState` keys that are parsed
//! but never consumed (ISO 32000-1 §11.6.6).
//!
//! Eleven `/ExtGState` keys affect output; **twelve more are parsed and
//! stored and then never read by anything**. They are kept as fields anyway,
//! because a structure dump reports them and a future overprint
//! implementation will want them — but nothing here may let them change
//! rendering, which is the one rule this module enforces.

use crate::transparency::SoftMask;
use std::sync::Arc;

/// A separable blend mode (ISO 32000-1 table 136).
///
/// An unrecognised `/BM` name is **`Normal`**, not an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum BlendMode {
    /// Paint the source over the backdrop.
    #[default]
    Normal,
    /// A synonym for [`Self::Normal`] kept for round-tripping.
    Compatible,
    /// Multiply the two.
    Multiply,
    /// Multiply the complements.
    Screen,
    /// Multiply or screen depending on the backdrop.
    Overlay,
    /// Take the darker.
    Darken,
    /// Take the lighter.
    Lighten,
    /// Brighten the backdrop to reflect the source.
    ColorDodge,
    /// Darken the backdrop to reflect the source.
    ColorBurn,
    /// Multiply or screen depending on the source.
    HardLight,
    /// Darken or lighten depending on the source.
    SoftLight,
    /// The absolute difference.
    Difference,
    /// A lower-contrast difference.
    Exclusion,
    /// The source's hue with the backdrop's saturation and luminosity.
    Hue,
    /// The source's saturation.
    Saturation,
    /// The source's hue and saturation.
    Color,
    /// The source's luminosity.
    Luminosity,
}

impl BlendMode {
    /// The mode a `/BM` name denotes, defaulting to [`Self::Normal`].
    #[must_use]
    pub fn from_name(name: &[u8]) -> Self {
        match name {
            b"Multiply" => Self::Multiply,
            b"Screen" => Self::Screen,
            b"Overlay" => Self::Overlay,
            b"Darken" => Self::Darken,
            b"Lighten" => Self::Lighten,
            b"ColorDodge" => Self::ColorDodge,
            b"ColorBurn" => Self::ColorBurn,
            b"HardLight" => Self::HardLight,
            b"SoftLight" => Self::SoftLight,
            b"Difference" => Self::Difference,
            b"Exclusion" => Self::Exclusion,
            b"Hue" => Self::Hue,
            b"Saturation" => Self::Saturation,
            b"Color" => Self::Color,
            b"Luminosity" => Self::Luminosity,
            b"Compatible" => Self::Compatible,
            // Every unrecognised name, including `/Normal` itself.
            _ => Self::Normal,
        }
    }

    /// Whether compositing in this mode needs the backdrop.
    ///
    /// PDFium marks a holder as needing background alpha for any mode past
    /// `Multiply`, which is what this reproduces.
    #[must_use]
    pub fn needs_backdrop(self) -> bool {
        !matches!(self, Self::Normal | Self::Compatible | Self::Multiply)
    }
}

/// The rendering intent an `/RI` names.
///
/// Stored as PDFium stores it — an integer tag — and **never consumed by
/// rendering**.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum RenderIntent {
    /// Anything that is not one of the three recognised names.
    #[default]
    Unknown,
    /// `/AbsoluteColorimetric`.
    Absolute,
    /// `/Saturation`.
    Saturation,
    /// `/Perceptual`.
    Perceptual,
}

impl RenderIntent {
    /// The intent a name denotes, matching on its **first four bytes** as
    /// PDFium does — so `/Percept` and `/Perceptual` are the same intent.
    #[must_use]
    pub fn from_name(name: &[u8]) -> Self {
        match name.get(..4) {
            Some(b"Abso") => Self::Absolute,
            Some(b"Satu") => Self::Saturation,
            Some(b"Perc") => Self::Perceptual,
            _ => Self::Unknown,
        }
    }
}

/// The `/ExtGState` parameters that reach compositing, plus the ones that do
/// not.
///
/// The several booleans are not a state machine to be folded into an enum:
/// each is an independent `/ExtGState` key that a file sets on its own, and
/// five of them are inert flags kept only so a dump can report them.
#[expect(
    clippy::struct_excessive_bools,
    reason = "each flag is an independent /ExtGState key, not a mode"
)]
#[derive(Debug, Clone, PartialEq)]
pub struct GeneralState {
    /// `/ca`, clamped to `0..=1`.
    pub fill_alpha: f32,
    /// `/CA`, clamped to `0..=1`.
    pub stroke_alpha: f32,
    /// `/BM`.
    pub blend: BlendMode,
    /// `/SMask`, shared because a state stack clones cheaply.
    pub soft_mask: Option<Arc<SoftMask>>,
    /// The transfer function `/TR` or `/TR2` names, sampled to three
    /// 256-entry tables.
    pub transfer: Option<Arc<crate::transfer::TransferFunc>>,

    // ---- Parsed and stored; nothing reads these ----
    /// `/RI`.
    pub render_intent: RenderIntent,
    /// `/OP`, the stroking overprint flag.
    pub stroke_overprint: bool,
    /// `/op`, the non-stroking one.
    pub fill_overprint: bool,
    /// `/OPM`.
    pub overprint_mode: i64,
    /// `/FL`, also settable by the `i` operator.
    pub flatness: f32,
    /// `/SM`.
    pub smoothness: f32,
    /// `/SA`.
    pub stroke_adjust: bool,
    /// `/AIS`.
    pub alpha_is_shape: bool,
    /// `/TK`.
    pub text_knockout: bool,
}

impl Default for GeneralState {
    fn default() -> Self {
        Self {
            fill_alpha: 1.0,
            stroke_alpha: 1.0,
            blend: BlendMode::Normal,
            soft_mask: None,
            transfer: None,
            render_intent: RenderIntent::Unknown,
            stroke_overprint: false,
            fill_overprint: false,
            overprint_mode: 0,
            flatness: 0.0,
            smoothness: 0.0,
            stroke_adjust: false,
            alpha_is_shape: false,
            text_knockout: false,
        }
    }
}

impl GeneralState {
    /// Reset the four parameters entering a transparency group clears.
    ///
    /// This is the group-isolation rule: a group starts compositing from a
    /// clean slate, so its contents cannot see the enclosing blend mode,
    /// alphas or soft mask (ISO 32000-1 §11.6.6).
    pub fn enter_transparency_group(&mut self) {
        self.blend = BlendMode::Normal;
        self.stroke_alpha = 1.0;
        self.fill_alpha = 1.0;
        self.soft_mask = None;
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

    use super::{BlendMode, GeneralState, RenderIntent};

    #[test]
    fn an_unknown_blend_name_is_normal() {
        assert_eq!(BlendMode::from_name(b"Multiply"), BlendMode::Multiply);
        assert_eq!(BlendMode::from_name(b"Luminosity"), BlendMode::Luminosity);
        assert_eq!(BlendMode::from_name(b"Normal"), BlendMode::Normal);
        assert_eq!(BlendMode::from_name(b"NotAMode"), BlendMode::Normal);
        assert_eq!(BlendMode::from_name(b""), BlendMode::Normal);
        // Case sensitive.
        assert_eq!(BlendMode::from_name(b"multiply"), BlendMode::Normal);
    }

    #[test]
    fn backdrop_is_needed_past_multiply() {
        assert!(!BlendMode::Normal.needs_backdrop());
        assert!(!BlendMode::Compatible.needs_backdrop());
        assert!(!BlendMode::Multiply.needs_backdrop());
        assert!(BlendMode::Screen.needs_backdrop());
        assert!(BlendMode::Luminosity.needs_backdrop());
    }

    #[test]
    fn render_intents_match_on_four_bytes() {
        assert_eq!(
            RenderIntent::from_name(b"AbsoluteColorimetric"),
            RenderIntent::Absolute
        );
        assert_eq!(RenderIntent::from_name(b"Abso"), RenderIntent::Absolute);
        assert_eq!(
            RenderIntent::from_name(b"Perceptual"),
            RenderIntent::Perceptual
        );
        assert_eq!(RenderIntent::from_name(b"Rel"), RenderIntent::Unknown);
        assert_eq!(RenderIntent::from_name(b""), RenderIntent::Unknown);
    }

    #[test]
    fn entering_a_group_clears_exactly_four_parameters() {
        let mut s = GeneralState {
            fill_alpha: 0.5,
            stroke_alpha: 0.25,
            blend: BlendMode::Multiply,
            overprint_mode: 7,
            flatness: 3.0,
            ..GeneralState::default()
        };
        s.enter_transparency_group();
        assert!((s.fill_alpha - 1.0).abs() < 1e-6);
        assert!((s.stroke_alpha - 1.0).abs() < 1e-6);
        assert_eq!(s.blend, BlendMode::Normal);
        assert!(s.soft_mask.is_none());
        // The inert fields are untouched.
        assert_eq!(s.overprint_mode, 7);
        assert!((s.flatness - 3.0).abs() < 1e-6);
    }

    #[test]
    fn the_defaults_are_fully_opaque_and_unblended() {
        let s = GeneralState::default();
        assert!((s.fill_alpha - 1.0).abs() < 1e-6);
        assert!((s.stroke_alpha - 1.0).abs() < 1e-6);
        assert_eq!(s.blend, BlendMode::Normal);
        assert!(s.soft_mask.is_none());
        assert!(s.transfer.is_none());
    }
}
