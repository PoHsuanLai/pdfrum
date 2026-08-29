//! Multiple Master: turning design coordinates into a weight vector.
//!
//! A Multiple-Master Type 1 font is *n* complete outline sets ("masters")
//! stored interleaved, plus a rule for mixing them. The rule has three parts,
//! all declared in the cleartext preamble:
//!
//! - `/BlendAxisTypes [/Weight /Width]` names the *k* design axes.
//! - `/BlendDesignPositions [[0 0][1 0][0 1][1 1]]` places each master in the
//!   *k*-dimensional unit cube — one row per master, and for a well-formed
//!   font the rows are exactly the 2ᵏ cube corners.
//! - `/BlendDesignMap [[[50 0][1450 1]] …]` maps each axis's *design* units
//!   (a weight of 50–1450, a width of 100–900) onto that unit interval, as a
//!   piecewise-linear curve given by its knots.
//!
//! From a design coordinate the blend runs: map through the design map to
//! normalized [0,1] per axis, then compute each master's weight as the product
//! over axes of `n` or `1-n` according to that master's corner. Those products
//! sum to 1 by construction, which is what makes them a partition of the
//! outline. The charstring interpreter then consumes the vector through
//! `callothersubr` 14–18.
//!
//! This is the only part of Type 1 that `read_fonts::ps::type1` does not cover
//! — it honours the `/WeightVector` a font ships with but exposes no way to
//! compute a different one, and parses none of the three declarations above.
//! Everything here exists to answer PDFium's `AdjustVariationParams`
//! (`core/fxge/cfx_face.cpp`), which drives the two Foxit fallback faces
//! entirely through design coordinates.

use pdfrum_common::{DiagKind, Diagnostics, Severity};

/// What a design axis controls. Type 1 names these with PostScript literals;
/// only the two the Foxit faces use have dedicated variants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AxisKind {
    /// `/Weight` — stroke thickness. PDFium's axis 0.
    Weight,
    /// `/Width` — horizontal proportion. PDFium's axis 1.
    Width,
    /// `/OpticalSize`, `/Style`, or anything else a font names.
    Other(Box<str>),
}

/// One design axis, in the design units a caller thinks in.
///
/// `min`/`max` come from the first and last knot of this axis's design map,
/// which is where FreeType's `FT_Get_MM_Var` gets them too — so the values a
/// caller compares against are the same ones PDFium reads out of
/// `FT_MM_Var::axis`.
#[derive(Debug, Clone, PartialEq)]
pub struct MmAxis {
    /// Smallest design coordinate the font interpolates.
    pub min: f32,
    /// The coordinate the font's own `/WeightVector` corresponds to.
    pub default: f32,
    /// Largest design coordinate the font interpolates.
    pub max: f32,
    /// What the axis controls.
    pub kind: AxisKind,
}

/// One axis's design-unit → normalized-position curve, as its knots.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DesignMap {
    /// `(design, normalized)` pairs, in ascending design order.
    pub knots: Vec<(f32, f32)>,
}

impl DesignMap {
    /// Piecewise-linear evaluation, clamped at both ends.
    fn map(&self, design: f32) -> f32 {
        let Some(&(first_d, first_n)) = self.knots.first() else {
            return 0.0;
        };
        if design <= first_d {
            return first_n;
        }
        for pair in self.knots.windows(2) {
            let (Some(&(d0, n0)), Some(&(d1, n1))) = (pair.first(), pair.get(1)) else {
                break;
            };
            if design <= d1 {
                let span = d1 - d0;
                if span.abs() < f32::EPSILON {
                    return n1;
                }
                return n0 + (n1 - n0) * (design - d0) / span;
            }
        }
        self.knots.last().map_or(0.0, |&(_, n)| n)
    }
}

/// Everything a font declared about its Multiple-Master structure.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Blend {
    /// One per axis.
    pub axes: Vec<MmAxis>,
    /// One row per master, each row one normalized coordinate per axis.
    pub design_positions: Vec<Vec<f32>>,
    /// One per axis.
    pub design_maps: Vec<DesignMap>,
    /// The weight vector the font shipped with — the blend at the default
    /// design coordinates, and what `read_fonts` would use.
    pub default_weights: Vec<f32>,
}

impl Blend {
    /// Assemble from the four parsed declarations, rejecting the combination
    /// if they disagree.
    ///
    /// A Type 1 font that says it has two axes, four masters and three design
    /// positions is not a font we can interpolate; treating it as
    /// non-variable and drawing its default master is strictly better than
    /// blending garbage, and is what FreeType does too.
    pub fn new(
        axis_kinds: Vec<AxisKind>,
        design_positions: Vec<Vec<f32>>,
        design_maps: Vec<DesignMap>,
        default_weights: Vec<f32>,
        diags: &mut Diagnostics,
    ) -> Option<Self> {
        let num_axes = axis_kinds.len();
        let num_masters = default_weights.len();
        let consistent = num_axes > 0
            && num_masters > 1
            && design_maps.len() == num_axes
            && design_positions.len() == num_masters
            && design_positions.iter().all(|row| row.len() == num_axes)
            && design_maps.iter().all(|m| m.knots.len() >= 2);
        if !consistent {
            diags.record(Severity::Suspicious, DiagKind::Type1BlendInconsistent, None);
            return None;
        }

        // Design-space min/max come from the outer knots. The default is the
        // design coordinate whose blend reproduces the shipped weight vector;
        // for the axis-aligned cube layout every real MM font uses, that is
        // recoverable by projecting the weight vector onto the axis.
        let axes = axis_kinds
            .into_iter()
            .enumerate()
            .map(|(i, kind)| {
                let map = design_maps.get(i);
                let knots = map.map_or(&[][..], |m| m.knots.as_slice());
                let (min, max) = (
                    knots.first().map_or(0.0, |k| k.0),
                    knots.last().map_or(1.0, |k| k.0),
                );
                let normalized_default = default_normalized(&design_positions, &default_weights, i);
                MmAxis {
                    min,
                    max,
                    default: map.map_or(normalized_default, |m| unmap(m, normalized_default)),
                    kind,
                }
            })
            .collect();

        Some(Self {
            axes,
            design_positions,
            design_maps,
            default_weights,
        })
    }

    /// The weight vector for a set of design coordinates.
    ///
    /// Coordinates are clamped to each axis's range, and a short `coords`
    /// leaves the remaining axes at their defaults — both because a caller
    /// asking for one axis of a two-axis font is the common case
    /// (`AdjustVariationParams` varies weight alone whenever `dest_width` is
    /// zero) and because refusing would mean no glyph at all.
    pub fn weights_for(&self, coords: &[f32]) -> Vec<f32> {
        let normalized: Vec<f32> = self
            .axes
            .iter()
            .enumerate()
            .map(|(i, axis)| {
                let design = coords
                    .get(i)
                    .copied()
                    .unwrap_or(axis.default)
                    .clamp(axis.min.min(axis.max), axis.max.max(axis.min));
                self.design_maps.get(i).map_or(0.0, |m| m.map(design))
            })
            .collect();

        self.design_positions
            .iter()
            .map(|corner| {
                corner
                    .iter()
                    .zip(&normalized)
                    .map(|(&c, &n)| if c > 0.5 { n } else { 1.0 - n })
                    .product()
            })
            .collect()
    }
}

/// Invert a design map: find the design coordinate whose normalized value is
/// `target`. Used only to report an axis default in design units.
fn unmap(map: &DesignMap, target: f32) -> f32 {
    let Some(&(first_d, first_n)) = map.knots.first() else {
        return 0.0;
    };
    if target <= first_n {
        return first_d;
    }
    for pair in map.knots.windows(2) {
        let (Some(&(d0, n0)), Some(&(d1, n1))) = (pair.first(), pair.get(1)) else {
            break;
        };
        if target <= n1 {
            let span = n1 - n0;
            if span.abs() < f32::EPSILON {
                return d1;
            }
            return d0 + (d1 - d0) * (target - n0) / span;
        }
    }
    map.knots.last().map_or(0.0, |&(d, _)| d)
}

/// Recover axis `i`'s normalized default from the shipped weight vector: sum
/// the weights of every master sitting at the axis's high end.
///
/// For the cube layout (`[[0 0][1 0][0 1][1 1]]`) this is exact — the weight
/// of a corner is the product over axes, so summing the high-side corners of
/// one axis factors out to that axis's normalized coordinate.
fn default_normalized(positions: &[Vec<f32>], weights: &[f32], axis: usize) -> f32 {
    positions
        .iter()
        .zip(weights)
        .filter(|(corner, _)| corner.get(axis).is_some_and(|&c| c > 0.5))
        .map(|(_, &w)| w)
        .sum::<f32>()
        .clamp(0.0, 1.0)
}

#[cfg(test)]
#[allow(
    clippy::indexing_slicing,
    clippy::float_cmp,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::similar_names
)]
mod tests {
    use super::{AxisKind, Blend, DesignMap};
    use pdfrum_common::{DiagKind, Diagnostics};

    /// The Foxit Sans MM declaration, verbatim.
    fn foxit_sans() -> Blend {
        let mut d = Diagnostics::default();
        Blend::new(
            vec![AxisKind::Weight, AxisKind::Width],
            vec![
                vec![0.0, 0.0],
                vec![1.0, 0.0],
                vec![0.0, 1.0],
                vec![1.0, 1.0],
            ],
            vec![
                DesignMap {
                    knots: vec![(50.0, 0.0), (1450.0, 1.0)],
                },
                DesignMap {
                    knots: vec![(50.0, 0.0), (1450.0, 1.0)],
                },
            ],
            vec![0.3158, 0.1349, 0.3849, 0.1644],
            &mut d,
        )
        .expect("consistent declaration")
    }

    #[test]
    fn corners_select_a_single_master() {
        let b = foxit_sans();
        assert_eq!(b.weights_for(&[50.0, 50.0]), vec![1.0, 0.0, 0.0, 0.0]);
        assert_eq!(b.weights_for(&[1450.0, 50.0]), vec![0.0, 1.0, 0.0, 0.0]);
        assert_eq!(b.weights_for(&[50.0, 1450.0]), vec![0.0, 0.0, 1.0, 0.0]);
        assert_eq!(b.weights_for(&[1450.0, 1450.0]), vec![0.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn weights_always_partition_unity() {
        let b = foxit_sans();
        for w in [50.0f32, 300.0, 750.0, 1200.0, 1450.0] {
            for x in [50.0f32, 400.0, 900.0, 1450.0] {
                let sum: f32 = b.weights_for(&[w, x]).iter().sum();
                assert!((sum - 1.0).abs() < 1e-5, "w={w} x={x} sum={sum}");
            }
        }
    }

    #[test]
    fn the_default_coordinates_reproduce_the_shipped_vector() {
        // The round trip that makes `MmAxis::default` meaningful: blending at
        // the reported defaults must return the font's own `/WeightVector`.
        let b = foxit_sans();
        let defaults: Vec<f32> = b.axes.iter().map(|a| a.default).collect();
        for (got, want) in b.weights_for(&defaults).iter().zip(&b.default_weights) {
            assert!((got - want).abs() < 1e-4, "{got} vs {want}");
        }
    }

    #[test]
    fn axis_ranges_come_from_the_design_map() {
        let b = foxit_sans();
        assert_eq!(b.axes.len(), 2);
        assert_eq!((b.axes[0].min, b.axes[0].max), (50.0, 1450.0));
        assert_eq!(b.axes[0].kind, AxisKind::Weight);
        assert_eq!(b.axes[1].kind, AxisKind::Width);
    }

    #[test]
    fn out_of_range_coordinates_clamp() {
        let b = foxit_sans();
        assert_eq!(
            b.weights_for(&[-999.0, -999.0]),
            b.weights_for(&[50.0, 50.0])
        );
        assert_eq!(b.weights_for(&[9e9, 9e9]), b.weights_for(&[1450.0, 1450.0]));
    }

    #[test]
    fn a_short_coordinate_list_leaves_the_rest_at_default() {
        let b = foxit_sans();
        let full = b.weights_for(&[1450.0, b.axes[1].default]);
        assert_eq!(b.weights_for(&[1450.0]), full);
    }

    #[test]
    fn inconsistent_declarations_are_refused_with_a_diagnostic() {
        let mut d = Diagnostics::default();
        // Two axes claimed, but only three of the four masters positioned.
        assert!(
            Blend::new(
                vec![AxisKind::Weight, AxisKind::Width],
                vec![vec![0.0, 0.0], vec![1.0, 0.0], vec![0.0, 1.0]],
                vec![
                    DesignMap {
                        knots: vec![(0.0, 0.0), (1.0, 1.0)]
                    },
                    DesignMap {
                        knots: vec![(0.0, 0.0), (1.0, 1.0)]
                    },
                ],
                vec![0.25, 0.25, 0.25, 0.25],
                &mut d,
            )
            .is_none()
        );
        assert!(d.contains(&DiagKind::Type1BlendInconsistent));
    }

    #[test]
    fn a_three_knot_design_map_is_piecewise() {
        let mut d = Diagnostics::default();
        // A weight axis whose middle is deliberately off-centre.
        let b = Blend::new(
            vec![AxisKind::Weight],
            vec![vec![0.0], vec![1.0]],
            vec![DesignMap {
                knots: vec![(100.0, 0.0), (400.0, 0.75), (900.0, 1.0)],
            }],
            vec![0.5, 0.5],
            &mut d,
        )
        .expect("consistent");
        // At the middle knot the normalized value is 0.75, not 0.5.
        let w = b.weights_for(&[400.0]);
        assert!((w[1] - 0.75).abs() < 1e-6, "{w:?}");
        // Halfway along the first segment.
        let w = b.weights_for(&[250.0]);
        assert!((w[1] - 0.375).abs() < 1e-6, "{w:?}");
    }
}
