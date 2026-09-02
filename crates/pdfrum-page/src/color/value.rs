//! A colour as the graphics state holds it: a space plus its components, or
//! a pattern (ISO 32000-1 §8.6.8).
//!
//! The rule that surprises people is in [`ColorValue::set_components`]:
//! **too few operands change nothing at all.** `1 2 sc` in a four-component
//! space leaves the previous colour standing rather than filling in zeros.
//! Combined with the fact that installing a colorspace *resets* the colour to
//! that space's default, this is why `cs` followed by an under-specified `sc`
//! paints the space's default rather than anything the operands suggested.

use super::{ColorSpace, MAX_PATTERN_COMPONENTS, Rgb};
use pdfrum_object::Name;
use smallvec::SmallVec;
use std::sync::Arc;

/// A colour in some space.
///
/// Cheap to clone — the space is shared, the components are inline.
#[derive(Debug, Clone, PartialEq)]
pub struct ColorValue {
    /// The space the components live in. `None` before any colour operator
    /// has run, which reads as `DeviceGray` on first use.
    pub space: Option<Arc<ColorSpace>>,
    /// The components, whose length always matches the space when set.
    pub components: SmallVec<[f32; 4]>,
    /// The pattern, when the space is `/Pattern`.
    pub pattern: Option<Box<PatternValue>>,
}

impl Default for ColorValue {
    /// Opaque black in `DeviceGray`, which is a page's initial fill and
    /// stroke colour.
    fn default() -> Self {
        Self {
            space: None,
            components: SmallVec::from_slice(&[0.0]),
            pattern: None,
        }
    }
}

/// A pattern colour: which pattern, and the components an uncoloured one
/// paints with.
#[derive(Debug, Clone, PartialEq)]
pub struct PatternValue {
    /// The pattern's name in the `/Pattern` resource dictionary.
    pub name: Name,
    /// The colour operands, capped at sixteen. Meaningless for a coloured
    /// (`/PaintType 1`) pattern, which supplies its own.
    pub components: SmallVec<[f32; 4]>,
    /// The pattern the name resolved to, when it resolved to one.
    ///
    /// Loading it here rather than at paint time is what lets the renderer
    /// stay free of the resolver: a tiling cell's page objects and a shading
    /// pattern's ramp are both already in hand by the time an object carrying
    /// this colour reaches the walk. `None` only where `scn` named a pattern
    /// the resources do not define, which is a no-op operator.
    pub loaded: Option<Arc<crate::pattern::Pattern>>,
}

/// Why [`ColorValue::set_components`] refused the values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SetComponentsError {
    /// The slice was shorter than the space's component count.
    #[error("need {needed} colour components, got {got}")]
    TooFew {
        /// Components the space requires.
        needed: usize,
        /// Components the caller supplied.
        got: usize,
    },
    /// The current space is a pattern; only `scn` with a name changes it.
    #[error("a pattern colour is not set by components")]
    PatternSpace,
}

impl ColorValue {
    /// Install a colorspace, **resetting** the components to its default
    /// colour.
    ///
    /// This is what makes `cs` discard whatever colour was current: the
    /// operands that follow are the only ones that count.
    pub fn set_space(&mut self, space: Arc<ColorSpace>) {
        self.components = space.default_color().into();
        self.pattern = None;
        self.space = Some(space);
    }

    /// Set the components without changing the space.
    ///
    /// **Too few values change nothing**; surplus values are kept, since
    /// PDFium stores the vector verbatim. With no space installed,
    /// `DeviceGray` is installed first.
    ///
    /// # Errors
    ///
    /// [`SetComponentsError::TooFew`] when `values` is shorter than the space
    /// requires. [`SetComponentsError::PatternSpace`] when the current space
    /// is a pattern — only `scn` with a name changes a pattern colour.
    pub fn set_components(&mut self, values: &[f32]) -> Result<(), SetComponentsError> {
        let space = self
            .space
            .get_or_insert_with(|| Arc::new(ColorSpace::DeviceGray));
        if space.n_components() > values.len() {
            return Err(SetComponentsError::TooFew {
                needed: space.n_components(),
                got: values.len(),
            });
        }
        if matches!(**space, ColorSpace::Pattern(_)) {
            // A pattern space keeps its pattern; only `scn` with a name
            // changes it.
            return Err(SetComponentsError::PatternSpace);
        }
        self.components = SmallVec::from_slice(values);
        Ok(())
    }

    /// Set both the space and the components in one step, as `g`, `rg` and
    /// `k` do.
    pub fn set_stock(&mut self, space: ColorSpace, values: &[f32]) {
        let space = Arc::new(space);
        self.set_space(Arc::clone(&space));
        if space.n_components() <= values.len() {
            self.components = SmallVec::from_slice(values);
        }
    }

    /// Install a pattern, with the operands an uncoloured one paints with.
    ///
    /// Two rules, both `CPDF_Color::SetValueForPattern`'s
    /// (`cpdf_color.cpp:52-66`) and neither obvious:
    ///
    /// - **An operand vector longer than sixteen is refused outright.** The
    ///   C++ returns before touching anything, so the colour keeps whatever it
    ///   had — the previous pattern, or no pattern at all.
    /// - **The space becomes `/Pattern` if it was not one already.** A
    ///   `/P1 scn` with no preceding `/Pattern cs` therefore *is* a pattern
    ///   colour: the pattern machinery drains it out of the ordinary draw and
    ///   the object paints the pattern rather than a colour. Leaving the space
    ///   alone instead resolves the operands through whatever space was
    ///   current — black in the default `DeviceGray` — and paints the object
    ///   solid, which on a page-sized rectangle is the whole page.
    pub fn set_pattern(
        &mut self,
        name: Name,
        values: &[f32],
        loaded: Option<Arc<crate::pattern::Pattern>>,
    ) {
        if values.len() > MAX_PATTERN_COMPONENTS {
            return;
        }
        if !self.is_pattern() {
            self.space = Some(Arc::new(ColorSpace::Pattern(Box::default())));
        }
        self.pattern = Some(Box::new(PatternValue {
            name,
            components: SmallVec::from_slice(values),
            loaded,
        }));
    }

    /// The colour to paint with, or `None` when the space cannot produce one
    /// — a `/None` separation, an out-of-range palette index, or a pattern.
    #[must_use]
    pub fn to_rgb(&self) -> Option<Rgb> {
        let space = self.space.as_ref()?;
        if let ColorSpace::Pattern(pattern_space) = &**space {
            let value = self.pattern.as_ref()?;
            return pattern_space.to_rgb(&value.components);
        }
        space.try_to_rgb(&self.components)
    }

    /// Whether this colour comes from a pattern.
    #[must_use]
    pub fn is_pattern(&self) -> bool {
        self.space
            .as_ref()
            .is_some_and(|s| matches!(**s, ColorSpace::Pattern(_)))
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

    use super::{ColorSpace, ColorValue, SetComponentsError};
    use pdfrum_object::Name;
    use smallvec::SmallVec;
    use std::sync::Arc;

    #[test]
    fn the_initial_colour_is_black_in_device_gray() {
        let c = ColorValue::default();
        assert_eq!(&c.components[..], &[0.0]);
        assert!(c.space.is_none());
    }

    #[test]
    fn installing_a_space_resets_the_components() {
        let mut c = ColorValue::default();
        c.set_stock(ColorSpace::DeviceRgb, &[1.0, 0.5, 0.25]);
        assert_eq!(&c.components[..], &[1.0, 0.5, 0.25]);
        // `cs` back to gray discards the RGB colour entirely.
        c.set_space(Arc::new(ColorSpace::DeviceGray));
        assert_eq!(&c.components[..], &[0.0]);
    }

    #[test]
    fn a_pattern_space_refuses_component_operands() {
        let mut c = ColorValue::default();
        c.set_space(Arc::new(ColorSpace::Pattern(Box::default())));
        let before = c.components.clone();
        assert_eq!(
            c.set_components(&[0.5]),
            Err(SetComponentsError::PatternSpace)
        );
        assert_eq!(c.components, before);
    }

    #[test]
    fn too_few_components_change_nothing_at_all() {
        let mut c = ColorValue::default();
        c.set_stock(ColorSpace::DeviceCmyk, &[0.1, 0.2, 0.3, 0.4]);
        assert_eq!(
            c.set_components(&[0.9, 0.9]),
            Err(SetComponentsError::TooFew { needed: 4, got: 2 })
        );
        assert_eq!(&c.components[..], &[0.1, 0.2, 0.3, 0.4]);
        // Exactly enough does change it.
        assert!(c.set_components(&[0.5, 0.5, 0.5, 0.5]).is_ok());
        assert_eq!(&c.components[..], &[0.5, 0.5, 0.5, 0.5]);
    }

    #[test]
    fn components_with_no_space_install_device_gray() {
        let mut c = ColorValue {
            space: None,
            components: SmallVec::new(),
            pattern: None,
        };
        assert!(c.set_components(&[0.75]).is_ok());
        assert_eq!(
            c.space.as_deref(),
            Some(&ColorSpace::DeviceGray),
            "an unset space becomes DeviceGray"
        );
    }

    #[test]
    fn a_pattern_operand_vector_over_sixteen_is_refused_outright() {
        // `SetValueForPattern` returns *before* touching anything, so the
        // name is not replaced either — not merely the components.
        let mut c = ColorValue::default();
        c.set_space(Arc::new(ColorSpace::Pattern(Box::default())));
        c.set_pattern(Name::from("P0"), &[0.5, 0.25], None);
        c.set_pattern(Name::from("P1"), &[1.0; 17], None);
        let p = c.pattern.as_ref().expect("pattern");
        assert_eq!(p.name.as_bytes(), b"P0", "the whole call is refused");
        assert_eq!(&p.components[..], &[0.5, 0.25]);
    }

    #[test]
    fn installing_a_pattern_installs_the_pattern_space() {
        // A `/P1 scn` with no preceding `/Pattern cs` is still a pattern
        // colour, because `SetValueForPattern` installs the stock space when
        // the current one is not already a pattern space. Leaving the space
        // alone resolves the operands through `DeviceGray` and paints the
        // object solid black — which on a page-sized rectangle is the whole
        // page, and is what four of the corpus's fuzz files exercise.
        let mut c = ColorValue::default();
        assert!(!c.is_pattern());
        c.set_pattern(Name::from("P1"), &[], None);
        assert!(c.is_pattern(), "the stock /Pattern space is installed");
        // And a pattern space that already has a base keeps it: the C++ only
        // installs the stock space when the current one is not a pattern.
        let mut with_base = ColorValue::default();
        with_base.set_space(Arc::new(ColorSpace::Pattern(Box::new(
            super::super::PatternSpace {
                base: Some(Box::new(ColorSpace::DeviceRgb)),
            },
        ))));
        with_base.set_pattern(Name::from("P1"), &[1.0, 0.0, 0.0], None);
        assert_eq!(
            with_base.to_rgb().map(super::Rgb::to_bytes),
            Some([255, 0, 0])
        );
    }

    #[test]
    fn a_pattern_colour_resolves_through_its_base_space() {
        let mut c = ColorValue::default();
        c.set_space(Arc::new(ColorSpace::Pattern(Box::new(
            super::super::PatternSpace {
                base: Some(Box::new(ColorSpace::DeviceGray)),
            },
        ))));
        assert!(c.is_pattern());
        // No pattern installed yet: no colour.
        assert!(c.to_rgb().is_none());
        c.set_pattern(Name::from("P0"), &[0.5], None);
        let rgb = c.to_rgb().expect("colour");
        assert!((rgb.r - 0.5).abs() < 1e-6);
    }
}
