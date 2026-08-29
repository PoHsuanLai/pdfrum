//! Applying an `/ExtGState` dictionary (ISO 32000-1 §8.4.5, table 58).
//!
//! Twenty-four keys, applied **in the dictionary's own order**, and a
//! null-or-unresolvable value skips its key entirely rather than clearing it.
//!
//! Four of the keys interact rather than standing alone, and each interaction
//! is a real file's behaviour:
//!
//! - **`/TR` is skipped when `/TR2` is present** in the same dictionary.
//! - **`/BG` is skipped when `/BG2` is present**, and `/UCR` when `/UCR2` is.
//! - **`/OP` also sets the non-stroking overprint flag unless `/op` is
//!   present** in the same dictionary.
//! - **`/Font` looks its name up in the `/Font` *resources***, not as an
//!   indirect font dictionary — so the spec's `[<ref> size]` form resolves
//!   the reference to an empty string and yields the fallback font.

use super::{BlendMode, GraphicsState, RenderIntent};
use crate::function::FunctionCache;
use crate::names;
use crate::transfer::TransferFunc;
use crate::transparency::SoftMask;
use kurbo::Affine;
use pdfrum_common::{Diagnostics, Limits};
use pdfrum_font::Font;
use pdfrum_object::{Dict, Object, Resolve};
use smallvec::SmallVec;
use std::sync::Arc;

/// Apply an `/ExtGState` dictionary to `state`.
///
/// One arm per key, in the dictionary's own order.
///
/// `find_font` resolves a `/Font` array's name through the `/Font` resources,
/// which is where the quirk in the module docs lives.
#[expect(
    clippy::too_many_lines,
    reason = "the key table is a flat dispatch by design: one arm per key"
)]
pub fn apply_ext_gstate<R: Resolve>(
    state: &mut GraphicsState,
    ext: &Dict,
    find_font: impl Fn(&[u8]) -> Option<Arc<Font>>,
    r: &R,
    functions: &mut FunctionCache,
    limits: &Limits,
    diags: &mut Diagnostics,
) {
    // Which keys the *whole dictionary* holds, since three of them are
    // suppressed by the presence of a sibling.
    let has_tr2 = ext.contains_key(names::TR2);
    let has_bg2 = ext.contains_key(names::BG2);
    let has_ucr2 = ext.contains_key(names::UCR2);
    let has_lower_op = ext.contains_key(names::OP_LOWER);

    for (key, raw) in ext.iter() {
        // A value that resolves to nothing skips its key entirely.
        let Ok(value) = raw.resolve(r) else {
            continue;
        };
        if value.is_null() {
            continue;
        }
        match key.as_bytes() {
            b"LW" => {
                if let Some(v) = value.number() {
                    state.stroke_params.width = v;
                }
            }
            b"LC" => {
                if let Some(v) = value.as_int() {
                    state.stroke_params.cap = crate::ops::LineCap::from_int(v);
                }
            }
            b"LJ" => {
                if let Some(v) = value.as_int() {
                    state.stroke_params.join = crate::ops::LineJoin::from_int(v);
                }
            }
            b"ML" => {
                if let Some(v) = value.number() {
                    state.stroke_params.miter_limit = v;
                }
            }
            b"D" => {
                // Must be an array whose **element 0 is itself an array**.
                if let Some(outer) = value.as_array()
                    && let Some(Object::Array(lengths)) = outer.raw_at(0)
                {
                    state.stroke_params.dash =
                        lengths.iter().map(|o| o.number().unwrap_or(0.0)).collect();
                    state.stroke_params.dash_phase = outer.number_at_or_zero(1);
                }
            }
            b"RI" => {
                state.general.render_intent = RenderIntent::from_name(&value.to_byte_string());
            }
            b"Font" => {
                if let Some(array) = value.as_array() {
                    state.text.font = find_font(&array.byte_string_at(0).unwrap_or_default())
                        .map(|f| (f, array.number_at_or_zero(1)));
                }
            }
            // `/TR` is skipped outright when `/TR2` is also present.
            b"TR" if has_tr2 => {}
            b"TR" | b"TR2" => {
                state.general.transfer =
                    TransferFunc::load(&value, r, functions, limits, diags).map(Arc::new);
            }
            b"BM" => {
                // An array takes element 0; anything else takes the object's
                // own string.
                let name = value
                    .as_array()
                    .and_then(|a| a.byte_string_at(0))
                    .unwrap_or_else(|| value.to_byte_string());
                state.general.blend = BlendMode::from_name(&name);
            }
            b"SMask" => {
                // A non-dictionary value — `/None` included — clears the
                // mask, which is the spec's semantics reached by accident.
                state.general.soft_mask =
                    SoftMask::load(&value, state.ctm, r, functions, limits, diags).map(Arc::new);
            }
            b"CA" => {
                if let Some(v) = value.number() {
                    state.general.stroke_alpha = v.clamp(0.0, 1.0);
                }
            }
            b"ca" => {
                if let Some(v) = value.number() {
                    state.general.fill_alpha = v.clamp(0.0, 1.0);
                }
            }
            b"OP" => {
                if let Some(v) = value.as_int() {
                    state.general.stroke_overprint = v != 0;
                    // …and the non-stroking flag too, unless `/op` states it.
                    if !has_lower_op {
                        state.general.fill_overprint = v != 0;
                    }
                }
            }
            b"op" => {
                if let Some(v) = value.as_int() {
                    state.general.fill_overprint = v != 0;
                }
            }
            b"OPM" => {
                if let Some(v) = value.as_int() {
                    state.general.overprint_mode = v;
                }
            }
            // Stored, never consumed.
            b"BG" if has_bg2 => {}
            b"UCR" if has_ucr2 => {}
            b"FL" => {
                if let Some(v) = value.number() {
                    state.general.flatness = v;
                }
            }
            b"SM" => {
                if let Some(v) = value.number() {
                    state.general.smoothness = v;
                }
            }
            b"SA" => {
                if let Some(v) = value.as_int() {
                    state.general.stroke_adjust = v != 0;
                }
            }
            b"AIS" => {
                if let Some(v) = value.as_int() {
                    state.general.alpha_is_shape = v != 0;
                }
            }
            b"TK" => {
                if let Some(v) = value.as_int() {
                    state.general.text_knockout = v != 0;
                }
            }
            // `/BG`, `/BG2`, `/UCR`, `/UCR2`, `/HT`, `/Type` and anything
            // else are read and discarded.
            _ => {}
        }
    }
}

/// The dash array an `/ExtGState` `/D` entry states, or `None` when the entry
/// is not the nested-array shape the key requires.
#[must_use]
pub fn ext_gstate_dash(ext: &Dict, r: &impl Resolve) -> Option<(SmallVec<[f32; 4]>, f32)> {
    let outer = ext.array(names::D, r)?;
    // Element 0 **must itself be an array**, or the key is skipped.
    let Some(Object::Array(lengths)) = outer.raw_at(0) else {
        return None;
    };
    Some((
        lengths.iter().map(|o| o.number().unwrap_or(0.0)).collect(),
        outer.number_at_or_zero(1),
    ))
}

/// The matrix in force when an `/ExtGState` installed a soft mask, which is
/// what places the mask.
#[must_use]
pub fn soft_mask_matrix(state: &GraphicsState) -> Affine {
    state.ctm
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

    use super::{apply_ext_gstate, ext_gstate_dash};
    use crate::function::FunctionCache;
    use crate::state::{BlendMode, GraphicsState, RenderIntent};
    use pdfrum_common::{Diagnostics, Limits};
    use pdfrum_object::{Array, Dict, Name, NoResolve, Object};

    fn apply(pairs: Vec<(Name, Object)>) -> GraphicsState {
        let mut state = GraphicsState::default();
        let mut funcs = FunctionCache::new();
        let mut diags = Diagnostics::default();
        apply_ext_gstate(
            &mut state,
            &Dict::from_pairs(pairs),
            |_| None,
            &NoResolve,
            &mut funcs,
            &Limits::default(),
            &mut diags,
        );
        state
    }

    #[test]
    fn the_simple_numeric_keys_land_where_they_belong() {
        let s = apply(vec![
            (Name::from("LW"), Object::Real(3.5)),
            (Name::from("ML"), Object::Real(4.0)),
            (Name::from("LC"), Object::Int(1)),
            (Name::from("LJ"), Object::Int(2)),
        ]);
        assert!((s.stroke_params.width - 3.5).abs() < 1e-6);
        assert!((s.stroke_params.miter_limit - 4.0).abs() < 1e-6);
        assert_eq!(s.stroke_params.cap, crate::ops::LineCap::Round);
        assert_eq!(s.stroke_params.join, crate::ops::LineJoin::Bevel);
    }

    #[test]
    fn alphas_are_clamped_to_zero_one() {
        let s = apply(vec![
            (Name::from("CA"), Object::Real(1.5)),
            (Name::from("ca"), Object::Real(-0.5)),
        ]);
        assert!((s.general.stroke_alpha - 1.0).abs() < 1e-6);
        assert!(s.general.fill_alpha.abs() < 1e-6);
    }

    #[test]
    fn a_null_value_skips_its_key() {
        let s = apply(vec![(Name::from("LW"), Object::Null)]);
        // The default survives.
        assert!((s.stroke_params.width - 1.0).abs() < 1e-6);
    }

    #[test]
    fn op_sets_the_fill_flag_only_when_lower_op_is_absent() {
        let s = apply(vec![(Name::from("OP"), Object::Int(1))]);
        assert!(s.general.stroke_overprint);
        assert!(s.general.fill_overprint, "/OP alone sets both");

        let s = apply(vec![
            (Name::from("OP"), Object::Int(1)),
            (Name::from("op"), Object::Int(0)),
        ]);
        assert!(s.general.stroke_overprint);
        assert!(!s.general.fill_overprint, "/op wins for the fill flag");
    }

    #[test]
    fn a_dash_entry_needs_a_nested_array() {
        let good = Dict::from_pairs([(
            Name::from("D"),
            Object::Array(Array::of([
                Object::Array(Array::of([Object::Int(3), Object::Int(2)])),
                Object::Int(1),
            ])),
        )]);
        let (lengths, phase) = ext_gstate_dash(&good, &NoResolve).expect("a dash");
        assert_eq!(&lengths[..], &[3.0, 2.0]);
        assert!((phase - 1.0).abs() < 1e-6);

        // Element 0 not an array: the key is skipped.
        let flat = Dict::from_pairs([(
            Name::from("D"),
            Object::Array(Array::of([Object::Int(3), Object::Int(2)])),
        )]);
        assert!(ext_gstate_dash(&flat, &NoResolve).is_none());
    }

    #[test]
    fn an_unknown_blend_name_is_normal_and_an_array_takes_element_zero() {
        let s = apply(vec![(
            Name::from("BM"),
            Object::Name(Name::from("NotAMode")),
        )]);
        assert_eq!(s.general.blend, BlendMode::Normal);

        let s = apply(vec![(
            Name::from("BM"),
            Object::Array(Array::of([
                Object::Name(Name::from("Multiply")),
                Object::Name(Name::from("Screen")),
            ])),
        )]);
        assert_eq!(s.general.blend, BlendMode::Multiply);
    }

    #[test]
    fn smask_none_clears_the_mask() {
        let s = apply(vec![(
            Name::from("SMask"),
            Object::Name(Name::from("None")),
        )]);
        assert!(s.general.soft_mask.is_none());
    }

    #[test]
    fn the_render_intent_is_stored_but_inert() {
        let s = apply(vec![(
            Name::from("RI"),
            Object::Name(Name::from("Saturation")),
        )]);
        assert_eq!(s.general.render_intent, RenderIntent::Saturation);
    }

    #[test]
    fn the_inert_keys_are_stored() {
        let s = apply(vec![
            (Name::from("OPM"), Object::Int(1)),
            (Name::from("FL"), Object::Real(2.0)),
            (Name::from("SM"), Object::Real(0.5)),
            (Name::from("SA"), Object::Int(1)),
            (Name::from("AIS"), Object::Int(1)),
            (Name::from("TK"), Object::Int(1)),
        ]);
        assert_eq!(s.general.overprint_mode, 1);
        assert!((s.general.flatness - 2.0).abs() < 1e-6);
        assert!((s.general.smoothness - 0.5).abs() < 1e-6);
        assert!(s.general.stroke_adjust);
        assert!(s.general.alpha_is_shape);
        assert!(s.general.text_knockout);
    }

    #[test]
    fn a_font_array_looks_its_name_up_in_the_resources() {
        // The resolver here always fails, so a `/Font` array clears the font
        // rather than installing one — which is the observable half of the
        // quirk: the spec's `[<ref> size]` form never resolves.
        let s = apply(vec![(
            Name::from("Font"),
            Object::Array(Array::of([Object::Name(Name::from("F1")), Object::Int(12)])),
        )]);
        assert!(s.text.font.is_none());
    }
}
