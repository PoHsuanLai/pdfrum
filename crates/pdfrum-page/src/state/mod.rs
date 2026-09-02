//! The graphics state and its stack (ISO 32000-1 §8.4).
//!
//! One record with the heavy members behind `Arc`, so `q` is a cheap clone
//! and `Q` is a pop. PDFium spreads the same data over five copy-on-write
//! classes; the sharing is the point, not the class structure, so the `Arc`s
//! reproduce it without the ceremony.
//!
//! `Q` on an empty stack is a **no-op**, not an error — an unbalanced content
//! stream simply keeps its current state.

mod clip;
mod extgstate;
mod general;
mod graph;
mod marks;
mod text;

pub use clip::{ClipEntry, ClipRule, ClipStack, MAX_TEXT_OBJECTS, TextClipLimit, TextClipRun};
pub use extgstate::apply_ext_gstate;
pub(crate) use general::RenderIntent;
pub use general::{BlendMode, GeneralState};
pub use graph::StrokeParams;
pub use marks::{ContentMarks, Mark};
pub use text::TextState;
pub(crate) use text::{TextCursor, glyph_matrix, kerning_shift};

use crate::color::ColorValue;
use kurbo::Affine;

/// Everything `q` saves and `Q` restores.
///
/// Cloning is cheap: the clip stack's paths and the soft mask are shared, and
/// the rest is a handful of scalars.
#[derive(Debug, Clone, PartialEq)]
pub struct GraphicsState {
    /// The current transformation matrix.
    pub ctm: Affine,
    /// The non-stroking colour.
    pub fill: ColorValue,
    /// The stroking colour.
    pub stroke: ColorValue,
    /// How paths are stroked.
    pub stroke_params: StrokeParams,
    /// The clipping path.
    pub clip: ClipStack,
    /// Alphas, blend mode, soft mask and the inert `/ExtGState` keys.
    pub general: GeneralState,
    /// The text-showing parameters.
    pub text: TextState,
}

impl Default for GraphicsState {
    /// A page's initial state: identity transform, **opaque black fill and
    /// stroke in `DeviceGray`**, no clip, fully opaque.
    fn default() -> Self {
        Self {
            ctm: Affine::IDENTITY,
            fill: ColorValue::default(),
            stroke: ColorValue::default(),
            stroke_params: StrokeParams::default(),
            clip: ClipStack::new(),
            general: GeneralState::default(),
            text: TextState::default(),
        }
    }
}

/// The `q`/`Q` stack.
///
/// Unbounded, matching PDFium — a content stream may nest as deeply as it
/// likes, and the memory cost is bounded by the stream's own length.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StateStack {
    saved: Vec<GraphicsState>,
}

impl StateStack {
    /// An empty stack.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// How deep the stack is.
    #[must_use]
    pub fn depth(&self) -> usize {
        self.saved.len()
    }

    /// Whether nothing has been saved.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.saved.is_empty()
    }

    /// `q`: save a copy of `state`.
    pub fn push(&mut self, state: &GraphicsState) {
        self.saved.push(state.clone());
    }

    /// `Q`: restore into `state`.
    ///
    /// Answers whether anything was restored — a question, not a failed
    /// mutation. An empty stack is a **no-op**, which is what keeps an
    /// unbalanced stream rendering.
    pub fn pop(&mut self, state: &mut GraphicsState) -> bool {
        match self.saved.pop() {
            Some(saved) => {
                *state = saved;
                true
            }
            None => false,
        }
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

    use super::{GraphicsState, StateStack};
    use crate::color::ColorSpace;
    use crate::ops::LineCap;
    use kurbo::Affine;

    #[test]
    fn the_initial_state_is_opaque_black_and_untransformed() {
        let s = GraphicsState::default();
        assert_eq!(s.ctm, Affine::IDENTITY);
        assert_eq!(&s.fill.components[..], &[0.0]);
        assert_eq!(&s.stroke.components[..], &[0.0]);
        assert!((s.general.fill_alpha - 1.0).abs() < 1e-6);
        assert!(s.clip.is_empty());
        assert!((s.stroke_params.width - 1.0).abs() < 1e-6);
    }

    #[test]
    fn save_and_restore_round_trip() {
        let mut stack = StateStack::new();
        let mut state = GraphicsState::default();
        stack.push(&state);
        assert_eq!(stack.depth(), 1);

        state.ctm = Affine::translate((10.0, 20.0));
        state.stroke_params.cap = LineCap::Round;
        state
            .fill
            .set_stock(ColorSpace::DeviceRgb, &[1.0, 0.0, 0.0]);

        assert!(stack.pop(&mut state));
        assert_eq!(state.ctm, Affine::IDENTITY);
        assert_eq!(state.stroke_params.cap, LineCap::Butt);
        assert_eq!(&state.fill.components[..], &[0.0]);
        assert!(stack.is_empty());
    }

    #[test]
    fn restoring_an_empty_stack_changes_nothing() {
        let mut stack = StateStack::new();
        let mut state = GraphicsState {
            ctm: Affine::scale(2.0),
            ..GraphicsState::default()
        };
        assert!(!stack.pop(&mut state));
        // The state is untouched: an unbalanced `Q` is harmless.
        assert_eq!(state.ctm, Affine::scale(2.0));
    }

    #[test]
    fn the_stack_nests_without_a_cap() {
        let mut stack = StateStack::new();
        let state = GraphicsState::default();
        for _ in 0..1000 {
            stack.push(&state);
        }
        assert_eq!(stack.depth(), 1000);
    }
}
