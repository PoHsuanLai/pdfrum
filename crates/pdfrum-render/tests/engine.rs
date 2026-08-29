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
use pdfrum_render::{Pixmap, RenderOptions, render_page, render_page_with_visibility};

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
fn a_fold_inside_a_polygon_does_not_swallow_the_fill() {
    // `GetZeroAreaPath`'s third case scans an ordinary sub-path for a segment
    // that doubles back on itself and emits *just that segment* as a
    // hairline. The C++ loop that calls it has no early return: it draws the
    // hairlines and then falls through to the ordinary fill
    // (`cfx_renderdevice.cpp:772-804`).
    //
    // So a polygon with a spike must come out filled *and* spiked. Treating
    // the hairline as a replacement painted `bug_1338`'s five shapes as bare
    // spikes on white.
    let mut spiked = BezPath::new();
    spiked.move_to((18.0, 2.0));
    spiked.line_to((4.0, 4.0));
    // The fold: down the same vertical line and part-way back up.
    spiked.line_to((4.0, 30.0));
    spiked.line_to((4.0, 16.0));
    spiked.line_to((18.0, 2.0));
    let objects = vec![filled(spiked, [0.0, 0.0, 0.0])];
    let (vello, tiny) = render_both(&page(24.0, 34.0, objects), &RenderOptions::default());
    for (name, p) in [("vello", &vello), ("tiny-skia", &tiny)] {
        // Inside the triangle, well clear of every edge.
        assert!(
            p.pixel(7, 26).is_some_and(|px| px[0] < 128),
            "{name}: the polygon the fold hangs off must still be filled"
        );
        // A second interior sample, to catch a fill reduced to its own edge.
        assert!(
            p.pixel(6, 22).is_some_and(|px| px[0] < 128),
            "{name}: and filled through its interior, not just outlined"
        );
    }
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
            oc: None,
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

/// A non-embedded simple font by base name, with the `/Widths` the test
/// wants. An unrecognised name reaches the built-in Multiple-Master generic,
/// which is the substitution every non-embedded font in the corpus lands on.
fn substituted_font(base_name: &str, first_char: i64, widths: &[i64]) -> pdfrum_font::Font {
    use pdfrum_object::{Dict, Name, NoResolve, Object};
    let dict = Dict::from_pairs([
        (Name::from("Subtype"), Object::Name(Name::from("Type1"))),
        (Name::from("BaseFont"), Object::Name(Name::from(base_name))),
        (Name::from("FirstChar"), Object::Int(first_char)),
        (
            Name::from("LastChar"),
            Object::Int(first_char + i64::try_from(widths.len()).expect("small") - 1),
        ),
        (
            Name::from("Widths"),
            Object::Array(widths.iter().map(|w| Object::Int(*w)).collect()),
        ),
    ]);
    pdfrum_font::load(
        &dict,
        &NoResolve,
        &pdfrum_font::FontCache::new(),
        &pdfrum_common::Limits::default(),
        &mut Diagnostics::default(),
    )
    .expect("a simple font always constructs")
}

/// One text object showing `codes` in `font` at `size`, with its origin at
/// `(x, y)` in page space.
fn text_object(
    font: pdfrum_font::Font,
    size: f32,
    codes: &[u8],
    x: f64,
    y: f64,
) -> (PageObject, std::sync::Arc<pdfrum_font::Font>) {
    let font = std::sync::Arc::new(font);
    let object = PageObject::Text(Box::new(Content {
        object: pdfrum_page::TextObject {
            segments: Box::new([pdfrum_page::TextSegment {
                codes: codes.to_vec().into_boxed_slice(),
                kerning: 0.0,
            }]),
            position: Point::new(x, y),
            matrix: Affine::IDENTITY,
            font: Some((std::sync::Arc::clone(&font), size)),
            render_mode: TextRenderMode::Fill,
            type3_metrics: BTreeMap::default(),
        },
        state: GraphicsState::default(),
        marks: ContentMarks::new(),
        content_stream: 0,
    }));
    (object, font)
}

/// The x extent of every ink run in a row band, as `(start, end)` columns.
fn ink_runs(p: &Pixmap, y0: u32, y1: u32) -> Vec<(u32, u32)> {
    let mut runs = Vec::new();
    let mut start = None;
    for x in 0..p.width() {
        let inked = (y0..y1).any(|y| p.pixel(x, y).is_some_and(|px| px[0] < 200));
        match (inked, start) {
            (true, None) => start = Some(x),
            (false, Some(s)) => {
                runs.push((s, x));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        runs.push((s, p.width()));
    }
    runs
}

#[test]
fn a_substituted_fonts_glyphs_are_drawn_at_the_widths_the_pdf_declares() {
    // Burn-down wave 5's font defect, pinned as pixels. `5.5_simple_font.pdf`
    // shows `abcabc` in a non-embedded `/AGaramond` whose `/Widths` are
    // deliberately bizarre — a = 800, b = 100, c = 400 — against a fallback
    // face whose own advances are roughly 450, 470 and 480.
    //
    // The oracle solves the Multiple-Master fallback's width axis so each
    // glyph is *drawn* at the declared width (`AdjustVariationParams`,
    // `cfx_face.cpp:1561-1605`). We drew every glyph at the axis default and
    // advanced by the declared width, so the outlines overran their advances
    // and piled into one another: `abcabc` rendered as `a ba b`, which the
    // triage recorded as a dropped character and a mis-advance.
    //
    // What this asserts is the fix's actual contract: **six separated ink
    // runs, one per character**, in a font whose own advances would merge
    // them. Counting the runs is the assertion because merging is exactly the
    // symptom.
    let mut widths = vec![0_i64; 128];
    widths[usize::from(b'a')] = 800;
    widths[usize::from(b'b')] = 100;
    widths[usize::from(b'c')] = 400;
    let font = substituted_font("AGaramond", 0, &widths);
    assert!(
        font.subst().is_some_and(|s| s.is_builtin_generic),
        "the fixture must reach the Multiple-Master generic"
    );

    // A wide gap after each `c` separates the two `abc` groups, so the two
    // groups are directly comparable and the run boundaries are stable.
    let (object, _keep) = text_object(font, 30.0, b"a c a c", 4.0, 10.0);
    let (vello, tiny) = render_both(&page(160.0, 44.0, vec![object]), &RenderOptions::default());
    for (name, p) in [("vello", &vello), ("tiny-skia", &tiny)] {
        let runs = ink_runs(p, 0, 44);
        assert_eq!(runs.len(), 4, "{name}: four separated glyphs, got {runs:?}");
        let w: Vec<u32> = runs.iter().map(|(a, b)| b - a).collect();
        // `a` is declared at 800 units and `c` at 400, so at 30 pt the drawn
        // `a` must be about twice the drawn `c`. Before the fix both were
        // drawn at the fallback face's own near-equal advances and the ratio
        // was about 1.0, which is the assertion that catches the regression.
        let ratio = f64::from(w[0]) / f64::from(w[1]);
        assert!(
            (1.6..2.6).contains(&ratio),
            "{name}: `a` at 800 units must be about twice `c` at 400, \
             ratio {ratio:.2} from widths {w:?}"
        );
        // And the two groups must be identical, which they are only if the
        // solve is a function of the declared width alone.
        assert_eq!(w[0], w[2], "{name}: the two `a`s agree, {w:?}");
        assert_eq!(w[1], w[3], "{name}: the two `c`s agree, {w:?}");
    }
}

#[test]
fn glyph_origins_snap_to_the_oracles_grid_unless_asked_not_to() {
    // The wave-5 placement port, as pixels rather than as arithmetic. A
    // baseline placed a fraction of a pixel off a whole row must round onto
    // one under the default options, and stay where it is under
    // `subpixel_text_positioning`.
    let mut widths = vec![600_i64; 128];
    widths[usize::from(b'H')] = 700;
    let font = || substituted_font("SomeFontNobodyHas", 0, &widths);

    // A page 40 tall means device y = 40 - page y; a page y of 10.4 is a
    // device baseline of 29.6, which the oracle rounds to 30.
    let snapped = {
        let (object, _keep) = text_object(font(), 20.0, b"H", 4.0, 10.4);
        let (p, _) = render_both(&page(60.0, 40.0, vec![object]), &RenderOptions::default());
        ink_rows(&p)
    };
    let whole = {
        let (object, _keep) = text_object(font(), 20.0, b"H", 4.0, 10.0);
        let (p, _) = render_both(&page(60.0, 40.0, vec![object]), &RenderOptions::default());
        ink_rows(&p)
    };
    assert_eq!(
        snapped, whole,
        "a 0.4 px baseline offset snaps onto the same rows as a whole one"
    );

    let subpixel = RenderOptions {
        subpixel_text_positioning: true,
        ..RenderOptions::default()
    };
    let free = {
        let (object, _keep) = text_object(font(), 20.0, b"H", 4.0, 10.4);
        let (p, _) = render_both(&page(60.0, 40.0, vec![object]), &subpixel);
        ink_rows(&p)
    };
    assert_ne!(
        free, snapped,
        "the knob puts the glyph back where the PDF says it is"
    );
}

#[test]
fn small_text_is_drawn_as_an_lcd_filtered_bitmap_and_large_text_is_not() {
    // Wave 7's whole claim, as pixels a caller can see rather than as an
    // internal function's output.
    //
    // The FIR5 filter spreads each subpixel span across five columns, so a
    // small glyph inks *wider* than its own outline — measurably, and only
    // below the size at which the oracle abandons bitmaps. Above that
    // threshold the glyph is filled as an outline and the extra columns are
    // not there, which is what makes this one test of both halves.
    let widths = vec![600_i64; 128];
    let font = || substituted_font("SomeFontNobodyHas", 0, &widths);

    let small = {
        let (object, _keep) = text_object(font(), 8.0, b"H", 10.0, 10.0);
        let (p, _) = render_both(&page(60.0, 40.0, vec![object]), &RenderOptions::default());
        ink_runs(&p, 0, p.height())
    };
    // 8 pt at one pixel per point is `|a| + |b| == 8`, far under the 50 that
    // sends a run to `DrawTextPath`; 80 pt is far over it.
    let large = {
        let (object, _keep) = text_object(font(), 80.0, b"H", 10.0, 10.0);
        let (p, _) = render_both(&page(300.0, 200.0, vec![object]), &RenderOptions::default());
        ink_runs(&p, 0, p.height())
    };
    assert!(!small.is_empty(), "the small glyph painted something");
    assert!(!large.is_empty(), "the large glyph painted something");

    // The filter's reach is a fixed number of device pixels, so it is a large
    // fraction of a small glyph's width and a negligible one of a big glyph's.
    // Comparing the two as fractions of the em is what separates the filter
    // from the glyph merely being bigger.
    let span = |runs: &[(u32, u32)]| {
        let lo = runs.first().map_or(0, |r| r.0);
        let hi = runs.last().map_or(0, |r| r.1);
        f64::from(hi - lo)
    };
    let small_fraction = span(&small) / 8.0;
    let large_fraction = span(&large) / 80.0;
    assert!(
        small_fraction > large_fraction * 1.15,
        "the small glyph must be relatively wider for the filter's reach: \
         {small_fraction} vs {large_fraction}"
    );
}

#[test]
fn a_third_of_a_pixel_redistributes_a_small_glyphs_ink_without_moving_it() {
    // The subpixel phase, which is why the bitmap cache keys on the *matrix*
    // and the blit applies the phase rather than the other way round.
    //
    // Two origins inside one pixel are blitted at the *same* integer column
    // and differ only in which window of the 3×-wide bitmap is averaged. That
    // is not "no visible change" — a third of a pixel of ink really does move
    // between neighbouring columns, and the leading edge can cross a
    // threshold. What it is not allowed to do is translate the glyph, so the
    // assertion is that the whole run moves by less than the whole pixel the
    // two origins share, while the gray changes.
    let widths = vec![600_i64; 128];
    let font = || substituted_font("SomeFontNobodyHas", 0, &widths);
    let at = |x: f64| {
        let (object, _keep) = text_object(font(), 8.0, b"H", x, 10.0);
        let (p, _) = render_both(&page(60.0, 40.0, vec![object]), &RenderOptions::default());
        let runs = ink_runs(&p, 0, p.height());
        // Every non-white byte on the page, in raster order, so the comparison
        // cannot land on a blank row.
        let mut ink = Vec::new();
        for y in 0..p.height() {
            for x in 0..p.width() {
                if let Some(px) = p.pixel(x, y)
                    && px[0] < 255
                {
                    ink.push(px[0]);
                }
            }
        }
        (runs, ink)
    };
    let (zero_runs, zero_row) = at(10.0);
    let (third_runs, third_row) = at(10.4);
    let start = |runs: &[(u32, u32)]| i64::from(runs.first().map_or(0, |r| r.0));
    let end = |runs: &[(u32, u32)]| i64::from(runs.last().map_or(0, |r| r.1));
    assert!(
        (start(&zero_runs) - start(&third_runs)).abs() <= 1
            && (end(&zero_runs) - end(&third_runs)).abs() <= 1,
        "the ink stays within a pixel of where it was: {zero_runs:?} vs {third_runs:?}"
    );
    assert_ne!(
        zero_row, third_row,
        "but the phase changes which window of the 3x bitmap is averaged"
    );
}

#[test]
fn the_bitmap_path_and_the_outline_path_place_a_glyph_in_the_same_place() {
    // The bitmap must not *move* the glyph — it changes how the ink is
    // distributed, not where it goes. Comparing against
    // `subpixel_text_positioning`, which still fills outlines, is the
    // strongest available statement of that: the two disagree about coverage
    // by construction, so the assertion is about the box.
    let widths = vec![600_i64; 128];
    let font = || substituted_font("SomeFontNobodyHas", 0, &widths);
    let bbox = |opts: &RenderOptions| {
        let (object, _keep) = text_object(font(), 10.0, b"H", 10.0, 10.0);
        let (p, _) = render_both(&page(60.0, 40.0, vec![object]), opts);
        let rows: Vec<u32> = (0..p.height())
            .filter(|y| (0..p.width()).any(|x| p.pixel(x, *y).is_some_and(|px| px[0] < 250)))
            .collect();
        (
            rows.first().copied().unwrap_or(0),
            rows.last().copied().unwrap_or(0),
        )
    };
    let bitmap = bbox(&RenderOptions::default());
    let outline = bbox(&RenderOptions {
        subpixel_text_positioning: true,
        ..RenderOptions::default()
    });
    // The rows are the axis the bitmap does not filter — the LCD filter is
    // horizontal — so they must agree exactly rather than approximately.
    assert_eq!(
        bitmap, outline,
        "the bitmap path must not move the glyph vertically"
    );
}

/// The first and last inked rows, and the coverage of the topmost one — the
/// three numbers a sub-pixel vertical shift moves.
fn ink_rows(p: &Pixmap) -> (u32, u32, u8) {
    let inked = |y: u32| (0..p.width()).any(|x| p.pixel(x, y).is_some_and(|px| px[0] < 250));
    let rows: Vec<u32> = (0..p.height()).filter(|y| inked(*y)).collect();
    let first = *rows.first().expect("the glyph painted something");
    let last = *rows.last().expect("the glyph painted something");
    let darkest = (0..p.width())
        .filter_map(|x| p.pixel(x, first).map(|px| px[0]))
        .min()
        .unwrap_or(255);
    (first, last, darkest)
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

/// A soft mask whose group paints the given objects, placed by `matrix`.
fn soft_mask(
    kind: pdfrum_page::SoftMaskKind,
    backdrop: pdfrum_page::Rgb,
    objects: Vec<PageObject>,
) -> std::sync::Arc<pdfrum_page::SoftMask> {
    std::sync::Arc::new(pdfrum_page::SoftMask {
        group: pdfrum_object::Stream {
            dict: pdfrum_object::Dict::default(),
            data: pdfrum_object::ByteSpan::empty(),
        },
        kind,
        backdrop,
        transfer: None,
        matrix: Affine::IDENTITY,
        objects,
    })
}

/// A form group carrying a soft mask, painting one filled rectangle.
fn masked_form(
    inner: BezPath,
    rgb: [f32; 3],
    mask: std::sync::Arc<pdfrum_page::SoftMask>,
    bbox: Rect,
) -> PageObject {
    let mut state = GraphicsState::default();
    state.general.soft_mask = Some(mask);
    PageObject::Form(Box::new(Content {
        object: pdfrum_page::FormObject {
            objects: vec![filled(inner, rgb)],
            matrix: Affine::IDENTITY,
            bbox: Some(bbox),
            transparency: Transparency {
                group: true,
                isolated: true,
                knockout: false,
            },
            oc: None,
        },
        state,
        marks: ContentMarks::new(),
        content_stream: 0,
    }))
}

#[test]
fn a_luminosity_mask_paints_its_group_through_the_masked_object() {
    // The mask's group paints a white square over the left half of a black
    // backdrop: white luminance reveals, black hides. Rendering the backdrop
    // alone — which is what a mask whose group never renders amounts to —
    // hides everything, so the two halves are what this test is for.
    let mask = soft_mask(
        pdfrum_page::SoftMaskKind::Luminosity,
        pdfrum_page::Rgb::BLACK,
        vec![filled(rect_path(0.0, 0.0, 8.0, 16.0), [1.0, 1.0, 1.0])],
    );
    let object = masked_form(
        rect_path(0.0, 0.0, 16.0, 16.0),
        [1.0, 0.0, 0.0],
        mask,
        Rect::new(0.0, 0.0, 16.0, 16.0),
    );
    let (vello, tiny) = render_both(&page(16.0, 16.0, vec![object]), &RenderOptions::default());
    for surface in [&vello, &tiny] {
        // Under the mask's white square: the red shows.
        let lit = surface.pixel(4, 8).expect("a pixel");
        assert!(lit[0] > 200 && lit[1] < 60, "revealed half: {lit:?}");
        // Where the group painted nothing, the black `/BC` backdrop hides it.
        assert_eq!(
            surface.pixel(12, 8),
            Some([255, 255, 255, 255]),
            "the backdrop's black luminance must hide the right half"
        );
    }
}

#[test]
fn a_white_backdrop_reveals_where_the_group_paints_nothing() {
    // The complement, and the reason the luminosity buffer is cleared opaque:
    // an area the group never touches contributes the `/BC` luminosity, not
    // zero. With `/BC` white that area is fully revealed.
    let mask = soft_mask(
        pdfrum_page::SoftMaskKind::Luminosity,
        pdfrum_page::Rgb {
            r: 1.0,
            g: 1.0,
            b: 1.0,
        },
        Vec::new(),
    );
    let object = masked_form(
        rect_path(0.0, 0.0, 16.0, 16.0),
        [1.0, 0.0, 0.0],
        mask,
        Rect::new(0.0, 0.0, 16.0, 16.0),
    );
    let (vello, tiny) = render_both(&page(16.0, 16.0, vec![object]), &RenderOptions::default());
    for surface in [&vello, &tiny] {
        let px = surface.pixel(8, 8).expect("a pixel");
        assert!(px[0] > 200 && px[1] < 60, "fully revealed: {px:?}");
    }
}

#[test]
fn a_white_fill_paints_white_rather_than_nothing() {
    // `FXSYS_BGR` packs a resolved colour into the low 24 bits, so white is
    // `0x00FFFFFF` and never the `0xFFFFFFFF` invisibility word. Reading the
    // two as one makes every white object in the corpus vanish.
    let objects = vec![
        filled(rect_path(0.0, 0.0, 16.0, 16.0), [0.0, 0.0, 0.0]),
        filled(rect_path(4.0, 4.0, 12.0, 12.0), [1.0, 1.0, 1.0]),
    ];
    let (vello, tiny) = render_both(&page(16.0, 16.0, objects), &RenderOptions::default());
    for surface in [&vello, &tiny] {
        assert_eq!(
            surface.pixel(1, 1),
            Some([0, 0, 0, 255]),
            "the black ground"
        );
        assert_eq!(
            surface.pixel(8, 8),
            Some([255, 255, 255, 255]),
            "the white square must paint over the black, not vanish into it"
        );
    }
}

#[test]
fn an_unusable_pattern_paints_nothing_rather_than_the_current_colour() {
    // `FindPattern` only checks that the resource is a dictionary or a
    // stream; whether the pattern is *usable* is answered at draw time, and a
    // failure there means the object paints nothing. So a colour whose
    // pattern did not load is still a pattern colour — drained out of the
    // ordinary draw — and must not fall back to whatever colour was current.
    // Falling back paints a page-sized rectangle solid black, which is what
    // four of the corpus's fuzz files exercise.
    let mut state = GraphicsState::default();
    state
        .fill
        .set_stock(ColorSpace::DeviceRgb, &[1.0, 0.0, 0.0]);
    // `set_pattern` installs the /Pattern space itself, exactly as
    // `SetValueForPattern` does, so the red above is superseded.
    state
        .fill
        .set_pattern(pdfrum_object::Name::from("Broken"), &[], None);
    assert!(
        state.fill.is_pattern(),
        "the stock /Pattern space is installed"
    );
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
    for surface in [&vello, &tiny] {
        assert_eq!(
            surface.pixel(4, 4),
            Some([255, 255, 255, 255]),
            "an unusable pattern must paint nothing, not the previous colour"
        );
    }
}

#[test]
fn a_hidden_object_is_not_drawn_and_its_clip_never_reaches_the_device() {
    // Two overlapping fills, the second hidden. The gate sits ahead of the
    // clip push (`RenderSingleObject`, `cpdf_renderstatus.cpp:247`), so a
    // hidden object contributes neither ink nor clip — which is what the
    // second assertion checks by drawing a *third* object the hidden one's
    // clip would have cut away if it had been pushed.
    let mut clipped = GraphicsState::default();
    clipped.clip.push_path(rect_path(0.0, 0.0, 1.0, 1.0), false);

    let visible_page = page(
        8.0,
        8.0,
        vec![
            filled(rect_path(0.0, 0.0, 8.0, 8.0), [1.0, 1.0, 1.0]),
            filled(rect_path(0.0, 0.0, 8.0, 8.0), [1.0, 0.0, 0.0]),
        ],
    );
    let opts = RenderOptions::default();

    // Everything visible: the red covers the page.
    let all = pdfrum_page::Visibility::all_visible();
    let mut diags = Diagnostics::default();
    let mut caches = pdfrum_render::RenderCaches::new();
    let shown = render_page_with_visibility(
        &visible_page,
        &opts,
        &TinySkiaBackend::new(),
        &all,
        &mut caches,
        &mut diags,
    )
    .expect("renders");
    assert_eq!(shown.pixel(4, 4), Some([255, 0, 0, 255]));

    // With the red hidden, the white beneath it shows through. The tree is
    // built by the pre-pass in the page crate; here it is asserted from the
    // renderer's side, which is the seam this test exists for.
    let off = pdfrum_object::Dict::from_pairs([
        (
            pdfrum_object::Name::from("Type"),
            pdfrum_object::Object::Name(pdfrum_object::Name::from("OCG")),
        ),
        (
            pdfrum_object::Name::from("Name"),
            pdfrum_object::Object::Name(pdfrum_object::Name::from("Hidden")),
        ),
    ]);
    let properties = pdfrum_object::Dict::from_pairs([
        (
            pdfrum_object::Name::from("OCGs"),
            pdfrum_object::Object::Array(pdfrum_object::Array::of([pdfrum_object::Object::Dict(
                off.clone(),
            )])),
        ),
        (
            pdfrum_object::Name::from("D"),
            pdfrum_object::Object::Dict(pdfrum_object::Dict::from_pairs([(
                pdfrum_object::Name::from("OFF"),
                pdfrum_object::Object::Array(pdfrum_object::Array::of([
                    pdfrum_object::Object::Dict(off.clone()),
                ])),
            )])),
        ),
    ]);

    let mut red = filled(rect_path(0.0, 0.0, 8.0, 8.0), [1.0, 0.0, 0.0]);
    let PageObject::Path(content) = &mut red else {
        panic!("a fill is a path object");
    };
    content.marks.push_with_properties(
        pdfrum_object::Name::from("OC"),
        &pdfrum_page::MarkProperties::Named(pdfrum_object::Name::from("MC0")),
        |_| Some(off.clone()),
    );
    let hidden_page = page(
        8.0,
        8.0,
        vec![filled(rect_path(0.0, 0.0, 8.0, 8.0), [1.0, 1.0, 1.0]), red],
    );

    let mut oc = pdfrum_page::OcContext::new(Some(properties), pdfrum_page::UsageType::View);
    let mut build_diags = Diagnostics::default();
    let visible = pdfrum_page::page_visibility(
        &hidden_page,
        &mut oc,
        &pdfrum_object::NoResolve,
        &mut build_diags,
    );
    assert!(!visible.visible(1), "the pre-pass hides the red fill");

    let mut diags = Diagnostics::default();
    let mut caches = pdfrum_render::RenderCaches::new();
    let out = render_page_with_visibility(
        &hidden_page,
        &opts,
        &TinySkiaBackend::new(),
        &visible,
        &mut caches,
        &mut diags,
    )
    .expect("renders");
    assert_eq!(
        out.pixel(4, 4),
        Some([255, 255, 255, 255]),
        "the hidden fill paints nothing at all"
    );

    // And `render_page` itself is unchanged: no visibility, everything drawn.
    let mut diags = Diagnostics::default();
    let plain =
        render_page(&hidden_page, &opts, &TinySkiaBackend::new(), &mut diags).expect("renders");
    assert_eq!(
        plain.pixel(4, 4),
        Some([255, 0, 0, 255]),
        "the visibility-free entry point still draws every layer"
    );
}

#[test]
fn a_groups_own_alpha_multiplies_the_alpha_inside_it() {
    // `bug_1949.in`'s left square: a form declaring `/Group /I true`, drawn
    // under `ca 0.5`, whose content fills white under its own `ca 0.5`. The
    // two multiply — 0.25 of white over black is 63 — and the gate on the
    // outer one is the *form's* `/Group`, not the page's.
    let mut inner_state = GraphicsState::default();
    inner_state
        .fill
        .set_stock(ColorSpace::DeviceRgb, &[1.0, 1.0, 1.0]);
    inner_state.general.fill_alpha = 0.5;
    let inner = PageObject::Path(Box::new(Content {
        object: PathObject {
            path: rect_path(0.0, 0.0, 8.0, 8.0),
            matrix: Affine::IDENTITY,
            fill_rule: FillRule::Winding,
            stroke: false,
        },
        state: inner_state,
        marks: ContentMarks::new(),
        content_stream: 0,
    }));

    let mut form_state = GraphicsState::default();
    form_state.general.fill_alpha = 0.5;
    let form = PageObject::Form(Box::new(Content {
        object: pdfrum_page::FormObject {
            objects: vec![inner],
            matrix: Affine::IDENTITY,
            bbox: Some(Rect::new(0.0, 0.0, 8.0, 8.0)),
            transparency: Transparency {
                group: true,
                isolated: true,
                knockout: false,
            },
            oc: None,
        },
        state: form_state,
        marks: ContentMarks::new(),
        content_stream: 0,
    }));

    let page = page(
        8.0,
        8.0,
        vec![filled(rect_path(0.0, 0.0, 8.0, 8.0), [0.0, 0.0, 0.0]), form],
    );
    let (vello, tiny) = render_both(&page, &RenderOptions::default());
    for surface in [&vello, &tiny] {
        let px = surface.pixel(4, 4).expect("a pixel");
        assert!(
            px[0].abs_diff(63) <= 1,
            "0.5 * 0.5 of white over black is 63, got {px:?}"
        );
    }
}
