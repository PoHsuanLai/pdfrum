//! Where the *engine* half of a render goes, split finer than the backend
//! seam.
//!
//! Records, per render, the time spent in each of the walk's own phases —
//! clip resolution, colour resolution, glyph placement, image preparation,
//! and the dispatch left over — and the allocations at each site in the walk
//! that makes one, counted and sized.
//!
//! **Costs nothing when the `profiling` feature is off**: every entry
//! point compiles to an empty inline function. The accumulator is a
//! thread-local, so a rayon render reports per-thread totals rather than a
//! contended one, and [`take`] resets it.

// Counters rather than a `GlobalAlloc` shim, which would need `unsafe impl`
// and `unsafe_code = "forbid"` is workspace-wide. Counting at the sites is
// enough for the question asked: the walk's allocation sites are enumerable
// by reading it.
//
// The reporting half — `Profile` itself, and `Phase`/`Site`'s
// `index`/`name`/`ALL` — carries `#[cfg(feature = "profiling")]` because
// with the feature off nothing ever produces a `Profile` to report. `Site`
// and `Phase`'s *variants* are unconditional: the recording half names them
// at every call site whether or not the feature is on.

#[cfg(feature = "profiling")]
use core::time::Duration;

/// One phase of the walk's engine-side work.
///
/// The set is closed and each variant names a span of code rather than a
/// function, because the question is where the *cost* is and several of these
/// are spread over more than one call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// `crate::clip::resolve`: a clip stack reduced to the device calls it
    /// becomes, per object. Allocates a `Vec<Clip>` and, for a non-rectangular
    /// entry, a transformed `BezPath`.
    Clip,
    /// `crate::color::resolve_argb` and the transfer function, per object.
    Color,
    /// `crate::text::place_glyphs` and its type-3 sibling: the advance
    /// arithmetic, the cache lookups, and the per-glyph `PlacedGlyph`.
    Glyphs,
    /// The image path's geometry, cache key and pixmap production — everything
    /// `crate::walk`'s image arms do that is not a device call.
    Image,
    /// The shading path's own evaluation, likewise.
    Shading,
    /// `crate::paint::draw_path`'s decision tree: the two-point test, the
    /// axis-aligned-rectangle test, the zero-area sub-path scan and the stroke
    /// split — everything a path object costs above the device call.
    PathPrep,
    /// The per-object cull test in `crate::walk::render_object_list`:
    /// `crate::walk`'s `object_bbox` and the four comparisons against the
    /// list's object-space clip box.
    Cull,
    /// `crate::path::path_rect` and `snap_rect` — `draw_path`'s case 2, the
    /// axis-aligned-rectangle fast path, which runs on every fill-only path
    /// object whether or not it is a rectangle.
    RectTest,
    /// `crate::zero_area::scan_into` — `draw_path`'s case 3, which runs on
    /// every fill-only non-glyph path object and on the corpus almost never
    /// finds anything.
    ZeroScan,
    /// The geometry `draw_path`'s ordinary case hands the device: the path
    /// transformed into device space and clamped by
    /// `crate::path::hard_clip`. One reserved `BezPath` per fill, per
    /// object — `crate::path::transform_hard_clip` fuses the transform and
    /// the clamp into a single pass.
    PathXform,
    /// `crate::shading::draw_patches` — the Coons and tensor mesh half of a
    /// shading, which rasterizes patch by patch through a scratch device
    /// rather than writing a buffer.
    ///
    /// It sits beside `shading`, not inside it: `Phase::Shading` wraps
    /// `draw_to_pixmap`, which the mesh kinds never reach. Like `PathPrep`,
    /// the span covers the device calls it makes.
    Patches,
}

/// One allocation site in the walk, named by what it allocates.
///
/// These are the sites an arena could plausibly serve: each produces a value
/// that does not outlive the object being drawn. A site whose value escapes the
/// walk is deliberately absent, because an arena cannot help it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Site {
    /// The `Vec<Clip>` `crate::clip::resolve` returns, once per object.
    ClipVec,
    /// A clip entry's transformed `BezPath`, for a clip that is not an
    /// axis-aligned rectangle.
    ClipPath,
    /// The `Vec<PlacedGlyph>` `crate::text::place_glyphs` returns, once per
    /// text object.
    GlyphVec,
    /// A `RenderOptions` cloned into a nested context — a form, a char proc, a
    /// tile cell, a soft mask.
    OptionsClone,
    /// A `RenderCtx` cloned by `crate::ctx::RenderCtx::deeper`.
    CtxClone,
    /// The device-space `BezPath`s `crate::paint::draw_path`'s ordinary case
    /// builds for a fill or a stroke: one per fill, since
    /// `crate::path::transform_hard_clip` fuses the transform and the clamp;
    /// three on the stroke arm, which still composes a nudge between them.
    PathGeometry,
}

/// Everything one render's walk accumulated.
///
/// A record of facts: the counters are public and the reporting lives in
/// whoever reads them.
#[cfg(feature = "profiling")]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Profile {
    /// Time in each phase, indexed as [`Phase`] orders them.
    pub phase_time: [Duration; 11],
    /// How many times each phase was entered.
    pub phase_calls: [u64; 11],
    /// How many allocations each site made, indexed as [`Site`] orders them.
    pub site_count: [u64; 6],
    /// How many bytes those allocations asked for, where the size is knowable
    /// from the value itself (a `Vec`'s capacity times its element size, a
    /// `BezPath`'s element count times a `PathEl`).
    pub site_bytes: [u64; 6],
}

impl Phase {
    /// The index this phase occupies in [`Profile::phase_time`].
    #[cfg(feature = "profiling")]
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Phase::Clip => 0,
            Phase::Color => 1,
            Phase::Glyphs => 2,
            Phase::Image => 3,
            Phase::Shading => 4,
            Phase::PathPrep => 5,
            Phase::Cull => 6,
            Phase::RectTest => 7,
            Phase::ZeroScan => 8,
            Phase::PathXform => 9,
            Phase::Patches => 10,
        }
    }

    /// The phases in the order the arrays index them.
    #[cfg(feature = "profiling")]
    pub const ALL: [Phase; 11] = [
        Phase::Clip,
        Phase::Color,
        Phase::Glyphs,
        Phase::Image,
        Phase::Shading,
        Phase::PathPrep,
        Phase::Cull,
        Phase::RectTest,
        Phase::ZeroScan,
        Phase::PathXform,
        Phase::Patches,
    ];

    /// A short name for a report column.
    #[cfg(feature = "profiling")]
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Phase::Clip => "clip",
            Phase::Color => "color",
            Phase::Glyphs => "glyphs",
            Phase::Image => "image",
            Phase::Shading => "shading",
            Phase::PathPrep => "path prep",
            Phase::Cull => "cull",
            Phase::RectTest => "rect test",
            Phase::ZeroScan => "zero scan",
            Phase::PathXform => "path xform",
            Phase::Patches => "patches",
        }
    }
}

impl Site {
    /// The index this site occupies in [`Profile::site_count`].
    #[cfg(feature = "profiling")]
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Site::ClipVec => 0,
            Site::ClipPath => 1,
            Site::GlyphVec => 2,
            Site::OptionsClone => 3,
            Site::CtxClone => 4,
            Site::PathGeometry => 5,
        }
    }

    /// The sites in the order the arrays index them.
    #[cfg(feature = "profiling")]
    pub const ALL: [Site; 6] = [
        Site::ClipVec,
        Site::ClipPath,
        Site::GlyphVec,
        Site::OptionsClone,
        Site::CtxClone,
        Site::PathGeometry,
    ];

    /// A short name for a report row.
    #[cfg(feature = "profiling")]
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Site::ClipVec => "clip Vec<Clip>",
            Site::ClipPath => "clip BezPath",
            Site::GlyphVec => "Vec<PlacedGlyph>",
            Site::OptionsClone => "RenderOptions clone",
            Site::CtxClone => "RenderCtx clone",
            Site::PathGeometry => "draw_path BezPath",
        }
    }
}

#[cfg(feature = "profiling")]
mod imp {
    use core::cell::Cell;
    use core::time::Duration;

    use super::{Phase, Profile, Site};

    thread_local! {
        /// The accumulator. A `Cell<Profile>` rather than a `RefCell`: the
        /// record is `Copy`, so a read-modify-write needs no borrow and cannot
        /// panic on a re-entrant one — and the walk *is* re-entrant, since a
        /// form's objects are walked from inside the phase timing an enclosing
        /// object.
        static PROFILE: Cell<Profile> = const { Cell::new(Profile {
            phase_time: [Duration::ZERO; 11],
            phase_calls: [0; 11],
            site_count: [0; 6],
            site_bytes: [0; 6],
        }) };
    }

    /// Read the accumulator and clear it.
    pub fn take() -> Profile {
        PROFILE.replace(Profile::default())
    }

    /// Record one allocation of `bytes` at `site`.
    pub fn alloc(site: Site, bytes: usize) {
        let mut p = PROFILE.get();
        let i = site.index();
        if let (Some(c), Some(b)) = (p.site_count.get_mut(i), p.site_bytes.get_mut(i)) {
            *c = c.saturating_add(1);
            *b = b.saturating_add(bytes as u64);
        }
        PROFILE.set(p);
    }

    /// Run `body` and charge its elapsed time to `phase`.
    ///
    /// **Nested phases are not double-counted into each other**, because each
    /// variant is charged to its own bucket and the buckets are summed rather
    /// than nested. A phase entered from inside another — glyph placement
    /// inside a clip resolution, which is what a text clip does — therefore
    /// appears in both, and the report says so rather than pretending the
    /// buckets partition the engine half.
    pub fn phase<T>(phase: Phase, body: impl FnOnce() -> T) -> T {
        let started = std::time::Instant::now();
        let out = body();
        let elapsed = started.elapsed();
        let mut p = PROFILE.get();
        let i = phase.index();
        if let (Some(t), Some(c)) = (p.phase_time.get_mut(i), p.phase_calls.get_mut(i)) {
            *t = t.saturating_add(elapsed);
            *c = c.saturating_add(1);
        }
        PROFILE.set(p);
        out
    }

    /// The moment a phase began, for a caller that cannot wrap its body in a
    /// closure — `crate::zero_area::scan_into`, whose result borrows the
    /// scratch it was handed, so a closure would have to hand that borrow back
    /// out of itself or run the scan twice.
    #[derive(Debug, Clone, Copy)]
    pub struct Started(std::time::Instant);

    impl Started {
        /// Charge the time since this was taken to `phase`.
        pub fn end(self, phase: Phase) {
            let elapsed = self.0.elapsed();
            let mut p = PROFILE.get();
            let i = phase.index();
            if let (Some(t), Some(c)) = (p.phase_time.get_mut(i), p.phase_calls.get_mut(i)) {
                *t = t.saturating_add(elapsed);
                *c = c.saturating_add(1);
            }
            PROFILE.set(p);
        }
    }

    /// Start a phase, closed by the returned guard's `end`.
    pub fn phase_start() -> Started {
        Started(std::time::Instant::now())
    }
}

#[cfg(not(feature = "profiling"))]
mod imp {
    #[cfg(feature = "profiling")]
    use super::Profile;
    use super::{Phase, Site};

    /// Nothing was recorded, because nothing is recording.
    #[cfg(feature = "profiling")]
    #[inline]
    pub fn take() -> Profile {
        Profile::default()
    }

    /// A no-op without the feature.
    #[inline]
    pub fn alloc(_site: Site, _bytes: usize) {}

    /// The body, unclocked, without the feature.
    #[inline]
    pub fn phase<T>(_phase: Phase, body: impl FnOnce() -> T) -> T {
        body()
    }

    /// A phase's start, which without the feature carries nothing.
    #[derive(Debug, Clone, Copy)]
    pub struct Started;

    impl Started {
        /// A no-op without the feature.
        #[inline]
        #[expect(
            clippy::unused_self,
            reason = "the feature-on twin takes `self` — the `Instant` it holds — and the two must have one signature"
        )]
        pub fn end(self, _phase: Phase) {}
    }

    /// A no-op without the feature.
    #[inline]
    pub fn phase_start() -> Started {
        Started
    }
}

#[cfg(feature = "profiling")]
pub use imp::take;
pub use imp::{alloc, phase, phase_start};

/// Record one allocation whose size is the capacity of a slice-shaped value.
///
/// A convenience over [`alloc`] for the common `Vec`-and-element-size case, so
/// a call site reads as the fact it is recording rather than as arithmetic.
#[inline]
pub fn alloc_items(site: Site, items: usize, item_size: usize) {
    alloc(site, items.saturating_mul(item_size));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The arrays exist only with the feature on, so their indexing does too.
    #[cfg(feature = "profiling")]
    #[test]
    fn the_indices_are_dense_and_distinct() {
        // The arrays are indexed by these, so a duplicate or a gap would
        // silently merge two rows of a report.
        let phases: Vec<usize> = Phase::ALL.iter().map(|p| p.index()).collect();
        assert_eq!(phases, (0..Phase::ALL.len()).collect::<Vec<_>>());
        let sites: Vec<usize> = Site::ALL.iter().map(|s| s.index()).collect();
        assert_eq!(sites, (0..Site::ALL.len()).collect::<Vec<_>>());
    }

    #[test]
    fn the_phase_wrapper_returns_the_body_s_value_either_way() {
        // The feature-off build must be transparent, not merely cheap.
        assert_eq!(phase(Phase::Color, || 7_u32), 7);
        alloc_items(Site::ClipVec, 3, 8);
    }

    #[cfg(feature = "profiling")]
    #[test]
    fn taking_the_profile_clears_it() {
        let _ = take();
        alloc_items(Site::CtxClone, 4, 16);
        let first = take();
        assert_eq!(first.site_count.get(Site::CtxClone.index()), Some(&1));
        assert_eq!(first.site_bytes.get(Site::CtxClone.index()), Some(&64));
        assert_eq!(take(), Profile::default());
    }
}
