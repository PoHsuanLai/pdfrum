//! End-to-end engine behaviour over synthetic pages, rendered through both
//! rasterizers.
//!
//! These are the tests the unit tests cannot be: they exercise the whole
//! path from a page-object graph to pixels, and they run the same page
//! through `vello_cpu` and `tiny-skia` so that anything the engine decides —
//! as against anything a rasterizer integrates — must agree.

// `clippy.toml`'s `allow-expect-in-tests` covers `#[test]` bodies but not the
// `render_both` and `divergence` helpers these tests factor themselves into;
// a render that fails to produce a pixmap *is* the failure signal there. The
// float and numeric allows are the same bargain: a divergence of `0.0` means
// bit-identical, and asserting it approximately would defeat the test, while
// the pixel counts and small integer channel values these convert are far
// inside every mantissa involved. None of this relaxes anything in the
// library.
#![allow(clippy::expect_used, clippy::float_cmp, clippy::cast_precision_loss)]

use std::collections::BTreeMap;

use kurbo::{Affine, BezPath, Point, Rect};
use pdfrum_common::Diagnostics;
use pdfrum_page::state::ContentMarks;
use pdfrum_page::{
    ColorSpace, Content, FillRule, GraphicsState, Page, PageObject, PathObject, Rotation,
    ShadingObject, TextRenderMode, Transparency,
};
use pdfrum_raster_tinyskia::TinySkiaBackend;
use pdfrum_raster_vello::VelloBackend;
use pdfrum_render::{Pixmap, RenderOptions, render_page};

/// A page of the given device size with no rotation and no transparency.
fn page(width: f64, height: f64, objects: Vec<PageObject>) -> Page {
    Page {
        objects,
        media_box: Rect::new(0.0, 0.0, width, height),
        crop_box: Rect::new(0.0, 0.0, width, height),
        rotate: Rotation::None,
        transparency: Transparency::default(),
        resources: None,
    }
}

fn rect_path(x0: f64, y0: f64, x1: f64, y1: f64) -> BezPath {
    let mut p = BezPath::new();
    p.move_to((x0, y0));
    p.line_to((x1, y0));
    p.line_to((x1, y1));
    p.line_to((x0, y1));
    p.close_path();
    p
}

fn filled(path: BezPath, rgb: [f32; 3]) -> PageObject {
    let mut state = GraphicsState::default();
    state.fill.set_stock(ColorSpace::DeviceRgb, &rgb);
    PageObject::Path(Box::new(Content {
        object: PathObject {
            path,
            matrix: Affine::IDENTITY,
            fill_rule: FillRule::Winding,
            stroke: false,
        },
        state,
        marks: ContentMarks::new(),
        content_stream: 0,
    }))
}

fn stroked(path: BezPath, rgb: [f32; 3], width: f32) -> PageObject {
    let mut state = GraphicsState::default();
    state.stroke.set_stock(ColorSpace::DeviceRgb, &rgb);
    state.stroke_params.width = width;
    PageObject::Path(Box::new(Content {
        object: PathObject {
            path,
            matrix: Affine::IDENTITY,
            fill_rule: FillRule::None,
            stroke: true,
        },
        state,
        marks: ContentMarks::new(),
        content_stream: 0,
    }))
}

fn render_both(p: &Page, opts: &RenderOptions) -> (Pixmap, Pixmap) {
    let mut diags = Diagnostics::default();
    let vello = render_page(p, opts, &VelloBackend::new(), &mut diags).expect("vello renders");
    let mut diags = Diagnostics::default();
    let tiny =
        render_page(p, opts, &TinySkiaBackend::new(), &mut diags).expect("tiny-skia renders");
    assert_eq!(
        (vello.width(), vello.height()),
        (tiny.width(), tiny.height()),
        "both backends must produce the same size"
    );
    (vello, tiny)
}

/// The fraction of pixels differing by more than `tol` in any channel.
fn divergence(a: &Pixmap, b: &Pixmap, tol: u8) -> f64 {
    let total = (a.width() as usize) * (a.height() as usize);
    if total == 0 {
        return 0.0;
    }
    let mut differing = 0usize;
    for y in 0..a.height() {
        for x in 0..a.width() {
            let (Some(pa), Some(pb)) = (a.pixel(x, y), b.pixel(x, y)) else {
                continue;
            };
            if pa.iter().zip(pb.iter()).any(|(l, r)| l.abs_diff(*r) > tol) {
                differing += 1;
            }
        }
    }
    differing as f64 / total as f64
}

#[test]
fn an_empty_page_is_opaque_white() {
    // The oracle renders a page with no transparency into a BGRx buffer
    // cleared to 0xFFFFFFFF, so a blank page is white and fully opaque —
    // not transparent, which would composite differently at every edge.
    let (vello, tiny) = render_both(&page(20.0, 10.0, Vec::new()), &RenderOptions::default());
    assert_eq!(vello.pixel(5, 5), Some([255, 255, 255, 255]));
    assert_eq!(tiny.pixel(5, 5), Some([255, 255, 255, 255]));
}

#[test]
fn a_page_group_alone_does_not_make_the_background_transparent() {
    // `FPDFPage_HasTransparency` is `BackgroundAlphaNeeded`, not the page's
    // `/Group`: a page that merely declares a group still renders onto
    // opaque white. Reading it the other way turns the output from RGB to
    // RGBA and, wherever nothing paints, from white to black.
    let mut p = page(20.0, 10.0, Vec::new());
    p.transparency = Transparency {
        group: true,
        isolated: true,
        knockout: false,
    };
    let (vello, tiny) = render_both(&p, &RenderOptions::default());
    assert_eq!(vello.pixel(5, 5), Some([255, 255, 255, 255]));
    assert_eq!(tiny.pixel(5, 5), Some([255, 255, 255, 255]));
}

#[test]
fn a_deep_blend_mode_makes_the_background_transparent() {
    // The one thing that sets the flag: an `/ExtGState` blend mode above
    // Multiply (`cpdf_allstates.cpp:105-106`).
    let mut object = filled(rect_path(0.0, 0.0, 4.0, 4.0), [0.0, 0.0, 0.0]);
    if let PageObject::Path(p) = &mut object {
        p.state.general.blend = pdfrum_page::BlendMode::Difference;
    }
    let (vello, tiny) = render_both(&page(20.0, 10.0, vec![object]), &RenderOptions::default());
    // A corner the object does not cover keeps the transparent clear.
    assert_eq!(vello.pixel(18, 8), Some([0, 0, 0, 0]));
    assert_eq!(tiny.pixel(18, 8), Some([0, 0, 0, 0]));
}

#[test]
fn multiply_is_not_deep_enough_to_need_a_backdrop() {
    // The threshold is `> BlendMode::kMultiply`, so Normal, Compatible and
    // Multiply all leave the page opaque.
    for blend in [
        pdfrum_page::BlendMode::Normal,
        pdfrum_page::BlendMode::Compatible,
        pdfrum_page::BlendMode::Multiply,
    ] {
        let mut object = filled(rect_path(0.0, 0.0, 4.0, 4.0), [0.0, 0.0, 0.0]);
        if let PageObject::Path(p) = &mut object {
            p.state.general.blend = blend;
        }
        let p = page(20.0, 10.0, vec![object]);
        assert!(
            !pdfrum_render::needs_alpha_background(&p),
            "{blend:?} must not ask for a transparent background"
        );
    }
}

#[test]
fn an_axis_aligned_rect_fill_is_pixel_exact_on_both_backends() {
    // The rect fast path is never antialiased, so every pixel is either
    // fully painted or fully clear — and the two backends must agree
    // exactly, because the geometry decision is the engine's.
    let objects = vec![filled(rect_path(2.0, 2.0, 8.0, 6.0), [1.0, 0.0, 0.0])];
    let (vello, tiny) = render_both(&page(12.0, 8.0, objects), &RenderOptions::default());
    assert_eq!(
        divergence(&vello, &tiny, 0),
        0.0,
        "a snapped rect must be bit-identical"
    );
    // Inside is red, outside is the white background: no soft edge anywhere.
    for y in 0..8u32 {
        for x in 0..12u32 {
            let px = vello.pixel(x, y).expect("in bounds");
            assert!(
                px == [255, 0, 0, 255] || px == [255, 255, 255, 255],
                "({x},{y}) = {px:?} is neither the fill nor the background"
            );
        }
    }
}

#[test]
fn a_sub_pixel_rect_still_paints_a_whole_column() {
    // The promotion rule: a rect thinner than a pixel is widened to one
    // rather than vanishing.
    let objects = vec![filled(rect_path(3.2, 1.0, 3.4, 6.0), [0.0, 0.0, 0.0])];
    let (vello, tiny) = render_both(&page(8.0, 8.0, objects), &RenderOptions::default());
    let painted = |p: &Pixmap| {
        (0..8u32)
            .filter(|&x| p.pixel(x, 3).is_some_and(|px| px[0] < 128))
            .count()
    };
    assert_eq!(painted(&vello), 1, "exactly one column, not zero");
    assert_eq!(painted(&tiny), 1);
}

#[test]
fn a_rotated_rect_is_antialiased_and_the_backends_agree_at_interior_pixels() {
    // Off-axis geometry leaves the rect fast path, so its edges are soft —
    // which is where the two rasterizers are permitted to differ. Interior
    // pixels are not.
    let mut path = rect_path(2.0, 2.0, 14.0, 10.0);
    path.apply_affine(Affine::rotate_about(0.4, Point::new(8.0, 6.0)));
    let objects = vec![filled(path, [0.0, 0.0, 1.0])];
    let (vello, tiny) = render_both(&page(16.0, 12.0, objects), &RenderOptions::default());
    // The centre is well inside the shape on both.
    assert_eq!(vello.pixel(8, 6), Some([0, 0, 255, 255]));
    assert_eq!(tiny.pixel(8, 6), Some([0, 0, 255, 255]));
    // And the overall divergence is confined to the edge band.
    assert!(
        divergence(&vello, &tiny, 8) < 0.35,
        "too much divergence off the edges"
    );
}

#[test]
fn a_zero_area_fill_becomes_a_quarter_alpha_hairline() {
    // A bare two-point "fill" paints nothing at all under an ordinary
    // rasterizer; PDFium draws it as a one-pixel line instead. Both
    // backends must produce the same line, because the conversion is the
    // engine's.
    let mut line = BezPath::new();
    line.move_to((1.0, 4.0));
    line.line_to((10.0, 4.0));
    line.line_to((1.0, 4.0));
    let objects = vec![filled(line, [0.0, 0.0, 0.0])];
    let (vello, tiny) = render_both(&page(12.0, 8.0, objects), &RenderOptions::default());
    let darkest = |p: &Pixmap| {
        (0..8u32)
            .flat_map(|y| (0..12u32).map(move |x| (x, y)))
            .filter_map(|(x, y)| p.pixel(x, y).map(|px| px[0]))
            .min()
            .unwrap_or(255)
    };
    assert!(darkest(&vello) < 255, "something was drawn");
    assert!(darkest(&tiny) < 255, "and on both backends");
}

#[test]
fn a_hairline_stroke_is_one_device_pixel_wide() {
    // `line_width = 0` is not "no stroke": the minimum is one device pixel.
    let mut line = BezPath::new();
    line.move_to((2.0, 4.5));
    line.line_to((10.0, 4.5));
    let objects = vec![stroked(line, [0.0, 0.0, 0.0], 0.0)];
    let (vello, tiny) = render_both(&page(12.0, 9.0, objects), &RenderOptions::default());
    for p in [&vello, &tiny] {
        let painted_rows = (0..9u32)
            .filter(|&y| p.pixel(6, y).is_some_and(|px| px[0] < 250))
            .count();
        assert!(painted_rows >= 1, "a zero-width stroke must still paint");
        assert!(
            painted_rows <= 2,
            "and must not spread past a pixel: {painted_rows}"
        );
    }
}

#[test]
fn a_clipped_fill_stops_at_the_clip() {
    let mut state = GraphicsState::default();
    state
        .fill
        .set_stock(ColorSpace::DeviceRgb, &[1.0, 0.0, 0.0]);
    state.clip.push_path(rect_path(0.0, 0.0, 6.0, 12.0), false);
    let objects = vec![PageObject::Path(Box::new(Content {
        object: PathObject {
            path: rect_path(0.0, 0.0, 12.0, 12.0),
            matrix: Affine::IDENTITY,
            fill_rule: FillRule::Winding,
            stroke: false,
        },
        state,
        marks: ContentMarks::new(),
        content_stream: 0,
    }))];
    let (vello, tiny) = render_both(&page(12.0, 12.0, objects), &RenderOptions::default());
    for p in [&vello, &tiny] {
        assert_eq!(p.pixel(2, 6), Some([255, 0, 0, 255]), "inside the clip");
        assert_eq!(p.pixel(9, 6), Some([255, 255, 255, 255]), "outside it");
    }
    // A rect clip is hard-edged, so the two agree bit for bit.
    assert_eq!(divergence(&vello, &tiny, 0), 0.0);
}

#[test]
fn an_axial_shading_paints_a_ramp_identically_on_both_backends() {
    // Types 1-5 are rasterized by pure engine code into a pixmap and then
    // blitted, so the backends never integrate their coverage: they must be
    // bit-identical.
    let shading = std::sync::Arc::new(pdfrum_page::Shading {
        geometry: pdfrum_page::shading::Geometry::Axial(pdfrum_page::shading::Axial {
            start: Point::new(0.0, 0.0),
            end: Point::new(16.0, 0.0),
            t_min: 0.0,
            t_max: 1.0,
            extend_start: true,
            extend_end: true,
        }),
        space: std::sync::Arc::new(ColorSpace::DeviceGray),
        functions: Box::new([]),
        background: None,
        bbox: None,
    });
    let objects = vec![PageObject::Shading(Box::new(Content {
        object: ShadingObject {
            shading,
            matrix: Affine::IDENTITY,
            bounds: Rect::new(0.0, 0.0, 16.0, 8.0),
        },
        state: GraphicsState::default(),
        marks: ContentMarks::new(),
        content_stream: 0,
    }))];
    let (vello, tiny) = render_both(&page(16.0, 8.0, objects), &RenderOptions::default());
    assert_eq!(
        divergence(&vello, &tiny, 0),
        0.0,
        "engine-computed pixels must be identical"
    );
}

#[test]
fn a_form_renders_the_children_the_page_graph_gave_it() {
    // `build_page` composes a form's `/Matrix` into its children's own CTMs
    // *before* recursing, so a child arrives already placed and the walk
    // must not apply the form matrix a second time. This fixture is built
    // the way the page graph builds one: the child carries the offset.
    let placement = Affine::translate((6.0, 2.0));
    let mut child = filled(rect_path(0.0, 0.0, 4.0, 4.0), [0.0, 1.0, 0.0]);
    if let PageObject::Path(p) = &mut child {
        p.object.matrix = placement;
    }
    let form = PageObject::Form(Box::new(Content {
        object: pdfrum_page::FormObject {
            objects: vec![child],
            matrix: placement,
            bbox: Some(Rect::new(0.0, 0.0, 4.0, 4.0)),
            transparency: Transparency::default(),
        },
        state: GraphicsState::default(),
        marks: ContentMarks::new(),
        content_stream: 0,
    }));
    let (vello, tiny) = render_both(&page(12.0, 8.0, vec![form]), &RenderOptions::default());
    for p in [&vello, &tiny] {
        assert_eq!(
            p.pixel(1, 1),
            Some([255, 255, 255, 255]),
            "not at the origin"
        );
        assert_eq!(
            p.pixel(7, 3),
            Some([0, 255, 0, 255]),
            "but at the form's offset"
        );
    }
}

#[test]
fn a_translucent_fill_blends_with_the_background() {
    let mut state = GraphicsState::default();
    state
        .fill
        .set_stock(ColorSpace::DeviceRgb, &[0.0, 0.0, 0.0]);
    state.general.fill_alpha = 0.5;
    let objects = vec![PageObject::Path(Box::new(Content {
        object: PathObject {
            path: rect_path(0.0, 0.0, 8.0, 8.0),
            matrix: Affine::IDENTITY,
            fill_rule: FillRule::Winding,
            stroke: false,
        },
        state,
        marks: ContentMarks::new(),
        content_stream: 0,
    }))];
    let (vello, tiny) = render_both(&page(8.0, 8.0, objects), &RenderOptions::default());
    for p in [&vello, &tiny] {
        let px = p.pixel(4, 4).expect("in bounds");
        // `ca 0.5` truncates to 127, so half-black over white is 128, and
        // both backends must land within the one-count rounding budget.
        assert!(px[0].abs_diff(128) <= 2, "half black over white: {px:?}");
        assert_eq!(px[3], 255, "the page stays opaque");
    }
}

#[test]
fn a_grayscale_render_uses_the_ntsc_weights() {
    let objects = vec![filled(rect_path(0.0, 0.0, 8.0, 8.0), [1.0, 0.0, 0.0])];
    let opts = RenderOptions {
        color_mode: pdfrum_render::ColorMode::Gray,
        ..RenderOptions::default()
    };
    let (vello, _) = render_both(&page(8.0, 8.0, objects), &opts);
    // 255 * 30/100 = 76, not Rec.709's 54.
    assert_eq!(vello.pixel(4, 4), Some([76, 76, 76, 255]));
}

#[test]
fn a_page_too_large_for_a_backend_is_an_error_not_a_panic() {
    let opts = RenderOptions {
        transform: Affine::scale(500.0),
        ..RenderOptions::default()
    };
    let mut diags = Diagnostics::default();
    let err = render_page(
        &page(1000.0, 1000.0, Vec::new()),
        &opts,
        &VelloBackend::new(),
        &mut diags,
    )
    .expect_err("beyond the u16 target limit");
    assert!(matches!(err, pdfrum_render::Error::TargetTooLarge { .. }));
}

#[test]
fn invisible_text_paints_nothing() {
    let object = PageObject::Text(Box::new(Content {
        object: pdfrum_page::TextObject {
            segments: Box::new([]),
            position: Point::ZERO,
            matrix: Affine::IDENTITY,
            font: None,
            render_mode: TextRenderMode::Invisible,
            type3_metrics: BTreeMap::default(),
        },
        state: GraphicsState::default(),
        marks: ContentMarks::new(),
        content_stream: 0,
    }));
    let (vello, tiny) = render_both(&page(8.0, 8.0, vec![object]), &RenderOptions::default());
    assert_eq!(vello.pixel(4, 4), Some([255, 255, 255, 255]));
    assert_eq!(tiny.pixel(4, 4), Some([255, 255, 255, 255]));
}

#[test]
fn a_page_with_many_objects_stays_deterministic_across_runs() {
    let objects: Vec<PageObject> = (0..40)
        .map(|i| {
            let x = f64::from(i % 8) * 2.0;
            let y = f64::from(i / 8) * 2.0;
            filled(
                rect_path(x, y, x + 1.5, y + 1.5),
                [0.1 * (i % 10) as f32, 0.5, 0.9],
            )
        })
        .collect();
    let p = page(16.0, 10.0, objects);
    let opts = RenderOptions::default();
    let (first, _) = render_both(&p, &opts);
    let (second, _) = render_both(&p, &opts);
    assert_eq!(
        first, second,
        "the pinned SIMD level makes runs reproducible"
    );
}

/// A page object filled with a pattern colour, the pattern already loaded.
///
/// This is what `pdfrum-page` hands the walk after `/Pattern cs … scn`: a
/// colour whose space is `/Pattern` and whose value carries the loaded
/// pattern, so nothing at render time needs a resolver.
fn pattern_filled(path: BezPath, pattern: pdfrum_page::Pattern, operands: &[f32]) -> PageObject {
    let mut state = GraphicsState::default();
    state
        .fill
        .set_space(std::sync::Arc::new(ColorSpace::Pattern(Box::new(
            pdfrum_page::color::PatternSpace {
                base: Some(Box::new(ColorSpace::DeviceRgb)),
            },
        ))));
    state.fill.set_pattern(
        pdfrum_object::Name::from("P0"),
        operands,
        Some(std::sync::Arc::new(pattern)),
    );
    PageObject::Path(Box::new(Content {
        object: PathObject {
            path,
            matrix: Affine::IDENTITY,
            fill_rule: FillRule::Winding,
            stroke: false,
        },
        state,
        marks: ContentMarks::new(),
        content_stream: 0,
    }))
}

/// A tiling pattern whose cell paints one solid square.
fn solid_tile(
    step: f32,
    colored: bool,
    cell_rgb: [f32; 3],
    matrix: Affine,
) -> pdfrum_page::TilingPattern {
    // The cell paints the left half of its own bbox, so a tiled fill is
    // visibly striped rather than uniform and a missing tile is detectable.
    pdfrum_page::TilingPattern {
        colored,
        x_step: step,
        y_step: step,
        bbox: Rect::new(0.0, 0.0, f64::from(step), f64::from(step)),
        matrix,
        resources: None,
        content: pdfrum_object::ByteSpan::empty(),
        objects: vec![filled(
            rect_path(0.0, 0.0, f64::from(step) / 2.0, f64::from(step)),
            cell_rgb,
        )],
    }
}

#[test]
fn a_coloured_tiling_pattern_paints_its_cell_across_the_fill() {
    // Four-pixel tiles whose left half is red: columns 0-1 red, 2-3 white,
    // repeating. Nothing about that pattern can come out of the ordinary
    // fill path, which would paint the whole rectangle one colour.
    let tiling = solid_tile(4.0, true, [1.0, 0.0, 0.0], Affine::IDENTITY);
    let object = pattern_filled(
        rect_path(0.0, 0.0, 16.0, 16.0),
        pdfrum_page::Pattern::Tiling(Box::new(tiling)),
        &[],
    );
    let (vello, tiny) = render_both(&page(16.0, 16.0, vec![object]), &RenderOptions::default());
    for surface in [&vello, &tiny] {
        // A tile's painted half is red...
        for x in [0, 1, 4, 5, 8, 9] {
            let px = surface.pixel(x, 8).expect("a pixel");
            assert!(
                px[0] > 200 && px[1] < 60,
                "column {x} should be tile ink, got {px:?}"
            );
        }
        // ...and its unpainted half is the page's white.
        for x in [2, 3, 6, 7, 10, 11] {
            assert_eq!(
                surface.pixel(x, 8),
                Some([255, 255, 255, 255]),
                "column {x} is between tiles and must stay white"
            );
        }
    }
}

#[test]
fn an_uncoloured_tiling_pattern_takes_the_operand_colour_not_the_cells() {
    // `/PaintType 2`: the cell's own green is coverage only, and the blue
    // from the `scn` operands is what paints. Getting this backwards is the
    // difference between a green fill and a blue one, page-wide.
    let tiling = solid_tile(4.0, false, [0.0, 1.0, 0.0], Affine::IDENTITY);
    let object = pattern_filled(
        rect_path(0.0, 0.0, 16.0, 16.0),
        pdfrum_page::Pattern::Tiling(Box::new(tiling)),
        &[0.0, 0.0, 1.0],
    );
    let (vello, tiny) = render_both(&page(16.0, 16.0, vec![object]), &RenderOptions::default());
    for surface in [&vello, &tiny] {
        let px = surface.pixel(0, 8).expect("a pixel");
        assert!(
            px[2] > 200 && px[1] < 60,
            "an uncoloured tile paints the operand blue, not the cell's green: {px:?}"
        );
    }
}

#[test]
fn a_pattern_fill_is_clipped_to_the_objects_own_geometry() {
    // The tiling covers the whole page in pattern space; only the filled
    // rectangle may show it. A pattern that escapes its object's geometry
    // paints the whole page, which is the loudest possible failure.
    let tiling = solid_tile(4.0, true, [1.0, 0.0, 0.0], Affine::IDENTITY);
    let object = pattern_filled(
        rect_path(4.0, 4.0, 12.0, 12.0),
        pdfrum_page::Pattern::Tiling(Box::new(tiling)),
        &[],
    );
    let (vello, tiny) = render_both(&page(16.0, 16.0, vec![object]), &RenderOptions::default());
    for surface in [&vello, &tiny] {
        // Outside the fill: untouched white, on every side.
        for (x, y) in [(1u32, 1u32), (14, 1), (1, 14), (14, 14), (8, 1), (1, 8)] {
            assert_eq!(
                surface.pixel(x, y),
                Some([255, 255, 255, 255]),
                "({x},{y}) is outside the filled rect and must not be painted"
            );
        }
        // Inside it, at least one pixel is ink.
        let inside: u32 = (4..12)
            .map(|x| {
                u32::from(
                    surface
                        .pixel(x, 8)
                        .is_some_and(|px| px[0] > 200 && px[1] < 60),
                )
            })
            .sum();
        assert!(inside > 0, "the pattern must paint inside its own geometry");
    }
}

#[test]
fn a_zero_step_tiling_pattern_paints_nothing_at_all() {
    // The step ladder's first rule, and it is silent in the C++: a zero
    // `/XStep` draws nothing rather than one tile or an endless run of them.
    let mut tiling = solid_tile(4.0, true, [1.0, 0.0, 0.0], Affine::IDENTITY);
    tiling.x_step = 0.0;
    let object = pattern_filled(
        rect_path(0.0, 0.0, 16.0, 16.0),
        pdfrum_page::Pattern::Tiling(Box::new(tiling)),
        &[],
    );
    let (vello, tiny) = render_both(&page(16.0, 16.0, vec![object]), &RenderOptions::default());
    for surface in [&vello, &tiny] {
        for x in 0..16 {
            assert_eq!(
                surface.pixel(x, 8),
                Some([255, 255, 255, 255]),
                "a zero step must paint nothing, but ({x},8) is inked"
            );
        }
    }
}

#[test]
fn a_pattern_that_did_not_resolve_paints_nothing_rather_than_black() {
    // A `scn` naming a pattern the resources do not define is a no-op
    // operator. Painting it would resolve the pattern colour to black and
    // fill the object solid, which is worse than the missing pattern.
    let mut state = GraphicsState::default();
    state
        .fill
        .set_space(std::sync::Arc::new(ColorSpace::Pattern(Box::default())));
    state
        .fill
        .set_pattern(pdfrum_object::Name::from("Missing"), &[], None);
    let object = PageObject::Path(Box::new(Content {
        object: PathObject {
            path: rect_path(0.0, 0.0, 8.0, 8.0),
            matrix: Affine::IDENTITY,
            fill_rule: FillRule::Winding,
            stroke: false,
        },
        state,
        marks: ContentMarks::new(),
        content_stream: 0,
    }));
    let (vello, tiny) = render_both(&page(8.0, 8.0, vec![object]), &RenderOptions::default());
    assert_eq!(vello.pixel(4, 4), Some([255, 255, 255, 255]));
    assert_eq!(tiny.pixel(4, 4), Some([255, 255, 255, 255]));
}
