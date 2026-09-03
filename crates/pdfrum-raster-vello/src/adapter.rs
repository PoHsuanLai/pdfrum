//! Asking `wgpu` for hardware, for a process that has none to lend us.
//!
//! [`crate::VelloBackend::new`] is the primary constructor and it borrows
//! the embedder's device, which is what this milestone exists to deliver. But
//! a benchmark, a test or a command-line thumbnailer has no device to borrow,
//! so it has to walk `wgpu`'s own chain — instance, adapter, device — itself.
//! That is what [`request_adapter`] does, and its name says so rather than
//! naming the ownership arrangement that falls out of it.
//!
//! # Why the device is leaked
//!
//! [`crate::VelloBackend`] borrows its device. A constructor that creates
//! one therefore has to produce a `&'static wgpu::Device` from a device it
//! just made, and the safe way to do that is [`Box::leak`] — the crate is
//! `forbid(unsafe_code)`, so a self-referential struct is not on the table.
//!
//! The cost is bounded rather than hidden, and it is a property of *this
//! function* rather than of the build: **one device and one queue per
//! process**, never freed. A headless caller opens one for the life of the
//! process, which is the intended use; a second call reuses the first call's
//! device rather than leaking another. The borrowing constructor has no such
//! cost at all, which is one more reason it is the primary one.
//!
//! # Why the software check comes before the leak
//!
//! The leak is only acceptable for a caller that goes on to *use* the device.
//! A caller about to reject the adapter — [`try_real_gpu`] on a machine whose
//! only adapter is `llvmpipe`, which is the ordinary CI case — must not pay
//! it, and until the check moved it did: the device was opened and leaked and
//! only then found to be software. The report is therefore built from the
//! *adapter*, which allocates nothing on the GPU, and the refusal happens
//! before `request_device` is ever called.

use std::sync::OnceLock;

use crate::block::block_on;
use crate::error::Error;
use crate::readback::AdapterReport;
use crate::wgpu;
use crate::{Limits, VelloBackend};

/// A device this backend requested itself, kept so its adapter can be
/// reported.
#[derive(Debug)]
pub struct OwnedDevice {
    /// What hardware was selected — so a benchmark can prove it was hardware.
    pub report: AdapterReport,
}

/// The one device this process ever opens for itself.
///
/// `Device` and `Queue` are both `Send + Sync`, so the leaked pair is shared
/// rather than re-leaked: a second [`request_adapter`] gets the same device
/// back and pays only for its own `Renderer`. This is a lazy cache, and the
/// *why* is the fourteen GPU tests in `tests/gpu.rs` — under `cargo test`
/// they share one process, and
/// one leaked device between them is the difference between a bounded cost and
/// a per-test one. `VelloBackend` itself cannot live here: its `RefCell`
/// makes it `!Sync`, and a `Renderer` is cheap enough beside a whole device
/// that the device is where the saving is.
static SHARED: OnceLock<Result<Opened, Error>> = OnceLock::new();

/// A leaked device, its queue, and what adapter they came from.
#[derive(Debug, Clone, Copy)]
struct Opened {
    device: &'static wgpu::Device,
    queue: &'static wgpu::Queue,
    report: &'static AdapterReport,
}

/// Request an adapter and a device, and build a backend on them.
///
/// Mirrors `wgpu::Instance::request_adapter`, which is the operation it
/// actually performs: it enumerates hardware and takes the highest-performance
/// adapter, then opens a device on it. Contrast
/// [`VelloBackend::new`][crate::VelloBackend::new], which takes a device
/// the caller already has.
///
/// **This leaks one device and one queue, once per process** — see the module
/// documentation for why it leaks, and why it does so only once.
///
/// A software adapter is refused here rather than after the device is open:
/// `llvmpipe` presents itself as an ordinary Vulkan adapter, so
/// `force_fallback_adapter: false` does not exclude it, and a caller that is
/// going to reject it should not have paid for a device first.
///
/// # Errors
///
/// [`Error::NoAdapter`] when `wgpu` enumerates no usable adapter, or when the
/// only adapter it enumerates is a software rasterizer — the ordinary outcome
/// in a container or on CI, and the caller is expected to skip rather than
/// fail. [`Error::Device`] and [`Error::Renderer`] for a device or pipeline
/// that could not be built.
pub fn request_adapter() -> Result<VelloBackend<'static>, Error> {
    let opened = SHARED.get_or_init(open_shared_device).as_ref().map_err(
        // `Error` is not `Clone` — it carries driver strings — and the cached
        // failure is a fact about this machine rather than a value to hand
        // out, so it is restated. `NoAdapter` keeps its identity because
        // callers branch on it; anything else was a device that refused.
        |e| match e {
            Error::NoAdapter => Error::NoAdapter,
            other => Error::Device(other.to_string()),
        },
    )?;

    let mut backend = VelloBackend::new(opened.device, opened.queue)?;
    backend.limits = Limits {
        max_dimension: opened.report.max_dimension,
    };
    backend.owned = Some(Box::new(OwnedDevice {
        report: opened.report.clone(),
    }));
    Ok(backend)
}

/// Enumerate an adapter, refuse a software one, and open the process's device.
///
/// Runs at most once; [`SHARED`] holds whatever it decided, success or
/// failure. The failure is cached too, because a machine with no GPU still has
/// none on the fourteenth ask, and re-enumerating adapters per test is the
/// other half of the cost this arrangement exists to remove.
fn open_shared_device() -> Result<Opened, Error> {
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
        // CPU numbers under a GPU label. It is not sufficient on its own —
        // see the `is_real_gpu` check below.
        force_fallback_adapter: false,
    }))
    .ok_or(Error::NoAdapter)?
    .map_err(|_| Error::NoAdapter)?;

    // Before the device, and the ordering is the point: `AdapterReport` is
    // built from the adapter, which allocates nothing on the GPU, so a
    // software adapter is refused without a device ever having been opened —
    // and therefore without one having been leaked.
    let report = AdapterReport::of(&adapter);
    if !report.is_real_gpu() {
        return Err(Error::NoAdapter);
    }

    let (device, queue) = block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("pdfrum-raster-vello"),
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

    // See the module doc: `VelloBackend` borrows, so a constructor that
    // creates a device must produce `'static` references, and leaking is the
    // safe way to do it. This is the only place in the crate that leaks, and
    // `SHARED` is what keeps it to one.
    Ok(Opened {
        device: Box::leak(Box::new(device)),
        queue: Box::leak(Box::new(queue)),
        report: Box::leak(Box::new(report)),
    })
}

/// A backend on this machine's GPU, or `None` when there is none.
///
/// The shape every GPU test and benchmark in this workspace uses: a container
/// or a CI runner has no adapter, and such an environment must **skip with a
/// clear message**, never fail the suite and
/// never hang. Returning `None` rather than an error is what makes the
/// skipping side of that a one-liner at each call site.
///
/// A software adapter is refused as firmly as no adapter at all — see
/// [`AdapterReport::is_real_gpu`] — and [`request_adapter`] refuses it
/// *before* opening a device, so the skip path allocates no device and leaks
/// nothing.
#[must_use]
pub fn try_real_gpu() -> Option<VelloBackend<'static>> {
    request_adapter().ok()
}
