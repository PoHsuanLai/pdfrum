//! The master walk: one page-object list, dispatched by kind
//! (`CPDF_RenderStatus::RenderObjectList` and `ProcessObjectNoClip`,
//! `cpdf_renderstatus.cpp:216-330`).
//!
//! `CPDF_RenderStatus` is a thousand-line class with twenty-four members and
//! a `Process*` method per object kind. STYLE §1 forbids reproducing it, so
//! what is here is a dispatch `match` plus one free function per kind, each
//! taking the small [`RenderCtx`] record and the backend's own device. The
//! walk is generic over the backend rather than dynamic, because
//! [`RasterBackend::snapshot`] needs the concrete device to read pixels back;
//! `dyn RenderDevice` survives only where a device is genuinely swappable —
//! the Coons scratch buffer.
//!
//! Two behaviours of the walk itself are load-bearing. The **cull test** is
//! computed once per list from the inverse-transformed device clip box and
//! uses **strict** inequalities, so an object exactly touching the clip edge
//! is kept. And a **shading that fails is never retried**: every other kind
//! falls back to a re-render that degenerates to an identical call on a
//! bitmap device, so a failure is a skipped object and a diagnostic.

use kurbo::{Affine, Rect, Shape};
use pdfrum_common::Diagnostics;
use pdfrum_page::{Page, PageObject, Transparency, Visibility};

use crate::clip;
use crate::color::{Argb, ObjectKind, resolve_argb};
use crate::ctx::{RenderCaches, RenderCtx};
use crate::device::{Brush, ImageQuality, MAX_TARGET_DIMENSION, RasterBackend, RenderDevice};
use crate::error::Error;
use crate::group::{GroupFinish, GroupInputs, needs_backdrop, needs_offscreen};
use crate::image::{effective_quality, overprint_blend, resample_quality, to_pixmap};
use crate::options::RenderOptions;
use crate::paint::{PathPaint, draw_path};
use crate::path::{IntRect, is_available_matrix, outer_rect};
use crate::pattern::PatternClip;
use crate::pixmap::{Pixmap, alpha_byte_rounding, alpha_byte_truncating};
use crate::shading;
use crate::text::{has_face, paint_kinds, place_glyphs};
use crate::transfer::TransferFunc;

/// Render a page into a pixmap.
///
/// The target size comes from the page's display box under
/// `opts.transform`; the background follows the oracle — opaque white for a
/// page without transparency, fully transparent for one with it — unless
/// overridden, and that choice is load-bearing rather than cosmetic.
///
/// # Errors
///
/// [`Error::TargetEmpty`] when the page's box under `opts.transform` is not
/// at least one pixel on both axes (a zero, negative or non-finite size), and
/// [`Error::TargetTooLarge`] when either axis exceeds
/// [`MAX_TARGET_DIMENSION`]. Damage inside the page is reported through
/// `diags` and never becomes an error.
pub fn render_page<B: RasterBackend>(
    page: &Page,
    opts: &RenderOptions,
    backend: &B,
    diags: &mut Diagnostics,
) -> Result<Pixmap, Error> {
    render_page_with_caches(page, opts, backend, &mut RenderCaches::new(), diags)
}

/// Render a page, drawing only what optional content leaves visible.
///
/// `visible` comes from [`pdfrum_page::page_visibility`], a **pre-pass** over
/// the same `page` with the document's `/OCProperties`. Splitting it out is
/// what keeps the render API free of a resolver: deciding visibility needs
/// indirect-object lookup and a mutable evaluation cache, and consuming the
/// answer needs neither — it is one index per object.
///
/// [`render_page`] is this with [`pdfrum_page::Visibility::all_visible`],
/// which is the right call for a document with no optional content and for
/// any caller that wants every layer drawn.
///
/// # Errors
///
/// As [`render_page`].
pub fn render_page_with_visibility<B: RasterBackend>(
    page: &Page,
    opts: &RenderOptions,
    backend: &B,
    visible: &Visibility,
    caches: &mut RenderCaches,
    diags: &mut Diagnostics,
) -> Result<Pixmap, Error> {
    render_page_inner(page, opts, backend, visible, caches, diags)
}

/// Render a page into a pixmap, reusing caller-owned [`RenderCaches`].
///
/// Identical to [`render_page`] except that the glyph cache outlives the
/// call, so a caller rendering many pages of one document flattens each
/// glyph outline once for the run rather than once per page.
///
/// # Determinism
///
/// Type-3 blue-zone snapping is order-dependent by design (see
/// [`RenderCaches`]), so a page rendered with a *warm* cache can differ by a
/// snapped pixel from the same page rendered with a cold one. Reuse across
/// pages of one document is the intended use and is what the oracle does;
/// reusing one set of caches across *unrelated* documents makes a page's
/// output depend on what was rendered before it. For a byte-identical
/// baseline — the conformance harness's case — call [`render_page`], which
/// gives every page fresh caches.
///
/// # Errors
///
/// As [`render_page`].
pub fn render_page_with_caches<B: RasterBackend>(
    page: &Page,
    opts: &RenderOptions,
    backend: &B,
    caches: &mut RenderCaches,
    diags: &mut Diagnostics,
) -> Result<Pixmap, Error> {
    render_page_inner(
        page,
        opts,
        backend,
        &Visibility::all_visible(),
        caches,
        diags,
    )
}

/// The body all three entry points share.
fn render_page_inner<B: RasterBackend>(
    page: &Page,
    opts: &RenderOptions,
    backend: &B,
    visible: &Visibility,
    caches: &mut RenderCaches,
    diags: &mut Diagnostics,
) -> Result<Pixmap, Error> {
    let (w, h) = target_size(page, opts)?;
    let clear = opts.background_for(needs_alpha_background(page));
    let mut device = backend.new_target(w, h, clear);
    let ctx = RenderCtx::new(opts.clone(), page.transparency);
    let to_device = page_matrix(page, opts);
    let device_box = Rect::new(0.0, 0.0, f64::from(w), f64::from(h));

    render_object_list(
        &ctx,
        &mut device,
        backend,
        caches,
        &page.objects,
        visible,
        to_device,
        device_box,
        diags,
    );
    Ok(backend.finish(device))
}

/// Whether the page renders onto a transparent background rather than white.
///
/// `pdfium_test` asks `FPDFPage_HasTransparency`, and the obvious reading of
/// that name is wrong: it is **not** whether the page declares a `/Group`.
/// It returns `CPDF_PageObjectHolder::BackgroundAlphaNeeded`, a flag the
/// content parser sets in exactly one place — when an `/ExtGState` names a
/// blend mode **above `Multiply`** (`cpdf_allstates.cpp:105-106`) — and
/// which then propagates up from a form to its holder
/// (`cpdf_streamcontentparser.cpp:835-838`).
///
/// So a page carrying a plain `/Group` still renders onto opaque white, and
/// only a page that actually asks for a backdrop-reading blend gets a
/// transparent one. Reading it as the `/Group` flag turns every such page's
/// output from RGB to RGBA and, where nothing paints, from white to black —
/// which is a whole-page difference, not a pixel one.
#[must_use]
pub fn needs_alpha_background(page: &Page) -> bool {
    fn any_deep_blend(objects: &[PageObject]) -> bool {
        objects.iter().any(|object| {
            let deep = matches!(
                object.state().general.blend,
                pdfrum_page::BlendMode::Screen
                    | pdfrum_page::BlendMode::Overlay
                    | pdfrum_page::BlendMode::Darken
                    | pdfrum_page::BlendMode::Lighten
                    | pdfrum_page::BlendMode::ColorDodge
                    | pdfrum_page::BlendMode::ColorBurn
                    | pdfrum_page::BlendMode::HardLight
                    | pdfrum_page::BlendMode::SoftLight
                    | pdfrum_page::BlendMode::Difference
                    | pdfrum_page::BlendMode::Exclusion
                    | pdfrum_page::BlendMode::Hue
                    | pdfrum_page::BlendMode::Saturation
                    | pdfrum_page::BlendMode::Color
                    | pdfrum_page::BlendMode::Luminosity
            );
            // A form's own objects carry the flag up to their holder.
            deep || match object {
                PageObject::Form(f) => any_deep_blend(&f.object.objects),
                _ => false,
            }
        })
    }
    any_deep_blend(&page.objects)
}

/// The device size a page renders at, and the errors that size can be.
///
/// Public so a caller can size a buffer, lay out a sheet or reject a page
/// *before* paying for a render — [`render_page`] answers the same question
/// only by doing the work.
///
/// The dimensions **truncate**, they do not round up: `pdfium_test` writes
/// `static_cast<int>(FPDF_GetPageWidthF(page) * scale)`
/// (`pdfium_test.cc:1513-1514`), so an A4 page's 595.276 points become 595
/// device pixels, not 596. Rounding up instead costs a one-pixel border on
/// every page whose size is not a whole number — which is most of the
/// non-US-Letter corpus, and a size mismatch rather than a pixel difference.
///
/// # Errors
///
/// [`Error::TargetEmpty`] when the page's box under `opts.transform` is not
/// at least one pixel on both axes, and [`Error::TargetTooLarge`] when either
/// axis exceeds [`MAX_TARGET_DIMENSION`].
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the `is_finite` and `>= 1.0` guards run before the successful \
              cast, and the `MAX_TARGET_DIMENSION` check runs after it; in \
              the error arm `max(0.0)` floors the value and Rust's saturating \
              float-to-int cast turns a huge or NaN size into a reported \
              number rather than wrapping"
)]
pub fn target_size(page: &Page, opts: &RenderOptions) -> Result<(u32, u32), Error> {
    let (pw, ph) = page.display_size();
    let corners = opts
        .transform
        .transform_rect_bbox(Rect::new(0.0, 0.0, pw, ph));
    let w = corners.width().trunc();
    let h = corners.height().trunc();
    if !w.is_finite() || !h.is_finite() || w < 1.0 || h < 1.0 {
        return Err(Error::TargetEmpty {
            width: w.max(0.0) as u32,
            height: h.max(0.0) as u32,
        });
    }
    let (w, h) = (w as u32, h as u32);
    if w > MAX_TARGET_DIMENSION || h > MAX_TARGET_DIMENSION {
        return Err(Error::TargetTooLarge {
            width: w,
            height: h,
            limit: MAX_TARGET_DIMENSION,
        });
    }
    Ok((w, h))
}

/// Page space to device space.
///
/// Three transforms compose, and the middle one is the easy omission: the
/// page's display matrix normalises the crop box's origin and applies the
/// `/Rotate`, but PDF user space is **y-up** and every device is y-down, so
/// the y axis must be flipped about the page's height before the caller's
/// own transform applies. Without it a page renders upside down, which the
/// symmetric fixtures hide and the asymmetric ones do not.
///
/// # The flip is about the *device* box, not the page box
///
/// `CPDF_Page::GetDisplayMatrixForRect` (`cpdf_page.cpp:160-220`) builds the
/// matrix from an `FX_RECT` — **integer** device coordinates — divided by the
/// page's own float size:
///
/// ```cpp
/// CFX_Matrix matrix((x2 - x0) / page_size_.width, ...,
///                   (y1 - y0) / page_size_.height, x0, y0);
/// ```
///
/// and `CPDFSDK_RenderPageWithContext` (`cpdfsdk_renderpage.cpp:116-118`)
/// passes it `FX_RECT(0, 0, size_x, size_y)`, the truncated bitmap size that
/// [`target_size`] computes. So an A4 page 841.89 points tall renders into
/// 841 device rows with a y scale of `841 / 841.89`, not of 1 — the page is
/// very slightly *squeezed* to fit the bitmap it was truncated into.
///
/// Flipping about the float height instead leaves a shear of up to a device
/// pixel between the top of the page and the bottom. That was invisible while
/// every glyph was filled at its true position, because it moves a stem edge
/// by a fraction of a count — and it stops being invisible the moment glyph
/// origins are **snapped**, since a y that was 0.49 off is then a whole row
/// off. It cost `tcpdf/example_007` and `example_017` about 0.035 SSIM each
/// before it was found; see `docs/status/pdfrum-render.md`, wave 5.
#[must_use]
pub fn page_matrix(page: &Page, opts: &RenderOptions) -> Affine {
    let (page_w, page_h) = page.display_size();
    // The device box the oracle fits the page into: the same truncation
    // `target_size` performs, since that is the bitmap that gets allocated.
    // A page that cannot be sized at all keeps the float box, which is what
    // the render is about to reject anyway.
    let device =
        target_size(page, opts).map_or((page_w, page_h), |(w, h)| (f64::from(w), f64::from(h)));
    let (dev_w, dev_h) = device;
    if page_w <= 0.0 || page_h <= 0.0 || !dev_w.is_finite() || !dev_h.is_finite() {
        return opts.transform * page.rotate.display_matrix(page.crop_box);
    }
    // `opts.transform` has already been consumed in choosing the device box,
    // exactly as `pdfium_test` consumes its `--scale` in sizing the bitmap
    // and then asks for the display matrix onto that size.
    let fit = Affine::new([dev_w / page_w, 0.0, 0.0, -dev_h / page_h, 0.0, dev_h]);
    fit * page.rotate.display_matrix(page.crop_box)
}

/// Walk one object list.
///
/// `device_box` is the device's own extent and is what the cull test works
/// against, transformed back into object space once for the whole list —
/// which is why an object's own matrix cannot change it.
///
/// `visible` describes *this* list, one entry per object in order. It is the
/// pre-pass's answer and the walk only reads it — see
/// [`render_page_with_visibility`]. A tree that hides nothing is the common
/// case and costs one `is_none_or` per object.
#[expect(
    clippy::too_many_arguments,
    reason = "the walk threads context, device, backend, caches and the \
              visibility describing this list"
)]
pub fn render_object_list<B: RasterBackend>(
    ctx: &RenderCtx<'_>,
    device: &mut B::Device,
    backend: &B,
    caches: &mut RenderCaches,
    objects: &[PageObject],
    visible: &Visibility,
    to_device: Affine,
    device_box: Rect,
    diags: &mut Diagnostics,
) {
    if !ctx.may_recurse() {
        return;
    }
    let cull = cull_rect(to_device, device_box);
    for (index, object) in objects.iter().enumerate() {
        // The visibility gate runs first, exactly where `RenderSingleObject`
        // puts it (`cpdf_renderstatus.cpp:247`): before the clip is pushed,
        // so a hidden object's clip never reaches the device either.
        if !visible.visible(index) {
            continue;
        }
        if let Some(cull) = cull
            && culled(object, cull)
        {
            continue;
        }
        render_object(
            ctx,
            device,
            backend,
            caches,
            object,
            &visible.children(index),
            to_device,
            device_box,
            diags,
        );
    }
}

/// The object-space rectangle the device clip box maps back to, computed once
/// per list.
fn cull_rect(to_device: Affine, device_box: Rect) -> Option<Rect> {
    let det = to_device.determinant();
    (det != 0.0 && det.is_finite())
        .then(|| to_device.inverse().transform_rect_bbox(device_box))
        .filter(|r| r.x0.is_finite() && r.y0.is_finite() && r.x1.is_finite() && r.y1.is_finite())
}

/// The cull test, with the **strict** inequalities `RenderObjectList` uses.
///
/// The progressive renderer spells the complement with `<=`/`>=`, so an
/// object exactly touching the clip edge is kept there and dropped here.
/// Only this spelling survives; recorded so nobody "fixes" it later.
fn culled(object: &PageObject, cull: Rect) -> bool {
    let Some(bbox) = object_bbox(object) else {
        return false;
    };
    bbox.x0 > cull.x1 || bbox.x1 < cull.x0 || bbox.y0 > cull.y1 || bbox.y1 < cull.y0
}

/// One object's own bounding box in the coordinate space its list is walked
/// in, or `None` when it has no meaningful extent.
fn object_bbox(object: &PageObject) -> Option<Rect> {
    match object {
        PageObject::Path(p) => Some((p.object.matrix * p.object.path.clone()).bounding_box()),
        PageObject::Image(i) => Some(i.object.matrix.transform_rect_bbox(unit_rect())),
        PageObject::Shading(s) => Some(s.object.bounds),
        PageObject::Form(f) => f
            .object
            .bbox
            .map(|b| f.object.matrix.transform_rect_bbox(b)),
        // A text object's extent needs the font's metrics; the cull is an
        // optimisation, so declining it is always safe.
        PageObject::Text(_) => None,
    }
}

fn unit_rect() -> Rect {
    Rect::new(0.0, 0.0, 1.0, 1.0)
}

/// Render one object, through a transparency group when the predicate says
/// so and directly otherwise.
///
/// `children` describes this object's own object list when it is a form, and
/// is ignored for every other kind.
#[expect(
    clippy::too_many_arguments,
    reason = "the walk threads context, device, backend, caches and a form's \
              child visibility"
)]
pub fn render_object<B: RasterBackend>(
    ctx: &RenderCtx<'_>,
    device: &mut B::Device,
    backend: &B,
    caches: &mut RenderCaches,
    object: &PageObject,
    children: &Visibility,
    to_device: Affine,
    device_box: Rect,
    diags: &mut Diagnostics,
) {
    let state = object.state();
    let clips = clip::resolve(&state.clip, to_device, &mut caches.glyphs, &ctx.opts);
    let pushed = clip::push(device, &clips);

    let initial_alpha = ctx.initial_fill.map_or(1.0, |_| 1.0);
    let inputs = GroupInputs::of(object, initial_alpha);
    if needs_offscreen(inputs) && ctx.may_recurse() {
        render_grouped(
            ctx, device, backend, caches, object, children, inputs, to_device, device_box, diags,
        );
    } else {
        render_direct(
            ctx, device, backend, caches, object, children, to_device, device_box, diags,
        );
    }

    clip::pop(device, pushed);
}

/// Render one object into its own buffer, then composite that buffer back.
#[expect(
    clippy::too_many_arguments,
    reason = "the group path needs every input the direct one had"
)]
fn render_grouped<B: RasterBackend>(
    ctx: &RenderCtx<'_>,
    device: &mut B::Device,
    backend: &B,
    caches: &mut RenderCaches,
    object: &PageObject,
    children: &Visibility,
    inputs: GroupInputs,
    to_device: Affine,
    device_box: Rect,
    diags: &mut Diagnostics,
) {
    let state = object.state();
    // The buffer is sized to the object's own device extent intersected with
    // the device, so a group off the page costs nothing.
    let bbox = object_bbox(object)
        .map_or(device_box, |b| to_device.transform_rect_bbox(b))
        .intersect(device_box);
    let rect = outer_rect(bbox).intersect(outer_rect(device_box));
    let (Ok(w), Ok(h)) = (u32::try_from(rect.width()), u32::try_from(rect.height())) else {
        return;
    };
    if w == 0 || h == 0 || w > MAX_TARGET_DIMENSION || h > MAX_TARGET_DIMENSION {
        return;
    }

    let transparency = match object {
        PageObject::Form(f) => f.object.transparency,
        _ => ctx.transparency,
    };
    // Isolated means the group starts transparent; non-isolated means it
    // starts from a copy of what is already on the page, and PDFium never
    // removes that copy before compositing back.
    let mut sub = if needs_backdrop(transparency) {
        let backdrop = backend.snapshot(device);
        let cropped = crop(&backdrop, rect);
        backend.new_target_with_backdrop(&cropped)
    } else {
        backend.new_target(w, h, peniko::Color::TRANSPARENT)
    };

    let offset = Affine::translate((-f64::from(rect.left), -f64::from(rect.top)));
    let inner_ctx = RenderCtx {
        transparency,
        in_group: true,
        std_cs: true,
        // The group does *not* inherit the parent's colour: `Initialize(null,
        // null)` in the C++.
        initial_fill: None,
        initial_stroke: None,
        ..ctx.deeper()
    };
    let inner_box = Rect::new(0.0, 0.0, f64::from(w), f64::from(h));
    render_direct(
        &inner_ctx,
        &mut sub,
        backend,
        caches,
        object,
        children,
        offset * to_device,
        inner_box,
        diags,
    );
    let mut pixels = backend.finish(sub);

    // The mask first, then the group alpha, then the inherited one — in that
    // order, and the last only outside an enclosing group.
    if let Some(mask) = &state.general.soft_mask {
        let rendered = render_soft_mask(&inner_ctx, backend, caches, mask, rect, to_device, diags);
        if let Some(m) = rendered {
            pixels.multiply_alpha_mask(&m);
        }
    }
    // `transparency` is the **form's own**, not the enclosing one
    // (`cpdf_renderstatus.cpp:646`, `:740-742`): the group alpha is applied
    // exactly when the object being drawn declares a group, and a form drawn
    // inside a page that declares one does not thereby inherit the multiply.
    // Reading the enclosing flag instead drops the group alpha on any form
    // whose own `/Group` is absent — and applies it to a non-form under a
    // page that has one, where `inputs.group_alpha` is 1.0 and it is
    // harmless, which is why this only ever showed up as a missing multiply.
    GroupFinish::of(inputs, transparency, ctx.in_group).apply(&mut pixels);

    // With a premultiplied RGBA target that is both readable and
    // alpha-capable, PDFium's five-armed compositor collapses to one arm: a
    // plain blended blit. The arms it does not take need an opaque target
    // that cannot report alpha, which ours never is.
    device.push_layer(state.general.blend, 1.0, None);
    device.draw_image(
        &pixels,
        Affine::translate((f64::from(rect.left), f64::from(rect.top))),
        ImageQuality::Nearest,
        1.0,
    );
    device.pop();
}

/// Render a soft mask's group and read it back as a device-sized coverage
/// plane (`LoadSMask`, `cpdf_renderstatus.cpp:1434-1542`).
///
/// Four contracts, all pixel-visible:
///
/// - **The mask renders at exactly the clip rect's device resolution**, on the
///   device's own pixel grid, so applying it never resamples.
/// - **A luminosity buffer is opaque**, cleared to the `/BC` backdrop
///   (default black), so an area the group never paints contributes the
///   backdrop's luminosity rather than zero. That is what makes an unpainted
///   corner of a `/BC`-white mask fully *reveal* rather than fully hide.
/// - **An alpha buffer starts at nothing** and the group renders in alpha
///   colour mode, where every drawing operation writes its alpha as gray.
/// - **The readback is `FXRGB2GRAY`, not BT.709.** Both rasterizers ship a
///   luminance helper and both use BT.709; neither may be used here.
fn render_soft_mask<B: RasterBackend>(
    ctx: &RenderCtx<'_>,
    backend: &B,
    caches: &mut RenderCaches,
    mask: &pdfrum_page::SoftMask,
    rect: IntRect,
    to_device: Affine,
    diags: &mut Diagnostics,
) -> Option<crate::pixmap::AlphaMask> {
    if !ctx.may_recurse() {
        return None;
    }
    let (Ok(w), Ok(h)) = (u32::try_from(rect.width()), u32::try_from(rect.height())) else {
        return None;
    };
    if w == 0 || h == 0 || w > MAX_TARGET_DIMENSION || h > MAX_TARGET_DIMENSION {
        return None;
    }
    let mut device = backend.new_target(w, h, crate::softmask::backdrop(mask));
    if !mask.objects.is_empty() {
        let inner = RenderCtx {
            opts: RenderOptions {
                color_mode: match mask.kind {
                    // An alpha mask's group paints alpha as gray; a
                    // luminosity one paints its real colours and the
                    // readback greys them.
                    pdfrum_page::SoftMaskKind::Alpha => crate::options::ColorMode::Alpha,
                    pdfrum_page::SoftMaskKind::Luminosity => crate::options::ColorMode::Normal,
                },
                ..ctx.opts.clone()
            },
            // The group renders from a clean slate: `Initialize(null, null)`.
            initial_fill: None,
            initial_stroke: None,
            type3: None,
            in_group: true,
            std_cs: true,
            ..ctx.deeper()
        };
        // The mask's own matrix already places it; only the shift into the
        // buffer's origin is added, so the mask lands on the device's grid.
        let offset = Affine::translate((-f64::from(rect.left), -f64::from(rect.top)));
        render_object_list(
            &inner,
            &mut device,
            backend,
            caches,
            &mask.objects,
            // A soft mask's group is its own object list, not the page's, so
            // the page's visibility tree says nothing about it.
            &Visibility::all_visible(),
            offset * to_device,
            Rect::new(0.0, 0.0, f64::from(w), f64::from(h)),
            diags,
        );
    }
    let rendered = backend.finish(device);
    Some(crate::softmask::readback(mask, &rendered))
}

/// Copy a sub-rectangle out of a pixmap.
fn crop(source: &Pixmap, rect: IntRect) -> Pixmap {
    let (Ok(w), Ok(h)) = (u32::try_from(rect.width()), u32::try_from(rect.height())) else {
        return Pixmap::new(0, 0);
    };
    let mut out = Pixmap::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let (sx, sy) = (
                i64::from(x) + i64::from(rect.left),
                i64::from(y) + i64::from(rect.top),
            );
            let (Ok(sx), Ok(sy)) = (u32::try_from(sx), u32::try_from(sy)) else {
                continue;
            };
            if let Some(px) = source.pixel(sx, sy) {
                out.set_pixel(x, y, px);
            }
        }
    }
    out
}

/// Dispatch one object to its handler.
#[expect(
    clippy::too_many_arguments,
    reason = "the walk threads context, device, backend and caches"
)]
fn render_direct<B: RasterBackend>(
    ctx: &RenderCtx<'_>,
    device: &mut B::Device,
    backend: &B,
    caches: &mut RenderCaches,
    object: &PageObject,
    children: &Visibility,
    to_device: Affine,
    device_box: Rect,
    diags: &mut Diagnostics,
) {
    match object {
        PageObject::Path(p) => render_path(
            ctx, device, backend, caches, &p.object, &p.state, to_device, device_box, diags,
        ),
        PageObject::Text(t) => render_text(
            ctx, device, backend, caches, &t.object, &t.state, to_device, device_box, diags,
        ),
        PageObject::Image(i) => render_image(
            ctx, device, backend, caches, &i.object, &i.state, to_device, device_box, diags,
        ),
        PageObject::Shading(s) => render_shading(
            ctx, device, backend, &s.object, &s.state, to_device, device_box,
        ),
        PageObject::Form(f) => {
            let inner = RenderCtx {
                initial_fill: ctx.initial_fill,
                initial_stroke: ctx.initial_stroke,
                ..ctx.deeper()
            };
            // The children's own matrices already carry the form's, because
            // `build_page` composes `/Matrix` into the CTM before recursing.
            // Composing it again here would apply it twice.
            render_object_list(
                &inner,
                device,
                backend,
                caches,
                &f.object.objects,
                children,
                to_device,
                device_box,
                diags,
            );
        }
    }
}

/// Resolve one object's fill and stroke colours.
fn colors(
    ctx: &RenderCtx<'_>,
    state: &pdfrum_page::GraphicsState,
    kind: ObjectKind,
) -> (crate::color::Argb, crate::color::Argb) {
    let transfer = state
        .general
        .transfer
        .as_ref()
        .map(|t| TransferFunc::new(t));
    // A type-3 char proc imposes its caller's colour on every uncoloured
    // operation, which is what makes a `d1` glyph take the text object's
    // colour rather than black.
    let fill = match ctx.type3 {
        Some(frame) if !frame.colored || state.fill.to_rgb().is_none() => frame.fill,
        _ => resolve_argb(
            &state.fill,
            state.general.fill_alpha,
            transfer.as_ref(),
            ctx.initial_fill,
            &ctx.opts,
            kind,
            false,
        ),
    };
    let stroke = resolve_argb(
        &state.stroke,
        state.general.stroke_alpha,
        transfer.as_ref(),
        ctx.initial_stroke,
        &ctx.opts,
        kind,
        true,
    );
    (fill, stroke)
}

/// Paint one object's geometry with the pattern its fill or stroke colour
/// names.
///
/// The uncoloured colour is resolved here rather than in `pattern.rs` because
/// it is a *colour* question — the `scn` operands read through the pattern
/// space's base — and because its two fallbacks differ by paint type: a
/// coloured tiling pattern whose colour will not resolve falls back to mid
/// grey, everything else to white.
#[expect(
    clippy::too_many_arguments,
    reason = "painting a pattern needs the context, device, backend, caches, \
              the object's state, which colour names it, its geometry, and \
              the page transform"
)]
fn paint_pattern<B: RasterBackend>(
    ctx: &RenderCtx<'_>,
    device: &mut B::Device,
    backend: &B,
    caches: &mut RenderCaches,
    state: &pdfrum_page::GraphicsState,
    stroking: bool,
    geometry: &PatternClip<'_>,
    to_device: Affine,
    device_box: Rect,
    diags: &mut Diagnostics,
) {
    let color = if stroking { &state.stroke } else { &state.fill };
    let Some(value) = color.pattern.as_ref() else {
        return;
    };
    let Some(pattern) = value.loaded.as_ref() else {
        return;
    };
    let alpha = if stroking {
        state.general.stroke_alpha
    } else {
        state.general.fill_alpha
    };
    let colored_tiling = matches!(&**pattern, pdfrum_page::Pattern::Tiling(t) if t.colored);
    let space = color
        .space
        .as_ref()
        .map_or(&pdfrum_page::ColorSpace::DeviceGray, |s| &**s);
    let rgb = pdfrum_page::pattern::uncolored_pattern_rgb(space, &value.components, colored_tiling);
    let [r, g, b] = rgb.to_bytes();
    let uncolored = Argb {
        a: alpha_byte_truncating(alpha),
        r,
        g,
        b,
    };
    crate::pattern::draw(
        ctx, device, backend, caches, pattern, geometry, to_device, device_box, alpha, uncolored,
        diags,
    );
}

#[expect(
    clippy::too_many_arguments,
    reason = "the pattern arm needs the caches, device box and diagnostics the \
              ordinary draw does not"
)]
fn render_path<B: RasterBackend>(
    ctx: &RenderCtx<'_>,
    device: &mut B::Device,
    backend: &B,
    caches: &mut RenderCaches,
    object: &pdfrum_page::PathObject,
    state: &pdfrum_page::GraphicsState,
    to_device: Affine,
    device_box: Rect,
    diags: &mut Diagnostics,
) {
    let (fill, stroke) = colors(ctx, state, ObjectKind::Path);
    let mut fills = object.fill_rule != pdfrum_page::FillRule::None;
    let mut strokes = object.stroke;

    // `ProcessPathPattern` runs *before* the ordinary draw and **drains**
    // pattern colours out of it: a pattern fill is painted by the pattern
    // machinery and the fill type is then set to none, and likewise for a
    // stroke, so a path with both is drawn twice and the residual ordinary
    // draw does nothing at all.
    //
    // The draining stands even where the pattern does not resolve. A pattern
    // colour has no components, so `to_rgb` reports none and the colour falls
    // back to black — which would paint a `scn`-with-no-paint-operator
    // rectangle solid black across the whole page where the oracle draws
    // nothing.
    let matrix = to_device * object.matrix;
    if fills && state.fill.is_pattern() {
        fills = false;
        paint_pattern(
            ctx,
            device,
            backend,
            caches,
            state,
            false,
            &PatternClip::Path {
                path: &object.path,
                to_device: matrix,
                stroking: false,
                rule: object.fill_rule.into(),
                stroke: &state.stroke_params,
            },
            to_device,
            device_box,
            diags,
        );
    }
    if strokes && state.stroke.is_pattern() {
        strokes = false;
        paint_pattern(
            ctx,
            device,
            backend,
            caches,
            state,
            true,
            &PatternClip::Path {
                path: &object.path,
                to_device: matrix,
                stroking: true,
                rule: object.fill_rule.into(),
                stroke: &state.stroke_params,
            },
            to_device,
            device_box,
            diags,
        );
    }
    if !fills && !strokes {
        return;
    }

    // Under a forced colour scheme a fill may be converted into a stroke
    // wholesale — the only place the two swap roles.
    let (fills, strokes) = if matches!(ctx.opts.color_mode, crate::options::ColorMode::Forced(_))
        && ctx.opts.convert_fill_to_stroke
        && fills
    {
        (false, true)
    } else {
        (fills, strokes)
    };
    let paint = PathPaint {
        fill: fills.then_some(fill),
        stroke: strokes.then_some(stroke),
        rule: object.fill_rule.into(),
        text_mode: false,
    };
    draw_path(
        device,
        backend,
        &object.path,
        // `PathObject::matrix` is already the CTM in force when the path was
        // emitted — `pdfrum-page` folds every enclosing form's matrix into
        // it — so only the page-to-device transform is added here. Composing
        // `state.ctm` as well would apply the CTM twice.
        to_device * object.matrix,
        paint,
        &state.stroke_params,
        &ctx.opts,
    );
}

/// A run whose fill is a pattern and which is not stroked
/// (`DrawTextPathWithPattern`'s first arm, `cpdf_renderstatus.cpp:1290-1310`).
///
/// No glyph is drawn. The run's **bounding rectangle** becomes a path object
/// carrying the text's own colour and general state, and the run itself is
/// appended to a copy of the current clip path; that object then goes through
/// the ordinary single-object render, where the pattern machinery paints the
/// rectangle and the text clip cuts it back to the glyph shapes.
///
/// Two things follow from it being a *clip* rather than a mask, and both are
/// visible: the glyphs are filled winding whatever the run's own mode said,
/// and a run whose rectangle is empty paints nothing at all.
#[expect(
    clippy::too_many_arguments,
    reason = "a synthetic path object needs everything the real one does"
)]
fn render_pattern_text<B: RasterBackend>(
    ctx: &RenderCtx<'_>,
    device: &mut B::Device,
    backend: &B,
    caches: &mut RenderCaches,
    object: &pdfrum_page::TextObject,
    state: &pdfrum_page::GraphicsState,
    to_device: Affine,
    device_box: Rect,
    diags: &mut Diagnostics,
) {
    let Some(rect) = crate::text::run_rect(object, state) else {
        return;
    };
    // `path.mutable_clip_path().CopyClipPath(last_clip_path_)` then
    // `AppendTexts(&pCopy)`: the synthetic object's clip is the current one
    // plus this run. The *current* half is already on the device — the caller
    // pushed the text object's stack before dispatching here — so only the
    // run's own contribution is added, and only it is popped.
    let mut clip = pdfrum_page::ClipStack::new();
    if !clip.push_text(vec![pdfrum_page::state::TextClipRun {
        object: object.clone(),
        char_space: state.text.char_space,
        word_space: state.text.word_space,
    }]) {
        return;
    }
    // The colour and general state are the text's; the clip is *not* carried
    // on the state, because it is pushed on the device around the draw rather
    // than resolved again inside it.
    let synthetic = pdfrum_page::GraphicsState {
        clip: pdfrum_page::ClipStack::new(),
        ctm: Affine::IDENTITY,
        ..state.clone()
    };
    // `RenderSingleObject` pushes the object's clip before drawing it, and the
    // whole point of this path is the clip it just built — so the clip has to
    // be pushed here rather than left to the caller, which has already pushed
    // the *text* object's stack and moved on.
    let clips = clip::resolve(&clip, to_device, &mut caches.glyphs, &ctx.opts);
    let pushed = clip::push(device, &clips);
    render_path::<B>(
        ctx,
        device,
        backend,
        caches,
        &pdfrum_page::PathObject {
            path: kurbo::Shape::to_path(&rect, 0.1),
            matrix: Affine::IDENTITY,
            fill_rule: pdfrum_page::FillRule::Winding,
            stroke: false,
        },
        &synthetic,
        to_device,
        device_box,
        diags,
    );
    clip::pop(device, pushed);
}

#[expect(
    clippy::too_many_arguments,
    reason = "the type-3 arm needs the device box and diagnostics the ordinary \
              glyph draw does not"
)]
fn render_text<B: RasterBackend>(
    ctx: &RenderCtx<'_>,
    device: &mut B::Device,
    backend: &B,
    caches: &mut RenderCaches,
    object: &pdfrum_page::TextObject,
    state: &pdfrum_page::GraphicsState,
    to_device: Affine,
    device_box: Rect,
    diags: &mut Diagnostics,
) {
    let Some((font, _)) = &object.font else {
        return;
    };
    let Some(kinds) = paint_kinds(object.render_mode, has_face(font)) else {
        return;
    };
    if !kinds.fill && !kinds.stroke {
        return; // Tr 7 contributes only to the clip, which the stack owns.
    }
    // A type-3 font has no outlines to fill: each character is a content
    // stream, walked with the same machinery a form is.
    if font.type3().is_some() {
        render_type3_text(
            ctx, device, backend, caches, object, state, to_device, device_box, diags,
        );
        return;
    }
    // A pattern-coloured glyph run goes to `DrawTextPathWithPattern`, which
    // returns before the ordinary draw. Its unstroked arm
    // (`cpdf_renderstatus.cpp:1290-1310`) does not draw glyphs at all: it
    // builds a **synthetic path object** — the run's own bounding rectangle,
    // filled winding, carrying the text's colour and general state, with the
    // run itself appended to a copy of the current clip path — and sends that
    // through `RenderSingleObject`. The pattern then paints the rectangle and
    // the text clip cuts it to the glyphs.
    //
    // That is the whole shape, and it is why this could not be written until
    // the clip stack learned to hold text runs.
    if kinds.fill && !kinds.stroke && state.fill.is_pattern() {
        render_pattern_text(
            ctx, device, backend, caches, object, state, to_device, device_box, diags,
        );
        return;
    }
    // With a stroke in play the run *is* drawn glyph by glyph, each outline
    // becoming its own path object — so the ordinary path below handles it,
    // and only the colour that will not resolve is drained. A pattern colour
    // has no components, so letting it through would paint the black that
    // `to_rgb`'s absence falls back to.
    let kinds = crate::text::TextPaintKinds {
        fill: kinds.fill && !state.fill.is_pattern(),
        stroke: kinds.stroke && !state.stroke.is_pattern(),
        ..kinds
    };
    if !kinds.fill && !kinds.stroke {
        return;
    }
    let (fill, stroke) = colors(ctx, state, ObjectKind::Text);
    // `TextObject::matrix` already carries the CTM, so only the
    // page-to-device transform is added — composing `state.ctm` again would
    // apply it twice.
    let glyphs = place_glyphs(
        object,
        state,
        &mut caches.glyphs,
        to_device,
        &ctx.opts,
        kinds,
    );
    for glyph in glyphs {
        // The oracle's small-text path: an alpha bitmap, blitted whole, rather
        // than an outline filled where it lands. It is the majority of the
        // text in the corpus, and reproducing it is what closes the coverage
        // band `docs/status/pdfrum-render.md` has carried since wave 3. When
        // it declines — an unhintable face is fine, but a glyph too large or
        // too degenerate to rasterize is not — the oracle skips the glyph
        // outright (`if (!glyph.glyph_) continue;`), and so does this.
        if let Some(placement) = glyph.bitmap
            && kinds.fill
            && !kinds.stroke
        {
            draw_glyph_bitmap(
                device,
                &mut caches.glyph_bitmaps,
                font,
                &glyph,
                placement,
                fill,
            );
            continue;
        }
        let paint = PathPaint {
            fill: kinds.fill.then_some(fill),
            stroke: kinds.stroke.then_some(stroke),
            rule: crate::device::FillRule::Winding,
            // The flag that keeps a glyph stem out of the zero-area
            // hairline conversion.
            text_mode: true,
        };
        draw_path(
            device,
            backend,
            &glyph.outline,
            glyph.matrix,
            paint,
            &state.stroke_params,
            &ctx.opts,
        );
    }
}

/// Blit one glyph as an alpha bitmap (`DrawNormalText`'s per-glyph body,
/// `cfx_renderdevice.cpp:1330-1367`).
///
/// The bitmap is rasterized about the glyph's **own** origin — the matrix's
/// translation is dropped — so that one bitmap serves the glyph wherever it
/// lands on the page, which is what makes the cache worth having and what makes
/// its key match the oracle's. The placement is then two integers and a phase.
///
/// The outline it rasterizes is the **hinted** one where the face has hinting
/// programs and a table directory, falling back to the unhinted outline
/// otherwise, because that is exactly the oracle's `!IsTtOt()` rule plus its
/// `FT_LOAD_PEDANTIC` retry.
fn draw_glyph_bitmap(
    device: &mut dyn RenderDevice,
    cache: &mut crate::glyph::BitmapCache,
    font: &pdfrum_font::Font,
    glyph: &crate::text::PlacedGlyph,
    placement: crate::text::BitmapPlacement,
    fill: Argb,
) {
    if fill.is_invisible() {
        return;
    }
    // The glyph's shape in device pixels, with its origin at zero. Dropping the
    // translation is what the key's four coefficients already assume.
    let [a, b, c, d, _, _] = glyph.matrix.as_coeffs();
    let shape = Affine::new([a, b, c, d, 0.0, 0.0]);
    let key = crate::glyph::BitmapKey::new(glyph.key, shape);

    let Some(lcd) = cache.get_or_insert(key, || {
        // A hinted outline is worth up to ten counts a pixel at 6 pt and costs
        // a bytecode run, so it is requested only here — on a cache miss — and
        // never on the outline path, which the oracle also draws unhinted.
        let outline = font
            .hinted_glyph_path(glyph.key.gid)
            .unwrap_or_else(|| glyph.outline.clone());
        crate::glyph::render_lcd(&(shape * outline))
    }) else {
        return;
    };
    let bitmap = lcd.to_gray(placement.phase);
    let Some(pixels) = crate::glyph::recolour(&bitmap, fill.to_peniko()) else {
        return;
    };
    // `draw_image` maps the image's own pixel grid, so a plain translation puts
    // texel (0, 0) at the bitmap's top-left corner. Both terms are whole
    // numbers: the snapped origin by construction, and the bitmap's corner
    // because FreeType's box is computed in whole pixels.
    device.draw_image(
        &pixels,
        Affine::translate((
            placement.origin.x + f64::from(bitmap.left),
            placement.origin.y + f64::from(bitmap.top),
        )),
        ImageQuality::Nearest,
        1.0,
    );
}

/// Whether a glyph procedure is the sole-image case *and* taking it through
/// the char-proc path would paint the wrong thing
/// (`LoadBitmapFromSoleImageOfForm`, `cpdf_type3char.cpp:35-52`).
///
/// An uncoloured procedure whose one object is an image has that image lifted
/// out and blitted as the glyph's 8bpp **mask**, in the text object's colour —
/// a path this engine does not have.
///
/// The distinction that matters is what the image *is*. A stencil
/// (`/ImageMask true`) already paints in the fill colour wherever its bits are
/// set, which is what the mask blit does, so walking it as a char proc lands
/// on the same pixels and it is *not* declined — and declining it would lose
/// every bitmap-font glyph in the corpus, which is what these procedures
/// overwhelmingly are. A colour image, by contrast, would paint its own
/// samples where the oracle paints a mask, so that one is declined.
///
/// Exactly one object either way: a procedure that draws an image *and* a
/// rule is a char proc like any other.
#[must_use]
fn sole_color_image(objects: &[PageObject]) -> bool {
    matches!(objects, [PageObject::Image(i)] if !i.object.is_mask)
}

/// Draw one type-3 text object, one glyph procedure at a time
/// (`ProcessType3Text`, `cpdf_renderstatus.cpp:933-1130`).
///
/// Four contracts, each of which changes pixels:
///
/// - **The colour comes from the *outer* text object**, and a `d1`
///   (uncoloured) procedure takes it for every drawing operation inside,
///   whatever colours the procedure sets. A `d0` (coloured) one keeps what it
///   sets and falls back to the text object's only where it sets none. That
///   is the [`Type3Frame`] the child context carries.
/// - **`bForceHalftone` and `bRectAA` are forced on**, and the second is
///   visible: `bRectAA` disables the axis-aligned rect snapping, so a
///   rectangle inside a glyph *is* antialiased where the same rectangle on the
///   page would not be.
/// - **A translucent procedure goes through its own buffer**, blitted with a
///   plain normal blend and no group semantics, so the procedure's own
///   overlapping strokes do not accumulate alpha against each other.
/// - **The recursion guard is a set of font identities, not a depth.** A font
///   may not appear twice anywhere in the ancestry, so a glyph that shows
///   text in its own font draws nothing rather than recursing sixty-four
///   levels first.
#[expect(
    clippy::too_many_arguments,
    reason = "a glyph procedure needs the context, device, backend, caches, \
              the text object, its state, and the page transform"
)]
fn render_type3_text<B: RasterBackend>(
    ctx: &RenderCtx<'_>,
    device: &mut B::Device,
    backend: &B,
    caches: &mut RenderCaches,
    object: &pdfrum_page::TextObject,
    state: &pdfrum_page::GraphicsState,
    to_device: Affine,
    device_box: Rect,
    diags: &mut Diagnostics,
) {
    let Some((font, _)) = &object.font else {
        return;
    };
    // The guard is by font identity and is checked before anything is drawn.
    if ctx.type3_font_is_active(font.id()) || !ctx.may_recurse() {
        return;
    }
    // `GetFillArgbForType3` skips the type-3 branch, so this is the outer
    // object's own colour even inside a nested procedure.
    let fill = resolve_argb(
        &state.fill,
        state.general.fill_alpha,
        state
            .general
            .transfer
            .as_ref()
            .map(|t| TransferFunc::new(t))
            .as_ref(),
        ctx.initial_fill,
        &ctx.opts,
        ObjectKind::Text,
        false,
    );
    if fill.is_invisible() {
        // A pattern-coloured run resolves to the `0xFFFFFFFF` sentinel, which
        // `GetFillArgbForType3` turns into a zero ARGB. There is no
        // `DrawTextPathWithPattern` for a type-3 run, so this is where such a
        // run stops: the glyphs paint nothing at all.
        return;
    }
    let mut ancestry: Vec<pdfrum_font::FontId> = ctx.type3_fonts.to_vec();
    ancestry.push(font.id());

    for placed in crate::text::place_type3_chars(object, state, to_device) {
        let Some(metrics) = object.type3_metrics.get(&placed.code) else {
            continue;
        };
        if metrics.objects.is_empty() || !is_available_matrix(placed.matrix) {
            continue;
        }
        // `LoadBitmapFromSoleImageOfForm`: an **uncoloured** procedure whose
        // one object is an image is not walked as a char proc at all — the
        // image becomes the glyph's *bitmap*, blitted as an 8bpp mask in the
        // text colour by a separate path this engine does not have. Walking it
        // here instead paints the image's own colours where the oracle paints
        // a mask, so the char-proc path declines it.
        if !metrics.colored && sole_color_image(&metrics.objects) {
            continue;
        }
        let inner = RenderCtx {
            opts: ctx.opts.for_type3_char_proc(),
            type3: Some(crate::ctx::Type3Frame {
                fill,
                colored: metrics.colored,
            }),
            type3_fonts: &ancestry,
            initial_fill: Some(fill),
            initial_stroke: Some(fill),
            ..ctx.deeper()
        };
        if fill.a == 255 {
            render_object_list(
                &inner,
                device,
                backend,
                caches,
                &metrics.objects,
                &Visibility::all_visible(), // the font's objects, not the page's
                placed.matrix,
                device_box,
                diags,
            );
            continue;
        }
        render_translucent_char_proc(
            &inner, device, backend, caches, metrics, &placed, fill, device_box, diags,
        );
    }
}

/// One type-3 glyph procedure drawn through its own buffer.
///
/// A translucent glyph cannot paint straight onto the page: its procedure may
/// overlap itself, and compositing each stroke at the object's alpha would
/// darken the overlaps. So the procedure runs into a buffer sized to the
/// glyph's device extent, and the buffer is blitted once.
///
/// **The alpha rides on the procedure's own fill colour, and the blit is
/// opaque.** That is one factor of the object's alpha, applied once — but
/// *where* it is applied is the whole of it: painting the procedure opaquely
/// and scaling at the blit instead is also one factor, and gives a different
/// answer wherever the procedure covers a pixel partially. A half-covered edge
/// pixel painted at alpha 128 and blitted opaquely keeps that 128; painted
/// opaquely to coverage 128 and blitted at half alpha becomes 64. The oracle
/// does the former, and `bug_1746` — whose glyph is a fax-coded image mask
/// under `ca 0.5` — is where the two visibly disagree.
///
/// **The buffer is sized from the objects' own extent, not the declared
/// `/FontBBox`.** A `d1` box is its operands scaled by a thousand and put
/// through the font matrix; with the conventional thousandth matrix the two
/// cancel, and with an identity `/FontMatrix` they do not — `bug_1746`'s box
/// lands at x 8050 on a 200-pixel page, so the buffer misses the glyph
/// entirely and nothing is drawn at all.
#[expect(
    clippy::too_many_arguments,
    reason = "the same inputs the opaque path takes, plus the placed glyph and \
              the colour its alpha is read from"
)]
fn render_translucent_char_proc<B: RasterBackend>(
    ctx: &RenderCtx<'_>,
    device: &mut B::Device,
    backend: &B,
    caches: &mut RenderCaches,
    metrics: &pdfrum_page::type3::Type3Metrics,
    placed: &crate::text::PlacedType3Char,
    fill: Argb,
    device_box: Rect,
    diags: &mut Diagnostics,
) {
    let bbox = placed
        .matrix
        .transform_rect_bbox(metrics.painted)
        .intersect(device_box);
    let rect = outer_rect(bbox).intersect(outer_rect(device_box));
    let (Ok(w), Ok(h)) = (u32::try_from(rect.width()), u32::try_from(rect.height())) else {
        return;
    };
    if w == 0 || h == 0 || w > MAX_TARGET_DIMENSION || h > MAX_TARGET_DIMENSION {
        return;
    }
    let mut sub = backend.new_target(w, h, peniko::Color::TRANSPARENT);
    let offset = Affine::translate((-f64::from(rect.left), -f64::from(rect.top)));
    let translucent = crate::ctx::Type3Frame {
        fill,
        colored: metrics.colored,
    };
    let inner = RenderCtx {
        type3: Some(translucent),
        initial_fill: Some(fill),
        initial_stroke: Some(fill),
        ..ctx.clone()
    };
    render_object_list(
        &inner,
        &mut sub,
        backend,
        caches,
        &metrics.objects,
        &Visibility::all_visible(), // the font's objects, not the page's
        offset * placed.matrix,
        Rect::new(0.0, 0.0, f64::from(w), f64::from(h)),
        diags,
    );
    let pixels = backend.finish(sub);
    device.draw_image(
        &pixels,
        Affine::translate((f64::from(rect.left), f64::from(rect.top))),
        ImageQuality::Nearest,
        1.0,
    );
}

/// The resample quality a stencil-as-mask is drawn at.
///
/// A thin wrapper over [`resample_quality`] that guards the destination
/// extent, which the ordinary image path guards separately.
#[expect(
    clippy::cast_possible_truncation,
    reason = "`image_value_fits` rejects a non-finite extent and anything at \
              or above MAX_IMAGE_VALUE (2^28), so both rounded values are \
              well inside i64"
)]
fn mask_quality(
    image: &pdfrum_page::ImageData,
    opts: &RenderOptions,
    extent: Rect,
) -> ImageQuality {
    if !crate::image::image_value_fits(extent.width())
        || !crate::image::image_value_fits(extent.height())
    {
        return ImageQuality::Nearest;
    }
    resample_quality(
        image,
        opts,
        image.width,
        image.height,
        extent.width().round() as i64,
        extent.height().round() as i64,
    )
}

/// Paint a stencil whose fill colour is a pattern (`DrawPatternImage`,
/// `cpdf_imagerenderer.cpp:325-374`).
///
/// The pattern is drawn into its own buffer over the stencil's device extent,
/// and the **stencil becomes that buffer's alpha** — so the pattern shows
/// through the set bits and nothing shows through the clear ones. Painting a
/// pattern colour through the ordinary image path instead paints the pattern's
/// *fallback* colour, which for a coloured tiling pattern is mid grey.
///
/// The object's alpha is deliberately **not** applied at the blit: unlike
/// `DrawMaskedImage`, the pattern path has already consumed it inside the
/// tiling cell's inherited state or the shading's rounded alpha.
#[expect(
    clippy::too_many_arguments,
    reason = "the stencil path needs the context, device, backend, caches, the \
              image, its state and the page transform"
)]
fn render_pattern_stencil<B: RasterBackend>(
    ctx: &RenderCtx<'_>,
    device: &mut B::Device,
    backend: &B,
    caches: &mut RenderCaches,
    object: &pdfrum_page::ImageObject,
    state: &pdfrum_page::GraphicsState,
    to_device: Affine,
    device_box: Rect,
    diags: &mut Diagnostics,
) {
    if !ctx.may_recurse() {
        return;
    }
    let matrix = to_device * object.matrix;
    let bbox = matrix
        .transform_rect_bbox(unit_rect())
        .intersect(device_box);
    let rect = outer_rect(bbox).intersect(outer_rect(device_box));
    let (Ok(w), Ok(h)) = (u32::try_from(rect.width()), u32::try_from(rect.height())) else {
        return;
    };
    if w == 0 || h == 0 || w > MAX_TARGET_DIMENSION || h > MAX_TARGET_DIMENSION {
        return;
    }
    let offset = Affine::translate((-f64::from(rect.left), -f64::from(rect.top)));
    let inner_box = Rect::new(0.0, 0.0, f64::from(w), f64::from(h));

    // The pattern, over the stencil's whole extent.
    let mut pattern_target = backend.new_target(w, h, peniko::Color::TRANSPARENT);
    let inner = RenderCtx {
        std_cs: true,
        ..ctx.deeper()
    };
    paint_pattern(
        &inner,
        &mut pattern_target,
        backend,
        caches,
        state,
        false,
        // An image object clips by its transformed bounding box, which here is
        // the whole buffer.
        &PatternClip::Rect(inner_box),
        offset * to_device,
        inner_box,
        diags,
    );
    let mut pixels = backend.finish(pattern_target);

    // The stencil, rasterized on the same grid so no resampling is needed
    // when it becomes the alpha. Its set bits are opaque white; the readback
    // takes the alpha channel, which is exactly the coverage.
    // The stencil is drawn as a coverage mask for a pattern, so its colour
    // is a placeholder and no transfer function applies to it.
    let stencil = to_pixmap(&object.image, Argb::opaque(255, 255, 255), None);
    if stencil.width() == 0 || stencil.height() == 0 {
        return;
    }
    let placement = offset
        * matrix
        * Affine::new([
            1.0 / f64::from(object.image.width),
            0.0,
            0.0,
            -1.0 / f64::from(object.image.height),
            0.0,
            1.0,
        ]);
    // The mask goes through the ordinary image renderer, so it gets the
    // ordinary resample selection — which is what puts a soft edge on a
    // scaled-up stencil rather than a hard one.
    let extent = placement.transform_rect_bbox(Rect::new(
        0.0,
        0.0,
        f64::from(object.image.width),
        f64::from(object.image.height),
    ));
    let quality = mask_quality(&object.image, &ctx.opts, extent);
    let (stencil, placement) =
        match crate::stretch::prescale(&stencil, placement, extent.width(), extent.height()) {
            Some((reduced, t)) => (reduced, t),
            None => (stencil, placement),
        };
    let mut mask_target = backend.new_target(w, h, peniko::Color::TRANSPARENT);
    mask_target.draw_image(
        &stencil,
        placement,
        effective_quality(quality, placement),
        1.0,
    );
    let mask = backend.finish(mask_target).alpha_mask();
    pixels.multiply_alpha_mask(&mask);

    let blend = overprint_blend(None, &state.general);
    let layered = !matches!(blend, pdfrum_page::BlendMode::Normal);
    if layered {
        device.push_layer(blend, 1.0, None);
    }
    device.draw_image(
        &pixels,
        Affine::translate((f64::from(rect.left), f64::from(rect.top))),
        ImageQuality::Nearest,
        1.0,
    );
    if layered {
        device.pop();
    }
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "`image_value_fits` has already rejected a non-finite extent and \
              anything at or above MAX_IMAGE_VALUE (2^28), so both rounded \
              values are well inside i64"
)]
#[expect(
    clippy::too_many_arguments,
    reason = "the pattern-stencil arm needs the backend, caches, device box \
              and diagnostics the ordinary image draw does not"
)]
fn render_image<B: RasterBackend>(
    ctx: &RenderCtx<'_>,
    device: &mut B::Device,
    backend: &B,
    caches: &mut RenderCaches,
    object: &pdfrum_page::ImageObject,
    state: &pdfrum_page::GraphicsState,
    to_device: Affine,
    device_box: Rect,
    diags: &mut Diagnostics,
) {
    let matrix = to_device * object.matrix;
    if !is_available_matrix(matrix) {
        return;
    }
    // `DrawPatternImage`: a stencil whose fill colour is a pattern paints the
    // *pattern* through the stencil, not a colour. The ordinary path would
    // paint the pattern's fallback colour — mid grey for a coloured tiling
    // one — across every set bit.
    if object.is_mask && state.fill.is_pattern() {
        render_pattern_stencil(
            ctx, device, backend, caches, object, state, to_device, device_box, diags,
        );
        return;
    }
    // `DrawMaskedImage`: a mask on a grid of its own is stretched to the
    // device by itself rather than through the base's samples.
    if object
        .image
        .mask
        .as_ref()
        .is_some_and(|m| !crate::image::is_coregistered(m, &object.image))
    {
        render_masked_image(ctx, device, backend, object, state, to_device, device_box);
        return;
    }
    let (fill, _) = colors(ctx, state, ObjectKind::Other);
    let image = &object.image;
    // `StartRenderDIBBase` runs a non-identity `/TR` over the image's own
    // samples, not only over the fill colour a stencil takes.
    let transfer = state
        .general
        .transfer
        .as_ref()
        .map(|t| TransferFunc::new(t));
    let pixels = to_pixmap(image, fill, transfer.as_ref());
    if pixels.width() == 0 || pixels.height() == 0 {
        return;
    }
    // The image's unit square maps through the object matrix, so the device
    // transform folds in the sample grid's own size and the y flip PDF's
    // image space needs.
    let placement = matrix
        * Affine::new([
            1.0 / f64::from(image.width),
            0.0,
            0.0,
            -1.0 / f64::from(image.height),
            0.0,
            1.0,
        ]);
    let corners = placement.transform_rect_bbox(Rect::new(
        0.0,
        0.0,
        f64::from(image.width),
        f64::from(image.height),
    ));
    if !crate::image::image_value_fits(corners.width())
        || !crate::image::image_value_fits(corners.height())
    {
        return;
    }
    let quality = resample_quality(
        image,
        &ctx.opts,
        image.width,
        image.height,
        corners.width().round() as i64,
        corners.height().round() as i64,
    );
    // A reduction is low-passed here rather than left to the backend's two-tap
    // kernel, which sees at most two of the many source pixels a shrunken
    // destination pixel covers.
    let (pixels, placement) =
        match crate::stretch::prescale(&pixels, placement, corners.width(), corners.height()) {
            Some((reduced, t)) => (reduced, t),
            None => (pixels, placement),
        };
    let blend = overprint_blend(None, &state.general);
    let layered = !matches!(blend, pdfrum_page::BlendMode::Normal);
    if layered {
        device.push_layer(blend, 1.0, None);
    }
    device.draw_image(
        &pixels,
        placement,
        effective_quality(quality, placement),
        state.general.fill_alpha,
    );
    if layered {
        device.pop();
    }
}

/// Paint an image whose mask has a resolution of its own (`DrawMaskedImage`,
/// `cpdf_imagerenderer.cpp:375-424`).
///
/// A mask is **never resolution-reduced** (design brief §1.19.9), so its
/// dimensions are its own and generally not the base's: `bug_1396266` puts a
/// 64×64 stencil on a 3×3 image, `bug_1236` a 100×100 `/SMask` on a 400×400
/// one. PDFium never reconciles the two grids. It renders the base into a
/// device-sized buffer, renders the mask into a second buffer over the same
/// device rect through the same matrix, and multiplies — so each is resampled
/// from its own resolution straight to the device and neither is ever sampled
/// at the other's coordinates.
///
/// Folding the mask into the base's pixels instead reads the wrong sample
/// everywhere the sizes differ, and — because [`pdfrum_page::ImageMask::alpha_at`]
/// reports out-of-range as opaque — leaves the majority of a base larger than
/// its mask completely unmasked.
fn render_masked_image<B: RasterBackend>(
    ctx: &RenderCtx<'_>,
    device: &mut B::Device,
    backend: &B,
    object: &pdfrum_page::ImageObject,
    state: &pdfrum_page::GraphicsState,
    to_device: Affine,
    device_box: Rect,
) {
    let image = &object.image;
    let Some(mask) = image.mask.as_ref() else {
        return;
    };
    let Some((mask_dict, mask_pixels)) = crate::image::separate_mask(mask) else {
        return;
    };
    let matrix = to_device * object.matrix;
    let bbox = matrix
        .transform_rect_bbox(unit_rect())
        .intersect(device_box);
    let rect = outer_rect(bbox).intersect(outer_rect(device_box));
    let (Ok(w), Ok(h)) = (u32::try_from(rect.width()), u32::try_from(rect.height())) else {
        return;
    };
    if w == 0 || h == 0 || w > MAX_TARGET_DIMENSION || h > MAX_TARGET_DIMENSION {
        return;
    }
    let offset = Affine::translate((-f64::from(rect.left), -f64::from(rect.top)));

    // The base, over its own extent, with no mask folded in.
    let (fill, _) = colors(ctx, state, ObjectKind::Other);
    let transfer = state
        .general
        .transfer
        .as_ref()
        .map(|t| TransferFunc::new(t));
    let base = to_pixmap(image, fill, transfer.as_ref());
    if base.width() == 0 || base.height() == 0 {
        return;
    }
    let mut base_target = backend.new_target(w, h, peniko::Color::TRANSPARENT);
    let base_placement = offset * matrix * sample_grid(image.width, image.height);
    let base_extent = base_placement.transform_rect_bbox(Rect::new(
        0.0,
        0.0,
        f64::from(image.width),
        f64::from(image.height),
    ));
    let base_quality = mask_quality(image, &ctx.opts, base_extent);
    // The base and its mask are reduced independently, each toward its own
    // device footprint — which is the same footprint, reached from two
    // different resolutions. Neither ever passes through the other's grid.
    let (base, base_placement) = match crate::stretch::prescale(
        &base,
        base_placement,
        base_extent.width(),
        base_extent.height(),
    ) {
        Some((reduced, t)) => (reduced, t),
        None => (base, base_placement),
    };
    base_target.draw_image(
        &base,
        base_placement,
        effective_quality(base_quality, base_placement),
        1.0,
    );
    let mut pixels = backend.finish(base_target);

    // The mask, over the *same* device rect through the *same* matrix, at its
    // own resolution — which is the whole point of the separate pass.
    let mask_placement = offset * matrix * sample_grid(mask_dict.width, mask_dict.height);
    let mask_extent = mask_placement.transform_rect_bbox(Rect::new(
        0.0,
        0.0,
        f64::from(mask_dict.width),
        f64::from(mask_dict.height),
    ));
    let mask_q = mask_quality(&mask_dict, &ctx.opts, mask_extent);
    let (mask_pixels, mask_placement) = match crate::stretch::prescale(
        &mask_pixels,
        mask_placement,
        mask_extent.width(),
        mask_extent.height(),
    ) {
        Some((reduced, t)) => (reduced, t),
        None => (mask_pixels, mask_placement),
    };
    let mut mask_target = backend.new_target(w, h, peniko::Color::TRANSPARENT);
    mask_target.draw_image(
        &mask_pixels,
        mask_placement,
        effective_quality(mask_q, mask_placement),
        1.0,
    );
    pixels.multiply_alpha_mask(&backend.finish(mask_target).alpha_mask());

    let blend = overprint_blend(None, &state.general);
    let layered = !matches!(blend, pdfrum_page::BlendMode::Normal);
    if layered {
        device.push_layer(blend, 1.0, None);
    }
    device.draw_image(
        &pixels,
        Affine::translate((f64::from(rect.left), f64::from(rect.top))),
        ImageQuality::Nearest,
        state.general.fill_alpha,
    );
    if layered {
        device.pop();
    }
}

/// The transform from an image's sample grid to its unit square, with the y
/// flip PDF image space needs.
fn sample_grid(width: u32, height: u32) -> Affine {
    Affine::new([
        1.0 / f64::from(width),
        0.0,
        0.0,
        -1.0 / f64::from(height),
        0.0,
        1.0,
    ])
}

fn render_shading<B: RasterBackend>(
    ctx: &RenderCtx<'_>,
    device: &mut B::Device,
    backend: &B,
    object: &pdfrum_page::ShadingObject,
    state: &pdfrum_page::GraphicsState,
    to_device: Affine,
    device_box: Rect,
) {
    let matrix = to_device * object.matrix;
    if !is_available_matrix(matrix) {
        return;
    }
    // A bare `sh` paints its whole clip region, with no geometry clip of its
    // own; the alpha here is **rounded**, unlike the truncation everywhere
    // else colour is resolved.
    let alpha = alpha_byte_rounding(state.general.fill_alpha);
    // A `sh` with no bounds of its own paints the whole device box.
    let mut bbox = if object.bounds.is_zero_area() {
        device_box
    } else {
        to_device.transform_rect_bbox(object.bounds)
    };
    if let Some(b) = object.shading.bbox {
        bbox = bbox.intersect(matrix.transform_rect_bbox(b));
    }
    let rect = outer_rect(bbox.intersect(device_box));
    if !rect.is_valid() {
        return;
    }
    draw_shading_into(ctx, device, backend, &object.shading, rect, matrix, alpha);
}

/// Rasterize any of the seven shading types into `rect` and blit it.
///
/// The seventh and sixth need a rasterizer rather than a buffer, so they take
/// a scratch device; the other five are pure engine code. Both `sh` and a
/// shading *pattern* reach a shading exactly this way, and routing them
/// through one function is what keeps a Coons pattern from being the silent
/// no-op it was when only `sh` knew about the scratch path.
pub(crate) fn draw_shading_into<B: RasterBackend>(
    ctx: &RenderCtx<'_>,
    device: &mut B::Device,
    backend: &B,
    shading: &pdfrum_page::Shading,
    rect: IntRect,
    matrix: Affine,
    alpha: u8,
) {
    let at = Affine::translate((f64::from(rect.left), f64::from(rect.top)));
    match shading.kind() {
        pdfrum_page::ShadingKind::CoonsMesh | pdfrum_page::ShadingKind::TensorMesh => {
            let tensor = shading.kind() == pdfrum_page::ShadingKind::TensorMesh;
            let (Ok(w), Ok(h)) = (u32::try_from(rect.width()), u32::try_from(rect.height())) else {
                return;
            };
            if w == 0 || h == 0 || w > MAX_TARGET_DIMENSION || h > MAX_TARGET_DIMENSION {
                return;
            }
            // Every cell goes into the scratch at full opacity, so abutting
            // cells overpaint identically on both backends; the shading's
            // alpha is applied exactly once, at the blit below.
            let mut scratch = backend.new_target(w, h, peniko::Color::TRANSPARENT);
            shading::draw_patches(&mut scratch, shading, rect, matrix, tensor);
            let pixels = backend.finish(scratch);
            device.draw_image(&pixels, at, ImageQuality::Nearest, f32::from(alpha) / 255.0);
        }
        _ => {
            let Some(pixels) = shading::draw_to_pixmap(shading, rect, matrix, alpha, &ctx.opts)
            else {
                return;
            };
            device.draw_image(&pixels, at, ImageQuality::Nearest, 1.0);
        }
    }
}

/// A solid brush, for callers that want one without reaching into `device`.
#[must_use]
pub fn solid(color: crate::color::Argb) -> Brush<'static> {
    Brush::Solid(color.to_peniko())
}

/// The transparency a bare page carries when none is declared.
#[must_use]
pub fn default_transparency() -> Transparency {
    Transparency::default()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use kurbo::BezPath;
    use pdfrum_page::state::ContentMarks;
    use pdfrum_page::{Content, GraphicsState, PathObject};

    use super::*;

    fn path_object(path: BezPath, state: GraphicsState) -> PageObject {
        PageObject::Path(Box::new(Content {
            object: PathObject {
                path,
                matrix: Affine::IDENTITY,
                fill_rule: pdfrum_page::FillRule::Winding,
                stroke: false,
            },
            state,
            marks: ContentMarks::new(),
            content_stream: 0,
            dirty: false,
            active: true,
        }))
    }

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> BezPath {
        let mut p = BezPath::new();
        p.move_to((x0, y0));
        p.line_to((x1, y0));
        p.line_to((x1, y1));
        p.line_to((x0, y1));
        p.close_path();
        p
    }

    #[test]
    fn cull_uses_strict_inequalities() {
        let cull = Rect::new(0.0, 0.0, 10.0, 10.0);
        // Exactly touching the right edge is kept, not dropped.
        let touching = path_object(rect(10.0, 0.0, 20.0, 5.0), GraphicsState::default());
        assert!(!culled(&touching, cull));
        // Strictly beyond it is dropped.
        let beyond = path_object(rect(10.1, 0.0, 20.0, 5.0), GraphicsState::default());
        assert!(culled(&beyond, cull));
    }

    #[test]
    fn a_text_object_is_never_culled() {
        // Its extent needs the font's metrics, so declining the cull is the
        // safe answer — it is an optimisation, not a correctness rule.
        let obj = PageObject::Text(Box::new(Content {
            object: pdfrum_page::TextObject {
                segments: Box::new([]),
                position: kurbo::Point::ZERO,
                matrix: Affine::IDENTITY,
                font: None,
                font_source: None,
                render_mode: pdfrum_page::TextRenderMode::Fill,
                type3_metrics: BTreeMap::default(),
            },
            state: GraphicsState::default(),
            marks: ContentMarks::new(),
            content_stream: 0,
            dirty: false,
            active: true,
        }));
        assert!(!culled(&obj, Rect::new(1000.0, 1000.0, 1001.0, 1001.0)));
    }

    #[test]
    fn a_degenerate_matrix_yields_no_cull_rect() {
        assert!(cull_rect(Affine::new([0.0; 6]), Rect::new(0.0, 0.0, 10.0, 10.0)).is_none());
    }

    #[test]
    fn target_size_rejects_an_empty_page() {
        let mut page = Page::empty();
        page.crop_box = Rect::new(0.0, 0.0, 0.0, 0.0);
        page.media_box = page.crop_box;
        let err = target_size(&page, &RenderOptions::default()).expect_err("empty");
        assert!(matches!(err, Error::TargetEmpty { .. }));
    }

    #[test]
    fn target_size_truncates_rather_than_rounding_up() {
        // A4 is 595.276 x 841.89 points; `pdfium_test` casts, so the bitmap
        // is 595x841. Rounding up costs a one-pixel border, which the
        // harness reports as a size mismatch rather than a pixel difference.
        let a4 = Rect::new(0.0, 0.0, 595.276, 841.89);
        let page = Page {
            media_box: a4,
            crop_box: a4,
            ..Page::empty()
        };
        assert_eq!(
            target_size(&page, &RenderOptions::default()).expect("renderable"),
            (595, 841)
        );
    }

    #[test]
    fn the_page_matrix_fits_the_page_to_the_truncated_device_box() {
        // `GetDisplayMatrixForRect` divides the *integer* device rect by the
        // page's float size (`cpdf_page.cpp:216-218`), and
        // `CPDFSDK_RenderPageWithContext` passes it the truncated bitmap
        // size. So an A4 page 841.89 tall renders into 841 rows: page y = 0
        // lands on device row 841 and page y = 841.89 on row 0, exactly.
        let a4 = Rect::new(0.0, 0.0, 595.276, 841.89);
        let page = Page {
            media_box: a4,
            crop_box: a4,
            ..Page::empty()
        };
        let m = page_matrix(&page, &RenderOptions::default());
        let bottom = m * kurbo::Point::new(0.0, 0.0);
        let top = m * kurbo::Point::new(595.276, 841.89);
        assert!((bottom.y - 841.0).abs() < 1e-9, "page bottom at {bottom:?}");
        assert!((top.y - 0.0).abs() < 1e-9, "page top at {top:?}");
        assert!((top.x - 595.0).abs() < 1e-9, "page right at {top:?}");
        // Flipping about the float height instead leaves the top of the page
        // 0.89 device px out — invisible while glyphs were filled at their
        // true position, and a whole row once their origins are snapped.
        let midpage = m * kurbo::Point::new(0.0, 420.945);
        assert!((midpage.y - 420.5).abs() < 1e-9, "midpage at {midpage:?}");
    }

    #[test]
    fn a_scaled_render_fits_the_page_to_its_own_truncated_box() {
        // The caller's transform is consumed in *sizing* the bitmap, exactly
        // as `pdfium_test` consumes `--scale`; the display matrix is then
        // built onto that size rather than composed on top of it.
        let a4 = Rect::new(0.0, 0.0, 595.276, 841.89);
        let page = Page {
            media_box: a4,
            crop_box: a4,
            ..Page::empty()
        };
        let opts = RenderOptions {
            transform: Affine::scale(2.0),
            ..RenderOptions::default()
        };
        let (w, h) = target_size(&page, &opts).expect("renderable");
        assert_eq!((w, h), (1190, 1683));
        let m = page_matrix(&page, &opts);
        let corner = m * kurbo::Point::new(595.276, 0.0);
        assert!((corner.x - 1190.0).abs() < 1e-9, "{corner:?}");
        assert!((corner.y - 1683.0).abs() < 1e-9, "{corner:?}");
    }

    #[test]
    fn target_size_rejects_an_oversized_page() {
        let opts = RenderOptions {
            transform: Affine::scale(200.0),
            ..RenderOptions::default()
        };
        let err = target_size(&Page::empty(), &opts).expect_err("too large");
        assert!(matches!(
            err,
            Error::TargetTooLarge {
                limit: MAX_TARGET_DIMENSION,
                ..
            }
        ));
    }

    #[test]
    fn crop_lifts_a_sub_rectangle() {
        let mut src = Pixmap::new(4, 4);
        src.set_pixel(2, 3, [1, 2, 3, 255]);
        let out = crop(
            &src,
            IntRect {
                left: 2,
                top: 2,
                right: 4,
                bottom: 4,
            },
        );
        assert_eq!((out.width(), out.height()), (2, 2));
        assert_eq!(out.pixel(0, 1), Some([1, 2, 3, 255]));
    }

    #[test]
    fn crop_of_an_out_of_range_rect_is_transparent() {
        let src = Pixmap::filled(2, 2, peniko::Color::from_rgba8(9, 9, 9, 255));
        let out = crop(
            &src,
            IntRect {
                left: 10,
                top: 10,
                right: 12,
                bottom: 12,
            },
        );
        assert_eq!(out.pixel(0, 0), Some([0, 0, 0, 0]));
    }
}
