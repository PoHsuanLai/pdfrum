//! Translating `pdfrum-render`'s kurbo/peniko vocabulary into tiny-skia's.
//!
//! Every conversion here is mechanical. The one thing that is not — the
//! `1/4096` extent below which tiny-skia silently *drops* a fill or a clip —
//! is deliberately **not** worked around: papering over it per call would
//! make this backend's geometry differ from `vello_cpu`'s, which is exactly
//! what the cross-backend contract forbids. The engine pre-guards degenerate
//! geometry instead, and a residual drop is a loud test failure rather than a
//! quiet one-pixel drift.

use kurbo::{Affine, BezPath, PathEl, Rect};
use pdfrum_page::BlendMode;
use pdfrum_render::{AntiAlias, FillRule, ImageQuality};
use tiny_skia::{FilterQuality, Path, PathBuilder, Transform};

/// A `kurbo::BezPath` as a tiny-skia `Path`.
///
/// Returns `None` for a path tiny-skia will not build — empty, or with a
/// non-finite coordinate. The engine's `hard_clip` has already bounded every
/// coordinate to +/-32000, so the second case only arises from a NaN the
/// engine failed to filter, and dropping it is right.
#[must_use]
#[expect(
    clippy::many_single_char_names,
    reason = "a/c/d are the Bezier control points and p the endpoint, matching \
              kurbo's own PathEl field naming"
)]
pub fn to_path(path: &BezPath) -> Option<Path> {
    let mut b = PathBuilder::new();
    #[expect(
        clippy::cast_possible_truncation,
        reason = "tiny-skia is an f32 rasterizer; the narrowing is the interface"
    )]
    let f = |v: f64| v as f32;
    for el in path.elements() {
        match *el {
            PathEl::MoveTo(p) => b.move_to(f(p.x), f(p.y)),
            PathEl::LineTo(p) => b.line_to(f(p.x), f(p.y)),
            PathEl::QuadTo(a, c) => b.quad_to(f(a.x), f(a.y), f(c.x), f(c.y)),
            PathEl::CurveTo(a, c, d) => {
                b.cubic_to(f(a.x), f(a.y), f(c.x), f(c.y), f(d.x), f(d.y));
            }
            PathEl::ClosePath => b.close(),
        }
    }
    b.finish()
}

/// A rectangle as a closed tiny-skia path.
#[must_use]
pub fn rect_path(rect: Rect) -> Option<Path> {
    #[expect(clippy::cast_possible_truncation, reason = "tiny-skia is f32")]
    let r = tiny_skia::Rect::from_ltrb(
        rect.x0 as f32,
        rect.y0 as f32,
        rect.x1 as f32,
        rect.y1 as f32,
    )?;
    Some(PathBuilder::from_rect(r))
}

/// A `kurbo::Affine` as a tiny-skia `Transform`.
#[must_use]
#[expect(
    clippy::many_single_char_names,
    reason = "a..f are the six affine matrix coefficients; any other names \
              would obscure the correspondence with the PDF `cm` operands"
)]
pub fn to_transform(m: Affine) -> Transform {
    let [a, b, c, d, e, f] = m.as_coeffs();
    #[expect(clippy::cast_possible_truncation, reason = "tiny-skia is f32")]
    Transform::from_row(a as f32, b as f32, c as f32, d as f32, e as f32, f as f32)
}

/// A `peniko::Color` as a tiny-skia straight-alpha `Color`.
#[must_use]
#[expect(
    clippy::many_single_char_names,
    reason = "r/g/b/a are the colour channels"
)]
pub fn to_color(c: peniko::Color) -> tiny_skia::Color {
    let [r, g, b, a] = c.to_rgba8().to_u8_array();
    tiny_skia::Color::from_rgba8(r, g, b, a)
}

/// The fill rule.
#[must_use]
pub fn to_fill_rule(rule: FillRule) -> tiny_skia::FillRule {
    match rule {
        FillRule::Winding => tiny_skia::FillRule::Winding,
        FillRule::EvenOdd => tiny_skia::FillRule::EvenOdd,
    }
}

/// Whether a primitive is antialiased.
#[must_use]
pub fn to_anti_alias(aa: AntiAlias) -> bool {
    matches!(aa, AntiAlias::On)
}

/// The sampling quality for an image draw.
#[must_use]
pub fn to_filter_quality(q: ImageQuality) -> FilterQuality {
    match q {
        ImageQuality::Nearest => FilterQuality::Nearest,
        ImageQuality::Bilinear => FilterQuality::Bilinear,
    }
}

/// A PDF blend mode as tiny-skia's.
///
/// `Compatible` is `Normal` by definition (ISO 32000-1 §11.3.5); the other
/// fifteen map one to one, and tiny-skia implements all four non-separable
/// ones natively.
#[must_use]
pub fn to_blend_mode(mode: BlendMode) -> tiny_skia::BlendMode {
    match mode {
        BlendMode::Normal | BlendMode::Compatible => tiny_skia::BlendMode::SourceOver,
        BlendMode::Multiply => tiny_skia::BlendMode::Multiply,
        BlendMode::Screen => tiny_skia::BlendMode::Screen,
        BlendMode::Overlay => tiny_skia::BlendMode::Overlay,
        BlendMode::Darken => tiny_skia::BlendMode::Darken,
        BlendMode::Lighten => tiny_skia::BlendMode::Lighten,
        BlendMode::ColorDodge => tiny_skia::BlendMode::ColorDodge,
        BlendMode::ColorBurn => tiny_skia::BlendMode::ColorBurn,
        BlendMode::HardLight => tiny_skia::BlendMode::HardLight,
        BlendMode::SoftLight => tiny_skia::BlendMode::SoftLight,
        BlendMode::Difference => tiny_skia::BlendMode::Difference,
        BlendMode::Exclusion => tiny_skia::BlendMode::Exclusion,
        BlendMode::Hue => tiny_skia::BlendMode::Hue,
        BlendMode::Saturation => tiny_skia::BlendMode::Saturation,
        BlendMode::Color => tiny_skia::BlendMode::Color,
        BlendMode::Luminosity => tiny_skia::BlendMode::Luminosity,
    }
}

#[cfg(test)]
mod tests {
    use kurbo::Shape;

    use super::*;

    #[test]
    fn every_blend_mode_maps() {
        // The four non-separable modes are native in tiny-skia, so none of
        // them silently degrades to SourceOver.
        assert_eq!(
            to_blend_mode(BlendMode::Luminosity),
            tiny_skia::BlendMode::Luminosity
        );
        assert_eq!(to_blend_mode(BlendMode::Hue), tiny_skia::BlendMode::Hue);
        assert_eq!(
            to_blend_mode(BlendMode::Compatible),
            tiny_skia::BlendMode::SourceOver
        );
        assert_eq!(
            to_blend_mode(BlendMode::SoftLight),
            tiny_skia::BlendMode::SoftLight
        );
    }

    #[test]
    fn an_empty_path_converts_to_nothing() {
        assert!(to_path(&BezPath::new()).is_none());
    }

    #[test]
    fn a_dashed_stroke_expands_to_one_outline_per_dash() {
        // The dash array reaches the rasterizer through `kurbo::stroke`'s own
        // expansion rather than a `tiny_skia::StrokeDash`, so this pins that
        // the dashes survive that route: a 10-unit line dashed 2-on/2-off
        // becomes three separate closed outlines, not one.
        let mut line = BezPath::new();
        line.move_to((0.0, 0.0));
        line.line_to((10.0, 0.0));
        let dashed = kurbo::Stroke::new(1.0).with_dashes(0.0, [2.0, 2.0]);
        let outline = kurbo::stroke(
            line.path_elements(0.1),
            &dashed,
            &kurbo::StrokeOpts::default(),
            0.1,
        );
        let subpaths = outline
            .elements()
            .iter()
            .filter(|el| matches!(el, kurbo::PathEl::MoveTo(_)))
            .count();
        assert_eq!(subpaths, 3, "2-on/2-off over 10 units is three dashes");
    }

    #[test]
    fn a_stroke_outline_is_symmetric_about_its_centre_line() {
        // The reason `stroke_path` expands rather than calling tiny-skia's
        // stroker: a unit-wide stroke centred on x = 10 must cover exactly
        // 9.5..10.5, so both neighbouring columns are half covered and AGG
        // writes them equally. tiny-skia's own stroker biases one side.
        let mut line = BezPath::new();
        line.move_to((10.0, 0.0));
        line.line_to((10.0, 4.0));
        let outline = kurbo::stroke(
            line.path_elements(0.1),
            &kurbo::Stroke::new(1.0),
            &kurbo::StrokeOpts::default(),
            0.1,
        );
        let bbox = outline.control_box();
        assert!(
            (bbox.x0 - 9.5).abs() < 1e-9 && (bbox.x1 - 10.5).abs() < 1e-9,
            "stroke outline spans {bbox:?}, not 9.5..10.5",
        );
    }
}
