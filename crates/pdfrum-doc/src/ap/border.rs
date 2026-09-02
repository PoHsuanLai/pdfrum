//! Border styles (ISO 32000-1 §12.5.4) and the paths that draw them.
//!
//! Two things surprise here. The style is chosen from the **first byte** of
//! `/S` alone, so `/S /Dotted` is a dash and `/S /Squiggly` is solid; and the
//! beveled and inset styles **double the stated width** before anyone else
//! sees it, which then propagates into how far the body rectangle is inset.

use kurbo::Rect;
use pdfrum_object::{Array, Dict, Resolve, names as obj_names};

use crate::ap::emit::{Content, Float, PaintOp, color_op};
use crate::color::Color;
use crate::geom;
use crate::names;

/// How a border is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BorderStyle {
    /// One solid line.
    #[default]
    Solid,
    /// A dashed line.
    Dash,
    /// A raised bevel.
    Beveled,
    /// A recessed bevel.
    Inset,
    /// A line along the bottom edge only.
    Underline,
}

/// A dash pattern's three numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dash {
    /// On length.
    pub on: i64,
    /// Off length.
    pub gap: i64,
    /// Phase offset.
    pub phase: i64,
}

impl Default for Dash {
    fn default() -> Self {
        Dash {
            on: 3,
            gap: 0,
            phase: 0,
        }
    }
}

/// A resolved border style.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BorderStyleInfo {
    /// The line width — already **doubled** for a beveled or inset style.
    pub width: f32,
    /// Which style.
    pub style: BorderStyle,
    /// The dash pattern.
    pub dash: Dash,
}

impl Default for BorderStyleInfo {
    fn default() -> Self {
        BorderStyleInfo {
            width: 1.0,
            style: BorderStyle::Solid,
            dash: Dash::default(),
        }
    }
}

/// Reads a `/BS` dictionary.
///
/// `/S` is matched on its first byte only, and an unrecognized one leaves the
/// style solid. A beveled or inset style then doubles the width in place.
#[must_use]
pub fn border_style_info<R: Resolve>(bs: Option<&Dict>, r: &R) -> BorderStyleInfo {
    let mut info = BorderStyleInfo::default();
    let Some(bs) = bs else {
        return info;
    };
    if bs.contains_key(names::W) {
        info.width = bs.number(names::W, r).unwrap_or(0.0);
    }
    match bs
        .byte_string(names::S, r)
        .as_deref()
        .and_then(<[u8]>::first)
    {
        Some(b'S') => info.style = BorderStyle::Solid,
        Some(b'D') => info.style = BorderStyle::Dash,
        Some(b'B') => {
            info.style = BorderStyle::Beveled;
            info.width *= 2.0;
        }
        Some(b'I') => {
            info.style = BorderStyle::Inset;
            info.width *= 2.0;
        }
        Some(b'U') => info.style = BorderStyle::Underline,
        _ => {}
    }
    if let Some(pattern) = bs.array(names::D, r) {
        info.dash = Dash {
            on: pattern.int_at(0).unwrap_or(0),
            gap: pattern.int_at(1).unwrap_or(0),
            phase: pattern.int_at(2).unwrap_or(0),
        };
    }
    info
}

/// The border path for one rectangle, or nothing at all.
///
/// A width at or below zero draws nothing. Each style has its own gate on the
/// colour: solid and the bevels need a fill colour, dash and underline need a
/// stroke colour, and a transparent one means the whole path is skipped —
/// except in the bevels, whose two grey wedges are drawn unconditionally.
#[must_use]
pub fn border_path(rect: Rect, info: BorderStyleInfo, color: Color) -> String {
    if info.width <= 0.0 {
        return String::new();
    }
    let (left, bottom, right, top) = (
        geom::left(rect),
        geom::bottom(rect),
        geom::right(rect),
        geom::top(rect),
    );
    let (width, half) = (info.width, info.width / 2.0);
    let mut out = Content::new();

    match info.style {
        BorderStyle::Solid => {
            let fill = color_op(color, PaintOp::Fill);
            if fill.is_empty() {
                return String::new();
            }
            // An even-odd donut: the outer rectangle minus one inset by the
            // full width.
            out.raw(&fill);
            out.rect(rect, Float::Shortest);
            out.raw("re\n");
            out.rect(geom::deflate(rect, width, width), Float::Shortest);
            out.raw("re f*\n");
        }
        BorderStyle::Dash => {
            let stroke = color_op(color, PaintOp::Stroke);
            if stroke.is_empty() {
                return String::new();
            }
            out.raw(&stroke);
            out.num(width, Float::Shortest);
            out.raw(&format!(
                "w [{} {}] {} d\n",
                info.dash.on, info.dash.gap, info.dash.phase
            ));
            out.point(left + half, bottom + half, Float::Shortest);
            out.raw("m\n");
            out.point(left + half, top - half, Float::Shortest);
            out.raw("l\n");
            out.point(right - half, top - half, Float::Shortest);
            out.raw("l\n");
            out.point(right - half, bottom + half, Float::Shortest);
            out.raw("l\n");
            out.point(left + half, bottom + half, Float::Shortest);
            out.raw("l S\n");
        }
        BorderStyle::Beveled | BorderStyle::Inset => {
            let beveled = info.style == BorderStyle::Beveled;
            let (top_left, bottom_right) = if beveled { (1.0, 0.5) } else { (0.5, 0.75) };

            out.raw(&color_op(Color::Gray(top_left), PaintOp::Fill));
            for (x, y, op) in [
                (left + half, bottom + half, "m\n"),
                (left + half, top - half, "l\n"),
                (right - half, top - half, "l\n"),
                (right - width, top - width, "l\n"),
                (left + width, top - width, "l\n"),
                (left + width, bottom + width, "l f\n"),
            ] {
                out.point(x, y, Float::Shortest);
                out.raw(op);
            }

            out.raw(&color_op(Color::Gray(bottom_right), PaintOp::Fill));
            for (x, y, op) in [
                (right - half, top - half, "m\n"),
                (right - half, bottom + half, "l\n"),
                (left + half, bottom + half, "l\n"),
                (left + width, bottom + width, "l\n"),
                (right - width, bottom + width, "l\n"),
                (right - width, top - width, "l f\n"),
            ] {
                out.point(x, y, Float::Shortest);
                out.raw(op);
            }

            let fill = color_op(color, PaintOp::Fill);
            if !fill.is_empty() {
                out.raw(&fill);
                out.rect(rect, Float::Shortest);
                out.raw("re\n");
                // Note the **half** width here, unlike the solid style's
                // full-width inset.
                out.rect(geom::deflate(rect, half, half), Float::Shortest);
                out.raw("re f*\n");
            }
        }
        BorderStyle::Underline => {
            let stroke = color_op(color, PaintOp::Stroke);
            if stroke.is_empty() {
                return String::new();
            }
            out.raw(&stroke);
            out.num(width, Float::Shortest);
            out.raw("w\n");
            out.point(left, bottom + half, Float::Shortest);
            out.raw("m\n");
            out.point(right, bottom + half, Float::Shortest);
            out.raw("l S\n");
        }
    }
    out.as_str().to_owned()
}

/// The **annotation-level** border width, which is a different lookup from
/// [`border_style_info`]'s.
///
/// `/BS /W` when `/BS` exists *and carries the key*; else `/Border[2]` when
/// `/Border` has **more than two** elements; else one.
#[must_use]
pub fn border_width<R: Resolve>(dict: &Dict, r: &R) -> f32 {
    if let Some(bs) = dict.dict(names::BS, r)
        && bs.contains_key(names::W)
    {
        return bs.number(names::W, r).unwrap_or(0.0);
    }
    if let Some(border) = dict.array(obj_names::BORDER, r)
        && border.len() > 2
    {
        return border.number_at_or_zero(2);
    }
    1.0
}

/// The dash array an annotation names.
///
/// `/BS /D` when `/BS /S` is exactly `D`; else `/Border[3]` when `/Border`
/// has **exactly four** elements.
#[must_use]
pub fn dash_array<R: Resolve>(dict: &Dict, r: &R) -> Option<Array> {
    if let Some(bs) = dict.dict(names::BS, r)
        && bs.byte_string(names::S, r).as_deref() == Some(b"D")
    {
        return bs.array(names::D, r);
    }
    let border = dict.array(obj_names::BORDER, r)?;
    if border.len() == 4 {
        return border.array_at(3, r);
    }
    None
}

/// The `d` operator for an annotation's dash pattern, or nothing.
///
/// At most ten elements, each followed by a space — so the closing bracket is
/// preceded by one — and the phase is always the literal zero, whatever the
/// pattern's own third number says.
#[must_use]
pub fn dash_pattern_string<R: Resolve>(dict: &Dict, r: &R) -> String {
    let Some(dashes) = dash_array(dict, r).filter(|a| !a.is_empty()) else {
        return String::new();
    };
    let mut out = String::from("[");
    for index in 0..dashes.len().min(10) {
        out.push_str(&crate::ap::fmt::shortest(dashes.number_at_or_zero(index)));
        out.push(' ');
    }
    out.push_str("] 0 d\n");
    out
}

#[cfg(test)]
mod tests {
    use super::{
        BorderStyle, BorderStyleInfo, Dash, border_path, border_style_info, border_width,
        dash_pattern_string,
    };
    use crate::color::Color;
    use crate::geom;
    use pdfrum_object::{Array, Dict, Name, NoResolve, Object, PdfString};

    fn dict(pairs: &[(&str, Object)]) -> Dict {
        Dict::from_pairs(
            pairs
                .iter()
                .map(|(k, v)| (Name::from(*k), v.clone()))
                .collect::<Vec<_>>(),
        )
    }

    fn style_of(spelling: &str) -> BorderStyleInfo {
        let bs = dict(&[
            ("W", Object::from(2.0_f32)),
            ("S", Object::Name(Name::from(spelling))),
        ]);
        border_style_info(Some(&bs), &NoResolve)
    }

    #[test]
    fn only_the_first_byte_of_the_style_name_is_examined() {
        assert_eq!(style_of("Solid").style, BorderStyle::Solid);
        assert_eq!(style_of("Dashed").style, BorderStyle::Dash);
        // `Dotted` starts with a D, so it dashes.
        assert_eq!(style_of("Dotted").style, BorderStyle::Dash);
        // `Squiggly` starts with an S, so it is solid.
        assert_eq!(style_of("Squiggly").style, BorderStyle::Solid);
        assert_eq!(style_of("Underline").style, BorderStyle::Underline);
    }

    #[test]
    fn the_bevelled_styles_double_the_stated_width() {
        let width = |spelling: &str| style_of(spelling).width;
        assert!((width("Beveled") - 4.0).abs() < f32::EPSILON);
        assert!((width("Inset") - 4.0).abs() < f32::EPSILON);
        assert!((width("Solid") - 2.0).abs() < f32::EPSILON);
        assert!((width("Dashed") - 2.0).abs() < f32::EPSILON);
    }

    #[test]
    fn an_absent_style_dictionary_is_a_solid_hairline() {
        let info = border_style_info(None, &NoResolve);
        assert!((info.width - 1.0).abs() < f32::EPSILON);
        assert_eq!(info.style, BorderStyle::Solid);
        assert_eq!(info.dash, Dash::default());
    }

    #[test]
    fn a_present_but_unreadable_width_reads_as_zero() {
        let bs = dict(&[("W", Object::Name(Name::from("thick")))]);
        assert!(border_style_info(Some(&bs), &NoResolve).width.abs() < f32::EPSILON);
    }

    #[test]
    fn a_width_at_or_below_zero_draws_nothing() {
        let info = BorderStyleInfo {
            width: 0.0,
            ..BorderStyleInfo::default()
        };
        let rect = geom::rect(0.0, 0.0, 10.0, 10.0);
        assert_eq!(border_path(rect, info, Color::Gray(0.0)), "");
    }

    #[test]
    fn a_solid_border_is_an_even_odd_donut_inset_by_the_full_width() {
        let rect = geom::rect(0.0, 0.0, 10.0, 10.0);
        let info = BorderStyleInfo {
            width: 2.0,
            style: BorderStyle::Solid,
            dash: Dash::default(),
        };
        assert_eq!(
            border_path(rect, info, Color::Gray(0.0)),
            "0 g\n0 0 10 10 re\n2 2 6 6 re f*\n"
        );
    }

    #[test]
    fn a_solid_border_with_no_colour_draws_nothing() {
        let rect = geom::rect(0.0, 0.0, 10.0, 10.0);
        assert_eq!(
            border_path(rect, BorderStyleInfo::default(), Color::Transparent),
            ""
        );
    }

    #[test]
    fn a_bevel_draws_its_two_wedges_even_with_no_border_colour() {
        let rect = geom::rect(0.0, 0.0, 10.0, 10.0);
        let info = BorderStyleInfo {
            width: 2.0,
            style: BorderStyle::Beveled,
            dash: Dash::default(),
        };
        let path = border_path(rect, info, Color::Transparent);
        // Both grey values are there; no donut follows.
        assert!(path.starts_with("1 g\n"), "{path}");
        assert!(path.contains("\n.5 g\n"), "{path}");
        assert!(!path.contains("re f*"), "{path}");

        // Inset uses the other two greys.
        let inset = BorderStyleInfo {
            style: BorderStyle::Inset,
            ..info
        };
        let path = border_path(rect, inset, Color::Transparent);
        assert!(path.starts_with(".5 g\n"), "{path}");
        assert!(path.contains("\n.75 g\n"), "{path}");
    }

    #[test]
    fn a_bevels_donut_insets_by_half_the_width_where_a_solids_uses_all_of_it() {
        let rect = geom::rect(0.0, 0.0, 10.0, 10.0);
        let info = BorderStyleInfo {
            width: 2.0,
            style: BorderStyle::Beveled,
            dash: Dash::default(),
        };
        let path = border_path(rect, info, Color::Gray(0.0));
        assert!(
            path.ends_with("0 g\n0 0 10 10 re\n1 1 8 8 re f*\n"),
            "{path}"
        );
    }

    #[test]
    fn a_dashed_border_writes_its_pattern_as_bare_integers() {
        let rect = geom::rect(0.0, 0.0, 10.0, 10.0);
        let info = BorderStyleInfo {
            width: 2.0,
            style: BorderStyle::Dash,
            dash: Dash {
                on: 3,
                gap: 1,
                phase: 0,
            },
        };
        let path = border_path(rect, info, Color::Gray(0.0));
        assert!(path.contains("2 w [3 1] 0 d\n"), "{path}");
        assert!(path.ends_with("1 1 l S\n"), "{path}");
    }

    #[test]
    fn an_underline_draws_only_the_bottom_edge() {
        let rect = geom::rect(0.0, 0.0, 10.0, 10.0);
        let info = BorderStyleInfo {
            width: 2.0,
            style: BorderStyle::Underline,
            dash: Dash::default(),
        };
        assert_eq!(
            border_path(rect, info, Color::Gray(0.0)),
            "0 G\n2 w\n0 1 m\n10 1 l S\n"
        );
    }

    #[test]
    fn the_annotation_border_width_prefers_the_style_dictionarys_key() {
        let both = dict(&[
            ("BS", Object::Dict(dict(&[("W", Object::from(3.0_f32))]))),
            (
                "Border",
                Object::Array(Array::of([0.0_f32, 0.0, 7.0].map(Object::from))),
            ),
        ]);
        assert!((border_width(&both, &NoResolve) - 3.0).abs() < f32::EPSILON);

        // A `/BS` without the key falls through to `/Border[2]`.
        let fallthrough = dict(&[
            ("BS", Object::Dict(Dict::new())),
            (
                "Border",
                Object::Array(Array::of([0.0_f32, 0.0, 7.0].map(Object::from))),
            ),
        ]);
        assert!((border_width(&fallthrough, &NoResolve) - 7.0).abs() < f32::EPSILON);

        // A short `/Border` does not count.
        let short = dict(&[(
            "Border",
            Object::Array(Array::of([0.0_f32, 0.0].map(Object::from))),
        )]);
        assert!((border_width(&short, &NoResolve) - 1.0).abs() < f32::EPSILON);
        assert!((border_width(&Dict::new(), &NoResolve) - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn the_dash_operator_caps_at_ten_elements_and_forces_a_zero_phase() {
        let many: Vec<Object> = (1..=12).map(Object::Int).collect();
        let annot = dict(&[(
            "BS",
            Object::Dict(dict(&[
                ("S", Object::Str(PdfString::literal(b"D"))),
                ("D", Object::Array(Array::of(many))),
            ])),
        )]);
        assert_eq!(
            dash_pattern_string(&annot, &NoResolve),
            "[1 2 3 4 5 6 7 8 9 10 ] 0 d\n"
        );
    }

    #[test]
    fn no_dash_array_writes_no_operator() {
        assert_eq!(dash_pattern_string(&Dict::new(), &NoResolve), "");
        let empty = dict(&[(
            "BS",
            Object::Dict(dict(&[
                ("S", Object::Name(Name::from("D"))),
                ("D", Object::Array(Array::new())),
            ])),
        )]);
        assert_eq!(dash_pattern_string(&empty, &NoResolve), "");
    }
}
