//! Gradient fills: a shape painted with an axial or radial shading
//! (ISO 32000-1 §8.7.4.5.3-4), the stops stitched into one function.
//!
//! PDF paints a shading with `sh` over the whole clip, so the shape becomes
//! the clip and the shading is painted through it. Colour and alpha are
//! separate in PDF: stops that share one alpha need only a constant opacity;
//! stops whose alpha varies get a luminosity soft mask painted with the same
//! geometry in grey, so a fade to transparent is a fade, not a fade to black.

use kurbo::{Affine, Point, Shape};
use pdfrum_object::{Array, ByteSpan, Dict, Name, Object, Stream, names as pdf_names};
use peniko::Color;

use super::{Canvas, Fill, as_f32};
use crate::write_matrix;

/// A colour ramp, in its own coordinate space.
#[derive(Debug, Clone, PartialEq)]
pub struct Gradient {
    /// Its geometry.
    pub kind: GradientKind,
    /// Colours along it, by offset from 0 (start) to 1 (end). Out-of-order
    /// or out-of-range offsets are clamped into order; beyond the ends the
    /// end colours extend.
    pub stops: Vec<GradientStop>,
}

/// Where a [`Gradient`] runs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GradientKind {
    /// Along the line from `start` to `end`, constant across it.
    Linear {
        /// Offset 0.
        start: Point,
        /// Offset 1.
        end: Point,
    },
    /// Between two circles, the start circle at offset 0 and the end circle
    /// at offset 1 (a two-point conical gradient, as CSS and SVG define).
    Radial {
        /// Centre of the start circle.
        start_center: Point,
        /// Radius of the start circle.
        start_radius: f64,
        /// Centre of the end circle.
        end_center: Point,
        /// Radius of the end circle.
        end_radius: f64,
    },
}

/// One colour of a [`Gradient`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GradientStop {
    /// Where, from 0 to 1.
    pub offset: f64,
    /// The colour there; its alpha is honoured.
    pub color: Color,
}

impl Canvas<'_, '_> {
    /// Fill `shape` by `rule` with `gradient`, placed by `transform` from
    /// the gradient's space into canvas space.
    ///
    /// A gradient with no stops paints nothing; one stop is a solid fill.
    ///
    /// ```
    /// use pdfrum_edit::{EditDoc, Fill, Gradient, GradientKind, GradientStop, Size, blank_document};
    /// use pdfrum_common::Limits;
    /// use kurbo::{Affine, Point, Rect};
    /// use peniko::Color;
    ///
    /// let base = blank_document(&[Size::new(200.0, 100.0)])?;
    /// let mut edit = EditDoc::new(&base);
    /// let ramp = Gradient {
    ///     kind: GradientKind::Linear { start: Point::new(0.0, 0.0), end: Point::new(200.0, 0.0) },
    ///     stops: vec![
    ///         GradientStop { offset: 0.0, color: Color::from_rgb8(255, 0, 0) },
    ///         GradientStop { offset: 1.0, color: Color::from_rgba8(0, 0, 255, 0) },
    ///     ],
    /// };
    /// edit.draw_page(0, &Limits::default(), |c| {
    ///     c.fill_gradient(Rect::new(0.0, 0.0, 200.0, 100.0), Fill::NonZero, &ramp, Affine::IDENTITY);
    /// })?;
    /// # Ok::<(), pdfrum_edit::Error>(())
    /// ```
    pub fn fill_gradient(
        &mut self,
        shape: impl Shape,
        rule: Fill,
        gradient: &Gradient,
        transform: Affine,
    ) {
        let stops = ordered(&gradient.stops);
        let path = shape.into_path(0.1);
        match stops.as_slice() {
            [] => return,
            [only] => return self.draw(path, super::Paint::Fill(only.color), rule),
            _ => {}
        }
        if path.elements().is_empty() {
            return;
        }
        let shading = shading(gradient.kind, &stops, Channel::Colour);
        let alphas: Vec<f32> = stops.iter().map(|stop| stop.color.components[3]).collect();
        let uniform = alphas
            .windows(2)
            .all(|pair| matches!(pair, [a, b] if (a - b).abs() < 1e-4));
        let bounds = transform.inverse().transform_rect_bbox(path.bounding_box());
        let name = self.realize(pdf_names::SHADING, Object::Dict(shading));
        self.out.push_str("q\n");
        self.write_path(&path);
        self.out.push_str(match rule {
            Fill::NonZero => " W n\n",
            Fill::EvenOdd => " W* n\n",
        });
        write_matrix(&mut self.out, transform);
        self.out.push_str(" cm\n");
        match (uniform, alphas.first()) {
            (true, Some(&alpha)) if alpha < 1.0 => self.opacity(f64::from(alpha)),
            (true, _) => {}
            (false, _) => self.alpha_mask(gradient.kind, &stops, bounds),
        }
        self.out.push('/');
        self.push_name(&name);
        self.out.push_str(" sh\nQ\n");
    }

    /// Set a luminosity soft mask that paints each stop's alpha as grey,
    /// over `bounds` in the gradient's space.
    fn alpha_mask(&mut self, kind: GradientKind, stops: &[GradientStop], bounds: kurbo::Rect) {
        let grey = shading(kind, stops, Channel::Alpha);
        let form = Dict::from_pairs([
            (
                pdf_names::TYPE.clone(),
                Object::Name(pdf_names::XOBJECT.clone()),
            ),
            (pdf_names::SUBTYPE.clone(), Object::Name(Name::from("Form"))),
            (
                Name::from("BBox"),
                Object::Array(Array::of(
                    [bounds.x0, bounds.y0, bounds.x1, bounds.y1].map(|v| Object::Real(as_f32(v))),
                )),
            ),
            (
                Name::from("Group"),
                Object::Dict(Dict::from_pairs([
                    (Name::from("S"), Object::Name(Name::from("Transparency"))),
                    (Name::from("CS"), Object::Name(Name::from("DeviceGray"))),
                ])),
            ),
            (
                pdf_names::RESOURCES.clone(),
                Object::Dict(Dict::from_pairs([(
                    pdf_names::SHADING.clone(),
                    Object::Dict(Dict::from_pairs([(Name::from("A"), Object::Dict(grey))])),
                )])),
            ),
        ]);
        let form = self.edit.add(Object::Stream(Box::new(Stream::new(
            form,
            ByteSpan::from(b"/A sh\n".to_vec()),
        ))));
        let state = Dict::from_pairs([(
            Name::from("SMask"),
            Object::Dict(Dict::from_pairs([
                (pdf_names::TYPE.clone(), Object::Name(Name::from("Mask"))),
                (Name::from("S"), Object::Name(Name::from("Luminosity"))),
                (Name::from("G"), Object::Ref(form)),
            ])),
        )]);
        let name = self.realize(pdf_names::EXT_G_STATE, Object::Dict(state));
        self.out.push('/');
        self.push_name(&name);
        self.out.push_str(" gs\n");
    }
}

/// Stops clamped into 0..=1 and into order.
fn ordered(stops: &[GradientStop]) -> Vec<GradientStop> {
    let mut out: Vec<GradientStop> = Vec::with_capacity(stops.len());
    for stop in stops {
        let floor = out.last().map_or(0.0, |last| last.offset);
        let offset = if stop.offset.is_finite() {
            stop.offset
        } else {
            0.0
        };
        out.push(GradientStop {
            offset: offset.clamp(floor, 1.0),
            color: stop.color,
        });
    }
    out
}

/// Which part of a stop's colour a shading paints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Channel {
    /// Red, green and blue, in `/DeviceRGB`.
    Colour,
    /// The alpha, as `/DeviceGray`, for a soft mask.
    Alpha,
}

/// The shading dictionary for `kind` over `stops`, extended at both ends.
fn shading(kind: GradientKind, stops: &[GradientStop], channel: Channel) -> Dict {
    let (shading_type, coords) = match kind {
        GradientKind::Linear { start, end } => (2, vec![start.x, start.y, end.x, end.y]),
        GradientKind::Radial {
            start_center,
            start_radius,
            end_center,
            end_radius,
        } => (
            3,
            vec![
                start_center.x,
                start_center.y,
                start_radius.max(0.0),
                end_center.x,
                end_center.y,
                end_radius.max(0.0),
            ],
        ),
    };
    let space = match channel {
        Channel::Colour => "DeviceRGB",
        Channel::Alpha => "DeviceGray",
    };
    Dict::from_pairs([
        (Name::from("ShadingType"), Object::Int(shading_type)),
        (Name::from("ColorSpace"), Object::Name(Name::from(space))),
        (
            Name::from("Coords"),
            Object::Array(Array::of(
                coords.into_iter().map(|v| Object::Real(as_f32(v))),
            )),
        ),
        (
            Name::from("Function"),
            Object::Dict(stitched(stops, channel)),
        ),
        (
            Name::from("Extend"),
            Object::Array(Array::of([Object::Bool(true), Object::Bool(true)])),
        ),
    ])
}

/// One stitching function (type 3) over a linear interpolation (type 2) per
/// adjacent pair of stops, with constant segments before the first stop and
/// after the last so offsets inside 0..1 still cover the whole domain.
fn stitched(stops: &[GradientStop], channel: Channel) -> Dict {
    let values = |stop: &GradientStop| -> Object {
        let [r, g, b, a] = stop.color.components;
        let components: Vec<f32> = match channel {
            Channel::Colour => vec![r, g, b],
            Channel::Alpha => vec![a],
        };
        Object::Array(Array::of(
            components
                .into_iter()
                .map(|c| Object::Real(c.clamp(0.0, 1.0))),
        ))
    };
    let mut ends: Vec<GradientStop> = stops.to_vec();
    if let Some(first) = stops.first().filter(|first| first.offset > 0.0) {
        ends.insert(
            0,
            GradientStop {
                offset: 0.0,
                ..*first
            },
        );
    }
    if let Some(last) = stops.last().filter(|last| last.offset < 1.0) {
        ends.push(GradientStop {
            offset: 1.0,
            ..*last
        });
    }
    let mut functions = Vec::new();
    let mut bounds = Vec::new();
    let mut encode = Vec::new();
    for pair in ends.windows(2) {
        let [from, to] = pair else { continue };
        functions.push(Object::Dict(Dict::from_pairs([
            (Name::from("FunctionType"), Object::Int(2)),
            (Name::from("Domain"), unit_domain()),
            (Name::from("C0"), values(from)),
            (Name::from("C1"), values(to)),
            (Name::from("N"), Object::Real(1.0)),
        ])));
        encode.push(Object::Real(0.0));
        encode.push(Object::Real(1.0));
        bounds.push(Object::Real(as_f32(to.offset)));
    }
    // One fewer bound than functions: the last stop's offset is the domain's end.
    bounds.pop();
    Dict::from_pairs([
        (Name::from("FunctionType"), Object::Int(3)),
        (Name::from("Domain"), unit_domain()),
        (Name::from("Functions"), Object::Array(Array::of(functions))),
        (Name::from("Bounds"), Object::Array(Array::of(bounds))),
        (Name::from("Encode"), Object::Array(Array::of(encode))),
    ])
}

fn unit_domain() -> Object {
    Object::Array(Array::of([Object::Real(0.0), Object::Real(1.0)]))
}

#[cfg(test)]
mod tests {
    use super::{Channel, GradientStop, ordered, stitched};
    use peniko::Color;

    fn stop(offset: f64) -> GradientStop {
        GradientStop {
            offset,
            color: Color::BLACK,
        }
    }

    #[test]
    fn stops_are_clamped_into_order() {
        let offsets: Vec<f64> = ordered(&[stop(-1.0), stop(0.6), stop(0.4), stop(2.0)])
            .iter()
            .map(|stop| stop.offset)
            .collect();
        assert_eq!(offsets, vec![0.0, 0.6, 0.6, 1.0]);
    }

    #[test]
    fn inner_stops_are_extended_to_the_ends() {
        let function = stitched(&[stop(0.25), stop(0.75)], Channel::Colour);
        let functions = function
            .raw(&pdfrum_object::Name::from("Functions"))
            .and_then(pdfrum_object::Object::as_array)
            .map(pdfrum_object::Array::len);
        assert_eq!(functions, Some(3), "flat, ramp, flat");
    }
}
