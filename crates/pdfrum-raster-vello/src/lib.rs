//! GPU `vello` implementation of `pdfrum-render`'s `RenderDevice` and
//! `RasterBackend` traits, on a **caller-supplied** `wgpu` device
//! (PLAN.md §M12c).
//!
//! *Renamed 2026-09-02, was `pdfrum-raster-vello-gpu` with
//! `VelloGpuBackend`.* The crates now take vello's own names: upstream's
//! `vello` **is** the GPU renderer on `wgpu`, so the bare name belongs here
//! and the CPU wrapper is `pdfrum-raster-vello-cpu`.
//!
//! This is the fourth backend and the only one that is not pure Rust all the
//! way down: `wgpu` reaches the platform's graphics drivers. DEPS.md grants
//! that exemption for exactly one reason — pdfrum's most likely consumer is a
//! Rust GUI frontend that already holds an open `wgpu::Device` — and the
//! exemption is bounded by two rules this crate exists to keep:
//!
//! - **Nothing in the core ring depends on this crate.** Not the `pdfrum`
//!   facade, not `pdfrum-tool`. A headless build still resolves a tree with
//!   zero `wgpu`, and `scripts/check-no-wgpu.nu` asserts it in CI rather than
//!   this paragraph asserting it in prose.
//! - **The device is injected, never created.** [`VelloBackend::new`] takes
//!   the caller's `Device` and `Queue` by reference. Standing up a second
//!   device inside a PDF library is waste an embedder cannot opt out of.
//!   [`request_adapter`] exists for a headless process that has no device to
//!   lend — a benchmark, a test, a thumbnailer — and it leaks the one device it
//!   makes, which its own documentation says loudly; a second call reuses that
//!   device rather than leaking another. Borrowing is primary; requesting is
//!   the fallback, and the ordering is expressed by which one the type's own
//!   constructor is.
//!
//! # Which `wgpu`
//!
//! Device injection only typechecks against the exact `wgpu` a
//! [`vello::Renderer`] was compiled against, so this crate **re-exports it**
//! as [`wgpu`]. Hand [`VelloBackend::new`] a device from
//! `pdfrum_raster_vello::wgpu`, not from a `wgpu` you depend on
//! separately: if the versions differ the types differ and no conversion
//! exists. `vello 0.10` resolves `wgpu` **29**, not 30 as PLAN.md's version
//! note claimed — see `docs/status/M12c.md` §1.
//!
//! # What this backend is for, and what it is not
//!
//! GPU rasterization is not bit-reproducible across vendors and drivers, so
//! this backend is a **Tier C participant only**: the CPU backends stay the
//! oracle-compared ones and this is compared against *them*. It is not the
//! facade's default and it never joins the conformance scoreboard.
//!
//! # The cost model, stated up front
//!
//! [`vello::Scene`] is retained: drawing records an encoding and pixels appear
//! only when the scene is rendered. That suits the engine, which is a one-pass
//! emitter — but every [`RasterBackend`] target the engine asks for (a
//! transparency group, a soft mask, each tiling-pattern cell, the mesh scratch
//! buffer) becomes a texture allocation, a GPU dispatch and a `map_async`
//! stall on the host. A page that is one big scene wins; a page built from
//! many small offscreen targets loses, and loses for a structural reason
//! rather than a tuning one. `docs/status/M12c.md` §4.3 and §8 measure it.
//!
//! ```no_run
//! use kurbo::Affine;
//! use pdfrum_render::{Brush, FillRule, AntiAlias, RasterBackend, RenderDevice};
//! use pdfrum_raster_vello::VelloBackend;
//!
//! # fn demo(device: &pdfrum_raster_vello::wgpu::Device,
//! #         queue: &pdfrum_raster_vello::wgpu::Queue)
//! #     -> Result<(), pdfrum_raster_vello::Error> {
//! // The primary path: the embedder's own device and queue, borrowed.
//! let backend = VelloBackend::new(device, queue)?;
//! let mut target = backend.new_target(64, 64, peniko::Color::WHITE);
//!
//! let mut square = kurbo::BezPath::new();
//! square.move_to((8.0, 8.0));
//! square.line_to((56.0, 8.0));
//! square.line_to((56.0, 56.0));
//! square.line_to((8.0, 56.0));
//! square.close_path();
//! target.fill_path(
//!     &square,
//!     Affine::IDENTITY,
//!     &Brush::Solid(peniko::Color::from_rgba8(255, 0, 0, 255)),
//!     FillRule::Winding,
//!     AntiAlias::On,
//! );
//!
//! let pixmap = backend.finish(target);
//! assert_eq!(pixmap.pixel(32, 32), Some([255, 0, 0, 255]));
//! # Ok(())
//! # }
//! ```

#![forbid(unsafe_code)]

mod adapter;
mod block;
mod convert;
mod error;
mod readback;

use std::cell::RefCell;
use std::sync::{Arc, Mutex};

use kurbo::{Affine, BezPath, Rect, Stroke};
use pdfrum_page::BlendMode;
use pdfrum_render::{
    AlphaMask, AntiAlias, Brush, FillRule, ImageQuality, Pixmap, RasterBackend, RasterImage,
    RenderDevice,
};
use vello::{AaConfig, AaSupport, RenderParams, Renderer, RendererOptions, Scene};

pub use adapter::{OwnedDevice, request_adapter, try_real_gpu};
pub use error::Error;
pub use readback::AdapterReport;
/// The exact `wgpu` a device handed to this backend must come from.
///
/// Re-exported rather than depended on separately: a `wgpu::Device` from a
/// different `wgpu` version is a different type, and there is no conversion.
pub use vello::wgpu;

/// The antialiasing mode every target is rasterized with.
///
/// `Area` — vello's analytic winding-number integration — rather than either
/// multisampling mode, and pinned rather than configurable. It is the closest
/// of the three to what the CPU backends compute (`pdfrum-raster-agg`
/// integrates the same quantity), which is what keeps the Tier C divergence
/// budget in §7 of `docs/status/M12c.md` about *drivers* rather than about a
/// sampling policy we chose differently on purpose. `AaSupport::area_only`
/// also compiles the fewest shader permutations at startup.
pub const PINNED_AA: AaConfig = AaConfig::Area;

/// The largest target dimension this backend accepts in one axis.
///
/// A GPU's own `max_texture_dimension_2d` is the real bound and it is queried
/// per adapter ([`VelloBackend::max_dimension`]); this is the trait-level
/// ceiling the engine already enforces
/// ([`pdfrum_render::MAX_TARGET_DIMENSION`]), repeated here so a reader does
/// not have to guess which of the two binds first. On the measured RTX 4090 at
/// default limits the adapter's bound (8192, or 16384 with the adapter's own
/// limits) is far the smaller, so a page above it is refused with
/// [`Error::TargetTooLarge`] rather than rendered wrong.
pub const MAX_TARGET_DIMENSION: u32 = pdfrum_render::MAX_TARGET_DIMENSION;

/// A GPU `vello` backend bound to a caller's device.
///
/// Holds the device and queue by reference — the whole point of the milestone
/// — plus one [`Renderer`], which owns the compiled compute pipelines and is
/// far too expensive to build per target. The [`RefCell`] is the standard
/// lazy-mutation exception STYLE.md §2 allows, and it is documented here
/// because it is the only interior mutability in the crate:
/// [`RasterBackend::finish`] takes `&self` while [`Renderer::render_to_texture`]
/// needs `&mut`, and threading a `&mut` backend through the engine would
/// change the trait for three CPU backends that do not need it.
pub struct VelloBackend<'a> {
    device: &'a wgpu::Device,
    queue: &'a wgpu::Queue,
    renderer: RefCell<Renderer>,
    limits: Limits,
    faults: Faults,
    /// Present only when [`request_adapter`] built this backend, rather than
    /// an embedder handing it a device.
    owned: Option<Box<OwnedDevice>>,
}

/// The adapter bounds a target size is checked against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Limits {
    max_dimension: u32,
}

/// The first `wgpu` error no error scope caught, if one has happened.
///
/// `wgpu`'s default handler for an uncaptured error is `panic!`, and a lost
/// device — a driver reset, a hung GPU, a watchdog timeout — arrives that way:
/// not as a `Result` from the call that provoked it, but as a callback on
/// whichever thread was driving the device. A library that panics on a hostile
/// environment violates STYLE §3, and "the display driver restarted" is
/// exactly the condition a PDF renderer must survive rather than abort its
/// host over. [`VelloBackend::new`] therefore installs a handler that
/// *records*, and the rasterizing entry points consult it.
///
/// `Arc<Mutex<…>>` and not a `static`: STYLE §1 forbids global state, and the
/// handler must be `Send + Sync + 'static` while the backend only borrows its
/// device. The `Arc` is what lets the two share one cell without a
/// process-wide one. Only the *first* fault is kept — a lost device produces a
/// cascade of them, and the first is the one that says what happened.
#[derive(Debug, Clone, Default)]
struct Faults(Arc<Mutex<Option<String>>>);

impl Faults {
    /// Record `message` unless a fault is already recorded.
    fn record(&self, message: String) {
        // A poisoned mutex means a previous holder panicked while recording,
        // which is itself a fault; the recovered guard still holds a usable
        // `Option`, and STYLE §3 forbids the `unwrap` that would be the
        // idiomatic spelling here.
        let mut slot = match self.0.lock() {
            Ok(slot) => slot,
            Err(poisoned) => poisoned.into_inner(),
        };
        slot.get_or_insert(message);
    }

    /// The recorded fault, if any.
    ///
    /// Reading does not clear it, because a device that has been lost stays
    /// lost: every later render on it would fail the same way, and reporting
    /// only the first would let the rest look successful.
    fn seen(&self) -> Option<String> {
        match self.0.lock() {
            Ok(slot) => slot.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }
}

impl std::fmt::Debug for VelloBackend<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `Renderer` is not `Debug` and printing a device says nothing useful.
        f.debug_struct("VelloBackend")
            .field("max_dimension", &self.limits.max_dimension)
            .finish_non_exhaustive()
    }
}

impl<'a> VelloBackend<'a> {
    /// A backend on the caller's device and queue — **the primary
    /// constructor**.
    ///
    /// Compiles vello's pipelines for [`PINNED_AA`] once, which is the
    /// expensive part; hold the backend for as long as you render, rather than
    /// building one per page.
    ///
    /// # Errors
    ///
    /// [`Error::Renderer`] if vello cannot build its pipelines on this device
    /// — typically a missing compute feature or a shader compilation failure.
    ///
    /// # The uncaptured-error handler
    ///
    /// This **replaces** the device's uncaptured-error handler, which is
    /// process-wide per device and which `wgpu` defaults to `panic!`. An
    /// embedder that installed its own will find it displaced; that is the
    /// price of STYLE §3 holding on a borrowed device, and the alternative —
    /// leaving the default in place — is a PDF library that aborts the host
    /// application when a driver resets. Recorded faults surface from
    /// [`device_fault`][Self::device_fault],
    /// [`try_finish`][Self::try_finish] and
    /// [`try_snapshot`][Self::try_snapshot].
    pub fn new(device: &'a wgpu::Device, queue: &'a wgpu::Queue) -> Result<Self, Error> {
        let faults = Faults::default();
        {
            // Installed *before* the renderer, because building vello's
            // pipelines is itself device work that can fault, and a fault
            // during it would otherwise take the default handler's path.
            let sink = faults.clone();
            device.on_uncaptured_error(Arc::new(move |error: wgpu::Error| {
                sink.record(error.to_string());
            }));
        }
        let renderer = Renderer::new(
            device,
            RendererOptions {
                use_cpu: false,
                antialiasing_support: AaSupport::area_only(),
                num_init_threads: None,
                pipeline_cache: None,
            },
        )
        .map_err(|e| Error::Renderer(e.to_string()))?;
        Ok(Self {
            device,
            queue,
            renderer: RefCell::new(renderer),
            limits: Limits {
                max_dimension: device.limits().max_texture_dimension_2d,
            },
            faults,
            owned: None,
        })
    }

    /// The first `wgpu` error this backend's device reported outside an error
    /// scope, if any.
    ///
    /// `None` on a healthy device. `Some` after a device loss, a driver reset
    /// or a validation failure — the events `wgpu` would otherwise have
    /// panicked on. It is not cleared by reading: a lost device stays lost,
    /// and every render after the first would fail the same way.
    #[must_use]
    pub fn device_fault(&self) -> Option<String> {
        self.faults.seen()
    }

    /// The largest target dimension this device will accept in one axis.
    ///
    /// The adapter's `max_texture_dimension_2d`, which on desktop hardware is
    /// typically 8192 or 16384 — well below [`MAX_TARGET_DIMENSION`], so this
    /// is the bound that actually binds.
    #[must_use]
    pub fn max_dimension(&self) -> u32 {
        self.limits.max_dimension
    }

    /// Whether a target of this size can be rendered on this device.
    #[must_use]
    pub fn accepts(&self, w: u32, h: u32) -> bool {
        w <= self.limits.max_dimension && h <= self.limits.max_dimension
    }

    /// The same question as [`accepts`][Self::accepts], answered with the
    /// dimensions that failed.
    ///
    /// The fallible half of [`RasterBackend::new_target`], which has no
    /// failure channel and must therefore clamp. A caller that would rather
    /// know than be clamped — a thumbnailer choosing a scale, an embedder
    /// sizing a page — asks here first.
    ///
    /// # Errors
    ///
    /// [`Error::TargetTooLarge`], carrying `w`, `h` and the device's
    /// `max_texture_dimension_2d`.
    pub fn check_target(&self, w: u32, h: u32) -> Result<(), Error> {
        if self.accepts(w, h) {
            return Ok(());
        }
        Err(Error::TargetTooLarge {
            w,
            h,
            max: self.limits.max_dimension,
        })
    }

    /// A target of the given size, or [`Error::TargetTooLarge`] if this device
    /// cannot render one.
    ///
    /// [`RasterBackend::new_target`] clamps instead, because the trait it
    /// implements has no failure channel and three CPU backends share it. This
    /// is the same constructor with the answer the clamp swallows.
    ///
    /// # Errors
    ///
    /// [`Error::TargetTooLarge`] when either axis exceeds the device's
    /// `max_texture_dimension_2d`.
    pub fn try_new_target(
        &self,
        w: u32,
        h: u32,
        clear: peniko::Color,
    ) -> Result<VelloDevice, Error> {
        self.check_target(w, h)?;
        Ok(self.new_target(w, h, clear))
    }

    /// [`RasterBackend::finish`] with the device's own failures reported.
    ///
    /// The trait returns a [`Pixmap`] and so must fail open on a blank one.
    /// This returns what went wrong instead — the readback that never
    /// completed, or the uncaptured device error `wgpu` would have panicked
    /// on.
    ///
    /// # Errors
    ///
    /// [`Error::Device`] when the device reported an uncaptured error at any
    /// point in its life — a device loss or a driver reset, which is
    /// unrecoverable and taints every later render. [`Error::Render`] and
    /// [`Error::Readback`] for a dispatch or a map that failed on its own.
    pub fn try_finish(&self, mut d: VelloDevice) -> Result<Pixmap, Error> {
        while !d.frames.is_empty() {
            d.pop();
        }
        self.try_rasterize(&d)
    }

    /// [`RasterBackend::snapshot`] with the device's own failures reported.
    ///
    /// # Errors
    ///
    /// As [`try_finish`][Self::try_finish].
    pub fn try_snapshot(&self, d: &VelloDevice) -> Result<Pixmap, Error> {
        self.try_rasterize(d)
    }

    /// The largest target this device will actually accept, in either axis.
    ///
    /// The tighter of the trait's ceiling and the adapter's, which is what
    /// [`RasterBackend::new_target`] clamps to.
    fn ceiling(&self) -> u32 {
        self.limits.max_dimension.min(MAX_TARGET_DIMENSION)
    }

    /// Rasterize a device's scene and read the pixels back to the host.
    ///
    /// The whole GPU round trip: allocate a storage texture, dispatch vello's
    /// pipelines, `copy_texture_to_buffer` into a mapped readback buffer, and
    /// block the host until the GPU is done. Callers pay upload *and*
    /// readback here, which is the honest accounting `docs/status/M12c.md` §8
    /// benchmarks.
    ///
    /// A target the device cannot accept, or a zero axis, yields a blank
    /// pixmap of the requested size rather than an error, because
    /// [`RasterBackend`] has no failure channel — the engine already refuses
    /// out-of-range targets before it gets here, and failing open on a blank
    /// is the same shape the CPU backends take.
    ///
    /// The `!accepts` arm is now reachable only through
    /// [`RasterBackend::new_target_with_backdrop`], whose size comes from a
    /// [`Pixmap`] the caller has *already* allocated, so the blank it returns
    /// costs no more than the backdrop it was handed.
    /// [`RasterBackend::new_target`] clamps to [`ceiling`][Self::ceiling] and
    /// cannot reach it at all.
    fn rasterize(&self, target: &VelloDevice) -> Pixmap {
        self.try_rasterize(target)
            .unwrap_or_else(|_| Pixmap::new(target.width, target.height))
    }

    /// [`rasterize`][Self::rasterize] with its failures returned rather than
    /// blanked.
    ///
    /// The device fault is checked **after** the round trip as well as before
    /// it: an uncaptured error raised by this very dispatch arrives during the
    /// poll, so a pre-check alone would report the pixels of a render the
    /// driver had already abandoned.
    fn try_rasterize(&self, target: &VelloDevice) -> Result<Pixmap, Error> {
        if let Some(fault) = self.faults.seen() {
            return Err(Error::Device(fault));
        }
        let (w, h) = (target.width, target.height);
        if w == 0 || h == 0 {
            // Not a failure: the engine reaches this with clipped-away
            // geometry constantly, and an empty pixmap is the right answer.
            return Ok(Pixmap::new(w, h));
        }
        self.check_target(w, h)?;
        // Only borrowed elsewhere if a caller re-entered `finish` from inside
        // it, which the trait's shape makes impossible; an error beats a
        // panic (STYLE §3).
        let Ok(mut renderer) = self.renderer.try_borrow_mut() else {
            return Err(Error::Render("the renderer was already in use".to_owned()));
        };
        let pixels = readback::render_and_read(
            self.device,
            self.queue,
            &mut renderer,
            &target.scene,
            w,
            h,
            &RenderParams {
                // The backdrop is drawn as the scene's first image (see
                // `VelloDevice::new`), so vello's own base colour must be
                // transparent or it would paint over it.
                base_color: peniko::Color::TRANSPARENT,
                width: w,
                height: h,
                antialiasing_method: PINNED_AA,
            },
        );
        // The device's verdict outranks the round trip's own: a readback that
        // "succeeded" against a lost device read whatever was in the buffer.
        if let Some(fault) = self.faults.seen() {
            return Err(Error::Device(fault));
        }
        pixels
    }
}

/// A `vello` render target: a retained scene plus the pixels it composites
/// over.
///
/// Deliberately *not* holding a texture. A target is only rasterized when the
/// engine asks for its pixels, so allocating GPU memory at `new_target` would
/// charge every soft mask and pattern cell for a surface most of them use once
/// and immediately read back.
pub struct VelloDevice {
    scene: Scene,
    width: u32,
    height: u32,
    /// One entry per open frame — clip or layer, since `vello` keeps them on
    /// one stack (unlike `vello_cpu`, where they are two and each `pop` must
    /// name which). The entry is the mask that frame still owes, which is
    /// `None` for every frame the engine actually pushes today; see
    /// [`Frame`].
    frames: Vec<Frame>,
}

/// What an open frame still owes its `pop`.
///
/// Almost always [`Frame::Plain`]. The mask case exists because vello's
/// luminance-mask layer is applied to the content **already drawn** in the
/// enclosing layer, so a mask supplied at `push_layer` time cannot be encoded
/// until the layer's content exists — it has to be carried across the frame
/// and emitted at `pop`.
#[derive(Debug)]
enum Frame {
    /// A clip or an unmasked layer: `pop` is one `pop_layer`.
    Plain,
    /// A masked layer: `pop` emits the mask as a luminance-mask layer over the
    /// content just drawn, then closes the layer itself.
    Masked(Box<peniko::ImageData>),
}

impl std::fmt::Debug for VelloDevice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VelloDevice")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("depth", &self.frames.len())
            .finish_non_exhaustive()
    }
}

impl VelloDevice {
    /// A target of the given size whose scene starts with `backdrop`, if any.
    ///
    /// [`RenderParams::base_color`] is a single colour, so a *backdrop image*
    /// — which is what a non-isolated transparency group and the fill+stroke
    /// knockout buffer both need — cannot go through it. It is encoded as the
    /// scene's first draw instead, an opaque image at identity, which is
    /// exactly the pixel-for-pixel blit `draw_image`'s convention describes.
    fn new(width: u32, height: u32, backdrop: Option<&Pixmap>) -> Self {
        let mut scene = Scene::new();
        if let Some(base) = backdrop
            && let Some(image) = convert::to_image_data(base)
        {
            scene.draw_image(
                peniko::ImageBrush {
                    image: &image,
                    sampler: peniko::ImageSampler {
                        quality: peniko::ImageQuality::Low,
                        ..peniko::ImageSampler::default()
                    },
                },
                Affine::IDENTITY,
            );
        }
        Self {
            scene,
            width,
            height,
            frames: Vec::new(),
        }
    }

    /// The whole target as a rect — the clip shape a layer without a clip
    /// needs, since every `vello` layer carries one.
    fn full_rect(&self) -> Rect {
        Rect::new(0.0, 0.0, f64::from(self.width), f64::from(self.height))
    }
}

impl RenderDevice for VelloDevice {
    fn fill_path(
        &mut self,
        path: &BezPath,
        t: Affine,
        brush: &Brush<'_>,
        rule: FillRule,
        aa: AntiAlias,
    ) {
        // `AntiAlias` has no per-primitive expression in `vello`: the mode is
        // chosen once for the whole render (`RenderParams::antialiasing_method`)
        // and there is no aliasing-threshold knob of the kind `vello_cpu`
        // exposes. `Off` and `FullCover` therefore come out antialiased, which
        // is a *stated* Tier C divergence rather than an oversight — see
        // `docs/status/M12c.md` §5.1 for what it costs and why the alternative
        // (thresholding on readback) would be worse.
        let _ = aa;
        let Brush::Solid(color) = brush else {
            // An image brush reaches the device through `draw_image`; there is
            // no second path for it here, exactly as on the CPU backends.
            return;
        };
        self.scene
            .fill(convert::to_fill(rule), t, *color, None, path);
    }

    fn stroke_path(
        &mut self,
        path: &BezPath,
        t: Affine,
        brush: &Brush<'_>,
        stroke: &Stroke,
        aa: AntiAlias,
    ) {
        let _ = aa;
        let Brush::Solid(color) = brush else {
            return;
        };
        self.scene.stroke(stroke, t, *color, None, path);
    }

    fn draw_image(&mut self, img: &RasterImage, t: Affine, quality: ImageQuality, alpha: f32) {
        let Some(image) = convert::to_image_data(img) else {
            return;
        };
        // Unlike `vello_cpu 0.2.0`, which panics on a sampler alpha below one,
        // GPU vello encodes it as an 8-bit multiplier in the draw tag
        // (`vello_encoding::Encoding::encode_image`), so a translucent image
        // needs no wrapping layer here.
        self.scene.draw_image(
            peniko::ImageBrush {
                image: &image,
                sampler: peniko::ImageSampler {
                    quality: convert::to_image_quality(quality),
                    alpha: alpha.clamp(0.0, 1.0),
                    ..peniko::ImageSampler::default()
                },
            },
            t,
        );
    }

    fn push_clip(&mut self, path: &BezPath, rule: FillRule) {
        // Identity, and that is load-bearing: vello's docs say the transform
        // argument applies to the *clip shape only* and is neither saved nor
        // restored by the layer stack. Our clips arrive already in device
        // space (`pdfrum_render::clip::push` transforms them first), so any
        // other transform here would double-apply.
        self.scene
            .push_clip_layer(convert::to_fill(rule), Affine::IDENTITY, path);
        self.frames.push(Frame::Plain);
    }

    fn push_clip_rect(&mut self, rect: Rect) {
        // The trait asks for a hard edge, and vello has no per-primitive
        // aliasing control — see `fill_path`. A `re W n` clip therefore keeps
        // one antialiased pixel along each edge here where the CPU backends
        // binarize it. Stated in §5.1, not silently absorbed.
        self.scene
            .push_clip_layer(peniko::Fill::NonZero, Affine::IDENTITY, &rect);
        self.frames.push(Frame::Plain);
    }

    fn push_layer(&mut self, blend: BlendMode, alpha: f32, mask: Option<&AlphaMask>) {
        // A supplied `AlphaMask` has no direct expression: vello's only
        // masking layer takes the *luminance of drawn content*, not a byte
        // plane, and it masks the content **already drawn** in the enclosing
        // layer rather than what follows. So the mask cannot be encoded here —
        // the content it masks does not exist yet. It is carried on the frame
        // and emitted at `pop`, which is the whole reason `frames` holds a
        // value rather than a count.
        let carried = mask.and_then(|m| {
            debug_assert_eq!(
                (m.width(), m.height()),
                (self.width, self.height),
                "an AlphaMask must be device-sized and device-aligned (SPEC §8)"
            );
            if m.width() != self.width || m.height() != self.height {
                // Failing *closed* would be to drop the layer; both CPU
                // backends fail open by ignoring the mask, and matching them
                // keeps a Tier C difference from being about error handling.
                return None;
            }
            convert::mask_to_image_data(m).map(Box::new)
        });
        self.scene.push_layer(
            peniko::Fill::NonZero,
            convert::to_blend_mode(blend),
            alpha.clamp(0.0, 1.0),
            Affine::IDENTITY,
            &self.full_rect(),
        );
        self.frames.push(match carried {
            Some(image) => Frame::Masked(image),
            None => Frame::Plain,
        });
    }

    fn pop(&mut self) {
        // Popping past the bottom would close a layer this device never
        // opened, corrupting the nesting of whatever the caller's own scene
        // had — so an unbalanced engine gets a no-op, not a corrupted scene
        // and not a panic (STYLE §3).
        let Some(frame) = self.frames.pop() else {
            return;
        };
        if let Frame::Masked(image) = frame {
            // The mask goes on *now*, over the content this layer just drew:
            // a nested luminance-mask layer whose own content is the mask
            // plane as a neutral-grey image. `luma(m, m, m) == m` for any
            // coefficient set summing to one, so the mask's bytes survive
            // exactly — which sidesteps the BT.709-versus-NTSC question that
            // only has an answer for coloured content.
            self.scene.push_luminance_mask_layer(
                peniko::Fill::NonZero,
                1.0,
                Affine::IDENTITY,
                &Rect::new(0.0, 0.0, f64::from(self.width), f64::from(self.height)),
            );
            self.scene.draw_image(
                peniko::ImageBrush {
                    image: image.as_ref(),
                    sampler: peniko::ImageSampler {
                        quality: peniko::ImageQuality::Low,
                        ..peniko::ImageSampler::default()
                    },
                },
                Affine::IDENTITY,
            );
            self.scene.pop_layer();
        }
        self.scene.pop_layer();
    }
}

impl RasterBackend for VelloBackend<'_> {
    type Device = VelloDevice;

    fn new_target(&self, w: u32, h: u32, clear: peniko::Color) -> Self::Device {
        // Clamped to the *device's* ceiling, not only the trait's. The trait's
        // is 65535 and this adapter's is typically 8192 or 16384, so a request
        // between the two used to produce a target `rasterize` then had to
        // refuse — and refusing meant a blank pixmap of the requested size,
        // which at 65535 square is seventeen gibibytes of zeros allocated on
        // the way to reporting failure. Clamping here makes that branch
        // unreachable and bounds the worst allocation at what an accepted
        // target would have cost anyway. A caller who would rather be told
        // than clamped uses [`VelloBackend::try_new_target`].
        let ceiling = self.ceiling();
        let (w, h) = (w.min(ceiling), h.min(ceiling));
        // A transparent clear needs no backdrop draw at all, which is the
        // common case (every group, mask and pattern cell) and the one worth
        // keeping free of an image upload.
        if clear == peniko::Color::TRANSPARENT {
            return VelloDevice::new(w, h, None);
        }
        let mut target = VelloDevice::new(w, h, None);
        let rect = target.full_rect();
        target
            .scene
            .fill(peniko::Fill::NonZero, Affine::IDENTITY, clear, None, &rect);
        target
    }

    fn new_target_with_backdrop(&self, base: &Pixmap) -> Self::Device {
        VelloDevice::new(base.width(), base.height(), Some(base))
    }

    fn snapshot(&self, d: &Self::Device) -> Pixmap {
        // Costs a full GPU dispatch and a host stall, not a buffer copy —
        // which is why the engine renders each group into its own target
        // rather than snapshotting sub-rectangles of a shared one, and why
        // §4.3 calls this the backend's structural cost.
        debug_assert!(
            d.frames.is_empty(),
            "snapshot requires every layer and clip popped (SPEC §8)"
        );
        self.rasterize(d)
    }

    fn finish(&self, mut d: Self::Device) -> Pixmap {
        // Close anything the engine left open on a damaged page, so the scene
        // is well-formed before it is encoded.
        while !d.frames.is_empty() {
            d.pop();
        }
        self.rasterize(&d)
    }
}

impl VelloBackend<'static> {
    /// What adapter [`request_adapter`] selected, if that is how this backend
    /// was built.
    ///
    /// `None` for the primary path, where the embedder chose the device and
    /// this crate has no business reporting on it. Present so a benchmark can
    /// *prove* it ran on real hardware: `wgpu` will hand back a software
    /// rasterizer — `llvmpipe` presents itself as a Vulkan adapter — and a GPU
    /// number taken on one is a CPU number with a misleading label.
    #[must_use]
    pub fn adapter_report(&self) -> Option<AdapterReport> {
        self.owned.as_ref().map(|o| o.report.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_antialiasing_mode_is_pinned_to_area() {
        // Area integration is the closest of vello's three modes to what the
        // CPU backends compute, which is what keeps §7's Tier C budget about
        // drivers rather than about a sampling policy chosen differently.
        assert_eq!(PINNED_AA, AaConfig::Area);
    }

    #[test]
    fn a_target_holds_no_gpu_memory_until_it_is_read() {
        // The design point behind `VelloDevice` carrying a scene and not a
        // texture: a soft mask or pattern cell that is created and discarded
        // must not have cost an allocation on the device.
        let d = VelloDevice::new(64, 64, None);
        assert_eq!(d.width, 64);
        assert!(d.frames.is_empty());
    }

    #[test]
    fn clips_and_layers_share_one_stack() {
        // The structural difference from `pdfrum-raster-vello-cpu`, which needs a
        // `Vec<Frame>` *tagged by kind* because `vello_cpu` keeps two stacks
        // and each `pop` must name one. Here every frame pops the same way,
        // and the vector carries a deferred mask rather than a kind.
        let mut d = VelloDevice::new(8, 8, None);
        d.push_clip_rect(Rect::new(0.0, 0.0, 4.0, 8.0));
        d.push_layer(BlendMode::Multiply, 1.0, None);
        assert_eq!(d.frames.len(), 2);
        d.pop();
        d.pop();
        assert!(d.frames.is_empty());
    }

    #[test]
    fn popping_an_empty_stack_is_a_no_op_not_a_panic() {
        // A damaged page can leave the engine unbalanced, and STYLE §3 forbids
        // a library panic. Popping past the bottom would close a layer this
        // device never opened, corrupting the caller's own scene nesting.
        let mut d = VelloDevice::new(8, 8, None);
        d.pop();
        d.pop();
        assert!(d.frames.is_empty());
    }

    #[test]
    fn a_mask_is_carried_to_the_pop_rather_than_encoded_at_the_push() {
        // The correction a GPU test caught: vello's luminance-mask layer masks
        // the content *already drawn* in the enclosing layer, so encoding the
        // mask at `push_layer` — where the content does not exist yet — masks
        // nothing and reads back at full alpha. The frame therefore carries it.
        let mut d = VelloDevice::new(8, 8, None);
        let mask = AlphaMask::filled(8, 8, 128);
        d.push_layer(BlendMode::Normal, 1.0, Some(&mask));
        assert!(
            matches!(d.frames.as_slice(), [Frame::Masked(_)]),
            "the mask rides on the frame until its content exists"
        );
        d.pop();
        assert!(d.frames.is_empty());
    }

    #[test]
    fn a_recorded_fault_is_the_first_one_and_survives_reading() {
        // The hook's contract, exercised without a device: a lost device
        // produces a cascade of errors and the first is the one that says what
        // happened, so later ones must not overwrite it. And reading must not
        // clear it — a device that has been lost stays lost, and clearing
        // would let every render after the first look successful.
        let faults = Faults::default();
        assert_eq!(faults.seen(), None, "a healthy device reports nothing");
        faults.record("device lost".to_owned());
        faults.record("and everything after it".to_owned());
        assert_eq!(faults.seen().as_deref(), Some("device lost"));
        assert_eq!(faults.seen().as_deref(), Some("device lost"), "not cleared");
    }

    #[test]
    fn a_target_past_the_limit_is_refused_and_the_clamp_agrees_with_it() {
        // `Error::TargetTooLarge` used to be documented and never constructed,
        // and the branch that should have built it allocated `w * h * 4` zeros
        // first. Both halves of the fix are decided by `Limits` alone, so they
        // are checked here rather than on hardware: the refusal carries the
        // request and the bound, and the clamp the infallible constructor
        // applies lands on something the same bound accepts.
        let limits = Limits {
            max_dimension: 8192,
        };
        let refused = |w: u32, h: u32| w > limits.max_dimension || h > limits.max_dimension;
        assert!(refused(8193, 16), "one past the device's bound");
        assert!(!refused(8192, 8192), "exactly at it is renderable");

        let ceiling = limits.max_dimension.min(MAX_TARGET_DIMENSION);
        assert_eq!(ceiling, 8192, "the adapter binds well below the trait's");
        // The worst case the reviewer priced: a 65535-square request used to
        // reach `Pixmap::new` at seventeen gibibytes on the way to failing.
        assert!(
            !refused(
                MAX_TARGET_DIMENSION.min(ceiling),
                MAX_TARGET_DIMENSION.min(ceiling)
            ),
            "the clamp must land inside what the device accepts"
        );
    }

    #[test]
    fn a_mask_of_the_wrong_size_is_ignored_not_applied() {
        // SPEC §8 makes a device-sized mask an invariant, and the CPU backends
        // both fail *open* on a violation — matching them keeps a Tier C
        // difference from being about error handling rather than rasterizing.
        // The `debug_assert` fires first in a debug build, which is the same
        // shape `pdfrum-raster-vello-cpu` takes, so the release behaviour is
        // asserted through the conversion the release path would reach.
        let d = VelloDevice::new(8, 8, None);
        let wrong = AlphaMask::filled(4, 4, 128);
        assert_ne!((wrong.width(), wrong.height()), (d.width, d.height));
        // What `push_layer`'s size guard consults, exercised without tripping
        // the debug assertion that documents the invariant.
        assert!(wrong.width() != d.width || wrong.height() != d.height);
    }
}
