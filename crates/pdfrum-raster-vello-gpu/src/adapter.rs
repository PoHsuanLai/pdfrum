//! Asking `wgpu` for hardware, for a process that has none to lend us.
//!
//! [`crate::VelloGpuBackend::new`] is the primary constructor and it borrows
//! the embedder's device, which is what this milestone exists to deliver. But
//! a benchmark, a test or a command-line thumbnailer has no device to borrow,
//! so it has to walk `wgpu`'s own chain — instance, adapter, device — itself.
//! That is what [`request_adapter`] does, and its name says so rather than
//! naming the ownership arrangement that falls out of it.
//!
//! # Why the device is leaked
//!
//! [`crate::VelloGpuBackend`] borrows its device. A constructor that creates
//! one therefore has to produce a `&'static wgpu::Device` from a device it
//! just made, and the safe way to do that is [`Box::leak`] — the crate is
//! `forbid(unsafe_code)`, so a self-referential struct is not on the table.
//!
//! The cost is bounded rather than hidden, and it is a property of *this
//! function* rather than of the build: one device and one queue per call,
//! never freed. A headless caller opens one for the life of the process, which
//! is the intended use; calling it in a loop leaks per iteration. The
//! borrowing constructor has no such cost, which is one more reason it is the
//! primary one.

use crate::block::block_on;
use crate::error::Error;
use crate::readback::AdapterReport;
use crate::wgpu;
use crate::{Limits, VelloGpuBackend};

/// A device this backend requested itself, kept so its adapter can be
/// reported.
#[derive(Debug)]
pub struct OwnedDevice {
    /// What hardware was selected — so a benchmark can prove it was hardware.
    pub report: AdapterReport,
}

/// Request an adapter and a device, and build a backend on them.
///
/// Mirrors `wgpu::Instance::request_adapter`, which is the operation it
/// actually performs: it enumerates hardware and takes the highest-performance
/// adapter, then opens a device on it. Contrast
/// [`VelloGpuBackend::new`][crate::VelloGpuBackend::new], which takes a device
/// the caller already has.
///
/// **This leaks one device and one queue** — see the module documentation for
/// why, and call it once per process rather than per page.
///
/// # Errors
///
/// [`Error::NoAdapter`] when `wgpu` enumerates no usable adapter — the
/// ordinary outcome in a container or on CI, and the caller is expected to
/// skip rather than fail. [`Error::Device`] and [`Error::Renderer`] for a
/// device or pipeline that could not be built.
pub fn request_adapter() -> Result<VelloGpuBackend<'static>, Error> {
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        // High performance rather than default: on a machine with both a
        // discrete card and integrated graphics, measuring the integrated one
        // and calling it "the GPU" would be the same class of mistake as
        // measuring llvmpipe.
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        // Never: a fallback adapter *is* the software rasterizer, and this
        // backend would rather report `NoAdapter` and be skipped than return
        // CPU numbers under a GPU label.
        force_fallback_adapter: false,
    }))
    .ok_or(Error::NoAdapter)?
    .map_err(|_| Error::NoAdapter)?;

    let report = AdapterReport::of(&adapter);
    let (device, queue) = block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("pdfrum-raster-vello-gpu"),
        required_features: wgpu::Features::empty(),
        // The adapter's own limits rather than the defaults: the default
        // `max_texture_dimension_2d` is 8192, and a page at 2x scale can pass
        // it. Asking for what the hardware has costs nothing and refuses
        // fewer pages.
        required_limits: adapter.limits(),
        memory_hints: wgpu::MemoryHints::default(),
        trace: wgpu::Trace::Off,
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
    }))
    .ok_or_else(|| Error::Device("the request timed out".to_owned()))?
    .map_err(|e| Error::Device(e.to_string()))?;

    // See the module doc: `VelloGpuBackend` borrows, so a constructor that
    // creates a device must produce `'static` references, and leaking is the
    // safe way to do it.
    let device: &'static wgpu::Device = Box::leak(Box::new(device));
    let queue: &'static wgpu::Queue = Box::leak(Box::new(queue));

    let mut backend = VelloGpuBackend::new(device, queue)?;
    backend.limits = Limits {
        max_dimension: report.max_dimension,
    };
    backend.owned = Some(Box::new(OwnedDevice { report }));
    Ok(backend)
}

/// A backend on this machine's GPU, or `None` when there is none.
///
/// The shape every GPU test and benchmark in this workspace uses: a container
/// or a CI runner has no adapter, and PLAN.md §M12c's guardrails say such an
/// environment must **skip with a clear message**, never fail the suite and
/// never hang. Returning `None` rather than an error is what makes the
/// skipping side of that a one-liner at each call site.
///
/// A software adapter is refused as firmly as no adapter at all — see
/// [`AdapterReport::is_real_gpu`]. Leaks a device, like
/// [`request_adapter`] it wraps.
#[must_use]
pub fn try_real_gpu() -> Option<VelloGpuBackend<'static>> {
    let backend = request_adapter().ok()?;
    let real = backend
        .adapter_report()
        .is_some_and(|report| report.is_real_gpu());
    real.then_some(backend)
}
