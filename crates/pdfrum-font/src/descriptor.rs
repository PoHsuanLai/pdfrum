//! `/FontDescriptor` — the metrics a font declares about itself, and the
//! derivation that fills in what it did not.
//!
//! Every non-Type3 font runs this. The order matters and so do several
//! sign and presence quirks that look like bugs and are load-bearing
//! (`docs/design/pdfrum-font.md` §1.2, §1.3).

use crate::{FontFlags, names};
use pdfrum_common::kurbo::Rect;
use pdfrum_object::{Dict, Resolve};

/// What a font says about its own shape and metrics.
///
/// A record: everything here is read straight from the dictionary, then
/// repaired in place by the two rules of §1.2 and §1.3. Behavior lives in the
/// free functions below.
#[derive(Debug, Clone, PartialEq)]
pub struct FontDescriptor {
    /// The `/Flags` word, with PDFium's own `USE_EXTERN_ATTR` bit possibly
    /// added.
    pub flags: FontFlags,
    /// The italic angle, **only when negative**. A zero or positive
    /// `/ItalicAngle` is read and then discarded (§1.2).
    pub italic_angle: i32,
    /// `/StemV`, the vertical stem thickness. Feeds the weight estimate when
    /// `/FontWeight` is absent.
    pub stem_v: i32,
    /// `/FontWeight`, kept only when strictly positive.
    pub font_weight: Option<i32>,
    /// Maximum height above the baseline, in 1000/em units.
    pub ascent: f32,
    /// Maximum depth below the baseline, normally negative.
    pub descent: f32,
    /// The glyph bounding box in 1000/em units.
    pub font_bbox: Rect,
}

impl Default for FontDescriptor {
    fn default() -> Self {
        Self {
            // A font with no descriptor at all is non-symbolic (bit 6).
            flags: FontFlags::DEFAULT,
            italic_angle: 0,
            stem_v: 0,
            font_weight: None,
            ascent: 0.0,
            descent: 0.0,
            font_bbox: Rect::ZERO,
        }
    }
}

impl FontDescriptor {
    /// The weight to request from substitution.
    ///
    /// `/FontWeight` when it was positive; otherwise estimated from `/StemV`
    /// by a two-piece linear rule PDFium calibrated empirically
    /// (`GetFontWeight`, §1.15).
    #[must_use]
    pub fn weight(&self) -> Option<i32> {
        if let Some(w) = self.font_weight {
            return Some(w);
        }
        let sv = i64::from(self.stem_v);
        let v = if self.stem_v < 140 {
            sv * 5
        } else {
            sv * 4 + 140
        };
        i32::try_from(v).ok()
    }

    /// The weight clamped to the range substitution accepts, or 400.
    #[must_use]
    pub fn subst_weight(&self) -> i32 {
        match self.weight() {
            Some(w) if (100..=900).contains(&w) => w,
            _ => 400,
        }
    }
}

/// Read a `/FontDescriptor` dictionary.
///
/// Four behaviors that read as bugs and are not:
///
/// - **A non-negative `/ItalicAngle` neither sets the italic flag nor is
///   remembered.** Only a negative angle does both.
/// - **`/CapHeight`'s value is never used** — only its presence, and only as
///   one term of the `USE_EXTERN_ATTR` conjunction.
/// - **`/FontWeight 0` is not stored**, so it falls through to the `/StemV`
///   estimate rather than meaning "weight zero".
/// - **A descent above 10 is negated**, unconditionally, including a
///   legitimately positive one.
#[must_use]
pub fn load(desc: &Dict, r: &impl Resolve) -> FontDescriptor {
    let mut d = FontDescriptor {
        flags: desc
            .int(names::FLAGS, r)
            .and_then(|f| u32::try_from(f).ok())
            .map_or(FontFlags::DEFAULT, FontFlags),
        ..FontDescriptor::default()
    };

    let has_italic_angle = desc.raw(names::ITALIC_ANGLE).is_some();
    if has_italic_angle {
        let angle = desc.int(names::ITALIC_ANGLE, r).unwrap_or(0);
        if angle < 0 {
            d.flags = d.flags.with(FontFlags::ITALIC);
            d.italic_angle = i32::try_from(angle).unwrap_or(i32::MIN);
        }
    }

    let has_stem_v = desc.raw(names::STEM_V).is_some();
    if has_stem_v {
        d.stem_v = desc
            .int(names::STEM_V, r)
            .and_then(|v| i32::try_from(v).ok())
            .unwrap_or(0);
    }

    let mut has_valid_weight = false;
    if desc.raw(names::FONT_WEIGHT).is_some()
        && let Some(w) = desc.int(names::FONT_WEIGHT, r).filter(|w| *w > 0)
    {
        d.font_weight = i32::try_from(w).ok();
        has_valid_weight = d.font_weight.is_some();
    }

    let has_ascent = desc.raw(names::ASCENT).is_some();
    if has_ascent {
        d.ascent = desc.int(names::ASCENT, r).unwrap_or(0) as f32;
    }
    let has_descent = desc.raw(names::DESCENT).is_some();
    if has_descent {
        d.descent = desc.int(names::DESCENT, r).unwrap_or(0) as f32;
    }

    // The single most consequential flag downstream: without it, substitution
    // throws away the caller's weight and slant entirely (§1.12 step 0).
    let has_cap_height = desc.raw(names::CAP_HEIGHT).is_some();
    if has_italic_angle
        && has_ascent
        && has_cap_height
        && has_descent
        && (has_stem_v || has_valid_weight)
    {
        d.flags = d.flags.with(FontFlags::USE_EXTERN_ATTR);
    }

    // Sign repair, unconditional.
    if d.descent > 10.0 {
        d.descent = -d.descent;
    }

    if let Some(bbox) = desc.array(names::FONT_BBOX, r) {
        // Read as (left, bottom, right, top) in declaration order, without
        // normalizing — an inverted box stays inverted, as the C++ leaves it.
        d.font_bbox = Rect::new(
            f64::from(bbox.number_at_or_zero(0)),
            f64::from(bbox.number_at_or_zero(1)),
            f64::from(bbox.number_at_or_zero(2)),
            f64::from(bbox.number_at_or_zero(3)),
        );
    }
    d
}

/// The 1000/em normalizer used for ascent, descent and bounding boxes.
///
/// Note `upem / 2` is **integer** division before the float divide, and that
/// this rounds where its sibling [`em_adjust`](crate::em_adjust) truncates —
/// the two disagree for half the inputs and both are live
/// (`NormalizeFontMetric`, §1.3).
#[must_use]
pub fn normalize_font_metric(value: i64, upem: u16) -> i32 {
    if upem == 0 {
        return i32::try_from(value).unwrap_or(if value < 0 { i32::MIN } else { i32::MAX });
    }
    let scaled = (value as f64 * 1000.0 + f64::from(upem / 2)) / f64::from(upem);
    saturating_cast_i32(scaled)
}

/// The *other* 1000/em normalizer: truncating integer division, no rounding
/// and no saturation. Used only for a glyph's advance width
/// (`CFX_Face::EmAdjust`, §1.3).
#[must_use]
pub fn em_adjust(value: i32, upem: u16) -> i32 {
    if upem == 0 {
        return value;
    }
    (i64::from(value) * 1000 / i64::from(upem)) as i32
}

fn saturating_cast_i32(v: f64) -> i32 {
    if v.is_nan() {
        0
    } else if v >= f64::from(i32::MAX) {
        i32::MAX
    } else if v <= f64::from(i32::MIN) {
        i32::MIN
    } else {
        v as i32
    }
}

/// Derive a bounding box, ascent and descent when the PDF declared none.
///
/// The bbox assignment is **deliberately flipped**: the face's box arrives in
/// a y-down convention and the swap is what makes the result y-up in text
/// space. The C++ carries a comment saying so, and the assignment is ported
/// literally rather than reasoned about (`CheckFontMetrics`, §1.3).
pub fn check_font_metrics(
    d: &mut FontDescriptor,
    face: Option<FaceMetrics>,
    char_bbox: impl Fn(u8) -> Rect,
) {
    if d.font_bbox == Rect::ZERO {
        if let Some(m) = face {
            let n = |v: i64| f64::from(normalize_font_metric(v, m.upem));
            d.font_bbox = Rect {
                x0: n(m.bbox_left),
                // `top` of the raw box becomes `bottom` of ours, and vice
                // versa. This is the flip.
                y0: n(m.bbox_top),
                x1: n(m.bbox_right),
                y1: n(m.bbox_bottom),
            };
            d.ascent = normalize_font_metric(m.ascender, m.upem) as f32;
            d.descent = normalize_font_metric(m.descender, m.upem) as f32;
        } else {
            // No face: union every non-degenerate per-code box.
            let mut first = true;
            for i in 0u8..=255 {
                let rect = char_bbox(i);
                if (rect.x0 - rect.x1).abs() < f64::EPSILON {
                    continue;
                }
                if first {
                    d.font_bbox = rect;
                    first = false;
                } else {
                    d.font_bbox = Rect {
                        x0: d.font_bbox.x0.min(rect.x0),
                        y0: d.font_bbox.y0.min(rect.y0),
                        x1: d.font_bbox.x1.max(rect.x1),
                        y1: d.font_bbox.y1.max(rect.y1),
                    };
                }
            }
        }
    }

    if d.ascent == 0.0 && d.descent == 0.0 {
        // `A` and `g` stand in for the extremes when nothing else is known.
        let a = char_bbox(b'A');
        d.ascent = if (a.y0 - a.y1).abs() < f64::EPSILON {
            d.font_bbox.y1 as f32
        } else {
            a.y1 as f32
        };
        let g = char_bbox(b'g');
        d.descent = if (g.y0 - g.y1).abs() < f64::EPSILON {
            d.font_bbox.y0 as f32
        } else {
            g.y0 as f32
        };
    }
}

/// The raw metrics a font face reports, before normalization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FaceMetrics {
    /// Design units per em; zero means the normalizers pass values through.
    pub upem: u16,
    /// Raw bounding box, y-down as the face reports it.
    pub bbox_left: i64,
    /// Raw bounding box top.
    pub bbox_top: i64,
    /// Raw bounding box right.
    pub bbox_right: i64,
    /// Raw bounding box bottom.
    pub bbox_bottom: i64,
    /// Raw ascender.
    pub ascender: i64,
    /// Raw descender.
    pub descender: i64,
}

#[cfg(test)]
mod tests {
    // Test expectations are exact values by design.
    #![allow(clippy::float_cmp)]
    use super::*;
    use pdfrum_object::{Array, NoResolve, Object};

    fn desc(pairs: Vec<(&pdfrum_object::Name, Object)>) -> Dict {
        Dict::from_pairs(pairs.into_iter().map(|(k, v)| (k.clone(), v)))
    }

    #[test]
    fn a_negative_italic_angle_sets_the_flag_and_is_remembered() {
        let d = load(
            &desc(vec![(names::ITALIC_ANGLE, Object::Int(-12))]),
            &NoResolve,
        );
        assert!(d.flags.is_italic());
        assert_eq!(d.italic_angle, -12);
    }

    #[test]
    fn a_non_negative_italic_angle_does_neither() {
        for angle in [0, 12] {
            let d = load(
                &desc(vec![(names::ITALIC_ANGLE, Object::Int(angle))]),
                &NoResolve,
            );
            assert!(!d.flags.is_italic(), "angle {angle}");
            assert_eq!(d.italic_angle, 0, "angle {angle}");
        }
    }

    #[test]
    fn font_weight_zero_is_not_stored() {
        let d = load(
            &desc(vec![
                (names::FONT_WEIGHT, Object::Int(0)),
                (names::STEM_V, Object::Int(100)),
            ]),
            &NoResolve,
        );
        assert_eq!(d.font_weight, None);
        // ...so the weight falls through to the StemV estimate.
        assert_eq!(d.weight(), Some(500));
    }

    #[test]
    fn the_weight_estimate_is_two_piece() {
        let below = load(&desc(vec![(names::STEM_V, Object::Int(139))]), &NoResolve);
        assert_eq!(below.weight(), Some(139 * 5));
        let above = load(&desc(vec![(names::STEM_V, Object::Int(140))]), &NoResolve);
        assert_eq!(above.weight(), Some(140 * 4 + 140));
        // The estimate for weight 700's stem is the unittest's 140.
        assert_eq!(above.subst_weight(), 700);
    }

    #[test]
    fn a_descent_above_ten_is_negated() {
        let d = load(&desc(vec![(names::DESCENT, Object::Int(200))]), &NoResolve);
        assert_eq!(d.descent, -200.0);
        // At exactly 10 it is left alone, positive.
        let d = load(&desc(vec![(names::DESCENT, Object::Int(10))]), &NoResolve);
        assert_eq!(d.descent, 10.0);
        // And a normal negative descent passes straight through.
        let d = load(&desc(vec![(names::DESCENT, Object::Int(-200))]), &NoResolve);
        assert_eq!(d.descent, -200.0);
    }

    #[test]
    fn use_extern_attr_needs_every_term() {
        let full = vec![
            (names::ITALIC_ANGLE, Object::Int(-10)),
            (names::ASCENT, Object::Int(700)),
            (names::CAP_HEIGHT, Object::Int(700)),
            (names::DESCENT, Object::Int(-200)),
            (names::STEM_V, Object::Int(80)),
        ];
        assert!(
            load(&desc(full.clone()), &NoResolve)
                .flags
                .uses_extern_attr()
        );

        // Drop each term in turn; every one is required.
        for drop_key in [
            names::ITALIC_ANGLE,
            names::ASCENT,
            names::CAP_HEIGHT,
            names::DESCENT,
        ] {
            let partial: Vec<_> = full
                .iter()
                .filter(|(k, _)| *k != drop_key)
                .cloned()
                .collect();
            assert!(
                !load(&desc(partial), &NoResolve).flags.uses_extern_attr(),
                "dropping {drop_key:?} should clear the flag"
            );
        }

        // The last term is a disjunction: StemV *or* a positive FontWeight.
        let mut with_weight: Vec<_> = full
            .iter()
            .filter(|(k, _)| *k != names::STEM_V)
            .cloned()
            .collect();
        with_weight.push((names::FONT_WEIGHT, Object::Int(700)));
        assert!(
            load(&desc(with_weight), &NoResolve)
                .flags
                .uses_extern_attr()
        );

        // ...and a FontWeight of 0 does not satisfy it.
        let mut zero_weight: Vec<_> = full
            .iter()
            .filter(|(k, _)| *k != names::STEM_V)
            .cloned()
            .collect();
        zero_weight.push((names::FONT_WEIGHT, Object::Int(0)));
        assert!(
            !load(&desc(zero_weight), &NoResolve)
                .flags
                .uses_extern_attr()
        );
    }

    #[test]
    fn cap_heights_value_is_never_read() {
        let base = vec![
            (names::ITALIC_ANGLE, Object::Int(-10)),
            (names::ASCENT, Object::Int(700)),
            (names::DESCENT, Object::Int(-200)),
            (names::STEM_V, Object::Int(80)),
        ];
        for value in [Object::Int(0), Object::Int(-5), Object::Null] {
            let mut with = base.clone();
            with.push((names::CAP_HEIGHT, value.clone()));
            assert!(
                load(&desc(with), &NoResolve).flags.uses_extern_attr(),
                "presence alone should suffice for {value:?}"
            );
        }
    }

    #[test]
    fn no_descriptor_means_non_symbolic() {
        let d = FontDescriptor::default();
        assert!(d.flags.is_non_symbolic());
        assert!(!d.flags.is_symbolic());
    }

    #[test]
    fn the_two_normalizers_disagree() {
        // The brief's worked example: they agree at value 1 and differ at 2.
        assert_eq!(normalize_font_metric(1, 3), 333);
        assert_eq!(em_adjust(1, 3), 333);
        assert_eq!(normalize_font_metric(2, 3), 667); // rounds
        assert_eq!(em_adjust(2, 3), 666); // truncates
    }

    #[test]
    fn a_zero_upem_passes_values_through() {
        assert_eq!(normalize_font_metric(1234, 0), 1234);
        assert_eq!(em_adjust(1234, 0), 1234);
        // ...and saturates rather than wrapping.
        assert_eq!(normalize_font_metric(i64::MAX, 0), i32::MAX);
        assert_eq!(normalize_font_metric(i64::MIN, 0), i32::MIN);
    }

    #[test]
    fn check_font_metrics_flips_top_and_bottom() {
        let mut d = FontDescriptor::default();
        check_font_metrics(
            &mut d,
            Some(FaceMetrics {
                upem: 1000,
                bbox_left: -100,
                bbox_top: 900,
                bbox_right: 800,
                bbox_bottom: -200,
                ascender: 750,
                descender: -250,
            }),
            |_| Rect::ZERO,
        );
        // Note -99, not -100: the normalizer's `+ upem/2` rounding term is
        // added *before* the divide rather than being symmetric, so a negative
        // value is biased one unit toward zero. That is the C++'s arithmetic
        // and it reaches the oracle's own output.
        assert_eq!(d.font_bbox.x0, -99.0);
        // The face's *top* landed in our y0, and its bottom in our y1.
        assert_eq!(d.font_bbox.y0, 900.0);
        assert_eq!(d.font_bbox.x1, 800.0);
        assert_eq!(d.font_bbox.y1, -199.0);
        assert_eq!(d.ascent, 750.0);
        assert_eq!(d.descent, -249.0);
    }

    #[test]
    fn the_rounding_term_biases_negatives_toward_zero() {
        // `(v * 1000 + upem/2) / upem` rounds half *up* rather than half away
        // from zero, so a negative multiple of the em shrinks by one unit
        // while its positive twin is exact. Worth pinning because it looks
        // like an off-by-one and is not.
        assert_eq!(normalize_font_metric(100, 1000), 100);
        assert_eq!(normalize_font_metric(-100, 1000), -99);
        assert_eq!(normalize_font_metric(0, 1000), 0);
        // `em_adjust` truncates instead, and truncation toward zero is
        // symmetric — so the two normalizers disagree in sign as well as in
        // magnitude.
        assert_eq!(em_adjust(-100, 1000), -100);
    }

    #[test]
    fn without_a_face_the_bbox_is_a_union_of_per_code_boxes() {
        let mut d = FontDescriptor::default();
        check_font_metrics(&mut d, None, |c| match c {
            b'A' => Rect::new(10.0, -20.0, 100.0, 700.0),
            b'B' => Rect::new(-5.0, -50.0, 90.0, 650.0),
            // A degenerate box (left == right) is skipped entirely.
            _ => Rect::new(0.0, 0.0, 0.0, 0.0),
        });
        assert_eq!(d.font_bbox, Rect::new(-5.0, -50.0, 100.0, 700.0));
    }

    #[test]
    fn ascent_and_descent_fall_back_to_a_and_g() {
        let mut d = FontDescriptor {
            font_bbox: Rect::new(0.0, -100.0, 500.0, 800.0),
            ..FontDescriptor::default()
        };
        check_font_metrics(&mut d, None, |c| match c {
            b'A' => Rect::new(0.0, 0.0, 400.0, 690.0),
            b'g' => Rect::new(0.0, -210.0, 400.0, 460.0),
            _ => Rect::ZERO,
        });
        assert_eq!(d.ascent, 690.0);
        assert_eq!(d.descent, -210.0);
    }

    #[test]
    fn a_degenerate_a_falls_back_to_the_font_box() {
        let mut d = FontDescriptor {
            font_bbox: Rect::new(0.0, -100.0, 500.0, 800.0),
            ..FontDescriptor::default()
        };
        check_font_metrics(&mut d, None, |_| Rect::new(0.0, 5.0, 400.0, 5.0));
        assert_eq!(d.ascent, 800.0);
        assert_eq!(d.descent, -100.0);
    }

    #[test]
    fn an_existing_bbox_is_left_alone() {
        let declared = Rect::new(1.0, 2.0, 3.0, 4.0);
        let mut d = FontDescriptor {
            font_bbox: declared,
            ascent: 1.0,
            ..FontDescriptor::default()
        };
        check_font_metrics(&mut d, None, |_| Rect::new(-99.0, -99.0, 99.0, 99.0));
        assert_eq!(d.font_bbox, declared);
    }

    #[test]
    fn a_font_bbox_array_is_read_in_declaration_order() {
        let d = load(
            &desc(vec![(
                names::FONT_BBOX,
                Object::Array(Array::of([
                    Object::Int(-100),
                    Object::Int(-200),
                    Object::Int(900),
                    Object::Int(800),
                ])),
            )]),
            &NoResolve,
        );
        assert_eq!(d.font_bbox, Rect::new(-100.0, -200.0, 900.0, 800.0));
    }
}
