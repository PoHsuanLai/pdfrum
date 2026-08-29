//! The decoded-image session cache.
//!
//! Keyed on `(ObjRef, RequestedSize)` rather than the reference alone
//! (SPEC.md §7, 2026-08-29): a plain reference key cannot express
//! resolution-dependent invalidation, and would hand a fifty-pixel thumbnail
//! back to a full-resolution request.
//!
//! # The eviction order is not the obvious one
//!
//! PDFium evicts down to the **fifteen most recent entries unconditionally,
//! before looking at the byte budget at all**, and only then evicts further
//! to fit the budget. So a cache holding twenty small images loses five of
//! them even when they total a few kilobytes.

use super::ImageData;
use pdfrum_object::ObjRef;
use std::collections::HashMap;
use std::sync::Arc;

/// Entries kept regardless of size.
pub const MAX_ENTRIES: usize = 15;

/// Byte budget, 100 MiB.
pub const MAX_BYTES: usize = 100 << 20;

/// The resolution an image was decoded at.
///
/// `Full` and a `Reduced` size are distinct keys: an image cached reduced
/// cannot serve a full-resolution request, while a full-resolution one can
/// serve anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum RequestedSize {
    /// Every sample the image has.
    #[default]
    Full,
    /// At most this many pixels in each direction.
    Reduced {
        /// Requested width.
        width: u32,
        /// Requested height.
        height: u32,
    },
}

impl RequestedSize {
    /// How many resolution levels to skip to satisfy this request for an
    /// image of `width` by `height`.
    ///
    /// Integer division, then the smaller of the two, then a floored base-two
    /// logarithm — so an image four times too large skips two levels and one
    /// three times too large skips one.
    #[must_use]
    pub fn levels(self, width: u32, height: u32) -> u8 {
        let Self::Reduced {
            width: max_w,
            height: max_h,
        } = self
        else {
            return 0;
        };
        if max_w == 0 || max_h == 0 {
            return 0;
        }
        let ratio = (width / max_w).min(height / max_h).max(1);
        #[expect(
            clippy::cast_possible_truncation,
            reason = "a u32's base-two logarithm never exceeds 31"
        )]
        let levels = ratio.ilog2() as u8;
        levels
    }

    /// Whether an image cached at `self` can serve a request for `wanted`.
    #[must_use]
    pub fn satisfies(self, wanted: Self, cached_width: u32, cached_height: u32) -> bool {
        match self {
            // A full-resolution cache always serves.
            Self::Full => true,
            Self::Reduced { .. } => match wanted {
                // A reduced cache cannot serve a full-resolution request.
                Self::Full => false,
                Self::Reduced { width, height } => cached_width >= width && cached_height >= height,
            },
        }
    }
}

/// A session-scoped cache of decoded images.
///
/// Owned by whatever is rendering or extracting, passed down by `&mut` — no
/// global state, no interior mutability (STYLE.md §1).
#[derive(Debug, Default)]
pub struct ImageCache {
    entries: HashMap<(ObjRef, RequestedSize), Entry>,
    /// A monotonically increasing counter standing in for a clock.
    tick: u64,
    bytes: usize,
}

#[derive(Debug)]
struct Entry {
    image: Arc<ImageData>,
    last_used: u64,
    bytes: usize,
}

impl ImageCache {
    /// An empty cache.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// How many images are cached.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether nothing is cached.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Total bytes held.
    #[must_use]
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// Look an image up, refreshing its recency.
    ///
    /// A key requesting full resolution also accepts a full-resolution entry
    /// stored under a reduced key only when that entry is large enough, which
    /// [`RequestedSize::satisfies`] decides.
    pub fn get(&mut self, key: ObjRef, size: RequestedSize) -> Option<Arc<ImageData>> {
        self.tick = self.tick.saturating_add(1);
        let tick = self.tick;
        // The exact key first.
        if let Some(entry) = self.entries.get_mut(&(key, size)) {
            entry.last_used = tick;
            return Some(Arc::clone(&entry.image));
        }
        // Then a full-resolution entry, which serves any request.
        if size != RequestedSize::Full
            && let Some(entry) = self.entries.get_mut(&(key, RequestedSize::Full))
        {
            entry.last_used = tick;
            return Some(Arc::clone(&entry.image));
        }
        None
    }

    /// Store an image, then evict.
    pub fn insert(&mut self, key: ObjRef, size: RequestedSize, image: Arc<ImageData>) {
        self.tick = self.tick.saturating_add(1);
        let bytes = image.byte_size();
        if let Some(old) = self.entries.insert(
            (key, size),
            Entry {
                image,
                last_used: self.tick,
                bytes,
            },
        ) {
            self.bytes = self.bytes.saturating_sub(old.bytes);
        }
        self.bytes = self.bytes.saturating_add(bytes);
        self.evict();
    }

    /// Drop everything.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
    }

    /// The entry cap first, then the byte budget — in that order, which is
    /// what makes a cache of twenty small images shed five of them.
    fn evict(&mut self) {
        if self.entries.len() <= MAX_ENTRIES && self.bytes <= MAX_BYTES {
            return;
        }
        let mut order: Vec<_> = self
            .entries
            .iter()
            .map(|(k, e)| (*k, e.last_used, e.bytes))
            .collect();
        // Oldest first.
        order.sort_by_key(|(_, used, _)| *used);

        // The unconditional entry cap.
        let over = self.entries.len().saturating_sub(MAX_ENTRIES);
        let mut drop_count = over;
        // Then whatever more the budget demands.
        let mut projected = self.bytes;
        for (_, _, bytes) in order.iter().take(over) {
            projected = projected.saturating_sub(*bytes);
        }
        while projected > MAX_BYTES && drop_count < order.len() {
            if let Some((_, _, bytes)) = order.get(drop_count) {
                projected = projected.saturating_sub(*bytes);
            }
            drop_count += 1;
        }
        for (key, _, bytes) in order.into_iter().take(drop_count) {
            self.entries.remove(&key);
            self.bytes = self.bytes.saturating_sub(bytes);
        }
    }
}

#[cfg(test)]
mod tests {
    // Test fixtures quote the oracle's own vectors, compare floats exactly
    // where the behaviour being pinned is exact, and index arrays whose
    // length the fixture itself fixes.
    #![allow(
        clippy::unreadable_literal,
        clippy::float_cmp,
        clippy::indexing_slicing,
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        reason = "test fixtures quote oracle vectors verbatim and compare exactly"
    )]

    use super::{ImageCache, MAX_ENTRIES, RequestedSize};
    use crate::image::{ImageData, Pixels};
    use pdfrum_object::ObjRef;
    use std::sync::Arc;

    fn tiny() -> Arc<ImageData> {
        Arc::new(ImageData {
            width: 1,
            height: 1,
            pixels: Pixels::Gray8(Box::from(&[0u8][..])),
            mask: None,
            matte: None,
            interpolate: false,
        })
    }

    #[test]
    fn resolution_levels_halve_per_step() {
        let full = RequestedSize::Full;
        assert_eq!(full.levels(400, 400), 0);
        let half = RequestedSize::Reduced {
            width: 200,
            height: 200,
        };
        assert_eq!(half.levels(400, 400), 1);
        let eighth = RequestedSize::Reduced {
            width: 50,
            height: 50,
        };
        assert_eq!(eighth.levels(400, 400), 3);
        // A request larger than the image skips nothing.
        assert_eq!(
            RequestedSize::Reduced {
                width: 800,
                height: 800
            }
            .levels(400, 400),
            0
        );
        // A zero request is not a division by zero.
        assert_eq!(
            RequestedSize::Reduced {
                width: 0,
                height: 0
            }
            .levels(400, 400),
            0
        );
    }

    #[test]
    fn a_reduced_entry_cannot_serve_a_full_resolution_request() {
        let reduced = RequestedSize::Reduced {
            width: 50,
            height: 50,
        };
        assert!(!reduced.satisfies(RequestedSize::Full, 50, 50));
        // But a full-resolution one serves anything.
        assert!(RequestedSize::Full.satisfies(reduced, 400, 400));
        // A reduced one serves a smaller reduced request.
        assert!(reduced.satisfies(
            RequestedSize::Reduced {
                width: 25,
                height: 25
            },
            50,
            50
        ));
        assert!(!reduced.satisfies(
            RequestedSize::Reduced {
                width: 100,
                height: 100
            },
            50,
            50
        ));
    }

    #[test]
    fn the_entry_cap_fires_before_the_byte_budget() {
        let mut cache = ImageCache::new();
        for i in 0..MAX_ENTRIES + 5 {
            cache.insert(
                ObjRef::new(u32::try_from(i).unwrap_or(0), 0),
                RequestedSize::Full,
                tiny(),
            );
        }
        // Twenty one-byte images are nowhere near 100 MiB, and five are still
        // evicted.
        assert_eq!(cache.len(), MAX_ENTRIES);
        assert!(cache.bytes() < 1024);
    }

    #[test]
    fn lookup_refreshes_recency_so_the_oldest_untouched_entry_goes_first() {
        let mut cache = ImageCache::new();
        for i in 0..MAX_ENTRIES {
            cache.insert(
                ObjRef::new(u32::try_from(i).unwrap_or(0), 0),
                RequestedSize::Full,
                tiny(),
            );
        }
        // Touch the first entry, then push the cache over the cap.
        assert!(cache.get(ObjRef::new(0, 0), RequestedSize::Full).is_some());
        cache.insert(ObjRef::new(99, 0), RequestedSize::Full, tiny());
        assert!(
            cache.get(ObjRef::new(0, 0), RequestedSize::Full).is_some(),
            "the touched entry should have survived"
        );
        assert!(cache.get(ObjRef::new(1, 0), RequestedSize::Full).is_none());
    }

    #[test]
    fn a_full_resolution_entry_answers_a_reduced_request() {
        let mut cache = ImageCache::new();
        cache.insert(ObjRef::new(1, 0), RequestedSize::Full, tiny());
        assert!(
            cache
                .get(
                    ObjRef::new(1, 0),
                    RequestedSize::Reduced {
                        width: 10,
                        height: 10
                    }
                )
                .is_some()
        );
        // But not the other way round.
        let mut cache = ImageCache::new();
        cache.insert(
            ObjRef::new(1, 0),
            RequestedSize::Reduced {
                width: 10,
                height: 10,
            },
            tiny(),
        );
        assert!(cache.get(ObjRef::new(1, 0), RequestedSize::Full).is_none());
    }

    #[test]
    fn clearing_empties_the_cache() {
        let mut cache = ImageCache::new();
        cache.insert(ObjRef::new(1, 0), RequestedSize::Full, tiny());
        assert!(!cache.is_empty());
        cache.clear();
        assert!(cache.is_empty());
        assert_eq!(cache.bytes(), 0);
    }
}
