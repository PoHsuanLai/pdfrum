//! anyrender's paints and pens in pdfrum's terms.

use anyrender::Paint;
use pdfrum_edit::{
    BlendMode as PdfBlend, Dash, Fill as PdfFill, Gradient as PdfGradient, GradientKind,
    GradientStop, LineCap, LineJoin, MiterLimit, Stroke as PdfStroke,
};
use peniko::kurbo::{Cap, Join, Stroke};
use peniko::{BlendMode, Color, Fill, Gradient, Mix};

/// The one colour `brush` paints, or its stops' average for a gradient where only a colour
/// fits (a stroke, text); `None` for a brush with no colour (an image, a renderer resource).
pub(crate) fn colour(brush: &Paint) -> Option<Color> {
    match brush {
        Paint::Solid(color) => Some(*color),
        Paint::Gradient(gradient) => Some(average(gradient)),
        Paint::Image(_) | Paint::Resource(_) | Paint::Custom(_) => None,
    }
}

/// The mean of a gradient's stops, component by component.
#[expect(clippy::cast_precision_loss, reason = "a stop count is far below 2^24")]
fn average(gradient: &Gradient) -> Color {
    let count = gradient.stops.len().max(1) as f32;
    let sum = gradient.stops.iter().fold([0.0f32; 4], |sum, stop| {
        let c = stop
            .color
            .to_alpha_color::<peniko::color::Srgb>()
            .components;
        [sum[0] + c[0], sum[1] + c[1], sum[2] + c[2], sum[3] + c[3]]
    });
    Color::new(sum.map(|component| component / count))
}

/// `gradient` as a pdfrum gradient, or `None` for a sweep (PDF has no conic shading).
pub(crate) fn gradient(gradient: &Gradient) -> Option<PdfGradient> {
    let kind = match gradient.kind {
        peniko::GradientKind::Linear(line) => GradientKind::Linear {
            start: line.start,
            end: line.end,
        },
        peniko::GradientKind::Radial(radial) => GradientKind::Radial {
            start_center: radial.start_center,
            start_radius: f64::from(radial.start_radius),
            end_center: radial.end_center,
            end_radius: f64::from(radial.end_radius),
        },
        peniko::GradientKind::Sweep(_) => return None,
    };
    let stops = gradient
        .stops
        .iter()
        .map(|stop| GradientStop {
            offset: f64::from(stop.offset),
            color: stop.color.to_alpha_color::<peniko::color::Srgb>(),
        })
        .collect();
    Some(PdfGradient { kind, stops })
}

/// `style` in `color` as pdfrum's stroke.
pub(crate) fn stroke(style: &Stroke, color: Color) -> PdfStroke {
    PdfStroke {
        color,
        width: style.width,
        cap: match style.start_cap {
            Cap::Butt => LineCap::Butt,
            Cap::Round => LineCap::Round,
            Cap::Square => LineCap::Square,
        },
        join: match style.join {
            Join::Bevel => LineJoin::Bevel,
            Join::Miter => LineJoin::Miter,
            Join::Round => LineJoin::Round,
        },
        miter_limit: MiterLimit::new(style.miter_limit),
        dash: Dash::new(&style.dash_pattern, style.dash_offset),
    }
}

pub(crate) fn fill(rule: Fill) -> PdfFill {
    match rule {
        Fill::NonZero => PdfFill::NonZero,
        Fill::EvenOdd => PdfFill::EvenOdd,
    }
}

/// The PDF blend mode for `blend`'s mix; `None` for normal, which needs no state. The
/// compositing operator has no PDF form and paints as source-over.
pub(crate) fn blend(blend: BlendMode) -> Option<PdfBlend> {
    let mode = match blend.mix {
        Mix::Normal => return None,
        Mix::Multiply => PdfBlend::Multiply,
        Mix::Screen => PdfBlend::Screen,
        Mix::Overlay => PdfBlend::Overlay,
        Mix::Darken => PdfBlend::Darken,
        Mix::Lighten => PdfBlend::Lighten,
        Mix::ColorDodge => PdfBlend::ColorDodge,
        Mix::ColorBurn => PdfBlend::ColorBurn,
        Mix::HardLight => PdfBlend::HardLight,
        Mix::SoftLight => PdfBlend::SoftLight,
        Mix::Difference => PdfBlend::Difference,
        Mix::Exclusion => PdfBlend::Exclusion,
        Mix::Hue => PdfBlend::Hue,
        Mix::Saturation => PdfBlend::Saturation,
        Mix::Color => PdfBlend::Color,
        Mix::Luminosity => PdfBlend::Luminosity,
    };
    Some(mode)
}

#[cfg(test)]
mod tests {
    use super::{average, gradient};
    use peniko::{Color, Gradient};

    #[test]
    fn a_sweep_has_no_pdf_shading_and_averages_instead() {
        let sweep = Gradient::new_sweep((0.0, 0.0), 0.0, std::f32::consts::TAU).with_stops([
            (0.0, Color::from_rgb8(255, 0, 0)),
            (1.0, Color::from_rgb8(0, 0, 255)),
        ]);
        assert!(gradient(&sweep).is_none());
        let mean = average(&sweep).to_rgba8();
        assert_eq!((mean.r, mean.g, mean.b), (128, 0, 128));
    }

    #[test]
    fn a_linear_gradient_keeps_its_stops() {
        let linear = Gradient::new_linear((0.0, 0.0), (10.0, 0.0)).with_stops([
            (0.0, Color::BLACK),
            (0.5, Color::WHITE),
            (1.0, Color::BLACK),
        ]);
        assert_eq!(gradient(&linear).map(|g| g.stops.len()), Some(3));
    }
}
