//! The GPU round trip: scene in, host pixels out.
//!
//! Four steps, and an embedder rendering a page to a texture pays the first
//! three of them whatever it does with the result, so the cost of a render is
//! all four and not just the dispatch:
//!
//! 1. allocate a storage texture the fine-rasterization stage can write,
//! 2. dispatch vello's pipelines into it,
//! 3. `copy_texture_to_buffer` into a `MAP_READ` buffer,
//! 4. `map_async` and block the host until the GPU has caught up.
//!
//! Step 3 is where `wgpu`'s row-alignment rule bites: a buffer copy's
//! `bytes_per_row` must be a multiple of 256, so a 100-pixel-wide target is
//! copied at 512 bytes per row and the readback has to un-pad it. Getting that
//! wrong shears the image diagonally, which looks like a rasterizer bug and is
//! not one.

use pdfrum_render::Pixmap;
use vello::{RenderParams, Renderer, Scene};

use crate::block;
use crate::error::Error;
use crate::wgpu;

/// `wgpu`'s required row alignment for a texture-to-buffer copy.
const COPY_BYTES_PER_ROW_ALIGNMENT: u32 = 256;

/// The texture format vello's fine stage writes and this reads.
///
/// `Rgba8Unorm` and not the `Srgb` variant: the engine's `Pixmap` holds
/// premultiplied sRGB *bytes* with no transfer applied at the device seam, and
/// asking the GPU for an sRGB view would apply one on write and another on
/// read. The CPU backends do neither, so matching them is what keeps a Tier C
/// difference about rasterization.
const TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// What adapter a backend is running on.
///
/// Kept so a benchmark can say so out loud. `wgpu` will happily select
/// `llvmpipe`, a software rasterizer that reports itself as a Vulkan adapter,
/// and a "GPU" measurement taken on one is a CPU measurement with a misleading
/// label — [`AdapterReport::is_real_gpu`] is the check that prevents it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterReport {
    /// The adapter's name, as the driver reports it.
    pub name: String,
    /// The graphics backend (Vulkan, Metal, DX12, GL).
    pub backend: String,
    /// The device class the driver claims.
    pub device_type: String,
    /// Driver name and version.
    pub driver: String,
    /// The adapter's `max_texture_dimension_2d`.
    pub max_dimension: u32,
}

impl AdapterReport {
    /// Whether this is hardware rather than a software rasterizer.
    ///
    /// `DeviceType::Cpu` is the software case and `Other` is "the driver would
    /// not say" — both are refused, because a benchmark that cannot name its
    /// hardware should not claim to have measured it.
    #[must_use]
    pub fn is_real_gpu(&self) -> bool {
        matches!(
            self.device_type.as_str(),
            "DiscreteGpu" | "IntegratedGpu" | "VirtualGpu"
        )
    }

    /// Build a report from an adapter.
    #[must_use]
    pub fn of(adapter: &wgpu::Adapter) -> Self {
        let info = adapter.get_info();
        Self {
            name: info.name,
            backend: format!("{:?}", info.backend),
            device_type: format!("{:?}", info.device_type),
            driver: format!("{} {}", info.driver, info.driver_info),
            max_dimension: adapter.limits().max_texture_dimension_2d,
        }
    }
}

impl std::fmt::Display for AdapterReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} ({}, {}, driver {})",
            self.name, self.backend, self.device_type, self.driver
        )
    }
}

/// Render `scene` and bring its pixels back to the host.
///
/// # Errors
///
/// [`Error::Render`] if vello's dispatch failed, [`Error::Readback`] if the
/// staging buffer could not be mapped.
pub fn render_and_read(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    renderer: &mut Renderer,
    scene: &Scene,
    width: u32,
    height: u32,
    params: &RenderParams,
) -> Result<Pixmap, Error> {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("pdfrum-vello-gpu target"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: TARGET_FORMAT,
        // `STORAGE_BINDING` because vello's fine stage is a compute shader
        // writing a storage texture, not a fragment shader writing an
        // attachment.
        usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

    renderer
        .render_to_texture(device, queue, scene, &view, params)
        .map_err(|e| Error::Render(e.to_string()))?;

    read_texture(device, queue, &texture, width, height)
}

/// Copy a rendered texture into host memory as a [`Pixmap`].
fn read_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    width: u32,
    height: u32,
) -> Result<Pixmap, Error> {
    let unpadded = width.saturating_mul(4);
    let padded = unpadded.next_multiple_of(COPY_BYTES_PER_ROW_ALIGNMENT);
    let size = u64::from(padded).saturating_mul(u64::from(height));

    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("pdfrum-vello-gpu readback"),
        size,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("pdfrum-vello-gpu readback"),
    });
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &staging,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);

    let slice = staging.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        // The receiver outlives this send in every path below, so a failed
        // send carries no information worth propagating.
        let _ = tx.send(result.is_ok());
    });

    // `map_async`'s callback fires *from inside* `Device::poll`, on this
    // thread. So the wait must be a poll-then-check loop and must never be a
    // blocking receive: a `recv()` would park the only thread that can make
    // the callback happen. This is where the host actually waits for the GPU
    // — and the honest place to account for the GPU backend's latency.
    match block::poll_until(device, || rx.try_recv().ok()) {
        Some(true) => {}
        Some(false) => return Err(Error::Readback("buffer map failed".to_owned())),
        None => {
            return Err(Error::Readback(
                "the device stopped making progress".to_owned(),
            ));
        }
    }

    let view = slice.get_mapped_range();
    let mut out = Vec::with_capacity((unpadded as usize).saturating_mul(height as usize));
    for row in 0..height {
        let start = (row as usize).saturating_mul(padded as usize);
        let end = start.saturating_add(unpadded as usize);
        let Some(bytes) = view.get(start..end) else {
            return Err(Error::Readback(format!(
                "row {row} outside the staging buffer"
            )));
        };
        out.extend_from_slice(bytes);
    }
    drop(view);
    staging.unmap();

    Pixmap::from_vec(width, height, out)
        .ok_or_else(|| Error::Readback("readback did not fill the pixmap".to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_row_is_padded_to_the_copy_alignment() {
        // The rule that shears an image diagonally when it is missed: a
        // 100-pixel row is 400 bytes of pixels copied at 512 bytes of stride.
        assert_eq!(
            (100u32 * 4).next_multiple_of(COPY_BYTES_PER_ROW_ALIGNMENT),
            512
        );
        // And a row already aligned must not grow, or every readback would
        // carry a spurious 256 bytes of padding it then skipped.
        assert_eq!(
            (64u32 * 4).next_multiple_of(COPY_BYTES_PER_ROW_ALIGNMENT),
            256
        );
    }

    #[test]
    fn a_software_adapter_is_not_a_gpu() {
        // The llvmpipe guard. It reports as a Vulkan adapter and would
        // otherwise produce a CPU number labelled GPU.
        let cpu = AdapterReport {
            name: "llvmpipe".to_owned(),
            backend: "Vulkan".to_owned(),
            device_type: "Cpu".to_owned(),
            driver: "llvmpipe".to_owned(),
            max_dimension: 16384,
        };
        assert!(!cpu.is_real_gpu());
        let gpu = AdapterReport {
            device_type: "DiscreteGpu".to_owned(),
            ..cpu.clone()
        };
        assert!(gpu.is_real_gpu());
    }

    #[test]
    fn an_unnamed_device_class_is_refused_too() {
        // `Other` is the GL backend's answer on this machine's 4090 — the
        // driver declining to say. A benchmark that cannot name its hardware
        // does not get to claim it measured it.
        let other = AdapterReport {
            name: "NVIDIA GeForce RTX 4090/PCIe/SSE2".to_owned(),
            backend: "Gl".to_owned(),
            device_type: "Other".to_owned(),
            driver: "3.3.0".to_owned(),
            max_dimension: 16384,
        };
        assert!(!other.is_real_gpu());
    }
}
