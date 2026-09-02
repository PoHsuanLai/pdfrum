//! The annotation colour value (`/C`, `/IC`, `/MK /BC`, `/MK /BG`) and the
//! three different ways PDFium converts it to bytes.
//!
//! A colour array's *length* alone chooses the space — 1 grey, 3 RGB, 4 CMYK
//! — and every other length, including zero, means "no colour at all". That
//! is not a fallback for damage: `/IC []` deliberately says "do not fill".
//!
//! ```
//! use pdfrum_doc::color::Color;
//! use pdfrum_object::{Array, Object};
//!
//! let rgb = Array::of([0.25, 0.5, 1.0].map(Object::from));
//! assert_eq!(Color::from_array(&rgb), Color::Rgb(0.25, 0.5, 1.0));
//! // Two components name no space.
//! let two = Array::of([0.25, 0.5].map(Object::from));
//! assert_eq!(Color::from_array(&two), Color::Transparent);
//! ```

// Every conversion below narrows on purpose: these reproduce a C float-to-
// integer conversion, whose truncation toward zero is the observable
// behaviour, not an approximation of it.
#![allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]

use pdfrum_object::Array;

/// A colour in whichever device space its component count named.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Color {
    /// No colour: the caller emits no colour operator at all.
    #[default]
    Transparent,
    /// `DeviceGray`.
    Gray(f32),
    /// `DeviceRGB`.
    Rgb(f32, f32, f32),
    /// `DeviceCMYK`.
    Cmyk(f32, f32, f32, f32),
}

impl Color {
    /// Reads a colour array, dispatching purely on its length.
    ///
    /// Components come through the non-resolving numeric accessor, so a
    /// missing or mistyped element reads as `0.0` rather than rejecting the
    /// array.
    #[must_use]
    pub fn from_array(array: &Array) -> Color {
        let at = |i: usize| array.number_at_or_zero(i);
        match array.len() {
            1 => Color::Gray(at(0)),
            3 => Color::Rgb(at(0), at(1), at(2)),
            4 => Color::Cmyk(at(0), at(1), at(2), at(3)),
            _ => Color::Transparent,
        }
    }

    /// Component-wise equality within `1e-4`.
    ///
    /// Deliberately not `PartialEq`: the derived one stays exact so a test
    /// The RGB bytes the `--annot` dump prints.
    ///
    /// CMYK converts multiplicatively (`255·(1−c)·(1−k)`), which is **not**
    /// the formula `Color::mk_rgb_bytes` uses for the same colour in a
    /// widget's `/MK` dictionary. Both exist upstream; keeping them named
    /// apart is the only defence against unifying them.
    ///
    /// Negative components saturate at zero rather than wrapping, which is
    /// what the C++'s float-to-`unsigned` conversion does in practice.
    #[must_use]
    pub fn annot_rgb_bytes(self) -> (u32, u32, u32) {
        let byte = |v: f32| {
            let scaled = v * 255.0;
            if scaled <= 0.0 {
                0
            } else if !scaled.is_finite() || scaled >= 4_294_967_296.0 {
                u32::MAX
            } else {
                scaled as u32
            }
        };
        match self {
            Color::Transparent => (0, 0, 0),
            Color::Gray(g) => (byte(g), byte(g), byte(g)),
            Color::Rgb(r, g, b) => (byte(r), byte(g), byte(b)),
            Color::Cmyk(c, m, y, k) => (
                byte((1.0 - c) * (1.0 - k)),
                byte((1.0 - m) * (1.0 - k)),
                byte((1.0 - y) * (1.0 - k)),
            ),
        }
    }

    /// The RGB bytes a widget's `/MK` colour converts to.
    ///
    /// CMYK converts subtractively here (`(1 − min(1, c+k))·255`), and every
    /// channel rounds by adding a half before truncating. See
    /// [`Color::annot_rgb_bytes`] for the other formula.
    #[must_use]
    #[cfg(test)]
    pub(crate) fn mk_rgb_bytes(self) -> (u8, u8, u8) {
        let byte = |v: f32| {
            let scaled = v * 255.0 + 0.5;
            if scaled <= 0.0 {
                0
            } else if scaled >= 255.0 {
                255
            } else {
                scaled as u8
            }
        };
        match self {
            Color::Transparent => (0, 0, 0),
            Color::Gray(g) => (byte(g), byte(g), byte(g)),
            Color::Rgb(r, g, b) => (byte(r), byte(g), byte(b)),
            Color::Cmyk(c, m, y, k) => (
                byte(1.0 - (c + k).min(1.0)),
                byte(1.0 - (m + k).min(1.0)),
                byte(1.0 - (y + k).min(1.0)),
            ),
        }
    }
}

/// Builds an RGB colour from 0–255 bytes, the way the hard-coded widget
/// chrome colours are written upstream.
#[must_use]
#[cfg(test)]
pub(crate) fn rgb_bytes(r: u8, g: u8, b: u8) -> Color {
    Color::Rgb(
        f32::from(r) / 255.0,
        f32::from(g) / 255.0,
        f32::from(b) / 255.0,
    )
}

#[cfg(test)]
mod tests {
    use super::{Color, rgb_bytes};
    use pdfrum_object::{Array, Object};

    fn array(values: &[f32]) -> Array {
        Array::of(values.iter().copied().map(Object::from))
    }

    #[test]
    fn length_alone_chooses_the_space() {
        assert_eq!(Color::from_array(&array(&[])), Color::Transparent);
        assert_eq!(Color::from_array(&array(&[0.5])), Color::Gray(0.5));
        assert_eq!(Color::from_array(&array(&[0.5, 0.25])), Color::Transparent);
        assert_eq!(
            Color::from_array(&array(&[0.0, 0.5, 1.0])),
            Color::Rgb(0.0, 0.5, 1.0)
        );
        assert_eq!(
            Color::from_array(&array(&[0.0, 0.1, 0.2, 0.3])),
            Color::Cmyk(0.0, 0.1, 0.2, 0.3)
        );
        assert_eq!(
            Color::from_array(&array(&[0.0, 0.1, 0.2, 0.3, 0.4])),
            Color::Transparent
        );
    }

    #[test]
    fn a_mistyped_component_reads_as_zero() {
        let mixed = Array::of([
            Object::Name(pdfrum_object::Name::from("Oops")),
            Object::from(0.5_f32),
            Object::from(1.0_f32),
        ]);
        assert_eq!(Color::from_array(&mixed), Color::Rgb(0.0, 0.5, 1.0));
    }

    #[test]
    fn the_two_cmyk_formulas_differ() {
        let c = Color::Cmyk(0.5, 0.0, 0.0, 0.5);
        // Multiplicative: 255 * 0.5 * 0.5 = 63.75 -> 63.
        assert_eq!(c.annot_rgb_bytes().0, 63);
        // Subtractive: (1 - min(1, 1.0)) * 255 + 0.5 = 0.5 -> 0.
        assert_eq!(c.mk_rgb_bytes().0, 0);
    }

    #[test]
    fn negative_components_saturate_rather_than_wrap() {
        assert_eq!(Color::Rgb(-1.0, 0.0, 1.0).annot_rgb_bytes(), (0, 0, 255));
    }

    #[test]
    fn byte_constructor_round_trips_through_the_annot_formula() {
        assert_eq!(rgb_bytes(220, 220, 220).annot_rgb_bytes(), (220, 220, 220));
        assert_eq!(rgb_bytes(0, 51, 113).annot_rgb_bytes(), (0, 51, 113));
    }
}
