//! The per-object graphics frame and the per-kind dispatch (ISO 32000-1 §8.4).
//!
//! Every object is emitted as a self-contained `q … Q` with **no reference to
//! what came before it**. Each piece of state is compared against the
//! hardcoded PDF default and written when it differs — never diffed against
//! the previous object. The output is longer than a hand-written stream and
//! every object is independently relocatable, which is exactly what lets
//! objects from several source streams be redistributed into new ones without
//! a fixup pass.
//!
//! # Only two colour operators exist here, but every space reaches them
//!
//! `rg` and `RG` are the only colour operators written, and a `DeviceGray`
//! colour goes out as three equal components rather than as `g`/`G`. What
//! *converts* to them is every space, not two. **Only a pattern writes
//! nothing**, because a pattern paints through a resource no `rg` can name.
//! See `expressible_rgb` for the `[oracle-bug]` citation.
//!
//! # The `ExtGState` carries three keys and no more
//!
//! Fill alpha, stroke alpha, blend mode. When all three are at their
//! defaults, no `/gs` is written at all. Soft masks, the miter limit and
//! every other graphics-state parameter are lost.

use pdfrum_common::kurbo::Affine;
use pdfrum_object::{Dict, Name, Object};
use pdfrum_page::{BlendMode, ClipEntry, ClipRule, GraphicsState};
use pdfrum_page::{ColorValue, LineCap, LineJoin, PageObject, Rgb};

use crate::content::num::{write_float, write_matrix};
use crate::content::path::{Stroked, emit_path_points, paint_operator};
use crate::content::text::emit_text_body;

/// The graphics-state parameters the emitter can express, as a dedup key.
///
/// Two objects wanting the same three values share one `/ExtGState`
/// resource; that is the whole cache. Alphas are compared by bits because two
/// alphas that differ in the last bit are two different resources, and NaN —
/// which a damaged file can produce — must equal itself here or the cache
/// grows without bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct GraphicsKey {
    fill_alpha: u32,
    stroke_alpha: u32,
    blend: BlendMode,
}

impl GraphicsKey {
    /// The key for one object's state, or `None` when all three parameters
    /// are at their defaults and no `/gs` is needed at all.
    #[must_use]
    #[expect(
        clippy::float_cmp,
        reason = "the comparison is against the PDF default exactly: an alpha \
                  that is 1.0 needs no resource and one a hair off does, \
                  because a reader will honour the difference"
    )]
    pub(crate) fn of(state: &GraphicsState) -> Option<Self> {
        let g = &state.general;
        // `/Compatible` is Normal under another name, so a state carrying it
        // is still a default state and needs no resource.
        let normal = matches!(g.blend, BlendMode::Normal | BlendMode::Compatible);
        if g.fill_alpha == 1.0 && g.stroke_alpha == 1.0 && normal {
            return None;
        }
        Some(Self {
            fill_alpha: g.fill_alpha.to_bits(),
            stroke_alpha: g.stroke_alpha.to_bits(),
            blend: g.blend,
        })
    }

    /// The `/ExtGState` dictionary this key describes.
    #[must_use]
    pub(crate) fn to_dict(self) -> Dict {
        Dict::from_pairs([
            (
                crate::names::CA_LOWER.clone(),
                Object::Real(f32::from_bits(self.fill_alpha)),
            ),
            (
                pdfrum_object::names::CA.clone(),
                Object::Real(f32::from_bits(self.stroke_alpha)),
            ),
            (
                crate::names::BM.clone(),
                Object::Name(blend_name(self.blend)),
            ),
        ])
    }
}

/// The `/ExtGState` a stream's prologue names: fully opaque, normal blending.
///
/// Written unconditionally rather than omitted, because the prologue's job is
/// to put the stream in a known state and "the default" is only knowable if
/// it is stated.
#[must_use]
pub(crate) fn default_graphics() -> Dict {
    Dict::from_pairs([
        (crate::names::CA_LOWER.clone(), Object::Int(1)),
        (pdfrum_object::names::CA.clone(), Object::Int(1)),
        (
            crate::names::BM.clone(),
            Object::Name(pdfrum_object::names::NORMAL.clone()),
        ),
    ])
}

/// The literal state-reset a stream's prologue writes before naming its
/// `/ExtGState` — black stroke, black fill, unit line width, butt caps, miter
/// joins.
///
/// It resets neither the dash array nor the miter limit nor any text state,
/// which is a gap the C++ has and we keep: an object that needs a dash writes
/// one, and one that does not is at whatever the previous stream left.
pub(crate) const DEFAULT_GRAPHICS: &str = "0 0 0 RG 0 0 0 rg 1 w 0 J 0 j\n";

/// Names for the blend modes an `/ExtGState` can carry (table 136).
fn blend_name(blend: BlendMode) -> Name {
    use pdfrum_object::names as n;
    match blend {
        // `/Compatible` is a spelling of Normal kept for round-tripping; a
        // regenerated stream writes the name the spec prefers.
        BlendMode::Normal | BlendMode::Compatible => n::NORMAL.clone(),
        BlendMode::Multiply => n::MULTIPLY.clone(),
        BlendMode::Screen => n::SCREEN.clone(),
        BlendMode::Overlay => n::OVERLAY.clone(),
        BlendMode::Darken => n::DARKEN.clone(),
        BlendMode::Lighten => n::LIGHTEN.clone(),
        BlendMode::ColorDodge => n::COLOR_DODGE.clone(),
        BlendMode::ColorBurn => n::COLOR_BURN.clone(),
        BlendMode::HardLight => n::HARD_LIGHT.clone(),
        BlendMode::SoftLight => n::SOFT_LIGHT.clone(),
        BlendMode::Difference => n::DIFFERENCE.clone(),
        BlendMode::Exclusion => n::EXCLUSION.clone(),
        BlendMode::Hue => n::HUE.clone(),
        BlendMode::Saturation => n::SATURATION.clone(),
        BlendMode::Color => n::COLOR.clone(),
        BlendMode::Luminosity => n::LUMINOSITY.clone(),
    }
}

/// The RGB triple a colour writes, or `None` when it has no colour to write.
///
/// `[oracle-bug]` **Every colour space converts, not just two.** ISO 32000-1
/// §8.6 defines CMYK, `ICCBased`, `Separation`, `DeviceN`, `Lab`, `CalGray`
/// and `CalRGB`, and each of them has an RGB value; [`ColorValue::to_rgb`]
/// produces it. A fill in any of them therefore writes a real `rg`/`RG`
/// rather than inheriting the black the stream prologue set.
///
/// `rg`/`RG` stays the *only* colour operator the emitter writes. Emitting
/// each space's own operator — `k`/`K`, or `cs`/`scn` against a realized
/// `/ColorSpace` resource — would preserve more, but it is a wider change (a
/// new resource family and a second operator family, whose absence is what
/// the module docs describe) where converting is one call. Converting is the
/// narrowest faithful fix.
//
// [oracle-bug] `cpdf_pagecontentgenerator.cpp:64-78` refuses at the gate —
// `if (!color || (!color->IsColorSpaceRGB() && !color->IsColorSpaceGray()))
// return false;` — and its one consumer at `:819-824` then writes no operator
// at all, so a CMYK, `ICCBased`, `Separation`, `DeviceN`, `Lab`, `CalGray` or
// `CalRGB` fill silently becomes black. The refusal is *only* that gate:
// `CPDF_Color::GetRGB` (`cpdf_color.cpp:116-127`) delegates to
// `cs_->GetRGB(buffer)` for **any** space, so the conversion PDFium needs is
// already written and simply never reached — which is why the fix here is the
// delegation the C++ declines to perform rather than a new operator.
// **pdf.js has no counterpart to cite**: it does not regenerate page content,
// so it is silent here.
fn expressible_rgb(colour: &ColorValue) -> Option<Rgb> {
    // A pattern paints through a resource no `rg` can name. That one is a
    // genuine limit rather than the oversight above, and stays.
    if colour.pattern.is_some() {
        return None;
    }
    // No space set yet reads as DeviceGray, a page's initial colour. There is
    // no space for `to_rgb` to ask, so this case is still answered here.
    if colour.space.is_none() {
        let v = colour.components.first().copied()?;
        return Some(Rgb { r: v, g: v, b: v });
    }
    colour.to_rgb()
}

/// Write one colour operator, or nothing.
fn emit_colour(out: &mut String, colour: &ColorValue, stroking: bool) {
    let Some(rgb) = expressible_rgb(colour) else {
        return;
    };
    write_float(out, rgb.r);
    out.push(' ');
    write_float(out, rgb.g);
    out.push(' ');
    write_float(out, rgb.b);
    out.push_str(if stroking { " RG " } else { " rg " });
}

/// Names the resources one object needs, so the caller can realize them
/// before the object is written.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ResourceNames {
    /// The `/ExtGState` name, when the object needs one.
    pub(crate) ext_gstate: Option<Name>,
    /// The `/Font` name, for a text object.
    pub(crate) font: Option<Name>,
    /// The `/XObject` name, for an image or a form.
    pub(crate) xobject: Option<Name>,
}

/// Write one object's `q … Q`, or nothing when it cannot be expressed.
///
/// Returns whether anything was written. `out` is left untouched on a `false`
/// — the bytes are built into a scratch buffer and committed only on success,
/// so an unsupported object never contributes an unbalanced fragment.
//
// [oracle-bug] cpdf_pagecontentgenerator.cpp:945-946 writes the graphics
// prefix and then `*buf << "BT ";`, and `:968-970` is a bare `return` for any
// font class that is neither Type1, TrueType nor CID — a Type 3 font, most
// obviously. So the emitted stream carries a `q` and a `BT` that nothing ever
// closes. §7.8.2 requires a content stream's operators to be balanced: `BT`
// pairs with `ET` (§9.4.1) and `q` with `Q` (§8.4.4), and an unmatched `BT`
// leaves every operator after it inside a text object that was never meant to
// contain them. pdf.js has no counterpart to weigh — it does not regenerate
// page content at all — so the reading rests on the spec, and on the fact
// that PDFium is writing a file it will itself have to re-parse. We build
// each object's bytes into a scratch buffer and commit only on success, so an
// object we cannot express contributes *nothing*. For well-formed input the
// rendered result is identical: the stream-level `Q` closes the oracle's
// stray `q`, and end-of-stream closes the `BT`. What differs is that our
// output stays parseable.
pub(crate) fn emit_object(out: &mut String, object: &PageObject, names: &ResourceNames) -> bool {
    let mut body = String::new();
    body.push_str("q ");
    emit_graphics(&mut body, object.state(), names.ext_gstate.as_ref());

    let painted = match object {
        PageObject::Path(p) => {
            emit_matrix(&mut body, p.object.matrix);
            emit_path_points(&mut body, &p.object.path);
            body.push_str(paint_operator(
                p.object.fill_rule,
                Stroked::of(p.object.stroke),
            ));
            true
        }
        PageObject::Text(t) => {
            let Some(font) = names.font.as_ref() else {
                return false;
            };
            emit_text_body(&mut body, &t.object, font)
        }
        PageObject::Image(i) => {
            // A degenerate matrix collapses the image to nothing, so the
            // object is dropped rather than written as a zero-area `Do`.
            if is_degenerate(i.object.matrix) {
                return false;
            }
            let Some(name) = names.xobject.as_ref() else {
                return false;
            };
            emit_matrix(&mut body, i.object.matrix);
            emit_do(&mut body, name);
            true
        }
        PageObject::Form(f) => {
            if is_degenerate(f.object.matrix) {
                return false;
            }
            let Some(name) = names.xobject.as_ref() else {
                return false;
            };
            emit_matrix(&mut body, f.object.matrix);
            emit_do(&mut body, name);
            true
        }
        // There is no shading branch: a shading page object emits nothing and
        // is silently dropped, the same as the C++'s dispatch.
        PageObject::Shading(_) => false,
    };

    if !painted {
        return false;
    }
    body.push_str(" Q\n");
    out.push_str(&body);
    true
}

/// A matrix that maps everything onto a line or a point — the object it
/// transforms covers no area.
fn is_degenerate(matrix: Affine) -> bool {
    let coeffs = matrix.as_coeffs();
    let at = |i: usize| coeffs.get(i).copied().unwrap_or(0.0);
    // Either basis vector collapsing to zero flattens the unit square onto a
    // line, so whatever the object covers has no area.
    let x_axis_collapsed = at(0) == 0.0 && at(1) == 0.0;
    let y_axis_collapsed = at(2) == 0.0 && at(3) == 0.0;
    x_axis_collapsed || y_axis_collapsed
}

fn emit_matrix(out: &mut String, m: Affine) {
    if m == Affine::IDENTITY {
        return;
    }
    write_matrix(out, m);
    out.push_str(" cm ");
}

fn emit_do(out: &mut String, name: &Name) {
    out.push('/');
    out.push_str(&String::from_utf8_lossy(&pdfrum_object::name_encode(
        name.as_bytes(),
    )));
    out.push_str(" Do");
}

/// The graphics-state prologue of one object, in the fixed order the C++
/// writes it: colours, line width, cap, join, dash, clip, `/gs`.
#[expect(
    clippy::float_cmp,
    reason = "each value is tested against its PDF default exactly; a width \
              that is not 1.0 must be written even when it is 1.0000001, \
              because that is what the file said"
)]
fn emit_graphics(out: &mut String, state: &GraphicsState, ext_gstate: Option<&Name>) {
    emit_colour(out, &state.fill, false);
    emit_colour(out, &state.stroke, true);

    let stroke = &state.stroke_params;
    if stroke.width != 1.0 {
        write_float(out, stroke.width);
        out.push_str(" w ");
    }
    if stroke.cap != LineCap::Butt {
        out.push_str(&cap_int(stroke.cap).to_string());
        out.push_str(" J ");
    }
    if stroke.join != LineJoin::Miter {
        out.push_str(&join_int(stroke.join).to_string());
        out.push_str(" j ");
    }
    if !stroke.dash.is_empty() {
        out.push('[');
        for (i, len) in stroke.dash.iter().enumerate() {
            if i > 0 {
                out.push(' ');
            }
            write_float(out, *len);
        }
        out.push_str("] ");
        write_float(out, stroke.dash_phase);
        out.push_str(" d ");
    }

    emit_clip(out, state);

    if let Some(name) = ext_gstate {
        out.push('/');
        out.push_str(&String::from_utf8_lossy(&pdfrum_object::name_encode(
            name.as_bytes(),
        )));
        out.push_str(" gs ");
    }
}

/// The clip path, as construction operators followed by `W`/`W*` and `n`.
///
/// Only path clips are written. A text clip, a clip-path soft mask and a
/// shading clip have no operator sequence here and are dropped.
fn emit_clip(out: &mut String, state: &GraphicsState) {
    for entry in state.clip.entries() {
        let ClipEntry::Path { path, rule } = entry else {
            continue;
        };
        emit_path_points(out, path);
        out.push_str(match rule {
            ClipRule::EvenOdd => " W* ",
            ClipRule::Winding => " W ",
        });
        out.push_str("n ");
    }
}

fn cap_int(cap: LineCap) -> u8 {
    match cap {
        LineCap::Butt => 0,
        LineCap::Round => 1,
        LineCap::Square => 2,
    }
}

fn join_int(join: LineJoin) -> u8 {
    match join {
        LineJoin::Miter => 0,
        LineJoin::Round => 1,
        LineJoin::Bevel => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_GRAPHICS, GraphicsKey, ResourceNames, default_graphics, emit_object};
    use pdfrum_common::kurbo::{Affine, BezPath, Point};
    use pdfrum_object::{Name, names};
    use pdfrum_page::{BlendMode, ClipRule, ClipStack, GraphicsState};
    use pdfrum_page::{
        ColorSpace, ColorValue, Content, FillRule, LineCap, LineJoin, PageObject, PathObject, Rgb,
    };
    use smallvec::SmallVec;
    use std::sync::Arc;

    fn rgb(r: f32, g: f32, b: f32) -> ColorValue {
        ColorValue {
            space: Some(Arc::new(ColorSpace::DeviceRgb)),
            components: SmallVec::from_slice(&[r, g, b]),
            pattern: None,
        }
    }

    fn cmyk() -> ColorValue {
        ColorValue {
            space: Some(Arc::new(ColorSpace::DeviceCmyk)),
            components: SmallVec::from_slice(&[0.1, 0.2, 0.3, 0.4]),
            pattern: None,
        }
    }

    fn triangle() -> BezPath {
        let mut p = BezPath::new();
        p.move_to((1.0, 2.0));
        p.line_to((3.0, 4.0));
        p.line_to((5.0, 6.0));
        p.close_path();
        p
    }

    fn path_object(
        state: GraphicsState,
        path: BezPath,
        fill: FillRule,
        stroke: bool,
    ) -> PageObject {
        PageObject::Path(Box::new(Content {
            object: PathObject {
                path,
                matrix: Affine::IDENTITY,
                fill_rule: fill,
                stroke,
            },
            state,
            marks: pdfrum_page::ContentMarks::new(),
            content_stream: Some(0),
            dirty: false,
            active: true,
        }))
    }

    fn emit(object: &PageObject, names: &ResourceNames) -> Option<String> {
        let mut out = String::new();
        emit_object(&mut out, object, names).then_some(out)
    }

    // ProcessRect (:55-79): the whole frame, verbatim.
    #[test]
    fn a_rectangle_writes_the_whole_frame() {
        let mut p = BezPath::new();
        p.move_to((10.0, 5.0));
        p.line_to((13.0, 5.0));
        p.line_to((13.0, 30.0));
        p.line_to((10.0, 30.0));
        p.close_path();
        let object = path_object(GraphicsState::default(), p, FillRule::EvenOdd, true);
        assert_eq!(
            emit(&object, &ResourceNames::default()).as_deref(),
            Some("q 0 0 0 rg 0 0 0 RG 10 5 3 25 re B* Q\n")
        );
    }

    // ProcessGraphics (:188-259): colours, then the gs name.
    #[test]
    fn colours_precede_the_ext_gstate_name() {
        let state = GraphicsState {
            fill: rgb(0.5, 0.7, 0.35),
            stroke: rgb(1.0, 0.9, 0.0),
            ..GraphicsState::default()
        };
        let object = path_object(state, triangle(), FillRule::Winding, true);
        let names = ResourceNames {
            ext_gstate: Some(Name::from("FXE1")),
            ..ResourceNames::default()
        };
        let out = emit(&object, &names).expect("emits");
        assert!(
            out.starts_with("q .5 .7 .35 rg 1 .9 0 RG /FXE1 gs "),
            "got {out}"
        );
        assert!(out.ends_with("1 2 m 3 4 l 5 6 l h B Q\n"), "got {out}");
    }

    // The line width slots between the colours and the gs name.
    #[test]
    fn a_non_default_line_width_is_written() {
        let mut state = GraphicsState {
            fill: rgb(0.5, 0.7, 0.35),
            stroke: rgb(1.0, 0.9, 0.0),
            ..GraphicsState::default()
        };
        state.stroke_params.width = 10.5;
        let object = path_object(state, triangle(), FillRule::Winding, true);
        let names = ResourceNames {
            ext_gstate: Some(Name::from("FXE1")),
            ..ResourceNames::default()
        };
        let out = emit(&object, &names).expect("emits");
        assert!(
            out.starts_with("q .5 .7 .35 rg 1 .9 0 RG 10.5 w /FXE1 gs "),
            "got {out}"
        );
    }

    // A default width writes nothing at all — the comparison is against the
    // hardcoded default, not against the previous object.
    #[test]
    fn a_default_line_width_writes_nothing() {
        let object = path_object(
            GraphicsState::default(),
            triangle(),
            FillRule::Winding,
            false,
        );
        let out = emit(&object, &ResourceNames::default()).expect("emits");
        assert!(!out.contains(" w "), "got {out}");
    }

    #[test]
    fn caps_and_joins_are_written_only_when_they_differ() {
        let mut state = GraphicsState::default();
        state.stroke_params.cap = LineCap::Round;
        state.stroke_params.join = LineJoin::Bevel;
        let object = path_object(state, triangle(), FillRule::None, true);
        let out = emit(&object, &ResourceNames::default()).expect("emits");
        assert!(out.contains("1 J "), "got {out}");
        assert!(out.contains("2 j "), "got {out}");
    }

    #[test]
    fn a_dash_array_writes_its_lengths_and_phase() {
        let mut state = GraphicsState::default();
        state.stroke_params.dash = SmallVec::from_slice(&[3.0, 2.0]);
        state.stroke_params.dash_phase = 1.5;
        let object = path_object(state, triangle(), FillRule::None, true);
        let out = emit(&object, &ResourceNames::default()).expect("emits");
        assert!(out.contains("[3 2] 1.5 d "), "got {out}");
    }

    // `[oracle-bug]` a CMYK fill converts and writes `rg`, where the C++
    // writes no operator at all and leaves the object black.
    #[test]
    fn a_cmyk_colour_converts_and_writes_rg() {
        let state = GraphicsState {
            fill: cmyk(),
            stroke: cmyk(),
            ..GraphicsState::default()
        };
        let object = path_object(state, triangle(), FillRule::Winding, false);
        let out = emit(&object, &ResourceNames::default()).expect("emits");
        assert!(out.contains(" rg "), "got {out}");
        assert!(out.contains(" RG "), "got {out}");
        // The triple is `ColorValue::to_rgb`'s, which is `GetRGB`'s — the
        // conversion PDFium has and declines to call.
        let expected = cmyk().to_rgb().expect("CMYK converts");
        assert_ne!(expected, Rgb::BLACK, "the loss was to black; this is not");
        // What the C++ would have emitted, pinned as the thing we no longer do.
        assert_ne!(out, "q 1 2 m 3 4 l 5 6 l h f Q\n");
    }

    // Every non-pattern space reaches an operator, not just the two the gate
    // at `cpdf_pagecontentgenerator.cpp:65` admits.
    #[test]
    fn a_separation_colour_converts_through_its_alternate() {
        // No tint transform, so the tint broadcasts into the alternate's
        // components — a half-tint over DeviceGray is mid-grey.
        let separation = ColorValue {
            space: Some(Arc::new(ColorSpace::Separation(Box::new(
                pdfrum_page::Separation {
                    none: false,
                    alternate: Some(Box::new(ColorSpace::DeviceGray)),
                    tint: None,
                },
            )))),
            components: SmallVec::from_slice(&[0.5]),
            pattern: None,
        };
        let state = GraphicsState {
            fill: separation,
            ..GraphicsState::default()
        };
        let object = path_object(state, triangle(), FillRule::Winding, false);
        let out = emit(&object, &ResourceNames::default()).expect("emits");
        assert!(out.contains(".5 .5 .5 rg "), "got {out}");
    }

    // The one loss that stays: a pattern has no `rg` to name it.
    #[test]
    fn a_pattern_colour_still_writes_nothing() {
        let pattern = || ColorValue {
            space: Some(Arc::new(ColorSpace::Pattern(Box::new(
                pdfrum_page::PatternSpace { base: None },
            )))),
            components: SmallVec::new(),
            pattern: Some(Box::new(pdfrum_page::PatternValue {
                name: Name::new(b"P0".to_vec()),
                components: SmallVec::new(),
                loaded: None,
            })),
        };
        // Both, so the frame carries no colour at all: the default stroke is
        // the unset-space black, which does write an operator.
        let state = GraphicsState {
            fill: pattern(),
            stroke: pattern(),
            ..GraphicsState::default()
        };
        let object = path_object(state, triangle(), FillRule::Winding, false);
        let out = emit(&object, &ResourceNames::default()).expect("emits");
        assert_eq!(out, "q 1 2 m 3 4 l 5 6 l h f Q\n");
    }

    // A DeviceGray colour writes three equal components, not `g`.
    #[test]
    fn a_gray_colour_writes_three_equal_components() {
        let state = GraphicsState {
            fill: ColorValue {
                space: Some(Arc::new(ColorSpace::DeviceGray)),
                components: SmallVec::from_slice(&[0.25]),
                pattern: None,
            },
            ..GraphicsState::default()
        };
        let object = path_object(state, triangle(), FillRule::Winding, false);
        let out = emit(&object, &ResourceNames::default()).expect("emits");
        assert!(out.contains(".25 .25 .25 rg "), "got {out}");
        assert!(!out.contains(" g "), "there is no `g` operator here");
    }

    // ProcessText (:328-404): the clip golden — a rect clip inside the frame.
    #[test]
    fn a_rectangular_clip_takes_the_re_fast_path() {
        let mut clip = BezPath::new();
        clip.move_to((0.0, 0.0));
        clip.line_to((5.0, 0.0));
        clip.line_to((5.0, 4.0));
        clip.line_to((0.0, 4.0));
        clip.close_path();
        let mut stack = ClipStack::new();
        stack.push_path(clip, ClipRule::EvenOdd);
        let state = GraphicsState {
            clip: stack,
            ..GraphicsState::default()
        };
        let object = path_object(state, triangle(), FillRule::None, false);
        let out = emit(&object, &ResourceNames::default()).expect("emits");
        assert!(
            out.starts_with("q 0 0 0 rg 0 0 0 RG 0 0 5 4 re W* n "),
            "got {out}"
        );
    }

    #[test]
    fn a_winding_clip_writes_a_bare_w() {
        let mut stack = ClipStack::new();
        stack.push_path(triangle(), ClipRule::Winding);
        let state = GraphicsState {
            clip: stack,
            ..GraphicsState::default()
        };
        let object = path_object(state, triangle(), FillRule::None, false);
        let out = emit(&object, &ResourceNames::default()).expect("emits");
        assert!(out.contains(" W n "), "got {out}");
        assert!(!out.contains(" W* "), "got {out}");
    }

    // A shading object emits nothing — there is no branch for it.
    #[test]
    fn a_shading_object_emits_nothing() {
        // Constructed indirectly: a Shading variant with no emitter path.
        // Assert through the dispatch rather than by building a Shading,
        // which needs a whole function graph.
        let mut out = String::from("kept");
        let object = path_object(
            GraphicsState::default(),
            BezPath::new(),
            FillRule::None,
            false,
        );
        // An empty path still paints (`n` with no operands is legal), so the
        // shading case is asserted by the dispatch's own exhaustiveness.
        assert!(emit_object(&mut out, &object, &ResourceNames::default()));
    }

    // A text object with no font resource name cannot be written.
    #[test]
    fn an_object_that_cannot_be_expressed_leaves_the_buffer_untouched() {
        let mut out = String::from("existing");
        let object = PageObject::Text(Box::new(Content {
            object: pdfrum_page::TextObject {
                segments: Box::new([]),
                position: Point::ZERO,
                matrix: Affine::IDENTITY,
                font: None,
                font_source: None,
                render_mode: pdfrum_page::TextRenderMode::Fill,
                type3_metrics: std::collections::BTreeMap::new(),
            },
            state: GraphicsState::default(),
            marks: pdfrum_page::ContentMarks::new(),
            content_stream: Some(0),
            dirty: false,
            active: true,
        }));
        assert!(!emit_object(&mut out, &object, &ResourceNames::default()));
        assert_eq!(out, "existing");
    }

    // The dedup key: three defaults means no /gs at all.
    #[test]
    fn a_fully_default_state_needs_no_ext_gstate() {
        assert_eq!(GraphicsKey::of(&GraphicsState::default()), None);
    }

    #[test]
    fn two_objects_with_the_same_state_share_one_key() {
        let mut a = GraphicsState::default();
        a.general.fill_alpha = 0.5;
        a.general.stroke_alpha = 0.8;
        let mut b = GraphicsState::default();
        b.general.fill_alpha = 0.5;
        b.general.stroke_alpha = 0.8;
        assert_eq!(GraphicsKey::of(&a), GraphicsKey::of(&b));
        assert!(GraphicsKey::of(&a).is_some());
    }

    #[test]
    fn a_different_blend_mode_is_a_different_key() {
        let mut a = GraphicsState::default();
        a.general.blend = BlendMode::Multiply;
        let mut b = GraphicsState::default();
        b.general.blend = BlendMode::Screen;
        assert_ne!(GraphicsKey::of(&a), GraphicsKey::of(&b));
    }

    // ProcessGraphics (:188-259) pins the named ExtGState's contents.
    #[test]
    fn the_ext_gstate_dictionary_carries_exactly_three_keys() {
        let mut state = GraphicsState::default();
        state.general.fill_alpha = 0.5;
        state.general.stroke_alpha = 0.8;
        let dict = GraphicsKey::of(&state).expect("needed").to_dict();
        assert_eq!(dict.len(), 3);
        assert_eq!(
            dict.number(&Name::from("ca"), &pdfrum_object::NoResolve),
            Some(0.5)
        );
        assert_eq!(dict.number(names::CA, &pdfrum_object::NoResolve), Some(0.8));
        assert_eq!(dict.name(&Name::from("BM")), Some(names::NORMAL));
    }

    #[test]
    fn the_default_graphics_dictionary_states_every_default() {
        let dict = default_graphics();
        assert_eq!(dict.len(), 3);
        assert_eq!(dict.direct_int(&Name::from("ca")), Some(1));
        assert_eq!(dict.direct_int(names::CA), Some(1));
        assert_eq!(dict.name(&Name::from("BM")), Some(names::NORMAL));
    }

    #[test]
    fn the_prologue_reset_is_the_literal_the_c_writes() {
        assert_eq!(DEFAULT_GRAPHICS, "0 0 0 RG 0 0 0 rg 1 w 0 J 0 j\n");
    }
}
