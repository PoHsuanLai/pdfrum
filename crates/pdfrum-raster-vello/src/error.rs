//! What can go wrong reaching a GPU, in the domain's own words.

/// A failure setting up or driving the GPU backend.
///
/// Split by *what a caller can do about it*, which is why
/// [`Error::NoAdapter`] is its own variant rather than a string inside
/// [`Error::Device`]: a headless test or a container is expected to meet it
/// and skip, and it must be distinguishable from a device that exists and
/// refused.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// `wgpu` enumerated no adapter this backend can use.
    ///
    /// Normal on CI and inside containers with no render node. Callers should
    /// skip, not fail.
    #[error("no usable wgpu adapter")]
    NoAdapter,

    /// An adapter exists but would not yield a device.
    #[error("could not open a wgpu device: {0}")]
    Device(String),

    /// `vello` could not build its pipelines on this device.
    #[error("could not build the vello renderer: {0}")]
    Renderer(String),

    /// The target is larger than the device's `max_texture_dimension_2d`.
    ///
    /// Carried rather than clamped: silently rendering a smaller page would be
    /// wrong in a way no pixel comparison would attribute correctly.
    #[error("target {w}x{h} exceeds the device's maximum dimension {max}")]
    TargetTooLarge {
        /// Requested width.
        w: u32,
        /// Requested height.
        h: u32,
        /// The device's per-axis maximum.
        max: u32,
    },

    /// The render dispatch itself failed.
    #[error("vello could not render the scene: {0}")]
    Render(String),

    /// The readback buffer could not be mapped back to the host.
    #[error("could not read the rendered texture back: {0}")]
    Readback(String),
}
