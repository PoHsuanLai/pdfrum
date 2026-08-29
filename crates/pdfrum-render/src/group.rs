//! Transparency groups: when an offscreen buffer is forced, and what happens
//! to it (`ProcessTransparency`, `cpdf_renderstatus.cpp:628-753`).
//!
//! The single most consequential function in the module, and the one whose
//! bail-out predicate decides whether a form draws directly or through a
//! whole extra buffer. What is *not* in that predicate matters as much as
//! what is: a non-isolated group with `/Group` present but `/I` absent, `ca`
//! at one and no soft mask draws **directly**, with no group semantics at
//! all.

use pdfrum_page::{BlendMode, GraphicsState, PageObject, Transparency};

/// Everything the offscreen decision reads, gathered from one object.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GroupInputs {
    /// Whether the object carries an `/ExtGState` soft mask that survives.
    pub has_soft_mask: bool,
    /// A form's own `ca`, used as the *group* alpha.
    pub group_alpha: f32,
    /// The blend mode in force.
    pub blend: BlendMode,
    /// Whether the object is a form whose own group is isolated.
    pub group_is_isolated: bool,
    /// The enclosing state's fill alpha, non-one only when a form inherited
    /// one — in practice the type-3 and pattern paths.
    pub initial_alpha: f32,
}

impl GroupInputs {
    /// Read the inputs off a page object.
    ///
    /// An image's own `/SMask` **wins over** the graphics state's: when both
    /// are present the state's is dropped, because the image already carries
    /// its mask through its own decode.
    #[must_use]
    pub fn of(object: &PageObject, initial_alpha: f32) -> Self {
        let state = object.state();
        let image_has_own_mask = match object {
            PageObject::Image(img) => img.object.image.mask.is_some(),
            _ => false,
        };
        let (group_alpha, group_is_isolated) = match object {
            PageObject::Form(form) => (state.general.fill_alpha, form.object.transparency.isolated),
            _ => (1.0, false),
        };
        Self {
            has_soft_mask: state.general.soft_mask.is_some() && !image_has_own_mask,
            group_alpha,
            blend: state.general.blend,
            group_is_isolated,
            initial_alpha,
        }
    }
}

/// Whether the object must be rendered through an offscreen buffer
/// (`cpdf_renderstatus.cpp:655-657`, negated).
///
/// The text-clip clause of the original predicate is a constant `false` for
/// us: it requires `!RenderCapSoftClip()`, and a bitmap device supports soft
/// clipping, so it never fires.
#[must_use]
#[expect(
    clippy::float_cmp,
    reason = "an exact `== 1.0f` is the upstream predicate: an alpha even one \
              ulp below opaque takes the offscreen path there, and an epsilon \
              here would silently take the other branch for those files"
)]
pub fn needs_offscreen(inputs: GroupInputs) -> bool {
    !(!inputs.has_soft_mask
        && inputs.group_alpha == 1.0
        && matches!(inputs.blend, BlendMode::Normal | BlendMode::Compatible)
        && !inputs.group_is_isolated
        && inputs.initial_alpha == 1.0)
}

/// Whether a group's buffer starts from the page's existing content rather
/// than from transparency.
///
/// **Isolated means transparent; non-isolated means a copy of what is already
/// there.** PDFium never removes that copy before compositing the group back,
/// so a non-isolated group under a non-Normal blend counts its backdrop
/// twice. That is not what ISO 32000 §11.4.6 specifies, and it is what the
/// oracle does; parity wins.
#[must_use]
pub fn needs_backdrop(transparency: Transparency) -> bool {
    !transparency.isolated
}

/// The order the mask and the two alphas are applied to a finished group
/// buffer (`cpdf_renderstatus.cpp:728-745`).
///
/// Named as a type rather than left implicit because the *order* is the
/// contract: the soft mask multiplies the alpha channel first, then the
/// group alpha, then — only outside an enclosing group — the inherited one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GroupFinish {
    /// The group's own `ca`, applied when the form declares a group.
    pub group_alpha: Option<f32>,
    /// The inherited alpha, applied only when not already inside a group.
    pub initial_alpha: Option<f32>,
}

impl GroupFinish {
    /// Work out which alphas apply.
    #[must_use]
    #[expect(
        clippy::float_cmp,
        reason = "an exact `!= 1.0f` is the upstream guard; an epsilon would \
                  skip the alpha multiply for a near-opaque value PDFium applies"
    )]
    pub fn of(inputs: GroupInputs, transparency: Transparency, in_group: bool) -> Self {
        Self {
            group_alpha: transparency.group.then_some(inputs.group_alpha),
            // The `!in_group` guard is what stops a nested group re-applying
            // the enclosing group's alpha.
            initial_alpha: (inputs.initial_alpha != 1.0 && !in_group)
                .then_some(inputs.initial_alpha),
        }
    }

    /// Apply the alphas to a group's pixels, in order.
    pub fn apply(&self, pixmap: &mut crate::pixmap::Pixmap) {
        if let Some(a) = self.group_alpha {
            pixmap.multiply_alpha(a);
        }
        if let Some(a) = self.initial_alpha {
            pixmap.multiply_alpha(a);
        }
    }
}

/// The transparency a finished group composites back under
/// (`cpdf_renderstatus.cpp:746-752`).
///
/// The **enclosing** transparency, with `group` forced on for a form — *not*
/// the form's own group flags. The isolated bit therefore comes from the
/// page, which sets it unconditionally, so it is almost always true at the
/// top level.
#[must_use]
pub fn composite_transparency(enclosing: Transparency, is_form: bool) -> Transparency {
    Transparency {
        group: enclosing.group || is_form,
        ..enclosing
    }
}

/// The graphics state a form's own content starts from.
///
/// Entering a transparency group clears the blend mode, both alphas and the
/// soft mask, because all four have already been consumed by the group's own
/// compositing — applying them again inside would double-count.
#[must_use]
pub fn state_inside_group(state: &GraphicsState) -> GraphicsState {
    let mut inner = state.clone();
    inner.general.enter_transparency_group();
    inner
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain() -> GroupInputs {
        GroupInputs {
            has_soft_mask: false,
            group_alpha: 1.0,
            blend: BlendMode::Normal,
            group_is_isolated: false,
            initial_alpha: 1.0,
        }
    }

    #[test]
    fn needs_offscreen_predicate_all_clauses() {
        // The baseline draws directly.
        assert!(!needs_offscreen(plain()));
        // Each clause independently forces a buffer.
        assert!(needs_offscreen(GroupInputs {
            has_soft_mask: true,
            ..plain()
        }));
        assert!(needs_offscreen(GroupInputs {
            group_alpha: 0.5,
            ..plain()
        }));
        assert!(needs_offscreen(GroupInputs {
            blend: BlendMode::Multiply,
            ..plain()
        }));
        assert!(needs_offscreen(GroupInputs {
            group_is_isolated: true,
            ..plain()
        }));
        assert!(needs_offscreen(GroupInputs {
            initial_alpha: 0.5,
            ..plain()
        }));
    }

    #[test]
    fn a_non_isolated_group_with_nothing_else_draws_directly() {
        // The clause that surprises: `/Group` present but `/I` absent, `ca`
        // one, no mask, Normal blend -> no group semantics whatsoever.
        assert!(!needs_offscreen(GroupInputs {
            group_is_isolated: false,
            ..plain()
        }));
    }

    #[test]
    fn compatible_counts_as_normal() {
        assert!(!needs_offscreen(GroupInputs {
            blend: BlendMode::Compatible,
            ..plain()
        }));
    }

    #[test]
    fn isolated_starts_transparent_non_isolated_copies_the_backdrop() {
        assert!(!needs_backdrop(Transparency {
            isolated: true,
            ..Transparency::default()
        }));
        assert!(needs_backdrop(Transparency {
            isolated: false,
            ..Transparency::default()
        }));
    }

    #[test]
    fn mask_then_group_alpha_then_initial_alpha_order() {
        let inputs = GroupInputs {
            group_alpha: 0.5,
            initial_alpha: 0.5,
            ..plain()
        };
        let group = Transparency {
            group: true,
            ..Transparency::default()
        };
        let finish = GroupFinish::of(inputs, group, false);
        assert_eq!(finish.group_alpha, Some(0.5));
        assert_eq!(finish.initial_alpha, Some(0.5));

        // Inside an enclosing group the inherited alpha is *not* re-applied.
        let nested = GroupFinish::of(inputs, group, true);
        assert_eq!(nested.group_alpha, Some(0.5));
        assert_eq!(nested.initial_alpha, None);
    }

    #[test]
    fn a_non_group_form_skips_the_group_alpha() {
        let inputs = GroupInputs {
            group_alpha: 0.5,
            ..plain()
        };
        let finish = GroupFinish::of(inputs, Transparency::default(), false);
        assert_eq!(finish.group_alpha, None);
    }

    #[test]
    fn alphas_apply_in_sequence_and_truncate() {
        let finish = GroupFinish {
            group_alpha: Some(0.5),
            initial_alpha: Some(0.5),
        };
        let mut p =
            crate::pixmap::Pixmap::filled(1, 1, peniko::Color::from_rgba8(255, 255, 255, 255));
        finish.apply(&mut p);
        // 255 * 127/255 = 127, then 127 * 127/255 = 63.
        assert_eq!(p.pixel(0, 0).map(|px| px[3]), Some(63));
    }

    #[test]
    fn a_form_composites_under_the_enclosing_transparency_with_group_forced() {
        let enclosing = Transparency {
            group: false,
            isolated: true,
            knockout: false,
        };
        let out = composite_transparency(enclosing, true);
        assert!(out.group, "a form forces the group bit on");
        assert!(out.isolated, "but the isolated bit stays the enclosing one");
    }

    #[test]
    #[expect(
        clippy::float_cmp,
        reason = "entering a group writes the literal 1.0; exact equality is \
                  what pins that the alpha was reset rather than scaled"
    )]
    fn entering_a_group_clears_what_it_already_consumed() {
        let mut state = GraphicsState::default();
        state.general.fill_alpha = 0.25;
        state.general.blend = BlendMode::Multiply;
        let inner = state_inside_group(&state);
        assert_eq!(inner.general.fill_alpha, 1.0);
        assert_eq!(inner.general.blend, BlendMode::Normal);
        assert!(inner.general.soft_mask.is_none());
    }
}
