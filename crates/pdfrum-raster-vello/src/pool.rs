//! Reuse of GPU targets and staging buffers across `finish` / `snapshot`.
//!
//! `VelloDevice` still holds a scene, not a texture: allocating at
//! `new_target` would charge every soft mask and pattern cell for a surface
//! most of them use once. The pool is populated at *rasterize* time, when the
//! engine has already asked for pixels, and it is what stops a page of
//! same-sized offscreens from creating a texture and a `MAP_READ` buffer per
//! cell.
//!
//! Capped, not unbounded. A pathological page that walks through fifty
//! distinct sizes must not pin fifty textures for the life of the backend;
//! [`MAX_POOLED`] is eight of each, which covers the common "the page, plus
//! a handful of group-sized buffers" shape and drops the rest.

use std::sync::Mutex;

use crate::stats::Counters;
use crate::wgpu;

/// How many textures, and how many staging buffers, the pool will hold.
///
/// Eight is more than the nested-group depth the engine actually reaches and
/// less than a page of unique annotation appearance sizes. Past it, creating
/// is cheaper than retaining a unique size forever.
const MAX_POOLED: usize = 8;
const _: () = assert!(MAX_POOLED >= 2 && MAX_POOLED <= 32);

/// The texture format vello's fine stage writes and this reads.
///
/// Duplicated from `readback` rather than shared: the pool must not grow a
/// dependency on the readback module, and a format mismatch would be a
/// reuse of the wrong object, which is worse than a missed reuse.
pub(crate) const TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// What a pooled target is allowed to do.
///
/// `STORAGE_BINDING` is vello's fine stage. `COPY_SRC` is the readback.
/// `TEXTURE_BINDING` is so an embedder that takes [`crate::VelloBackend::render_to_texture`]
/// can sample the result without a second copy. Pooled and returned textures
/// share the mask so a pooled object is always legal for either path.
pub(crate) const TARGET_USAGE: wgpu::TextureUsages = wgpu::TextureUsages::STORAGE_BINDING
    .union(wgpu::TextureUsages::COPY_SRC)
    .union(wgpu::TextureUsages::TEXTURE_BINDING);

/// Idle GPU objects waiting for a rasterize of the same size.
#[derive(Debug, Default)]
pub(crate) struct GpuPool {
    textures: Mutex<Vec<PooledTexture>>,
    buffers: Mutex<Vec<PooledBuffer>>,
}

struct PooledTexture {
    texture: wgpu::Texture,
    width: u32,
    height: u32,
}

struct PooledBuffer {
    buffer: wgpu::Buffer,
    size: u64,
}

impl std::fmt::Debug for PooledTexture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PooledTexture")
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for PooledBuffer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PooledBuffer")
            .field("size", &self.size)
            .finish_non_exhaustive()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    // A poisoned mutex means a previous holder panicked mid-pool, which is
    // itself a fault; the recovered guard still names a usable `Vec`, and
    // STYLE §3 forbids the `unwrap` that would be the idiomatic spelling.
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

impl GpuPool {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// A storage texture of `(width, height)`, from the pool if one is
    /// waiting at that size and freshly allocated otherwise.
    pub(crate) fn acquire_texture(
        &self,
        device: &wgpu::Device,
        width: u32,
        height: u32,
        counters: &Counters,
    ) -> wgpu::Texture {
        {
            let mut textures = lock(&self.textures);
            if let Some(index) = textures
                .iter()
                .position(|t| t.width == width && t.height == height)
            {
                counters.add_texture_reused();
                return textures.swap_remove(index).texture;
            }
        }
        counters.add_texture_created();
        device.create_texture(&wgpu::TextureDescriptor {
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
            usage: TARGET_USAGE,
            view_formats: &[],
        })
    }

    /// Return a texture the GPU is no longer using.
    ///
    /// Call only after the readback that consumed it has completed, or after
    /// a dispatch that failed before any work was submitted. A texture still
    /// in flight would be reissued into a later dispatch.
    pub(crate) fn release_texture(&self, texture: wgpu::Texture, width: u32, height: u32) {
        let mut textures = lock(&self.textures);
        if textures.len() >= MAX_POOLED {
            return;
        }
        textures.push(PooledTexture {
            texture,
            width,
            height,
        });
    }

    /// A `MAP_READ` buffer of `size` bytes, pooled by exact size.
    pub(crate) fn acquire_buffer(&self, device: &wgpu::Device, size: u64) -> wgpu::Buffer {
        {
            let mut buffers = lock(&self.buffers);
            if let Some(index) = buffers.iter().position(|b| b.size == size) {
                return buffers.swap_remove(index).buffer;
            }
        }
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("pdfrum-vello-gpu readback"),
            size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    /// Return a staging buffer that has been `unmap`ped.
    pub(crate) fn release_buffer(&self, buffer: wgpu::Buffer, size: u64) {
        let mut buffers = lock(&self.buffers);
        if buffers.len() >= MAX_POOLED {
            return;
        }
        buffers.push(PooledBuffer { buffer, size });
    }

    /// Drop everything. A lost device makes every handle in here unusable.
    pub(crate) fn clear(&self) {
        lock(&self.textures).clear();
        lock(&self.buffers).clear();
    }
}
