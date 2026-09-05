//! The render session's context and caches.
//!
//! [`RenderCtx`] is a record every field of which some free function reads
//! and none of which is read by all; [`RenderCaches`] is owned by the session
//! rather than by a global.

use pdfrum_common::Deadline;
use pdfrum_font::{FontId, GlyphCache};
use pdfrum_page::Transparency;

use crate::color::Argb;
use crate::options::RenderOptions;

/// The render-recursion cap.
///
/// Upstream keeps this in a *process-global* counter, shared across
/// concurrent renders in the same process; ours is a field, which is strictly
/// more correct and never less permissive. The cap counts form, char-proc,
/// pattern and soft-mask recursion, independently of the page layer's own
/// parse-time form guard.
pub const MAX_RECURSION_DEPTH: u32 = 64;

/// A type-3 char proc's imposed colour and the char it is drawing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Type3Frame {
    /// The colour every uncoloured drawing operation inside the proc takes.
    pub fill: Argb,
    /// Whether the glyph procedure declared its own colour (`d0`) rather
    /// than only a width (`d1`).
    pub colored: bool,
}

/// The mutable state a render carries down through nested forms, patterns,
/// glyph procedures and soft masks.
#[derive(Debug, Clone)]
pub struct RenderCtx<'a> {
    /// The caller's options, plus whatever a nested context forced on.
    pub opts: RenderOptions,
    /// How deep the render recursion is, capped at [`MAX_RECURSION_DEPTH`].
    pub depth: u32,
    /// The enclosing state's colours, which an object with none of its own
    /// inherits (`initial_states_`).
    pub initial_fill: Option<Argb>,
    /// The same for strokes.
    pub initial_stroke: Option<Argb>,
    /// The type-3 frame, when inside a glyph procedure.
    pub type3: Option<Type3Frame>,
    /// The fonts already on the type-3 ancestry.
    ///
    /// A **set**, not a depth counter: a font may not appear twice anywhere
    /// above the current procedure, which is what stops a glyph that draws
    /// itself.
    pub type3_fonts: &'a [FontId],
    /// The enclosing holder's group flags. Note that a group composites back
    /// under the *enclosing* transparency with `group` forced on, not under
    /// its own.
    pub transparency: Transparency,
    /// Whether this context is already inside a transparency group, which
    /// stops a nested group re-applying the enclosing group's alpha.
    pub in_group: bool,
    /// The run's deadline, borrowed from the [`RenderSession`](crate::RenderSession)
    /// so that the record every nested context clones grows by a pointer and
    /// not by the deadline itself. `None` is no limit.
    pub deadline: Option<&'a Deadline>,
}

impl RenderCtx<'_> {
    /// A fresh top-level context.
    #[must_use]
    pub fn new(opts: RenderOptions, transparency: Transparency) -> Self {
        Self {
            opts,
            depth: 0,
            initial_fill: None,
            initial_stroke: None,
            type3: None,
            type3_fonts: &[],
            transparency,
            in_group: false,
            deadline: None,
        }
    }

    /// Whether the run's deadline is set and has passed — the per-object
    /// read, kept to a branch when unset.
    #[must_use]
    pub fn out_of_time(&self) -> bool {
        self.deadline.is_some_and(Deadline::passed)
    }

    /// Whether another level of recursion is permitted.
    #[must_use]
    pub fn may_recurse(&self) -> bool {
        self.depth < MAX_RECURSION_DEPTH
    }

    /// The same context one level deeper.
    #[must_use]
    pub fn deeper(&self) -> Self {
        // A `RenderCtx` clone is a `RenderOptions` clone plus a handful of
        // `Copy` fields, and `RenderOptions` is `Copy`-shaped but derives only
        // `Clone` — so the interesting question is how often this runs, not
        // how many bytes it moves. Counted at one byte per field-set so the
        // count is the number and the byte column is not read as heap traffic.
        crate::walkprofile::alloc_items(
            crate::walkprofile::Site::CtxClone,
            1,
            core::mem::size_of::<Self>(),
        );
        Self {
            depth: self.depth.saturating_add(1),
            ..self.clone()
        }
    }

    /// Whether a font is already on the type-3 ancestry, which is what stops
    /// a glyph procedure recursing into its own font.
    #[must_use]
    pub fn type3_font_is_active(&self, font: FontId) -> bool {
        self.type3_fonts.contains(&font)
    }
}

/// Caches owned by one render session, passed down by the owner.
///
/// Scoping the glyph cache here rather than to a process makes the type-3
/// blue-zone snapping deterministic: it is order-dependent by design, so a
/// shared cache would make output depend on what else had been rendered.
#[derive(Debug, Default)]
pub struct RenderCaches {
    /// Glyph outlines, keyed as `pdfrum-font` keys them.
    ///
    /// Feeds the *path* side of text: display type above the size threshold,
    /// a stroked or pattern-coloured run, and a caller who asked for fractional
    /// placement.
    pub(crate) glyphs: GlyphCache,
    /// Glyph bitmaps, keyed by the outline key plus the quantised device
    /// matrix ([`crate::glyph::BitmapKey`]).
    ///
    /// Feeds the ordinary small-text path, which is most of the text in the
    /// corpus. It is a second cache rather than a second field on the first
    /// because the two are keyed differently — a bitmap depends on the size it
    /// is drawn at and an outline does not — and because the outline cache
    /// lives in `pdfrum-font`, which has no notion of a device.
    pub(crate) glyph_bitmaps: crate::glyph::BitmapCache,
    /// Rendered images: decoded samples converted to a premultiplied pixmap
    /// and box-reduced toward their device footprint, keyed by the `XObject`
    /// they came from and the shape of the request
    /// (`crate::imagecache::PixmapRequest`).
    ///
    /// A third cache rather than a field on either of the others because it
    /// is keyed by neither's key and holds neither's kind of thing: the
    /// decoded samples upstream of it are `pdfrum-page`'s to cache, and what
    /// is cached here is the two pure functions *downstream* of them.
    pub(crate) images: crate::imagecache::RenderedImageCache,
    /// The degenerate-sub-path scan's working buffers.
    ///
    /// Not a cache — nothing is remembered between paths, and it would be
    /// wrong to remember anything, since the scan's answer depends on the
    /// path. What is reused is the *memory*: the scan runs on every fill-only
    /// path object, building a point list and a result list to answer
    /// "nothing degenerate here" on the overwhelming majority of them.
    pub(crate) zero_area: crate::zero_area::Scratch,
    /// One text object's placed glyphs, refilled per object.
    ///
    /// The same kind of thing as [`Self::zero_area`] and for the same reason:
    /// one `Vec<PlacedGlyph>` per text object, otherwise allocated afresh
    /// thousands of times per render. Nothing is remembered between objects;
    /// the memory is.
    pub(crate) placed_glyphs: Vec<crate::text::PlacedGlyph>,
    /// One glyph blit's two working buffers, refilled per glyph.
    ///
    /// The same kind of thing as [`Self::placed_glyphs`], one level finer.
    /// The blit runs per glyph *occurrence* — tens of thousands of times on a
    /// text-heavy page, against a few hundred distinct glyphs — and each
    /// occurrence built a coverage `Vec` and a premultiplied `Pixmap` of its
    /// own. Neither is a cache: the bytes depend on the glyph and the fill
    /// colour and are rewritten in full every time. Only the memory is reused.
    pub(crate) glyph_blit: GlyphBlitScratch,
}

/// The per-occurrence buffers a glyph blit fills.
///
/// Held on [`RenderCaches`] rather than built per glyph. Both are fully
/// rewritten on every use, so nothing carries between glyphs but the
/// allocation.
#[derive(Debug)]
pub(crate) struct GlyphBlitScratch {
    /// [`crate::glyph::LcdBitmap::gray_coverage_into`]'s output.
    pub(crate) coverage: Vec<u8>,
    /// The premultiplied pixmap that coverage is recoloured into.
    pub(crate) pixels: crate::Pixmap,
}

impl Default for GlyphBlitScratch {
    fn default() -> Self {
        Self {
            coverage: Vec::new(),
            // Zero-sized: `Pixmap::new` allocates nothing at this size, and
            // the first glyph resizes it to its own. `Pixmap` is public and
            // deliberately has no `Default`, so this is spelt out rather than
            // derived.
            pixels: crate::Pixmap::new(0, 0),
        }
    }
}

impl RenderCaches {
    /// Empty caches for a new session.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// How much room [`Self::placed_glyphs`] is holding, for the integration
    /// test that checks the walk hands the buffer back.
    ///
    /// Hidden rather than public: it is a fact about an internal buffer and
    /// no caller has a use for it. It exists because the property it pins —
    /// the walk `mem::take`s the buffer and must put it back — is invisible
    /// to every pixel test, since output is byte-identical either way.
    #[doc(hidden)]
    #[must_use]
    pub fn glyph_buffer_capacity(&self) -> usize {
        self.placed_glyphs.capacity()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn depth_cap_is_64() {
        let mut ctx = RenderCtx::new(RenderOptions::default(), Transparency::default());
        for _ in 0..MAX_RECURSION_DEPTH {
            assert!(ctx.may_recurse());
            ctx = ctx.deeper();
        }
        assert!(!ctx.may_recurse(), "the 65th level is refused");
    }

    #[test]
    fn deeper_keeps_everything_but_the_depth() {
        let ctx = RenderCtx {
            initial_fill: Some(Argb::opaque(1, 2, 3)),
            in_group: true,
            ..RenderCtx::new(RenderOptions::default(), Transparency::default())
        };
        let child = ctx.deeper();
        assert_eq!(child.initial_fill, ctx.initial_fill);
        assert!(child.in_group);
        assert_eq!(child.depth, 1);
    }

    #[test]
    fn the_type3_guard_is_a_set_not_a_depth() {
        let fonts = [FontId(7), FontId(9)];
        let ctx = RenderCtx {
            type3_fonts: &fonts,
            ..RenderCtx::new(RenderOptions::default(), Transparency::default())
        };
        assert!(ctx.type3_font_is_active(FontId(7)));
        assert!(ctx.type3_font_is_active(FontId(9)));
        assert!(!ctx.type3_font_is_active(FontId(8)));
    }
}
