//! The SVG document under construction: an element buffer, a clip-path
//! definition table, and the open-group stack that keeps the two balanced.
//!
//! The accumulator is append-only. Every device call becomes text immediately
//! rather than a retained node, because nothing in the mapping ever needs to
//! revisit an element it has already written — a clip is a `<clipPath>`
//! defined once and referenced, and a layer is a `<g>` whose closing tag is
//! written when the engine pops it.

use core::fmt::Write as _;

use kurbo::{Affine, BezPath, Rect, Stroke};
use pdfrum_page::BlendMode;
use pdfrum_render::{AntiAlias, FillRule};

use crate::xml;

/// The `<g>`/`</g>` an open clip or layer owes the document.
///
/// One variant per thing the engine can push, so [`Svg::pop`] closes exactly
/// what was opened and an unbalanced stack is impossible to write rather than
/// something to assert at the end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Open {
    /// A clip: one `<g clip-path=...>`.
    Clip,
    /// A layer: one `<g>` carrying opacity and blend mode.
    Layer,
}

/// An SVG document being built from a sequence of device calls.
#[derive(Debug, Default)]
pub struct Svg {
    /// The `<clipPath>` definitions, which must all precede their uses inside
    /// one `<defs>` block.
    defs: String,
    /// The drawing elements, in order.
    body: String,
    /// What is currently open, innermost last.
    stack: Vec<Open>,
    /// Next free id, so `clip0`, `clip1`, ... are unique within the document.
    next_id: usize,
    /// The page's device size, which becomes `width`/`height`/`viewBox`.
    size: (u32, u32),
}

impl Svg {
    /// A document for a page of `width` x `height` device pixels.
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            size: (width, height),
            ..Self::default()
        }
    }

    /// A fresh element id.
    fn id(&mut self) -> String {
        let id = self.next_id;
        self.next_id += 1;
        format!("c{id}")
    }

    /// Indentation for the current depth, so the output is readable when a
    /// person opens it.
    fn indent(&self) -> String {
        "  ".repeat(self.stack.len() + 1)
    }

    /// `shape-rendering` for a primitive drawn under `aa`.
    ///
    /// The two hard-edged modes both become `crispEdges`. That is the closest
    /// SVG has: it turns antialiasing off, which is exactly
    /// [`AntiAlias::Off`], and for [`AntiAlias::FullCover`] it is the nearer
    /// of the two available answers — full-cover asks for *more* ink than
    /// antialiasing gives, and `crispEdges` at least does not leave the
    /// seam-thinning that `auto` would. Full-cover fills only ever appear
    /// inside a mesh subtree, which this crate rasterizes anyway, so the
    /// approximation never reaches a shipped element.
    fn rendering(aa: AntiAlias) -> Option<&'static str> {
        match aa {
            AntiAlias::On => None,
            AntiAlias::Off | AntiAlias::FullCover => Some("crispEdges"),
        }
    }

    /// The `fill-rule` / `clip-rule` keyword.
    fn rule_name(rule: FillRule) -> &'static str {
        match rule {
            FillRule::Winding => "nonzero",
            FillRule::EvenOdd => "evenodd",
        }
    }

    /// A PDF blend mode as a CSS `mix-blend-mode` keyword.
    ///
    /// All sixteen map one to one: CSS's separable and non-separable blend
    /// modes are defined by the same formulas as ISO 32000 §11.3.5, because
    /// the CSS spec took them from PDF. `Normal` and `Compatible` produce no
    /// attribute rather than `normal`, which is the default anyway and would
    /// otherwise land on every layer.
    fn blend_name(blend: BlendMode) -> Option<&'static str> {
        match blend {
            BlendMode::Normal | BlendMode::Compatible => None,
            BlendMode::Multiply => Some("multiply"),
            BlendMode::Screen => Some("screen"),
            BlendMode::Overlay => Some("overlay"),
            BlendMode::Darken => Some("darken"),
            BlendMode::Lighten => Some("lighten"),
            BlendMode::ColorDodge => Some("color-dodge"),
            BlendMode::ColorBurn => Some("color-burn"),
            BlendMode::HardLight => Some("hard-light"),
            BlendMode::SoftLight => Some("soft-light"),
            BlendMode::Difference => Some("difference"),
            BlendMode::Exclusion => Some("exclusion"),
            BlendMode::Hue => Some("hue"),
            BlendMode::Saturation => Some("saturation"),
            BlendMode::Color => Some("color"),
            BlendMode::Luminosity => Some("luminosity"),
        }
    }

    /// A filled `<path>`.
    pub fn fill_path(
        &mut self,
        path: &BezPath,
        t: Affine,
        color: peniko::Color,
        rule: FillRule,
        aa: AntiAlias,
    ) {
        let indent = self.indent();
        let _ = write!(
            self.body,
            "{indent}<path d=\"{}\" fill=\"{}\"",
            xml::path_data(path),
            xml::rgb_hex(color)
        );
        self.write_opacity("fill-opacity", xml::alpha_of(color));
        if matches!(rule, FillRule::EvenOdd) {
            let _ = write!(self.body, " fill-rule=\"{}\"", Self::rule_name(rule));
        }
        self.write_transform(t);
        self.write_rendering(aa);
        self.body.push_str("/>\n");
    }

    /// A stroked `<path>`.
    ///
    /// The engine has already resolved the device width, the minimum-width
    /// bump and the dash normalisation, so every one of these attributes is a
    /// direct copy of a `kurbo::Stroke` field.
    pub fn stroke_path(
        &mut self,
        path: &BezPath,
        t: Affine,
        color: peniko::Color,
        stroke: &Stroke,
        aa: AntiAlias,
    ) {
        let indent = self.indent();
        let _ = write!(
            self.body,
            "{indent}<path d=\"{}\" fill=\"none\" stroke=\"{}\" stroke-width=\"",
            xml::path_data(path),
            xml::rgb_hex(color)
        );
        xml::number(&mut self.body, stroke.width);
        self.body.push('"');
        self.write_opacity("stroke-opacity", xml::alpha_of(color));
        let _ = write!(
            self.body,
            " stroke-linecap=\"{}\" stroke-linejoin=\"{}\"",
            cap_name(stroke.start_cap),
            join_name(stroke.join)
        );
        if matches!(stroke.join, kurbo::Join::Miter) {
            self.body.push_str(" stroke-miterlimit=\"");
            xml::number(&mut self.body, stroke.miter_limit);
            self.body.push('"');
        }
        if !stroke.dash_pattern.is_empty() {
            self.body.push_str(" stroke-dasharray=\"");
            for (i, d) in stroke.dash_pattern.iter().enumerate() {
                if i > 0 {
                    self.body.push(' ');
                }
                xml::number(&mut self.body, *d);
            }
            self.body.push_str("\" stroke-dashoffset=\"");
            xml::number(&mut self.body, stroke.dash_offset);
            self.body.push('"');
        }
        self.write_transform(t);
        self.write_rendering(aa);
        self.body.push_str("/>\n");
    }

    /// An `<image>` carrying a PNG `data:` URI.
    ///
    /// `t` is the engine's pixel-grid transform: it maps image pixel `(0, 0)`
    /// to its device position, so the element is placed at the origin at its
    /// own pixel size and the whole placement rides on the `transform`. That
    /// is the same reading `RenderDevice::draw_image` documents, and reading
    /// it as a unit-square mapping instead collapses a page-sized image onto
    /// one pixel.
    pub fn draw_image(&mut self, png: &[u8], width: u32, height: u32, t: Affine, alpha: f32) {
        let indent = self.indent();
        let _ = write!(
            self.body,
            "{indent}<image x=\"0\" y=\"0\" width=\"{width}\" height=\"{height}\" \
             preserveAspectRatio=\"none\" xlink:href=\"data:image/png;base64,"
        );
        // The payload is base64, which contains none of XML's five
        // metacharacters, but it goes through the escaper anyway: the rule is
        // that nothing reaches the output unescaped.
        let encoded = xml::base64(png);
        xml::escape(&mut self.body, &encoded);
        self.body.push('"');
        self.write_opacity("opacity", alpha);
        self.write_transform(t);
        self.body.push_str("/>\n");
    }

    /// The same as [`Self::draw_image`], tagged with the cause that made the
    /// region pixels.
    ///
    /// The tag is a `data-cause` attribute: SVG ignores it, every parser
    /// keeps it, and it means a person reading the file sees the same answer
    /// the [`RasterReport`](crate::RasterReport) gives programmatically.
    pub fn draw_raster_region(
        &mut self,
        png: &[u8],
        width: u32,
        height: u32,
        t: Affine,
        alpha: f32,
        cause: crate::RasterCause,
    ) {
        let indent = self.indent();
        let _ = writeln!(self.body, "{indent}<g data-cause=\"{}\">", cause.name());
        self.stack.push(Open::Layer);
        self.draw_image(png, width, height, t, alpha);
        self.stack.pop();
        let _ = writeln!(self.body, "{indent}</g>");
    }

    /// Open a `<g>` clipped by `path`.
    pub fn push_clip(&mut self, path: &BezPath, rule: FillRule) {
        let id = self.id();
        let indent = self.indent();
        let _ = write!(
            self.defs,
            "    <clipPath id=\"{id}\" clipPathUnits=\"userSpaceOnUse\">\
             <path d=\"{}\"",
            xml::path_data(path)
        );
        if matches!(rule, FillRule::EvenOdd) {
            let _ = write!(self.defs, " clip-rule=\"{}\"", Self::rule_name(rule));
        }
        self.defs.push_str("/></clipPath>\n");
        let _ = writeln!(self.body, "{indent}<g clip-path=\"url(#{id})\">");
        self.stack.push(Open::Clip);
    }

    /// Open a `<g>` clipped by an axis-aligned rectangle.
    ///
    /// A `<rect>` rather than the equivalent four-segment `<path>`: it is
    /// shorter, and it is the shape a renderer can take a rectangle fast path
    /// on, which is the same reason the engine gives this its own device
    /// call.
    pub fn push_clip_rect(&mut self, rect: Rect) {
        let id = self.id();
        let indent = self.indent();
        let _ = write!(
            self.defs,
            "    <clipPath id=\"{id}\" clipPathUnits=\"userSpaceOnUse\"><rect x=\""
        );
        xml::number(&mut self.defs, rect.x0);
        self.defs.push_str("\" y=\"");
        xml::number(&mut self.defs, rect.y0);
        self.defs.push_str("\" width=\"");
        xml::number(&mut self.defs, rect.width().max(0.0));
        self.defs.push_str("\" height=\"");
        xml::number(&mut self.defs, rect.height().max(0.0));
        self.defs.push_str("\"/></clipPath>\n");
        let _ = writeln!(self.body, "{indent}<g clip-path=\"url(#{id})\">");
        self.stack.push(Open::Clip);
    }

    /// Open a `<g>` for a transparency layer.
    ///
    /// `mask` is deliberately not a parameter: a device-sized
    /// [`AlphaMask`](pdfrum_render::AlphaMask) is a coverage plane, and the
    /// only faithful SVG for it is a `<mask>` holding that plane as an image
    /// — which is a raster region, so the caller records it as one rather
    /// than letting this method approximate it.
    pub fn push_layer(&mut self, blend: BlendMode, alpha: f32) {
        let indent = self.indent();
        let _ = write!(self.body, "{indent}<g");
        self.write_opacity("opacity", alpha);
        if let Some(name) = Self::blend_name(blend) {
            let _ = write!(self.body, " style=\"mix-blend-mode:{name}\"");
        }
        self.body.push_str(">\n");
        self.stack.push(Open::Layer);
    }

    /// Close the innermost clip or layer.
    ///
    /// A `pop` with nothing open is ignored rather than a panic: the engine
    /// balances its own pushes, and a library that aborts on a malformed
    /// document is the behaviour forbids.
    pub fn pop(&mut self) {
        if self.stack.pop().is_some() {
            let indent = self.indent();
            let _ = writeln!(self.body, "{indent}</g>");
        }
    }

    /// ` transform="matrix(...)"`, or nothing for the identity.
    fn write_transform(&mut self, t: Affine) {
        if let Some(m) = xml::matrix(t) {
            let _ = write!(self.body, " transform=\"{m}\"");
        }
    }

    /// ` name="a"`, or nothing when `a` is fully opaque.
    fn write_opacity(&mut self, name: &str, alpha: f32) {
        if alpha < 1.0 {
            let _ = write!(self.body, " {name}=\"");
            xml::number(&mut self.body, f64::from(alpha.max(0.0)));
            self.body.push('"');
        }
    }

    /// ` shape-rendering="crispEdges"`, or nothing.
    fn write_rendering(&mut self, aa: AntiAlias) {
        if let Some(mode) = Self::rendering(aa) {
            let _ = write!(self.body, " shape-rendering=\"{mode}\"");
        }
    }

    /// The finished document, root element and all.
    ///
    /// Any group the engine left open is closed here rather than reported:
    /// an unbalanced stack means the walk stopped early — a deadline, a
    /// recursion limit — and the useful output is the part that did draw.
    pub fn finish(mut self) -> String {
        while !self.stack.is_empty() {
            self.pop();
        }
        let (w, h) = self.size;
        let mut out = String::with_capacity(self.defs.len() + self.body.len() + 256);
        let _ = writeln!(
            out,
            "<svg xmlns=\"http://www.w3.org/2000/svg\" \
             xmlns:xlink=\"http://www.w3.org/1999/xlink\" \
             width=\"{w}\" height=\"{h}\" viewBox=\"0 0 {w} {h}\">"
        );
        if !self.defs.is_empty() {
            out.push_str("  <defs>\n");
            out.push_str(&self.defs);
            out.push_str("  </defs>\n");
        }
        out.push_str(&self.body);
        out.push_str("</svg>\n");
        out
    }
}

/// A `kurbo::Cap` as an SVG `stroke-linecap` keyword.
fn cap_name(cap: kurbo::Cap) -> &'static str {
    match cap {
        kurbo::Cap::Butt => "butt",
        kurbo::Cap::Round => "round",
        kurbo::Cap::Square => "square",
    }
}

/// A `kurbo::Join` as an SVG `stroke-linejoin` keyword.
///
/// SVG 2 has no `bevel`-vs-`miter-clip` distinction worth using here, and
/// kurbo's three joins are exactly SVG 1.1's three.
fn join_name(join: kurbo::Join) -> &'static str {
    match join {
        kurbo::Join::Bevel => "bevel",
        kurbo::Join::Miter => "miter",
        kurbo::Join::Round => "round",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square() -> BezPath {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((4.0, 0.0));
        p.line_to((4.0, 4.0));
        p.close_path();
        p
    }

    #[test]
    fn an_empty_document_is_a_well_formed_root() {
        let out = Svg::new(10, 20).finish();
        assert!(out.starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\""));
        assert!(out.contains("viewBox=\"0 0 10 20\""));
        assert!(out.trim_end().ends_with("</svg>"));
        assert!(!out.contains("<defs>"), "no clips, no defs block");
    }

    #[test]
    fn an_opaque_fill_carries_no_opacity_attribute() {
        let mut svg = Svg::new(4, 4);
        svg.fill_path(
            &square(),
            Affine::IDENTITY,
            peniko::Color::from_rgba8(255, 0, 0, 255),
            FillRule::Winding,
            AntiAlias::On,
        );
        let out = svg.finish();
        assert!(out.contains("fill=\"#ff0000\""));
        assert!(!out.contains("fill-opacity"));
        assert!(!out.contains("fill-rule"), "nonzero is the default");
        assert!(!out.contains("shape-rendering"), "On is the default");
    }

    #[test]
    fn even_odd_and_alpha_and_crisp_edges_all_appear() {
        let mut svg = Svg::new(4, 4);
        svg.fill_path(
            &square(),
            Affine::translate((1.0, 2.0)),
            peniko::Color::from_rgba8(0, 0, 0, 128),
            FillRule::EvenOdd,
            AntiAlias::Off,
        );
        let out = svg.finish();
        assert!(out.contains("fill-rule=\"evenodd\""));
        assert!(out.contains("fill-opacity=\"0.502\""));
        assert!(out.contains("transform=\"matrix(1 0 0 1 1 2)\""));
        assert!(out.contains("shape-rendering=\"crispEdges\""));
    }

    #[test]
    fn a_stroke_carries_its_whole_pen() {
        let mut svg = Svg::new(4, 4);
        let stroke = Stroke::new(2.5)
            .with_caps(kurbo::Cap::Round)
            .with_join(kurbo::Join::Bevel)
            .with_dashes(1.5, [3.0, 2.0]);
        svg.stroke_path(
            &square(),
            Affine::IDENTITY,
            peniko::Color::from_rgba8(0, 0, 255, 255),
            &stroke,
            AntiAlias::On,
        );
        let out = svg.finish();
        assert!(out.contains("stroke=\"#0000ff\""));
        assert!(out.contains("stroke-width=\"2.5\""));
        assert!(out.contains("stroke-linecap=\"round\""));
        assert!(out.contains("stroke-linejoin=\"bevel\""));
        assert!(out.contains("stroke-dasharray=\"3 2\""));
        assert!(out.contains("stroke-dashoffset=\"1.5\""));
        assert!(!out.contains("miterlimit"), "only a miter join needs one");
    }

    #[test]
    fn clips_become_defs_and_groups_and_close_in_order() {
        let mut svg = Svg::new(8, 8);
        svg.push_clip_rect(Rect::new(0.0, 0.0, 4.0, 8.0));
        svg.push_clip(&square(), FillRule::EvenOdd);
        svg.pop();
        svg.pop();
        let out = svg.finish();
        assert_eq!(out.matches("<clipPath").count(), 2);
        assert_eq!(out.matches("clip-path=\"url(#").count(), 2);
        assert_eq!(out.matches("</g>").count(), 2);
        assert!(out.contains("clip-rule=\"evenodd\""));
        assert!(out.contains("<rect x=\"0\" y=\"0\" width=\"4\" height=\"8\""));
    }

    #[test]
    fn a_layer_carries_opacity_and_blend_mode() {
        let mut svg = Svg::new(4, 4);
        svg.push_layer(BlendMode::Multiply, 0.5);
        svg.pop();
        let out = svg.finish();
        assert!(out.contains("opacity=\"0.5\""));
        assert!(out.contains("mix-blend-mode:multiply"));
    }

    #[test]
    fn a_normal_opaque_layer_is_a_bare_group() {
        let mut svg = Svg::new(4, 4);
        svg.push_layer(BlendMode::Compatible, 1.0);
        svg.pop();
        assert!(svg.finish().contains("<g>"));
    }

    #[test]
    fn every_blend_mode_but_the_two_normals_has_a_css_keyword() {
        let all = [
            BlendMode::Multiply,
            BlendMode::Screen,
            BlendMode::Overlay,
            BlendMode::Darken,
            BlendMode::Lighten,
            BlendMode::ColorDodge,
            BlendMode::ColorBurn,
            BlendMode::HardLight,
            BlendMode::SoftLight,
            BlendMode::Difference,
            BlendMode::Exclusion,
            BlendMode::Hue,
            BlendMode::Saturation,
            BlendMode::Color,
            BlendMode::Luminosity,
        ];
        for mode in all {
            assert!(Svg::blend_name(mode).is_some(), "{mode:?}");
        }
        assert_eq!(Svg::blend_name(BlendMode::Normal), None);
        assert_eq!(Svg::blend_name(BlendMode::Compatible), None);
    }

    #[test]
    fn an_unbalanced_stack_is_closed_by_finish() {
        let mut svg = Svg::new(4, 4);
        svg.push_layer(BlendMode::Normal, 1.0);
        svg.push_clip_rect(Rect::new(0.0, 0.0, 1.0, 1.0));
        let out = svg.finish();
        assert_eq!(out.matches("</g>").count(), 2);
        assert!(out.trim_end().ends_with("</svg>"));
    }

    #[test]
    fn a_pop_with_nothing_open_is_ignored() {
        let mut svg = Svg::new(4, 4);
        svg.pop();
        assert!(!svg.finish().contains("</g>"));
    }

    #[test]
    fn a_raster_region_is_tagged_with_its_cause() {
        let mut svg = Svg::new(4, 4);
        svg.draw_raster_region(
            b"not-a-real-png",
            2,
            2,
            Affine::translate((1.0, 1.0)),
            1.0,
            crate::RasterCause::MeshShading,
        );
        let out = svg.finish();
        assert!(out.contains("data-cause=\"mesh-shading\""));
        assert!(out.contains("data:image/png;base64,"));
        assert!(out.contains("width=\"2\" height=\"2\""));
    }
}
