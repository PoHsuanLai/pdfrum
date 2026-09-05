//! The rendered-image cache: decoded samples, converted and reduced, kept for
//! the next draw of the same image at the same size.
//!
//! Downstream of [`pdfrum_page::ImageCache`], which holds **decoded** images
//! and stops a page decoding the same `XObject` twice. This one holds the
//! result of two pure functions applied to those samples —
//! [`crate::image::to_pixmap`] and [`crate::stretch::prescale`] — both
//! `O(source pixels)`, both otherwise re-run on every draw.
//!
//! Keyed by the source object's [`ObjRef`] and the *shape* of the request.
//! An image with no reference — an inline `BI…EI`, or one reached through an
//! annotation's `/AP` — is not cached at all.

// **`ObjRef` and not the `Arc<ImageData>`'s address.** An `Arc::as_ptr` is
// stable only while that `Arc` is alive; the allocator reuses the address
// afterwards, and a cache keyed on one would hand a freed image's pixels to
// whatever landed on its bytes. Not caching an inline image is the correct
// answer anyway: its samples live in the content stream, are decoded once by
// the page build, and are drawn once.
//
// **The reduction is keyed by its *output* dimensions, not by the device
// footprint that produced them.** `prescale` maps a fractional footprint
// through `reduced_len` — a `ceil` and a clamp — onto an integer pixel count,
// and it is only that integer the reduced pixmap depends on. Keying on the
// `f64` footprint instead would miss on two draws of the same image that
// differ in the seventh decimal of their placement.
//
// The byte budget is a backstop, not the working mechanism: every caller in
// this workspace builds `crate::RenderCaches` per page, so a page's rendered
// images are freed with the page. That is `CPDF_PageImageCache`'s own
// lifetime — constructed from a `CPDF_Page`, dying with it. Upstream's own
// 15-entry / 100 MiB budget runs only under `bLimitedImageCache`, which is
// `false` by default, so the oracle's default is an unbounded per-page cache
// and ours is a bounded one.

use std::collections::HashMap;

use pdfrum_common::FxBuildHasher;
use pdfrum_object::ObjRef;

use crate::color::Argb;
use crate::pixmap::Pixmap;

/// How many bytes of rendered images one render session keeps.
///
/// Deliberately smaller than [`pdfrum_page::MAX_BYTES`], the 100 MiB the
/// *decoded* cache is allowed: this cache holds the same images again at four
/// bytes per pixel, and holding both at the same budget would double a
/// session's floor for no gain. 64 MiB holds every image in the bench corpus
/// except the three pathological ones, each of which is larger than any budget
/// worth setting and is instead served by the single-entry case below — it is
/// drawn once per page, so it is inserted, hit on the next render, and never
/// competes with anything.
pub const RENDERED_CACHE_BUDGET: usize = 64 * 1024 * 1024;

/// The shape of one rendered-image request: everything downstream of the
/// decoded samples that the resulting pixels depend on.
///
/// Every field is here because changing it changes a pixel. Nothing else is:
/// the placement transform is *not* part of the key, because `prescale`
/// returns the caller's transform pre-scaled by a ratio the cached dimensions
/// already determine, and the caller recomputes it on a hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PixmapRequest {
    /// The ink a stencil is painted in, or [`Argb::BLACK`] for an image that
    /// has colours of its own.
    ///
    /// Normalized by [`PixmapRequest::for_image`] so that a non-stencil does
    /// not key on a colour it never reads.
    pub stencil_color: Argb,
    /// A digest of the `/TR` transfer function's samples, or `None` when there
    /// is none or it is the identity.
    ///
    /// A digest rather than the function, because the key must own its data:
    /// holding a `&TransferFunc` would tie the cache to a borrow that ends
    /// with the graphics state, and holding the 768 sample bytes would put
    /// three quarters of a kilobyte in every key of a map most of whose
    /// entries have no transfer function at all.
    pub transfer: Option<u64>,
    /// The width `prescale` reduces to, or the source width when it does not
    /// reduce.
    pub width: u32,
    /// The height, likewise.
    pub height: u32,
}

impl PixmapRequest {
    /// The request for drawing `image` at a reduction of `width` x `height`.
    ///
    /// `stencil_color` is kept only for a stencil, which is the only kind of
    /// image whose pixels depend on it; every other image normalizes to
    /// [`Argb::BLACK`] so that the same photograph drawn under two different
    /// fill colours is one cache entry rather than two.
    #[must_use]
    pub fn for_image(
        image: &pdfrum_page::ImageData,
        stencil_color: Argb,
        transfer: Option<&crate::transfer::TransferFunc<'_>>,
        width: u32,
        height: u32,
    ) -> Self {
        let is_stencil = image.samples.is_stencil();
        Self {
            stencil_color: if is_stencil {
                stencil_color
            } else {
                Argb::BLACK
            },
            transfer: transfer.filter(|t| !t.is_identity()).map(digest_transfer),
            width,
            height,
        }
    }
}

/// Hash a transfer function's samples into the value the key carries.
///
/// [`FxBuildHasher`]'s algorithm over the 768 sample bytes: they are bytes a
/// *file* supplies, so this is the one place in the key where an attacker
/// chooses the input — but a collision here can only mis-serve a pixmap
/// between two transfer functions inside one document the caller already
/// opened, and the alternative is a 768-byte key. `pdfrum_common`'s docs on
/// which keys may use the fast hasher say the same.
fn digest_transfer(t: &crate::transfer::TransferFunc<'_>) -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = FxBuildHasher::default().build_hasher();
    for channel in t.samples() {
        h.write(channel);
    }
    h.finish()
}

/// A cached pixmap, or one rendered past a full cache.
///
/// The two are the same value to a caller. The distinction exists so that a
/// hit can hand back a borrow — a rendered image is megabytes, and cloning one
/// out of the cache would give back most of what the cache saved.
#[derive(Debug)]
pub enum Rendered<'a> {
    /// Held by the cache, borrowed for this draw.
    Hit(&'a Pixmap),
    /// Produced past the budget, or for an image with no reference to key on,
    /// and dropped after this draw.
    Uncached(Pixmap),
}

impl std::ops::Deref for Rendered<'_> {
    type Target = Pixmap;

    fn deref(&self) -> &Pixmap {
        match self {
            Self::Hit(p) => p,
            Self::Uncached(p) => p,
        }
    }
}

/// Rendered images for one render session, keyed by source object and request
/// shape.
///
/// Owned by [`crate::RenderCaches`], passed down by `&mut` — no global state,
/// no interior mutability, and under `rayon` each worker keeps its own.
#[derive(Debug, Default)]
pub struct RenderedImageCache {
    /// Keyed with [`FxBuildHasher`] rather than `SipHash`.
    ///
    /// The key is an object number this workspace assigns plus four integers
    /// and a digest this crate computes. The one file-supplied component is
    /// already digested. See `pdfrum_common::FxBuildHasher` for the rule.
    entries: HashMap<(ObjRef, PixmapRequest), Pixmap, FxBuildHasher>,
    bytes: usize,
}

impl RenderedImageCache {
    /// The pixmap for `source` at `request`, producing it through `render` on
    /// a miss.
    ///
    /// `source` is `None` for an inline image or one reached through an
    /// annotation's appearance stream: there is no reference to key on, so
    /// `render` runs and its result is returned uncached. `render` is a closure
    /// because producing the pixmap *is* the cost — a hit must not pay for it.
    pub fn get_or_render(
        &mut self,
        source: Option<ObjRef>,
        request: PixmapRequest,
        render: impl FnOnce() -> Pixmap,
    ) -> Rendered<'_> {
        let Some(source) = source else {
            return Rendered::Uncached(render());
        };
        let key = (source, request);
        if !self.entries.contains_key(&key) {
            let pixmap = render();
            let size = pixmap_bytes(&pixmap);
            // `!is_empty` so that a single image larger than the whole budget
            // is still cached: it is the case §3.6 measured, it is drawn once
            // per render, and refusing it would leave the largest cost in the
            // corpus exactly where it was.
            if self.bytes.saturating_add(size) > RENDERED_CACHE_BUDGET && !self.entries.is_empty() {
                return Rendered::Uncached(pixmap);
            }
            self.bytes = self.bytes.saturating_add(size);
            self.entries.insert(key, pixmap);
        }
        match self.entries.get(&key) {
            Some(pixmap) => Rendered::Hit(pixmap),
            // Unreachable: the entry was just inserted or already present.
            // Answered rather than unwrapped, because STYLE.md §3 forbids a
            // panic in library code even for a case the code above excludes.
            None => Rendered::Uncached(render_empty()),
        }
    }
}

/// What one pixmap costs the budget.
fn pixmap_bytes(p: &Pixmap) -> usize {
    p.data().len()
}

/// The empty pixmap the unreachable arm of [`RenderedImageCache::get_or_render`]
/// answers with.
fn render_empty() -> Pixmap {
    Pixmap::new(0, 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdfrum_page::{ImageData, Pixels, Samples};

    fn gray(width: u32, height: u32) -> ImageData {
        ImageData {
            width,
            height,
            samples: Samples::Whole(Pixels::Gray8(
                vec![0u8; (width as usize) * (height as usize)].into(),
            )),
            mask: None,
            matte: None,
            interpolate: false,
        }
    }

    fn stencil(width: u32, height: u32) -> ImageData {
        let row_bytes = (width as usize).div_ceil(8);
        ImageData {
            width,
            height,
            samples: Samples::Whole(Pixels::Stencil(pdfrum_page::BitImage {
                width,
                height,
                row_bytes,
                bits: vec![0u8; row_bytes * (height as usize)],
            })),
            mask: None,
            matte: None,
            interpolate: false,
        }
    }

    fn filled(width: u32, height: u32) -> Pixmap {
        Pixmap::filled(width, height, peniko::Color::WHITE)
    }

    #[test]
    fn a_second_draw_of_the_same_image_does_not_re_render_it() {
        let mut cache = RenderedImageCache::default();
        let key = PixmapRequest::for_image(&gray(4, 4), Argb::BLACK, None, 4, 4);
        let mut renders = 0;
        for _ in 0..5 {
            let p = cache.get_or_render(Some(ObjRef::new(1, 0)), key, || {
                renders += 1;
                filled(4, 4)
            });
            assert_eq!(p.width(), 4);
        }
        assert_eq!(renders, 1, "four of the five draws should be hits");
        assert_eq!(cache.entries.len(), 1);
    }

    #[test]
    fn two_sizes_of_one_image_are_two_entries() {
        let img = gray(8, 8);
        let mut cache = RenderedImageCache::default();
        let full = PixmapRequest::for_image(&img, Argb::BLACK, None, 8, 8);
        let half = PixmapRequest::for_image(&img, Argb::BLACK, None, 4, 4);
        assert_ne!(full, half);
        let _ = cache.get_or_render(Some(ObjRef::new(1, 0)), full, || filled(8, 8));
        let _ = cache.get_or_render(Some(ObjRef::new(1, 0)), half, || filled(4, 4));
        assert_eq!(cache.entries.len(), 2, "a reduction is a different pixmap");
    }

    #[test]
    fn a_stencils_ink_is_in_the_key_and_a_photographs_is_not() {
        // Two fill colours over a stencil are two different pixmaps.
        let bits = stencil(8, 8);
        let red = PixmapRequest::for_image(&bits, Argb::opaque(255, 0, 0), None, 8, 8);
        let blue = PixmapRequest::for_image(&bits, Argb::opaque(0, 0, 255), None, 8, 8);
        assert_ne!(red, blue, "a stencil takes its ink from the fill colour");

        // The same two colours over an image with samples of its own are one.
        let photo = gray(8, 8);
        let red = PixmapRequest::for_image(&photo, Argb::opaque(255, 0, 0), None, 8, 8);
        let blue = PixmapRequest::for_image(&photo, Argb::opaque(0, 0, 255), None, 8, 8);
        assert_eq!(
            red, blue,
            "a non-stencil never reads the fill colour, so it must not key on it"
        );
    }

    #[test]
    fn an_image_with_no_reference_is_never_cached() {
        let mut cache = RenderedImageCache::default();
        let key = PixmapRequest::for_image(&gray(4, 4), Argb::BLACK, None, 4, 4);
        let mut renders = 0;
        for _ in 0..3 {
            let p = cache.get_or_render(None, key, || {
                renders += 1;
                filled(4, 4)
            });
            assert_eq!(p.width(), 4, "and the caller still gets its pixels");
        }
        assert_eq!(renders, 3, "an inline image has no key");
        assert!(cache.entries.is_empty());
    }

    #[test]
    fn a_full_cache_stops_inserting_and_still_answers() {
        let mut cache = RenderedImageCache::default();
        // One entry that fills the budget on its own: 4097^2 * 4 bytes is just
        // over 64 MiB.
        let side = 4097;
        let key = PixmapRequest::for_image(&gray(side, side), Argb::BLACK, None, side, side);
        let _ = cache.get_or_render(Some(ObjRef::new(1, 0)), key, || filled(side, side));
        assert_eq!(cache.entries.len(), 1);
        assert!(cache.bytes > RENDERED_CACHE_BUDGET);

        // A second one does not fit, and is answered without being kept.
        let key2 = PixmapRequest::for_image(&gray(side, side), Argb::BLACK, None, side, side);
        let p = cache.get_or_render(Some(ObjRef::new(2, 0)), key2, || filled(8, 8));
        assert_eq!(p.width(), 8, "the caller gets its pixmap regardless");
        assert_eq!(cache.entries.len(), 1, "and the cache did not grow");
    }

    #[test]
    fn one_image_larger_than_the_whole_budget_is_still_cached() {
        // The `image_bug_718762` case: a 5000x5000 image is 100 MB
        // premultiplied, larger than the 64 MB budget, and is exactly the
        // entry M12.md §3.6 exists to make a hit.
        let mut cache = RenderedImageCache::default();
        let side = 5000;
        let key = PixmapRequest::for_image(&gray(side, side), Argb::BLACK, None, side, side);
        let mut renders = 0;
        for _ in 0..2 {
            let _ = cache.get_or_render(Some(ObjRef::new(1, 0)), key, || {
                renders += 1;
                filled(side, side)
            });
        }
        assert_eq!(renders, 1, "the empty-cache case admits an oversized image");
    }

    #[test]
    fn clearing_empties_the_cache() {
        let mut cache = RenderedImageCache::default();
        let key = PixmapRequest::for_image(&gray(4, 4), Argb::BLACK, None, 4, 4);
        let _ = cache.get_or_render(Some(ObjRef::new(1, 0)), key, || filled(4, 4));
        assert!(!cache.entries.is_empty());
        cache.entries.clear();
        cache.bytes = 0;
        assert!(cache.entries.is_empty());
        assert_eq!(cache.bytes, 0);
    }

    #[test]
    fn two_generations_of_one_object_number_are_two_entries() {
        let mut cache = RenderedImageCache::default();
        let key = PixmapRequest::for_image(&gray(4, 4), Argb::BLACK, None, 4, 4);
        let _ = cache.get_or_render(Some(ObjRef::new(7, 0)), key, || filled(4, 4));
        let _ = cache.get_or_render(Some(ObjRef::new(7, 1)), key, || filled(4, 4));
        assert_eq!(cache.entries.len(), 2);
    }
}
