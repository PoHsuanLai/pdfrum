//! Patterns, tiling and shading (ISO 32000-1 §8.7.3).
//!
//! # The matrix rule
//!
//! A pattern's own `/Matrix` composes with the **parent matrix** — the
//! form's or page's coordinate system — not with the current transformation
//! matrix. Patterns are anchored to the space they were declared in, so
//! translating the CTM before filling does not slide the tiling. This is a
//! frequent source of bugs and is worth restating whenever it comes up.
//!
//! In C++ notation the composition is `pattern_matrix * parent_matrix`;
//! `CFX_Matrix` multiplies left-to-right where `kurbo::Affine` multiplies
//! right-to-left, so the code writes `parent * pattern` (design brief D14).
//!
//! One asymmetry to know: a **bare shading** reached through the `sh`
//! operator ignores any `/Matrix` on its dictionary entirely, because the C++
//! only composes the matrix for a pattern, not for a shading object.

mod tiling;

pub use tiling::{TileRange, TilingPattern};

use crate::color::ColorSpace;
use crate::function::FunctionCache;
use crate::names;
use crate::shading::Shading;
use kurbo::Affine;
use pdfrum_common::{Diagnostics, Limits};
use pdfrum_object::{Dict, Object, Resolve};
use std::sync::Arc;

/// A loaded pattern.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Pattern {
    /// `/PatternType 1`: a content stream tiled across the fill.
    Tiling(Box<TilingPattern>),
    /// `/PatternType 2`: a shading painted across the fill.
    Shading(Box<ShadingPattern>),
}

/// A `/PatternType 2` pattern: a shading plus the matrix placing it.
#[derive(Debug, Clone, PartialEq)]
pub struct ShadingPattern {
    /// The shading itself.
    pub shading: Arc<Shading>,
    /// The pattern's own space composed with the parent matrix.
    pub matrix: Affine,
    /// The pattern's `/ExtGState`, which a shading pattern may carry.
    pub ext_g_state: Option<Dict>,
}

impl Pattern {
    /// The matrix taking the pattern's own space to the parent's.
    #[must_use]
    pub fn matrix(&self) -> Affine {
        match self {
            Self::Tiling(p) => p.matrix,
            Self::Shading(p) => p.matrix,
        }
    }

    /// Load a pattern from the object a `/Pattern` resource names.
    ///
    /// `parent_matrix` is the form's or page's coordinate system, **not** the
    /// current transformation matrix. Dispatch is on `/PatternType`: 1 is
    /// tiling, 2 is shading, and anything else — including a missing key —
    /// yields no pattern at all.
    #[must_use]
    pub fn load<R: Resolve>(
        obj: &Object,
        parent_matrix: Affine,
        resources: Option<&Dict>,
        r: &R,
        functions: &mut FunctionCache,
        limits: &Limits,
        diags: &mut Diagnostics,
    ) -> Option<Self> {
        let resolved = obj.resolve(r).ok()?;
        let dict = match &*resolved {
            Object::Dict(d) => d.clone(),
            Object::Stream(s) => s.dict.clone(),
            _ => return None,
        };
        // `/Matrix` must have exactly six elements or it reads as identity.
        let own = dict.matrix(names::MATRIX, r);
        let matrix = parent_matrix * own;

        match dict.int(names::PATTERN_TYPE, r)? {
            1 => {
                let stream = resolved.as_stream()?;
                Some(Self::Tiling(Box::new(TilingPattern::load(
                    stream, matrix, r, limits, diags,
                ))))
            }
            2 => {
                let shading_obj = dict.raw(names::SHADING)?;
                let shading = Shading::load(
                    shading_obj,
                    resources,
                    // A pattern is not a shading object, so `/Background`
                    // *is* honoured here.
                    false,
                    r,
                    functions,
                    limits,
                    diags,
                )?;
                Some(Self::Shading(Box::new(ShadingPattern {
                    shading: Arc::new(shading),
                    matrix,
                    ext_g_state: dict.dict(names::EXT_G_STATE, r),
                })))
            }
            _ => None,
        }
    }
}

/// The colour an uncoloured pattern paints with, and the fallbacks when none
/// resolves.
///
/// PDFium's two sentinels are worth naming: a **coloured** tiling pattern
/// whose colour will not resolve falls back to mid grey (`0xBFBFBF`), while
/// everything else falls back to white. The grey is what makes an unpainted
/// coloured tile visible rather than invisible.
#[must_use]
pub fn uncolored_pattern_rgb(
    space: &ColorSpace,
    components: &[f32],
    colored_tiling: bool,
) -> crate::color::Rgb {
    if let ColorSpace::Pattern(p) = space
        && let Some(rgb) = p.to_rgb(components, false)
    {
        return rgb;
    }
    if colored_tiling {
        // Mid grey, `0x00BFBFBF`.
        crate::color::Rgb {
            r: 191.0 / 255.0,
            g: 191.0 / 255.0,
            b: 191.0 / 255.0,
        }
    } else {
        crate::color::Rgb {
            r: 1.0,
            g: 1.0,
            b: 1.0,
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

    use super::{Pattern, uncolored_pattern_rgb};
    use crate::color::{ColorSpace, PatternSpace};
    use crate::function::FunctionCache;
    use kurbo::Affine;
    use pdfrum_common::{Diagnostics, Limits};
    use pdfrum_object::{Dict, Name, NoResolve, Object};

    fn load(dict: Dict, parent: Affine) -> Option<Pattern> {
        let mut funcs = FunctionCache::new();
        let mut diags = Diagnostics::default();
        Pattern::load(
            &Object::Dict(dict),
            parent,
            None,
            &NoResolve,
            &mut funcs,
            &Limits::default(),
            &mut diags,
        )
    }

    #[test]
    fn an_unknown_pattern_type_yields_no_pattern() {
        for kind in [0i64, 3, -1] {
            let dict = Dict::from_pairs([(Name::from("PatternType"), Object::Int(kind))]);
            assert!(load(dict, Affine::IDENTITY).is_none(), "type {kind}");
        }
        // A missing `/PatternType` likewise.
        assert!(load(Dict::new(), Affine::IDENTITY).is_none());
    }

    #[test]
    fn a_tiling_pattern_must_be_a_stream() {
        // A plain dictionary with `/PatternType 1` is not enough.
        let dict = Dict::from_pairs([(Name::from("PatternType"), Object::Int(1))]);
        assert!(load(dict, Affine::IDENTITY).is_none());
    }

    #[test]
    fn uncolored_fallbacks_differ_by_paint_type() {
        let no_base = ColorSpace::Pattern(Box::default());
        let grey = uncolored_pattern_rgb(&no_base, &[0.5], true);
        assert!((grey.r - 191.0 / 255.0).abs() < 1e-5);
        let white = uncolored_pattern_rgb(&no_base, &[0.5], false);
        assert!((white.r - 1.0).abs() < 1e-6);

        // With a base the operands resolve normally and no fallback applies.
        let with_base = ColorSpace::Pattern(Box::new(PatternSpace {
            base: Some(Box::new(ColorSpace::DeviceGray)),
        }));
        let resolved = uncolored_pattern_rgb(&with_base, &[0.25], true);
        assert!((resolved.r - 0.25).abs() < 1e-6);
    }
}
