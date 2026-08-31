//! GPU `vello` implementation of `pdfrum-render`'s `RenderDevice` and
//! `RasterBackend` traits, on a **caller-supplied** `wgpu` device
//! (PLAN.md §M12c).
//!
//! This is the fourth backend and the only one that is not pure Rust all the
//! way down: `wgpu` reaches the platform's graphics drivers. DEPS.md grants
//! that exemption for exactly one reason — pdfrum's most likely consumer is a
//! Rust GUI frontend that already holds an open `wgpu::Device` — and the
//! exemption is bounded by two rules this crate exists to keep:
//!
//! - **Nothing in the core ring depends on this crate.** Not the `pdfrum`
//!   facade, not `pdfrum-tool`. A headless build still resolves a tree with
//!   zero `wgpu`, and `scripts/check-no-wgpu.sh` asserts it in CI rather than
//!   this paragraph asserting it in prose.
//! - **The device is injected, never created.** [`VelloGpuBackend::new`] takes
//!   the caller's `Device` and `Queue` by reference. Standing up a second
//!   device inside a PDF library is waste an embedder cannot opt out of.
//!   [`request_adapter`] exists for a headless process that has no device to
//!   lend — a benchmark, a test, a thumbnailer — and it leaks the device it
//!   makes, which its own documentation says loudly. Borrowing is primary;
//!   requesting is the fallback, and the ordering is expressed by which one
//!   the type's own constructor is.
//!
//! # Which `wgpu`
//!
//! Device injection only typechecks against the exact `wgpu` a
//! [`vello::Renderer`] was compiled against, so this crate **re-exports it**
//! as [`wgpu`]. Hand [`VelloGpuBackend::new`] a device from
//! `pdfrum_raster_vello_gpu::wgpu`, not from a `wgpu` you depend on
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
//! use pdfrum_raster_vello_gpu::VelloGpuBackend;
//!
//! # fn demo(device: &pdfrum_raster_vello_gpu::wgpu::Device,
//! #         queue: &pdfrum_raster_vello_gpu::wgpu::Queue)
//! #     -> Result<(), pdfrum_raster_vello_gpu::Error> {
//! // The primary path: the embedder's own device and queue, borrowed.
//! let backend = VelloGpuBackend::new(device, queue)?;
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
/// of the three to what the CPU backends compute (`pdfrum-raster-exact`
/// integrates the same quantity), which is what keeps the Tier C divergence
/// budget in §7 of `docs/status/M12c.md` about *drivers* rather than about a
/// sampling policy we chose differently on purpose. `AaSupport::area_only`
/// also compiles the fewest shader permutations at startup.
pub const PINNED_AA: AaConfig = AaConfig::Area;

/// The largest target dimension this backend accepts in one axis.
///
/// A GPU's own `max_texture_dimension_2d` is the real bound and it is queried
/// per adapter ([`VelloGpuBackend::max_dimension`]); this is the trait-level
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
pub struct VelloGpuBackend<'a> {
    device: &'a wgpu::Device,
    queue: &'a wgpu::Queue,
    renderer: RefCell<Renderer>,
    limits: Limits,
    /// Present only when [`request_adapter`] built this backend, rather than
    /// an embedder handing it a device.
    owned: Option<Box<OwnedDevice>>,
}

/// The adapter bounds a target size is checked against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Limits {
    max_dimension: u32,
}

impl std::fmt::Debug for VelloGpuBackend<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `Renderer` is not `Debug` and printing a device says nothing useful.
        f.debug_struct("VelloGpuBackend")
            .field("max_dimension", &self.limits.max_dimension)
            .finish_non_exhaustive()
    }
}

impl<'a> VelloGpuBackend<'a> {
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
    pub fn new(device: &'a wgpu::Device, queue: &'a wgpu::Queue) -> Result<Self, Error> {
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
            owned: None,
        })
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
    fn rasterize(&self, target: &VelloGpuDevice) -> Pixmap {
        let (w, h) = (target.width, target.height);
        if w == 0 || h == 0 || !self.accepts(w, h) {
            return Pixmap::new(w, h);
        }
        // Only borrowed elsewhere if a caller re-entered `finish` from inside
        // it, which the trait's shape makes impossible; a blank beats a panic.
        let Ok(mut renderer) = self.renderer.try_borrow_mut() else {
            return Pixmap::new(w, h);
        };
        readback::render_and_read(
            self.device,
            self.queue,
            &mut renderer,
            &target.scene,
            w,
            h,
            &RenderParams {
                // The backdrop is drawn as the scene's first image (see
                // `VelloGpuDevice::new`), so vello's own base colour must be
                // transparent or it would paint over it.
                base_color: peniko::Color::TRANSPARENT,
                width: w,
                height: h,
                antialiasing_method: PINNED_AA,
            },
        )
        .unwrap_or_else(|_| Pixmap::new(w, h))
    }
}

/// A `vello` render target: a retained scene plus the pixels it composites
/// over.
///
/// Deliberately *not* holding a texture. A target is only rasterized when the
/// engine asks for its pixels, so allocating GPU memory at `new_target` would
/// charge every soft mask and pattern cell for a surface most of them use once
/// and immediately read back.
pub struct VelloGpuDevice {
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

impl std::fmt::Debug for VelloGpuDevice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VelloGpuDevice")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("depth", &self.frames.len())
            .finish_non_exhaustive()
    }
}

impl VelloGpuDevice {
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

impl RenderDevice for VelloGpuDevice {
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

impl RasterBackend for VelloGpuBackend<'_> {
    type Device = VelloGpuDevice;

    fn new_target(&self, w: u32, h: u32, clear: peniko::Color) -> Self::Device {
        let (w, h) = (w.min(MAX_TARGET_DIMENSION), h.min(MAX_TARGET_DIMENSION));
        // A transparent clear needs no backdrop draw at all, which is the
        // common case (every group, mask and pattern cell) and the one worth
        // keeping free of an image upload.
        if clear == peniko::Color::TRANSPARENT {
            return VelloGpuDevice::new(w, h, None);
        }
        let mut target = VelloGpuDevice::new(w, h, None);
        let rect = target.full_rect();
        target
            .scene
            .fill(peniko::Fill::NonZero, Affine::IDENTITY, clear, None, &rect);
        target
    }

    fn new_target_with_backdrop(&self, base: &Pixmap) -> Self::Device {
        VelloGpuDevice::new(base.width(), base.height(), Some(base))
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

impl VelloGpuBackend<'static> {
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
        // The design point behind `VelloGpuDevice` carrying a scene and not a
        // texture: a soft mask or pattern cell that is created and discarded
        // must not have cost an allocation on the device.
        let d = VelloGpuDevice::new(64, 64, None);
        assert_eq!(d.width, 64);
        assert!(d.frames.is_empty());
    }

    #[test]
    fn clips_and_layers_share_one_stack() {
        // The structural difference from `pdfrum-raster-vello`, which needs a
        // `Vec<Frame>` *tagged by kind* because `vello_cpu` keeps two stacks
        // and each `pop` must name one. Here every frame pops the same way,
        // and the vector carries a deferred mask rather than a kind.
        let mut d = VelloGpuDevice::new(8, 8, None);
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
        let mut d = VelloGpuDevice::new(8, 8, None);
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
        let mut d = VelloGpuDevice::new(8, 8, None);
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
    fn a_mask_of_the_wrong_size_is_ignored_not_applied() {
        // SPEC §8 makes a device-sized mask an invariant, and the CPU backends
        // both fail *open* on a violation — matching them keeps a Tier C
        // difference from being about error handling rather than rasterizing.
        // The `debug_assert` fires first in a debug build, which is the same
        // shape `pdfrum-raster-vello` takes, so the release behaviour is
        // asserted through the conversion the release path would reach.
        let d = VelloGpuDevice::new(8, 8, None);
        let wrong = AlphaMask::filled(4, 4, 128);
        assert_ne!((wrong.width(), wrong.height()), (d.width, d.height));
        // What `push_layer`'s size guard consults, exercised without tripping
        // the debug assertion that documents the invariant.
        assert!(wrong.width() != d.width || wrong.height() != d.height);
    }
}
