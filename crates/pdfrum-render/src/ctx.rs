//! The render session's context and caches.
//!
//! `CPDF_RenderStatus` is a thousand-line class with twenty-four members;
//! STYLE §1 forbids reproducing it, and none of those members needs the
//! others. What remains is [`RenderCtx`], a record every field of which some
//! free function reads and none of which is read by all, plus
//! [`RenderCaches`], owned by the session rather than by a global.

use pdfrum_font::{FontId, GlyphCache};
use pdfrum_page::{Conversion, Transparency};

use crate::color::Argb;
use crate::options::RenderOptions;

/// The render-recursion cap (`kRenderMaxRecursionDepth`,
/// `cpdf_renderstatus.cpp:82`).
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
    /// Which `DeviceCMYK` formula an image decoded under this status would
    /// use, mirroring `CPDF_RenderStatus::std_cs_` (`cpdf_renderstatus.h:62`).
    ///
    /// [`Conversion::Standard`] on every *offscreen* sub-render, exactly where
    /// the oracle calls `SetStdCS(true)`; [`Conversion::Managed`] at top
    /// level, where it leaves the default.
    ///
    /// # Why nothing reads it, and why that is the oracle's own answer
    ///
    /// This field records where the oracle sets the flag. It is deliberately
    /// not plumbed into the image path, because reading the C++ end to end
    /// shows the flag **cannot change a pixel that this port renders**:
    ///
    /// - The only consumer in the entire C++ tree is `CPDF_DeviceCS`, and only
    ///   its `kDeviceCMYK` arm — `GetRGB` (`cpdf_devicecs.cpp:65`) and
    ///   `TranslateImageLine` (`cpdf_devicecs.cpp:119`). `ICCBased`, `Lab` and
    ///   `CalRGB` never consult it; `CPDF_BasedCS` only forwards the counter to
    ///   a base space (`cpdf_basedcs.cpp:13`).
    /// - `CPDF_DIB` opens the bracket *after* the image's own colour work is
    ///   done. `StartLoadDIBBase` (`cpdf_dib.cpp:199`) runs `LoadInternal` —
    ///   and with it `LoadPalette` (`:184`), which builds an `Indexed`
    ///   palette — and `CreateDecoder` **before** `ContinueToLoadMask` raises
    ///   the counter at `:153`. It is lowered again at `:244` before the
    ///   function returns.
    /// - The image body is never translated inside that bracket at all.
    ///   `TranslateImageLine` is reached only from `TranslateScanline24bpp`
    ///   (`:1007`), called only from `CPDF_DIB::GetScanline` (`:1129`) — a
    ///   `const` accessor over `mutable` buffers that the *rasterizer* pulls
    ///   during compositing, long after the counter went back to zero. So
    ///   `cpdf_devicecs.cpp:119`'s `IsStdConversionEnabled()` is always false.
    ///
    /// What is left inside the bracket is one conversion: the `/Matte` colour
    /// at `cpdf_dib.cpp:839`. So the flag can only alter a `DeviceCMYK` image
    /// that carries an `/SMask` with a `/Matte` array **and** is drawn
    /// offscreen. No file in `testing/corpus` or `testing/resources` pairs
    /// `/Matte` with `DeviceCMYK`, and the mask's own DIB — which
    /// `StartLoadMaskDIB` (`:832`) hands `bStdCS=true` unconditionally — is
    /// always `DeviceGray`, which ignores the flag.
    ///
    /// Wiring it would therefore mean threading a parameter across the
    /// page-build/render boundary and widening
    /// `pdfrum_page::ImageCache`'s key — decoding happens once per page in
    /// `build_page`, so one image drawn both inside and outside a group would
    /// need two entries — to reproduce a formula switch that is unreachable.
    /// The field stays, typed, as the record of where the oracle sets it.
    #[allow(
        dead_code,
        reason = "records where the oracle calls SetStdCS; provably cannot change a rendered pixel, see above"
    )]
    pub std_cs: Conversion,
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
            std_cs: Conversion::Managed,
        }
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

/// Caches owned by one render session (STYLE §1: no globals; the owner passes
/// them down).
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
    /// A third cache rather than a field on either of the others because it is
    /// keyed by neither's key and holds neither's kind of thing: the decoded
    /// samples upstream of it are `pdfrum-page`'s to cache (SPEC.md §7), and
    /// what is cached here is the two pure functions *downstream* of those
    /// samples, which `docs/status/M12.md` §3.6 measured re-running on every
    /// render of an image that had not changed.
    pub(crate) images: crate::imagecache::RenderedImageCache,
    /// The degenerate-sub-path scan's working buffers.
    ///
    /// Not a cache — nothing is remembered between paths, and it would be wrong
    /// to remember anything, since the scan's answer depends on the path. What
    /// is reused is the *memory*: the scan runs on every fill-only path object,
    /// building a point list and a result list to answer "nothing degenerate
    /// here" on the overwhelming majority of them, and on `vector_paths_1751`
    /// that was 9866 allocations per render for 4925 answers of "no"
    /// (`docs/status/M12b-P2.md` §4). It sits here because this is where a
    /// render session's reusable memory lives and because the alternative —
    /// a fresh `Vec` per path — is what the measurement was about.
    pub(crate) zero_area: crate::zero_area::Scratch,
    /// One text object's placed glyphs, refilled per object.
    ///
    /// The same kind of thing as [`Self::zero_area`] and for the same reason:
    /// after the outline copies went (`docs/status/M12b-P2.md` §6) this was the
    /// last per-object allocation left in the walk — one `Vec<PlacedGlyph>` per
    /// text object, 3654 of them on `text_tcpdf_055`. Nothing is remembered
    /// between objects; the memory is.
    pub(crate) placed_glyphs: Vec<crate::text::PlacedGlyph>,
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
