//! What the backend does on a real device.
//!
//! Every test here needs a GPU, and PLAN.md §M12c's guardrails are explicit
//! about what that means for a suite that may run without one: **skip with a
//! clear message, never fail, never hang.** So each test opens a device
//! through [`try_real_gpu`], which refuses a software adapter as firmly as no
//! adapter at all, and returns early with a printed reason when there is none.
//! `cargo nextest run` shows the message; CI stays green.
//!
//! These are correctness assertions, not measurements. The numbers live in
//! `docs/status/M12c.md` §8, where they can carry the adapter they were taken
//! on.

use kurbo::{Affine, BezPath, Rect};
use pdfrum_page::BlendMode;
use pdfrum_raster_vello::{Error, VelloBackend, try_real_gpu};
use pdfrum_render::{
    AlphaMask, AntiAlias, Brush, FillRule, ImageQuality, Pixmap, RasterBackend, RenderDevice,
};

/// A backend on this machine's GPU, or `None` with the reason printed.
fn gpu() -> Option<VelloBackend<'static>> {
    let found = try_real_gpu();
    if found.is_none() {
        println!("skipping: no hardware wgpu adapter on this machine");
    }
    found
}

fn square(x0: f64, y0: f64, x1: f64, y1: f64) -> BezPath {
    let mut p = BezPath::new();
    p.move_to((x0, y0));
    p.line_to((x1, y0));
    p.line_to((x1, y1));
    p.line_to((x0, y1));
    p.close_path();
    p
}

#[test]
fn a_cleared_target_keeps_its_colour() {
    let Some(backend) = gpu() else { return };
    let device = backend.new_target(16, 16, peniko::Color::WHITE);
    let out = backend.finish(device);
    assert_eq!(out.pixel(8, 8), Some([255, 255, 255, 255]));
}

#[test]
fn a_solid_fill_paints_where_it_should_and_nowhere_else() {
    let Some(backend) = gpu() else { return };
    let mut device = backend.new_target(64, 64, peniko::Color::TRANSPARENT);
    device.fill_path(
        &square(16.0, 16.0, 48.0, 48.0),
        Affine::IDENTITY,
        &Brush::Solid(peniko::Color::from_rgba8(255, 0, 0, 255)),
        FillRule::Winding,
        AntiAlias::On,
    );
    let out = backend.finish(device);
    assert_eq!(out.pixel(32, 32), Some([255, 0, 0, 255]), "inside");
    assert_eq!(out.pixel(4, 4), Some([0, 0, 0, 0]), "outside");
}

#[test]
fn a_clip_confines_a_fill() {
    let Some(backend) = gpu() else { return };
    let mut device = backend.new_target(64, 16, peniko::Color::TRANSPARENT);
    device.push_clip_rect(Rect::new(0.0, 0.0, 32.0, 16.0));
    device.fill_path(
        &square(0.0, 0.0, 64.0, 16.0),
        Affine::IDENTITY,
        &Brush::Solid(peniko::Color::from_rgba8(0, 0, 255, 255)),
        FillRule::Winding,
        AntiAlias::On,
    );
    device.pop();
    let out = backend.finish(device);
    assert_eq!(out.pixel(8, 8), Some([0, 0, 255, 255]), "inside the clip");
    assert_eq!(out.pixel(48, 8), Some([0, 0, 0, 0]), "outside it");
}

#[test]
fn a_clip_and_a_layer_unwind_in_the_right_order() {
    // The structural difference from `vello_cpu`, exercised end to end: one
    // stack here rather than two, so a `pop` that guessed the wrong kind would
    // leave the scene's nesting wrong and the clip would outlive the layer.
    let Some(backend) = gpu() else { return };
    let mut device = backend.new_target(64, 16, peniko::Color::TRANSPARENT);
    device.push_clip_rect(Rect::new(0.0, 0.0, 32.0, 16.0));
    device.push_layer(BlendMode::Normal, 1.0, None);
    device.fill_path(
        &square(0.0, 0.0, 64.0, 16.0),
        Affine::IDENTITY,
        &Brush::Solid(peniko::Color::from_rgba8(0, 255, 0, 255)),
        FillRule::Winding,
        AntiAlias::On,
    );
    device.pop(); // the layer
    device.pop(); // the clip
    // A second fill, now unclipped, must reach the right-hand half.
    device.fill_path(
        &square(48.0, 0.0, 64.0, 16.0),
        Affine::IDENTITY,
        &Brush::Solid(peniko::Color::from_rgba8(255, 0, 0, 255)),
        FillRule::Winding,
        AntiAlias::On,
    );
    let out = backend.finish(device);
    assert_eq!(out.pixel(8, 8), Some([0, 255, 0, 255]), "clipped fill");
    assert_eq!(out.pixel(56, 8), Some([255, 0, 0, 255]), "after the pop");
}

#[test]
fn a_layer_composites_with_its_alpha() {
    let Some(backend) = gpu() else { return };
    let mut device = backend.new_target(16, 16, peniko::Color::from_rgba8(0, 255, 0, 255));
    device.push_layer(BlendMode::Normal, 0.5, None);
    device.fill_path(
        &square(0.0, 0.0, 16.0, 16.0),
        Affine::IDENTITY,
        &Brush::Solid(peniko::Color::from_rgba8(255, 0, 0, 255)),
        FillRule::Winding,
        AntiAlias::On,
    );
    device.pop();
    let out = backend.finish(device);
    let px = out.pixel(8, 8).expect("in bounds");
    assert!(px[0] > 100 && px[0] < 160, "half red over green: {px:?}");
    assert!(px[1] > 100 && px[1] < 160, "half red over green: {px:?}");
}

#[test]
fn a_device_sized_mask_halves_what_it_covers() {
    // The path §4.2 of the status doc builds by hand, because vello has no
    // layer that takes a supplied coverage plane. Today's engine passes `None`
    // at every call site, so without this test the mapping would be untested
    // code shipped on a contract.
    let Some(backend) = gpu() else { return };
    let mut device = backend.new_target(16, 16, peniko::Color::TRANSPARENT);
    let mask = AlphaMask::filled(16, 16, 128);
    device.push_layer(BlendMode::Normal, 1.0, Some(&mask));
    device.fill_path(
        &square(0.0, 0.0, 16.0, 16.0),
        Affine::IDENTITY,
        &Brush::Solid(peniko::Color::from_rgba8(255, 255, 255, 255)),
        FillRule::Winding,
        AntiAlias::On,
    );
    device.pop();
    let out = backend.finish(device);
    let px = out.pixel(8, 8).expect("in bounds");
    assert!(
        px[3] > 100 && px[3] < 160,
        "the mask should halve the alpha, got {}",
        px[3]
    );
}

#[test]
fn a_masked_layer_popped_with_no_draws_is_transparent() {
    // The degenerate half of the deferred-mask design. `pop` emits the
    // luminance-mask layer over "the content this layer just drew" — and here
    // there is none, so the mask multiplies nothing and the result must be
    // transparent rather than the mask's own grey plane leaking through as
    // pixels. Getting this wrong would paint a soft mask's coverage as visible
    // ink on every empty group a page contains.
    let Some(backend) = gpu() else { return };
    let mut device = backend.new_target(16, 16, peniko::Color::TRANSPARENT);
    let mask = AlphaMask::filled(16, 16, 200);
    device.push_layer(BlendMode::Normal, 1.0, Some(&mask));
    device.pop();
    let out = backend.finish(device);
    assert_eq!(
        out.pixel(8, 8),
        Some([0, 0, 0, 0]),
        "an empty masked layer must contribute nothing"
    );
}

#[test]
fn nested_masked_layers_multiply_and_unwind_in_order() {
    // Nesting is where the one-stack design could go wrong: each `pop` of a
    // masked frame emits *two* extra `pop_layer`s beyond the frame's own, and
    // those extra layers are deliberately not on `frames`. If the LIFO were
    // misaligned the inner mask would close the outer layer and the second
    // fill would escape its clip. Two half-masks nested must land near a
    // quarter, and the fill after both pops must be unaffected.
    let Some(backend) = gpu() else { return };
    let mut device = backend.new_target(32, 16, peniko::Color::TRANSPARENT);
    let half = AlphaMask::filled(32, 16, 128);
    device.push_layer(BlendMode::Normal, 1.0, Some(&half));
    device.push_layer(BlendMode::Normal, 1.0, Some(&half));
    device.fill_path(
        &square(0.0, 0.0, 16.0, 16.0),
        Affine::IDENTITY,
        &Brush::Solid(peniko::Color::from_rgba8(255, 255, 255, 255)),
        FillRule::Winding,
        AntiAlias::On,
    );
    device.pop(); // the inner masked layer
    device.pop(); // the outer masked layer
    // Both masks are closed, so this must arrive at full alpha.
    device.fill_path(
        &square(16.0, 0.0, 32.0, 16.0),
        Affine::IDENTITY,
        &Brush::Solid(peniko::Color::from_rgba8(0, 0, 255, 255)),
        FillRule::Winding,
        AntiAlias::On,
    );
    let out = backend.finish(device);
    let masked = out.pixel(8, 8).expect("in bounds");
    assert!(
        masked[3] > 40 && masked[3] < 90,
        "two half masks should compound to about a quarter, got {}",
        masked[3]
    );
    assert_eq!(
        out.pixel(24, 8),
        Some([0, 0, 255, 255]),
        "the fill after both pops is outside every mask"
    );
}

#[test]
fn a_target_past_the_device_limit_is_refused_rather_than_allocated() {
    // `Error::TargetTooLarge` was documented and never constructed, and the
    // path that should have produced it allocated `w * h * 4` zeros — up to
    // seventeen gibibytes — on its way to reporting failure. The fallible
    // constructor now refuses first, and the infallible one clamps rather than
    // building a target the device cannot render.
    let Some(backend) = gpu() else { return };
    let over = backend.max_dimension().saturating_add(1);
    let err = backend
        .try_new_target(over, 16, peniko::Color::TRANSPARENT)
        .expect_err("a target past the device's limit must be refused");
    assert!(
        matches!(err, Error::TargetTooLarge { w, max, .. }
            if w == over && max == backend.max_dimension()),
        "the error must carry the request and the limit, got {err}"
    );
    // And the trait's own constructor, which has no failure channel, clamps to
    // something renderable instead of handing back a target `finish` would
    // have to refuse.
    let clamped = backend.new_target(over, 16, peniko::Color::TRANSPARENT);
    let out = backend.finish(clamped);
    assert!(
        backend.accepts(out.width(), out.height()),
        "the clamped target must be one this device can render, got {}x{}",
        out.width(),
        out.height()
    );
}

#[test]
fn the_process_opens_one_device_however_many_backends_ask() {
    // The leak `request_adapter` documents is one device per *process*, not
    // one per call: fourteen GPU tests in one binary must not leak fourteen
    // devices. Proved through the adapter report, which is the only observable
    // the shared device has — two backends built independently must be looking
    // at the same adapter.
    let Some(first) = gpu() else { return };
    let Some(second) = gpu() else { return };
    assert_eq!(
        first.adapter_report(),
        second.adapter_report(),
        "both backends must be on the process's one device"
    );
}

#[test]
fn a_healthy_device_reports_no_fault() {
    // The uncaptured-error hook records rather than panics, which is only
    // observable in the negative on working hardware: a render that succeeded
    // must leave the fault slot empty, or the hook is recording noise and
    // every later `try_finish` would fail on a device that is fine.
    let Some(backend) = gpu() else { return };
    let mut device = backend.new_target(16, 16, peniko::Color::TRANSPARENT);
    device.fill_path(
        &square(0.0, 0.0, 16.0, 16.0),
        Affine::IDENTITY,
        &Brush::Solid(peniko::Color::from_rgba8(9, 9, 9, 255)),
        FillRule::Winding,
        AntiAlias::On,
    );
    let out = backend
        .try_finish(device)
        .expect("a healthy device renders without a fault");
    assert_eq!(out.pixel(8, 8), Some([9, 9, 9, 255]));
    assert_eq!(backend.device_fault(), None);
}

#[test]
fn an_image_lands_on_its_own_pixel_grid() {
    // `draw_image`'s convention: the transform maps the image's pixel grid,
    // not its unit square. Reading it the other way collapses a whole-page
    // image onto one pixel — silently and totally — which is exactly the
    // mistake SPEC §8 item 8 records having been made once already.
    let Some(backend) = gpu() else { return };
    let mut device = backend.new_target(32, 32, peniko::Color::TRANSPARENT);
    let img = Pixmap::filled(16, 16, peniko::Color::from_rgba8(0, 0, 255, 255));
    device.draw_image(
        &img,
        Affine::translate((8.0, 8.0)),
        ImageQuality::Nearest,
        1.0,
    );
    let out = backend.finish(device);
    assert_eq!(out.pixel(16, 16), Some([0, 0, 255, 255]), "under the image");
    assert_eq!(out.pixel(2, 2), Some([0, 0, 0, 0]), "outside it");
}

#[test]
fn a_translucent_image_needs_no_wrapping_layer() {
    // `vello_cpu 0.2.0` panics on a sampler alpha below one and
    // `pdfrum-raster-vello-cpu` wraps every such draw in an opacity layer. GPU
    // vello encodes the multiplier in the draw tag, so the wrapper is not
    // needed here — asserted rather than assumed, because adding it back
    // "for symmetry" would change the rounding.
    let Some(backend) = gpu() else { return };
    let mut device = backend.new_target(16, 16, peniko::Color::TRANSPARENT);
    let img = Pixmap::filled(16, 16, peniko::Color::from_rgba8(255, 0, 0, 255));
    device.draw_image(&img, Affine::IDENTITY, ImageQuality::Nearest, 0.5);
    let out = backend.finish(device);
    let px = out.pixel(8, 8).expect("in bounds");
    assert!(px[3] > 100 && px[3] < 160, "half-opacity image: {px:?}");
}

#[test]
fn a_backdrop_survives_into_the_new_target() {
    // `RenderParams::base_color` is one colour, so a backdrop *image* — what a
    // non-isolated group and the knockout buffer both need — has to be encoded
    // as the scene's first draw instead.
    let Some(backend) = gpu() else { return };
    let base = Pixmap::filled(16, 16, peniko::Color::from_rgba8(10, 20, 30, 255));
    let device = backend.new_target_with_backdrop(&base);
    let out = backend.finish(device);
    assert_eq!(out.pixel(8, 8), Some([10, 20, 30, 255]));
}

#[test]
fn a_snapshot_does_not_consume_the_target() {
    let Some(backend) = gpu() else { return };
    let mut device = backend.new_target(16, 16, peniko::Color::TRANSPARENT);
    device.fill_path(
        &square(0.0, 0.0, 16.0, 16.0),
        Affine::IDENTITY,
        &Brush::Solid(peniko::Color::from_rgba8(1, 2, 3, 255)),
        FillRule::Winding,
        AntiAlias::On,
    );
    let snap = backend.snapshot(&device);
    assert_eq!(snap.pixel(8, 8), Some([1, 2, 3, 255]));
    // Still usable afterwards, which is what "without consuming it" means.
    let out = backend.finish(device);
    assert_eq!(out.pixel(8, 8), Some([1, 2, 3, 255]));
}

#[test]
fn an_unbalanced_page_still_produces_pixels() {
    // A damaged page can leave layers open. `finish` closes them rather than
    // handing vello a malformed scene, and no panic is allowed anywhere in the
    // library (STYLE §3).
    let Some(backend) = gpu() else { return };
    let mut device = backend.new_target(16, 16, peniko::Color::WHITE);
    device.push_clip_rect(Rect::new(0.0, 0.0, 8.0, 16.0));
    device.push_layer(BlendMode::Multiply, 1.0, None);
    // Deliberately no pops.
    let out = backend.finish(device);
    assert_eq!((out.width(), out.height()), (16, 16));
}

#[test]
fn a_non_multiple_of_256_width_reads_back_unsheared() {
    // The row-alignment rule: a 100-pixel row is 400 bytes copied at a 512
    // byte stride. Missing the un-padding shears the image diagonally, which
    // looks like a rasterizer bug and is not one — so the test uses a width
    // whose row is *not* already aligned and checks a column near the right
    // edge, where a shear would show first.
    let Some(backend) = gpu() else { return };
    let mut device = backend.new_target(100, 8, peniko::Color::TRANSPARENT);
    device.fill_path(
        &square(0.0, 0.0, 100.0, 8.0),
        Affine::IDENTITY,
        &Brush::Solid(peniko::Color::from_rgba8(200, 100, 50, 255)),
        FillRule::Winding,
        AntiAlias::On,
    );
    let out = backend.finish(device);
    for x in [0, 50, 99] {
        assert_eq!(
            out.pixel(x, 7),
            Some([200, 100, 50, 255]),
            "column {x} of the last row"
        );
    }
}

#[test]
fn the_adapter_is_named_and_is_not_software() {
    let Some(backend) = gpu() else { return };
    let report = backend
        .adapter_report()
        .expect("a requested adapter reports itself");
    println!("running on {report}");
    assert!(report.is_real_gpu(), "{report} is not hardware");
    assert!(report.max_dimension >= 4096, "{report} is very limited");
}

/// The GPU backend satisfies [`pdfrum::Page::render_on`]'s bound.
///
/// The facade's `Backend` enum could never name this crate — naming it would
/// have put `wgpu` in every `cargo add pdfrum` tree, which is the thing
/// `scripts/check-no-wgpu.sh` exists to forbid. Since 2026-09-02 the backend
/// is an argument instead, so a caller who *does* hold a `wgpu::Device` hands
/// it straight to `render_on` and the facade never learns the crate exists.
///
/// This is the assertion that the arrangement type-checks. It runs a real
/// render when there is a GPU and skips cleanly when there is not, exactly as
/// every other test in this file does — and it lives here rather than in the
/// facade precisely because a dev-dependency edge points this way and not
/// back.
#[test]
fn the_facade_renders_a_page_on_this_backend() {
    let Some(backend) = gpu() else { return };
    let doc = pdfrum::Document::open(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../pdfrum/tests/fixtures/hello_world.pdf"
    ))
    .expect("open");
    let page = doc.page(0).expect("page");
    let pixmap = page
        .render_on(&backend, &pdfrum::RenderOptions::default())
        .expect("the GPU backend satisfies Page::render_on's bound");
    assert_eq!((pixmap.width(), pixmap.height()), (200, 200));
}
