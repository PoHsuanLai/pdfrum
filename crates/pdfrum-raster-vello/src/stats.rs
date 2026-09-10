//! Roundtrip counters for the GPU backend.
//!
//! A page is not one dispatch: every transparency group, soft mask, pattern
//! cell and knockout buffer is a `finish` or a `snapshot`, and each of those
//! is a host stall. The G3 bench times a whole `render_page` and cannot tell
//! a 1-dispatch page from a 40-dispatch one; these counters are what can.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// What one backend has done since it was built, or since the last
/// [`crate::VelloBackend::reset_roundtrip_stats`].
///
/// Counts, not times. The G3 bench still reports wall-clock; this is the
/// *shape* of the work that wall-clock contains.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RoundtripStats {
    /// [`crate::RasterBackend::new_target`] and
    /// [`crate::RasterBackend::new_target_with_backdrop`] calls.
    pub new_targets: u64,
    /// [`crate::RasterBackend::snapshot`] / [`crate::VelloBackend::try_snapshot`]
    /// calls, including those that returned a blank pixmap.
    pub snapshots: u64,
    /// [`crate::RasterBackend::finish`] / [`crate::VelloBackend::try_finish`]
    /// calls, including a zero-sized target that never reaches the GPU.
    pub finishes: u64,
    /// Pixels copied from a staging buffer into a [`pdfrum_render::Pixmap`].
    ///
    /// Zero-sized targets and failed readbacks add nothing. A snapshot of a
    /// 100×50 parent followed by a crop on the host still counts `5000`: the
    /// GPU copied the whole target.
    pub pixels_read: u64,
    /// Bytes copied into a `peniko::Blob` for an image or a mask plane.
    ///
    /// Recorded when the scene encodes the image, not when vello later
    /// uploads it. A blob that is encoded and then never rasterized still
    /// counts: the copy already happened.
    pub bytes_uploaded: u64,
    /// Storage textures allocated because the pool had no matching size.
    pub textures_created: u64,
    /// Storage textures taken from the pool rather than created.
    pub textures_reused: u64,
}

/// Shared atomics. `Relaxed` is enough: these are a scoreboard, not a
/// happens-before.
#[derive(Debug, Default)]
pub(crate) struct Counters {
    new_targets: AtomicU64,
    snapshots: AtomicU64,
    finishes: AtomicU64,
    pixels_read: AtomicU64,
    bytes_uploaded: AtomicU64,
    textures_created: AtomicU64,
    textures_reused: AtomicU64,
}

impl Counters {
    /// A fresh counter cell, shared with every device this backend creates.
    pub(crate) fn shared() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub(crate) fn snapshot(&self) -> RoundtripStats {
        let load = |a: &AtomicU64| a.load(Ordering::Relaxed);
        RoundtripStats {
            new_targets: load(&self.new_targets),
            snapshots: load(&self.snapshots),
            finishes: load(&self.finishes),
            pixels_read: load(&self.pixels_read),
            bytes_uploaded: load(&self.bytes_uploaded),
            textures_created: load(&self.textures_created),
            textures_reused: load(&self.textures_reused),
        }
    }

    pub(crate) fn reset(&self) {
        let store = |a: &AtomicU64| a.store(0, Ordering::Relaxed);
        store(&self.new_targets);
        store(&self.snapshots);
        store(&self.finishes);
        store(&self.pixels_read);
        store(&self.bytes_uploaded);
        store(&self.textures_created);
        store(&self.textures_reused);
    }

    pub(crate) fn add_new_target(&self) {
        self.new_targets.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn add_snapshot(&self) {
        self.snapshots.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn add_finish(&self) {
        self.finishes.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn add_pixels_read(&self, pixels: u64) {
        self.pixels_read.fetch_add(pixels, Ordering::Relaxed);
    }

    pub(crate) fn add_bytes_uploaded(&self, bytes: u64) {
        self.bytes_uploaded.fetch_add(bytes, Ordering::Relaxed);
    }

    pub(crate) fn add_texture_created(&self) {
        self.textures_created.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn add_texture_reused(&self) {
        self.textures_reused.fetch_add(1, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_counter_reads_as_zero() {
        assert_eq!(Counters::default().snapshot(), RoundtripStats::default());
    }

    #[test]
    fn a_reset_clears_every_field() {
        let c = Counters::default();
        c.add_finish();
        c.add_pixels_read(12);
        c.add_bytes_uploaded(48);
        c.reset();
        assert_eq!(c.snapshot(), RoundtripStats::default());
    }

    #[test]
    fn a_single_finish_increments_exactly_once() {
        // The integration test lives in `tests/gpu.rs` (it needs a backend).
        // This is the counter's own contract, so a later call-site miss still
        // has a unit that says what "once" means.
        let c = Counters::default();
        c.add_finish();
        let s = c.snapshot();
        assert_eq!(s.finishes, 1);
        assert_eq!(s.snapshots, 0);
        assert_eq!(s.new_targets, 0);
    }
}
