//! Blend modes (ISO 32000-1 §11.3.5): how what is drawn next combines with
//! what is already on the page.

use pdfrum_object::{Dict, Name, Object, names as pdf_names};

use super::Canvas;

/// A separable or non-separable blend mode, ISO 32000-1 tables 136-137.
///
/// An enum rather than the `/BM` name so a misspelt mode cannot compile; the
/// set is PDF's own and closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BlendMode {
    /// The source replaces the backdrop. PDF's default.
    #[default]
    Normal,
    /// `Multiply`.
    Multiply,
    /// `Screen`.
    Screen,
    /// `Overlay`.
    Overlay,
    /// `Darken`.
    Darken,
    /// `Lighten`.
    Lighten,
    /// `ColorDodge`.
    ColorDodge,
    /// `ColorBurn`.
    ColorBurn,
    /// `HardLight`.
    HardLight,
    /// `SoftLight`.
    SoftLight,
    /// `Difference`.
    Difference,
    /// `Exclusion`.
    Exclusion,
    /// `Hue`.
    Hue,
    /// `Saturation`.
    Saturation,
    /// `Color`.
    Color,
    /// `Luminosity`.
    Luminosity,
}

impl BlendMode {
    /// The `/BM` name.
    fn name(self) -> &'static str {
        match self {
            Self::Normal => "Normal",
            Self::Multiply => "Multiply",
            Self::Screen => "Screen",
            Self::Overlay => "Overlay",
            Self::Darken => "Darken",
            Self::Lighten => "Lighten",
            Self::ColorDodge => "ColorDodge",
            Self::ColorBurn => "ColorBurn",
            Self::HardLight => "HardLight",
            Self::SoftLight => "SoftLight",
            Self::Difference => "Difference",
            Self::Exclusion => "Exclusion",
            Self::Hue => "Hue",
            Self::Saturation => "Saturation",
            Self::Color => "Color",
            Self::Luminosity => "Luminosity",
        }
    }
}

impl Canvas<'_, '_> {
    /// Blend everything drawn after it by `mode`, as an `/ExtGState` naming
    /// `/BM`.
    ///
    /// Scoped by [`Canvas::saved`], like [`Canvas::opacity`].
    ///
    /// ```
    /// use pdfrum_edit::{BlendMode, EditDoc, Size, blank_document};
    /// use pdfrum_common::Limits;
    /// use kurbo::Rect;
    /// use peniko::Color;
    ///
    /// let base = blank_document(&[Size::new(100.0, 100.0)])?;
    /// let mut edit = EditDoc::new(&base);
    /// edit.draw_page(0, &Limits::default(), |c| {
    ///     c.saved(|c| {
    ///         c.blend(BlendMode::Multiply);
    ///         c.fill_rect(Rect::new(0.0, 0.0, 50.0, 50.0), Color::from_rgb8(255, 200, 0));
    ///     });
    /// })?;
    /// # Ok::<(), pdfrum_edit::Error>(())
    /// ```
    pub fn blend(&mut self, mode: BlendMode) {
        let state = Dict::from_pairs([(Name::from("BM"), Object::Name(Name::from(mode.name())))]);
        let name = self.realize(pdf_names::EXT_G_STATE, Object::Dict(state));
        self.out.push('/');
        self.push_name(&name);
        self.out.push_str(" gs\n");
    }
}
