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
//! - **`/Font` takes both spellings of its first element.** Table 58 makes it
//!   an *indirect reference to a font dictionary*, which is what we resolve
//!   first; the oracle instead reads the element as a byte string and looks
//!   that up in the `/Font` resources, so its own form never resolves the
//!   spec's. We keep the resource lookup as tolerance for files written
//!   against it. See the `[oracle-bug]` note on the `Font` arm.

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
/// `find_font` is given the `/Font` array's **first element, unresolved** —
/// an indirect reference to a font dictionary in the spec's form, a name in
/// the oracle's — and returns the font either names. See the `[oracle-bug]`
/// note on the `Font` arm.
#[expect(
    clippy::too_many_lines,
    reason = "the key table is a flat dispatch by design: one arm per key"
)]
pub fn apply_ext_gstate<R: Resolve>(
    state: &mut GraphicsState,
    ext: &Dict,
    find_font: impl Fn(Option<&Object>) -> Option<Arc<Font>>,
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
            // [oracle-bug] cpdf_allstates.cpp:87-89 reads the array's first
            // element as a **byte string** — `FindFont(font->GetByteStringAt(0))`
            // — and looks that up in the page's `/Font` resources. Table 58
            // specifies `[font size]` where *font* is an **indirect reference
            // to a font dictionary**, so on the spec's own form
            // `GetByteStringAt(0)` yields `""`, the resource lookup misses,
            // and `cpdf_streamcontentparser.cpp:1239` substitutes stock
            // Helvetica: the conformant spelling silently draws the wrong
            // font. pdf.js resolves the reference — `handleSetFont` is given
            // `value[0]` straight (evaluator.js:1142-1154) and `loadFont`'s
            // first branch is literally "Loading by ref",
            // `if (font instanceof Ref) { fontRef = font; }`
            // (evaluator.js:1256-1261). We resolve the reference first, and
            // keep the name lookup as **tolerance** for the oracle's form,
            // which real files written against PDFium will use.
            b"Font" => {
                if let Some(array) = value.as_array() {
                    state.text.font =
                        find_font(array.raw_at(0)).map(|f| (f, array.number_at_or_zero(1)));
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
        //
        // Kept as written 2026-09-02 (audit A18) because it still holds: a
        // resolver that finds nothing installs nothing either way. What
        // changed is *what the lookup is given* — see the two tests below.
        let s = apply(vec![(
            Name::from("Font"),
            Object::Array(Array::of([Object::Name(Name::from("F1")), Object::Int(12)])),
        )]);
        assert!(s.text.font.is_none());
    }

    /// What the `/Font` arm hands its resolver, for each spelling of the
    /// array's first element.
    fn font_lookup_argument(first: Object) -> Option<Object> {
        use std::cell::RefCell;
        let seen: RefCell<Option<Object>> = RefCell::new(None);
        let mut state = GraphicsState::default();
        let mut funcs = FunctionCache::new();
        let mut diags = Diagnostics::default();
        apply_ext_gstate(
            &mut state,
            &Dict::from_pairs(vec![(
                Name::from("Font"),
                Object::Array(Array::of([first, Object::Int(12)])),
            )]),
            |o| {
                *seen.borrow_mut() = o.cloned();
                None
            },
            &NoResolve,
            &mut funcs,
            &Limits::default(),
            &mut diags,
        );
        seen.into_inner()
    }

    /// Table 58's own form — `[<ref> size]` — reaches the resolver **as the
    /// reference**, which is what lets it be followed to a font dictionary.
    /// The oracle turns it into `""` at `cpdf_allstates.cpp:88`
    /// (`GetByteStringAt(0)` on a reference), and this test fails against
    /// that reading. pdf.js takes the same reference at
    /// `evaluator.js:1256-1261` ("Loading by ref").
    #[test]
    fn the_specs_indirect_reference_form_reaches_the_lookup_intact() {
        let seen = font_lookup_argument(Object::Ref(pdfrum_object::ObjRef::new(7, 0)));
        assert_eq!(seen, Some(Object::Ref(pdfrum_object::ObjRef::new(7, 0))));
    }

    /// The oracle's form still reaches the lookup as a name, so the resource
    /// spelling keeps working. This is tolerance, not the specification.
    #[test]
    fn the_oracles_resource_name_form_still_reaches_the_lookup() {
        let seen = font_lookup_argument(Object::Name(Name::from("F1")));
        assert_eq!(seen, Some(Object::Name(Name::from("F1"))));
    }
}
