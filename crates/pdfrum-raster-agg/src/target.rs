//! The pixel buffer a device draws into, and the two things that modulate a
//! draw: the clip stack and the span blitter.
//!
//! A [`Target`] is a premultiplied RGBA8 buffer plus the clip in force. Every
//! primitive reaches it as *spans* — a row, a start column, a length and a
//! coverage byte — because that is what the analytic rasterizer produces and
//! because it lets the clip apply as one multiply per pixel rather than as a
//! second rasterization pass.

use std::sync::Arc;

use pdfrum_page::BlendMode;
use pdfrum_render::{AlphaMask, Pixmap, blend, pixmap};

use crate::image::scale_alpha;

/// A premultiplied RGBA8 render target with a clip.
#[derive(Debug, Clone)]
pub struct Target {
    pixels: Pixmap,
    /// The clip in force, or `None` for "everything is visible".
    ///
    /// A clip is a coverage plane the size of the target, and intersecting two
    /// is the truncating product `a * b / 255`. Truncating rather than
    /// rounding is what makes a clipped edge land where the oracle's does.
    ///
    /// **Shared, not owned.** The plane is device-sized — half a megabyte on a
    /// letter page — and the clip stack and the target hold the *same* one:
    /// `push_clip_mask` computes the intersection once and `sync_clip` points
    /// the target at it, on every push and again on every pop. Owning it made
    /// that pointing a half-megabyte `memcpy` twice per clip level, which on a
    /// page of annotation appearances is two hundred and fifty of them per
    /// render. Nothing mutates a clip once it is on the stack — an
    /// intersection builds a *new* plane from the incoming coverage — so the
    /// share is of an immutable value and `sync_clip` becomes a refcount bump.
    clip: Option<Arc<AlphaMask>>,
}

impl Target {
    /// A target of the given size, every pixel `clear`.
    #[must_use]
    pub fn new(width: u32, height: u32, clear: peniko::Color) -> Self {
        Self {
            pixels: if clear == peniko::Color::TRANSPARENT {
                Pixmap::new(width, height)
            } else {
                Pixmap::filled(width, height, clear)
            },
            clip: None,
        }
    }

    /// A target seeded with an existing image's pixels.
    #[must_use]
    pub fn from_pixmap(base: Pixmap) -> Self {
        Self {
            pixels: base,
            clip: None,
        }
    }

    /// The target's width in pixels.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.pixels.width()
    }

    /// The target's height in pixels.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.pixels.height()
    }

    /// The current pixels.
    #[must_use]
    pub fn pixels(&self) -> &Pixmap {
        &self.pixels
    }

    /// Consume the target, yielding its pixels.
    #[must_use]
    pub fn into_pixmap(self) -> Pixmap {
        self.pixels
    }

    /// The clip in force.
    ///
    /// Only the tests read it. The engine's own composites take the clip from
    /// the field directly and the layer composite is unclipped by
    /// construction, so a getter reachable from outside would be a surface
    /// with no caller; `composite_layer_by_pixels` — the per-pixel spelling
    /// kept as that hoist's specification — is the one thing that needs to
    /// save and restore it, exactly as [`Target::clip_at`] is kept for
    /// [`clip_span`].
    #[cfg(test)]
    #[must_use]
    fn clip(&self) -> Option<&Arc<AlphaMask>> {
        self.clip.as_ref()
    }

    /// Replace the clip.
    pub fn set_clip(&mut self, clip: Option<Arc<AlphaMask>>) {
        self.clip = clip;
    }

    /// The clip's coverage at a pixel: `255` where there is no clip.
    ///
    /// **Not on the hot path.** [`clip_span`] takes the whole span's bytes at
    /// once instead. This is kept as that function's *specification*: it is
    /// the simple, obviously correct
    /// spelling, and `clip_span_reproduces_clip_at_exactly` requires the fast
    /// one to agree with it on every position of a deliberately awkward mask.
    /// Deleting it would leave the fast path with nothing to be checked
    /// against.
    #[cfg(test)]
    #[must_use]
    fn clip_at(&self, x: u32, y: u32) -> u8 {
        let Some(mask) = self.clip.as_ref() else {
            return 255;
        };
        let Some(i) = (y as usize)
            .checked_mul(mask.width() as usize)
            .and_then(|row| row.checked_add(x as usize))
        else {
            return 0;
        };
        mask.data().get(i).copied().unwrap_or(0)
    }

    /// Composite one span of constant coverage in a solid colour.
    ///
    /// `x`/`len` may run outside the target; the span is clipped to it rather
    /// than wrapping or panicking, because a path's cells are unbounded while
    /// the buffer is not.
    pub fn blend_span(
        &mut self,
        x: i32,
        len: i32,
        y: i32,
        coverage: u8,
        src: Source,
        mode: BlendMode,
    ) {
        let Some((x0, x1, row)) = self.span_range(x, len, y) else {
            return;
        };
        // M12: the span's clip bytes are taken once, as a slice, instead of
        // `clip_at` re-deciding per pixel whether there is a clip at all and
        // recomputing `row * width + col` from scratch each time. Both are
        // loop-invariant; leaving them inside meant the optimizer could not
        // see that the row is contiguous, and the per-pixel `Option` on
        // `self.clip` defeated any chance of vectorizing the coverage product.
        // The *arithmetic* is untouched — `clip_span` reproduces `clip_at`'s
        // answer byte for byte, including its out-of-range zeroes, which is
        // what `a_clip_narrower_than_the_target_still_reads_zero_outside_it`
        // pins.
        // The two fields are borrowed apart rather than through `self`: the
        // clip is read while the pixels are written, and only a field-wise
        // split lets the borrow checker see that those are different objects.
        let Self { pixels, clip } = self;
        let clip = clip_span(clip.as_deref(), x0, x1, row);
        let width = pixels.width() as usize;
        let Some(start) = (row as usize).checked_mul(width) else {
            return;
        };
        let Some(pixels) = pixels
            .data_mut()
            .get_mut((start + x0 as usize) * 4..(start + x1 as usize) * 4)
        else {
            return;
        };
        if let Some(clip) = clip {
            for (dest, &mask) in pixels.chunks_exact_mut(4).zip(clip.iter()) {
                let cov = pixmap::mul255(coverage, mask);
                if cov == 0 {
                    continue;
                }
                blend_into(dest, src, cov, mode);
            }
        } else {
            if coverage == 0 {
                return;
            }
            for dest in pixels.chunks_exact_mut(4) {
                blend_into(dest, src, coverage, mode);
            }
        }
    }

    /// Composite one span whose source colour varies per pixel.
    ///
    /// `sample` returns the premultiplied source pixel for a target column, or
    /// `None` where the source has nothing there — an image draw's outside.
    pub fn blend_span_with(
        &mut self,
        x: i32,
        len: i32,
        y: i32,
        coverage: u8,
        mode: BlendMode,
        mut sample: impl FnMut(u32, u32) -> Option<[u8; 4]>,
    ) {
        let Some((x0, x1, row)) = self.span_range(x, len, y) else {
            return;
        };
        // Same hoist as `blend_span`. The per-pixel `sample` closure stays —
        // it is the whole point of this entry point, and an image's source
        // pixel genuinely does have to be computed per column — but the clip
        // lookup and the destination indexing no longer happen inside it.
        // The two fields are borrowed apart rather than through `self`: the
        // clip is read while the pixels are written, and only a field-wise
        // split lets the borrow checker see that those are different objects.
        let Self { pixels, clip } = self;
        let clip = clip_span(clip.as_deref(), x0, x1, row);
        let width = pixels.width() as usize;
        let Some(start) = (row as usize).checked_mul(width) else {
            return;
        };
        let Some(pixels) = pixels
            .data_mut()
            .get_mut((start + x0 as usize) * 4..(start + x1 as usize) * 4)
        else {
            return;
        };
        for (i, dest) in pixels.chunks_exact_mut(4).enumerate() {
            let cov = match &clip {
                Some(clip) => match clip.get(i) {
                    Some(&mask) => pixmap::mul255(coverage, mask),
                    None => continue,
                },
                None => coverage,
            };
            if cov == 0 {
                continue;
            }
            let Ok(col) = u32::try_from(x0 as usize + i) else {
                continue;
            };
            let Some(src) = sample(col, row) else {
                continue;
            };
            blend_into(dest, Source::Premultiplied(src), cov, mode);
        }
    }

    /// Composite a whole image at a whole-pixel offset, one row at a time.
    ///
    /// The blit `AggDevice::draw_image` degenerates to when its transform is a
    /// whole-pixel translation: every device pixel takes exactly one texel and
    /// the source column is the destination column minus `dx`.
    ///
    /// # Why this is not a loop over [`Self::blend_span_with`]
    ///
    /// It was, and the row scaffolding cost more than the pixels. Every row of
    /// a blit has the **same** column range — `x` and `len` do not vary with
    /// the row — so [`Self::span_range`]'s clamp, the source-column offset and
    /// the destination row's stride are loop-invariant, and re-deriving them
    /// per row paid a fixed cost against rows that are eight pixels wide on a
    /// glyph. Hoisting them leaves each row as three slice takes and a `zip`.
    ///
    /// The arithmetic is untouched and is the same
    /// [`blend_into`] over the same [`Source::Premultiplied`] at the same
    /// coverage, so a blit lands on exactly the pixels the span loop landed on;
    /// `an_integer_blit_agrees_with_the_general_path` is what says so against
    /// the sampled path, and `a_blit_matches_the_span_loop_it_replaced` against
    /// the spelling this replaced.
    ///
    /// The clip still applies, per row, as the same byte-for-byte coverage
    /// product — including a clip narrower than the target, which reads zero
    /// past its edge rather than wrapping into the next row.
    pub fn blit_image(&mut self, img: &Pixmap, dx: i32, dy: i32, alpha: u8) {
        // The column range, once. `span_range` derived it per row from `x`,
        // `len` and the target's width, and a blit varies none of the three
        // with the row.
        let end = i64::from(dx) + i64::from(img.width());
        let (Ok(x0), Ok(x1)) = (
            u32::try_from(dx.max(0)),
            u32::try_from(end.clamp(0, i64::from(self.width()))),
        ) else {
            return;
        };
        if x0 >= x1 {
            return;
        }
        let span = (x1 - x0) as usize;
        // The source column the leftmost painted device column reads. `x0` is
        // `max(dx, 0)`, so `x0 - dx` is non-negative and no wider than `x0`.
        let Ok(src_x0) = usize::try_from(i64::from(x0) - i64::from(dx)) else {
            return;
        };

        // The row band, once: the image rows whose `row + dy` lands inside the
        // target. `span_range` returned `None` outside it, which painted
        // nothing, so the band and the per-row rejection are the same answer.
        let height = i64::from(img.height());
        let (Ok(first), Ok(last)) = (
            u32::try_from(i64::from(-dy).clamp(0, height)),
            u32::try_from((i64::from(self.height()) - i64::from(dy)).clamp(0, height)),
        ) else {
            return;
        };
        if first >= last {
            return;
        }

        // `scale_alpha` is the identity at 255 — the glyph blit's whole
        // traffic — so the branch is taken once here rather than per pixel.
        let opaque = alpha == 255;
        let Self { pixels, clip } = self;
        let dest_stride = pixels.width() as usize * 4;
        let src_stride = img.width() as usize * 4;
        // A clip in force whose rows do not reach `x1` reads zero across the
        // whole span — `clip_span`'s `Owned` arm — and zero coverage paints
        // nothing, so such a blit is a no-op rather than an unclipped one.
        // Confusing the two is the one way this hoist could have painted a
        // pixel the span loop did not, and
        // `a_clip_narrower_than_the_blit_paints_nothing` pins it. It is decided
        // here rather than per row because `x1` does not vary with the row.
        let clip = clip.as_deref();
        let clip_width = clip.map_or(0, |mask| mask.width() as usize);
        if clip.is_some() && (x1 as usize) > clip_width {
            return;
        }

        for row in first..last {
            let Ok(y) = usize::try_from(i64::from(row) + i64::from(dy)) else {
                continue;
            };
            let Some(dest) = y
                .checked_mul(dest_stride)
                .and_then(|start| {
                    let lo = start.checked_add(x0 as usize * 4)?;
                    Some(lo..lo.checked_add(span * 4)?)
                })
                .and_then(|range| pixels.data_mut().get_mut(range))
            else {
                continue;
            };
            let Some(src) = (row as usize)
                .checked_mul(src_stride)
                .and_then(|start| {
                    let lo = start.checked_add(src_x0 * 4)?;
                    Some(lo..lo.checked_add(span * 4)?)
                })
                .and_then(|range| img.data().get(range))
            else {
                continue;
            };
            match clip {
                None => {
                    for (dest, src) in dest.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
                        let Ok(px) = <[u8; 4]>::try_from(src) else {
                            continue;
                        };
                        let px = if opaque { px } else { scale_alpha(px, alpha) };
                        blend_into(dest, Source::Premultiplied(px), 255, BlendMode::Normal);
                    }
                }
                Some(mask) => {
                    // The mask row, taken whole — the same bytes `clip_span`
                    // borrowed. A row past the mask's end reads zero for every
                    // pixel, which paints nothing, so it is skipped rather than
                    // walked.
                    let Some(band) = y
                        .checked_mul(clip_width)
                        .and_then(|start| {
                            let lo = start.checked_add(x0 as usize)?;
                            Some(lo..lo.checked_add(span)?)
                        })
                        .and_then(|range| mask.data().get(range))
                    else {
                        continue;
                    };
                    for ((dest, src), &cov) in dest
                        .chunks_exact_mut(4)
                        .zip(src.chunks_exact(4))
                        .zip(band.iter())
                    {
                        if cov == 0 {
                            continue;
                        }
                        let Ok(px) = <[u8; 4]>::try_from(src) else {
                            continue;
                        };
                        let px = if opaque { px } else { scale_alpha(px, alpha) };
                        blend_into(dest, Source::Premultiplied(px), cov, BlendMode::Normal);
                    }
                }
            }
        }
    }

    /// Composite a whole layer back onto this target, one row at a time.
    ///
    /// A layer's pixels are device-sized and device-aligned, so every
    /// destination pixel takes exactly the source pixel at the same
    /// coordinates and there is neither an offset to apply nor a column range
    /// to clamp. The composite is unclipped: the layer already carries the
    /// clip that was in force when it was pushed, applied on the way in, and
    /// folding it in a second time would darken every clipped edge by the
    /// clip's own coverage squared.
    ///
    /// # Why this is not a loop over [`Self::blend_span`]
    ///
    /// It was, at one pixel per call, and the per-call scaffolding cost two
    /// orders of magnitude more than the blend it wrapped: [`Self::span_range`]
    /// re-derived the row and the column clamp, [`clip_span`] re-decided
    /// whether a clip was in force, the destination offset was recomputed from
    /// scratch, and a `chunks_exact_mut` was set up — all of it to reach a
    /// single four-byte pixel. None of it varies with the column, and on a
    /// letter page that is half a million calls per layer.
    ///
    /// The arithmetic is untouched: the same [`blend_into`] over the same
    /// [`Source::Premultiplied`] at the same full coverage in the same order,
    /// so a composited layer lands on exactly the pixels the per-pixel loop
    /// landed on. `a_layer_composite_matches_the_span_loop_it_replaced` is what
    /// says the two agree, over every blend mode and a source with transparent,
    /// partial and opaque pixels in it.
    ///
    /// The fully transparent source pixel is skipped **as an optimisation and
    /// nothing more**. A zero-alpha source is already the identity under every
    /// blend mode — the composite weights the blended colour by the source's
    /// alpha, so at zero the destination survives whatever the mode computed —
    /// and `a_transparent_source_pixel_is_the_identity_under_every_mode` checks
    /// that over the whole destination space rather than leaving it argued.
    /// The skip is worth keeping because a layer is mostly transparent, but
    /// removing it would change no pixel.
    pub fn composite_layer(&mut self, layer: &Pixmap, mode: BlendMode) {
        // Rows and columns in common. A layer is built at the device's own
        // size, so in practice these are both dimensions whole; taking the
        // minimum rather than asserting keeps a mismatched layer to the
        // overlap the per-pixel loop painted rather than panicking on it.
        let rows = self.height().min(layer.height()) as usize;
        let cols = self.width().min(layer.width()) as usize;
        if rows == 0 || cols == 0 {
            return;
        }
        let dest_stride = self.width() as usize * 4;
        let src_stride = layer.width() as usize * 4;
        let span = cols * 4;
        let pixels = self.pixels.data_mut();
        let src_all = layer.data();
        for row in 0..rows {
            let (Some(dest), Some(src)) = (
                row.checked_mul(dest_stride)
                    .and_then(|lo| pixels.get_mut(lo..lo.checked_add(span)?)),
                row.checked_mul(src_stride)
                    .and_then(|lo| src_all.get(lo..lo.checked_add(span)?)),
            ) else {
                continue;
            };
            for (dest, src) in dest.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
                let Ok(px) = <[u8; 4]>::try_from(src) else {
                    continue;
                };
                if px[3] == 0 {
                    continue;
                }
                blend_into(dest, Source::Premultiplied(px), 255, mode);
            }
        }
    }

    /// Merge one `ClearType` glyph pixel: three coverages, three destination
    /// channels, each merged on its own alpha.
    ///
    /// The coverages arrive **already gamma-adjusted** — the table is applied
    /// where the triples are demultiplexed — so this is only the per-channel
    /// alpha product and the merge:
    ///
    /// - `a_c = coverage_c · colour_alpha / 255`, truncating;
    /// - `dest_c = (dest_c·(255 − a_c) + colour_c·a_c) / 255`, truncating;
    /// - and the destination is left **opaque**: the alpha byte is forced to
    ///   255 rather than being merged.
    ///
    /// That last line is why this is not source-over and cannot be one. Three
    /// independent alphas have no single-alpha expression, so the oracle
    /// resolves the coverage into the colour and declares the pixel solid; it
    /// gets away with it because `DrawNormalText` seeds its scratch bitmap with
    /// the real backdrop (`GetDIBits`) before the merge. Here the destination
    /// *is* the backdrop, so the merge happens in place and the same thing is
    /// true for the same reason.
    ///
    /// The clip still applies, as a coverage multiplied into each channel's
    /// alpha — the same product every other primitive folds it in with.
    /// Out-of-range coordinates are a no-op rather than a panic.
    pub fn merge_lcd_pixel(
        &mut self,
        x: i32,
        y: i32,
        colour: [u8; 3],
        colour_alpha: u8,
        coverage: [u8; 3],
    ) {
        let (Ok(col), Ok(row)) = (u32::try_from(x), u32::try_from(y)) else {
            return;
        };
        if col >= self.width() || row >= self.height() {
            return;
        }
        // One pixel, so the span hoist `blend_span` makes would buy nothing:
        // the clip byte is read the simple way.
        let mask = match self.clip.as_ref() {
            None => 255,
            Some(mask) => (row as usize)
                .checked_mul(mask.width() as usize)
                .and_then(|r| r.checked_add(col as usize))
                .and_then(|i| mask.data().get(i).copied())
                .unwrap_or(0),
        };
        if mask == 0 {
            return;
        }
        let width = self.pixels.width() as usize;
        let Some(start) = (row as usize)
            .checked_mul(width)
            .map(|r| (r + col as usize) * 4)
        else {
            return;
        };
        let Some(dest) = self.pixels.data_mut().get_mut(start..start + 4) else {
            return;
        };
        for i in 0..3 {
            let (Some(&cov), Some(&ch)) = (coverage.get(i), colour.get(i)) else {
                continue;
            };
            let alpha = pixmap::mul255(pixmap::mul255(cov, colour_alpha), mask);
            if alpha == 0 {
                continue;
            }
            if let Some(slot) = dest.get_mut(i) {
                *slot = pixmap::alpha_merge(*slot, ch, alpha);
            }
        }
        // `SetAlpha`: the pixel is declared opaque once any channel is written.
        if let Some(slot) = dest.get_mut(3) {
            *slot = 255;
        }
    }

    /// The in-bounds column range and row of a span, or `None` when it misses
    /// the target entirely.
    fn span_range(&self, x: i32, len: i32, y: i32) -> Option<(u32, u32, u32)> {
        if len <= 0 || y < 0 {
            return None;
        }
        let row = u32::try_from(y).ok()?;
        if row >= self.height() {
            return None;
        }
        let end = i64::from(x).checked_add(i64::from(len))?;
        let x0 = u32::try_from(x.max(0)).ok()?;
        let x1 = u32::try_from(end.clamp(0, i64::from(self.width()))).ok()?;
        (x0 < x1).then_some((x0, x1, row))
    }
}

/// The clip bytes covering `[x0, x1)` on `row`, or `None` when unclipped.
///
/// `None` means "no clip is in force", which the callers turn into full
/// coverage — it is *not* "the clip is empty here". When a clip is in force but
/// does not cover the span — a mask narrower than the target, or a row past its
/// end — the answer is a zero-filled slice, because that is what
/// [`Target::clip_at`] returned per pixel and this has to reproduce it byte for
/// byte. Folding that case into `None` would turn a fully clipped span into a
/// fully painted one, which is the one way this optimization could have changed
/// a pixel; `a_clip_narrower_than_the_target_reads_zero_past_its_edge` pins it.
///
/// A free function taking the mask rather than a method on `Target`, so the
/// caller can hold `&self.clip` and `&mut self.pixels` at once — the borrow
/// checker sees two fields, where a `&self` method would take the whole struct.
///
/// The `Cow` is `Borrowed` on every path that matters. The `Owned` arm
/// allocates only for the out-of-range case, which is a clip that does not
/// reach the span at all and therefore paints nothing.
fn clip_span(
    mask: Option<&AlphaMask>,
    x0: u32,
    x1: u32,
    row: u32,
) -> Option<std::borrow::Cow<'_, [u8]>> {
    let mask = mask?;
    let width = mask.width() as usize;
    let len = (x1.saturating_sub(x0)) as usize;
    let start = (row as usize)
        .checked_mul(width)
        .and_then(|row| row.checked_add(x0 as usize));
    let slice = start.and_then(|start| {
        // The end has to be inside *this row* of the mask as well as inside its
        // buffer: a mask narrower than the target would otherwise let a span
        // read the beginning of the next row as though it were the end of this
        // one.
        ((x1 as usize) <= width)
            .then_some(())
            .and_then(|()| mask.data().get(start..start.checked_add(len)?))
    });
    Some(slice.map_or_else(
        || std::borrow::Cow::Owned(vec![0_u8; len]),
        std::borrow::Cow::Borrowed,
    ))
}

/// The colour a span composites, in the alpha convention it arrived in.
///
/// The distinction is not cosmetic and it is not a micro-optimisation: a
/// premultiplied byte at alpha `a` can express only `a + 1` of the 256
/// straight channel values, so storing a straight colour premultiplied and
/// reading it back **quantises it**. Every solid brush already has its
/// straight colour in hand — the engine hands the device a `peniko::Color` —
/// and the oracle's own AGG targets are straight-alpha `kBgra`, so premultiplying
/// on the way in was a loss with nothing on the other side of it.
///
/// [`Source::Premultiplied`] is for a source that has no straight form left to
/// preserve: an image texel, or a layer's own pixels being composited back.
#[derive(Debug, Clone, Copy)]
pub enum Source {
    /// A straight RGB triple plus its alpha — a solid brush.
    Straight([u8; 3], u8),
    /// A premultiplied RGBA pixel — an image sample or a layer's pixel.
    Premultiplied([u8; 4]),
}

/// Composite one source pixel into a four-byte destination slot.
///
/// A free function over `&mut [u8]` rather than a method taking `(x, y)`,
/// because the span loops above already hold the destination row as a slice
/// and re-deriving an index from coordinates inside the loop cost measurably
/// more. It is a no-op on a slot that is not exactly four bytes, which
/// `chunks_exact_mut(4)` guarantees it always is — the check is there because
/// `unsafe_code = "forbid"` means the alternative is an index that could
/// panic, and a rasterizer must not panic on a crafted file.
///
/// The blend arithmetic is `pdfrum-render`'s, not this crate's: a pixel this
/// backend composites and a pixel the engine composites in its own offscreen
/// buffers must agree exactly, and one authority is how that is guaranteed
/// rather than hoped for.
fn blend_into(dest: &mut [u8], src: Source, coverage: u8, mode: BlendMode) {
    let (Some(&r), Some(&g), Some(&b), Some(&a)) =
        (dest.first(), dest.get(1), dest.get(2), dest.get(3))
    else {
        return;
    };
    let out = match src {
        Source::Straight(rgb, alpha) => {
            blend::composite_solid([r, g, b, a], rgb, alpha, coverage, mode)
        }
        Source::Premultiplied(px) => {
            blend::composite_premultiplied([r, g, b, a], px, coverage, mode)
        }
    };
    if let Some(slot) = dest.get_mut(..4) {
        slot.copy_from_slice(&out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The spelling [`Target::blit_image`] replaced: one
    /// [`Target::blend_span_with`] per image row, the source pixel recovered
    /// from the destination column. Kept in the tests as the *specification*
    /// the fast one is checked against, exactly as [`Target::clip_at`] is kept
    /// for [`clip_span`]. Deleting it would leave the blit with nothing to be
    /// compared with.
    fn blit_by_spans(target: &mut Target, img: &Pixmap, dx: i32, dy: i32, alpha: u8) {
        for row in 0..img.height() {
            let Ok(row_i32) = i32::try_from(row) else {
                continue;
            };
            let Some(y) = row_i32.checked_add(dy) else {
                continue;
            };
            let Ok(width) = i32::try_from(img.width()) else {
                continue;
            };
            target.blend_span_with(dx, width, y, 255, BlendMode::Normal, |col, _| {
                let src_col = u32::try_from(i64::from(col) - i64::from(dx)).ok()?;
                img.pixel(src_col, row).map(|px| scale_alpha(px, alpha))
            });
        }
    }

    /// A deterministic pseudo-random image, so the comparison below runs over
    /// real colours and real alphas rather than over one flat value.
    fn noisy(w: u32, h: u32, seed: u64) -> Pixmap {
        let mut p = Pixmap::new(w, h);
        let mut state = seed | 1;
        for y in 0..h {
            for x in 0..w {
                let mut next = || {
                    state ^= state << 13;
                    state ^= state >> 7;
                    state ^= state << 17;
                    u8::try_from(state & 0xff).unwrap_or(0)
                };
                let a = next();
                // Premultiplied, so no channel may exceed the alpha.
                let px = [
                    pixmap::mul255(next(), a),
                    pixmap::mul255(next(), a),
                    pixmap::mul255(next(), a),
                    a,
                ];
                p.set_pixel(x, y, px);
            }
        }
        p
    }

    /// The fast blit and the span loop it replaced agree **byte for byte**, on
    /// every offset that puts the image off each edge and each corner of the
    /// target, clipped and unclipped.
    ///
    /// This is the test the whole rewrite rests on: the hoist is only sound if
    /// the column clamp, the source offset, the row band and the clip lookup
    /// are the same answers the per-row derivation gave, and the way to know
    /// that is to run both and compare the pixels.
    #[test]
    fn a_blit_matches_the_span_loop_it_replaced() {
        let img = noisy(5, 4, 0x9E37_79B9);
        for clip in [None, Some(0), Some(1)] {
            for dy in -6_i32..=8 {
                for dx in -6_i32..=10 {
                    for alpha in [255_u8, 128, 0] {
                        let seed = |t: &mut Target| {
                            if let Some(kind) = clip {
                                let mut mask = AlphaMask::filled(8, 6, 200);
                                if kind == 1 {
                                    // A ragged clip, so a row's bytes differ.
                                    for (i, b) in mask.data_mut().iter_mut().enumerate() {
                                        *b = u8::try_from((i * 37) % 256).unwrap_or(0);
                                    }
                                }
                                t.set_clip(Some(Arc::new(mask)));
                            }
                        };
                        let mut fast = Target::new(8, 6, peniko::Color::WHITE);
                        seed(&mut fast);
                        fast.blit_image(&img, dx, dy, alpha);

                        let mut slow = Target::new(8, 6, peniko::Color::WHITE);
                        seed(&mut slow);
                        blit_by_spans(&mut slow, &img, dx, dy, alpha);

                        assert_eq!(
                            fast.pixels().data(),
                            slow.pixels().data(),
                            "dx={dx} dy={dy} alpha={alpha} clip={clip:?}"
                        );
                    }
                }
            }
        }
    }

    /// Every blend mode, so a test over "all of them" is one list rather than
    /// a list per test that could drift from the enum.
    const ALL_BLEND_MODES: [BlendMode; 17] = [
        BlendMode::Normal,
        BlendMode::Compatible,
        BlendMode::Multiply,
        BlendMode::Screen,
        BlendMode::Overlay,
        BlendMode::Darken,
        BlendMode::Lighten,
        BlendMode::ColorDodge,
        BlendMode::ColorBurn,
        BlendMode::HardLight,
        BlendMode::SoftLight,
        BlendMode::Difference,
        BlendMode::Exclusion,
        BlendMode::Hue,
        BlendMode::Saturation,
        BlendMode::Color,
        BlendMode::Luminosity,
    ];

    /// The spelling [`Target::composite_layer`] replaced: one
    /// [`Target::blend_span`] per **pixel**, with the target's clip saved,
    /// cleared and restored around the walk. Kept in the tests as the
    /// *specification* the fast one is checked against, exactly as
    /// [`blit_by_spans`] is kept for [`Target::blit_image`].
    fn composite_layer_by_pixels(target: &mut Target, layer: &Pixmap, mode: BlendMode) {
        let (w, h) = (layer.width(), layer.height());
        let saved = target.clip().map(Arc::clone);
        target.set_clip(None);
        for y in 0..h {
            for x in 0..w {
                let Some(src) = layer.pixel(x, y) else {
                    continue;
                };
                if src[3] == 0 {
                    continue;
                }
                let (Ok(col), Ok(row)) = (i32::try_from(x), i32::try_from(y)) else {
                    continue;
                };
                target.blend_span(col, 1, row, 255, Source::Premultiplied(src), mode);
            }
        }
        target.set_clip(saved);
    }

    /// Compositing a layer row-at-a-time is byte-for-byte the per-pixel walk it
    /// replaced, on every blend mode.
    ///
    /// The source is deliberately mixed — `noisy` gives it transparent, partial
    /// and opaque pixels — because the transparent skip and the coverage are the
    /// two things the hoist could have got wrong, and a flat source would hide
    /// both. Every mode is run rather than `Normal` alone: the separable modes'
    /// arithmetic is what the skipped pixel is skipped *for*, so a mode that
    /// composited a zero source differently would be caught here and nowhere
    /// else.
    ///
    /// A clip is set on both arms and must change neither: the layer already
    /// carries the clip it was pushed under, and the composite is unclipped by
    /// construction. A `composite_layer` that read `self.clip` would fail this.
    #[test]
    fn a_layer_composite_matches_the_span_loop_it_replaced() {
        let layer = noisy(9, 7, 0x0123_4567);
        for mode in ALL_BLEND_MODES {
            for clip in [false, true] {
                let seed = |t: &mut Target| {
                    if clip {
                        let mut mask = AlphaMask::filled(9, 7, 255);
                        for (i, b) in mask.data_mut().iter_mut().enumerate() {
                            *b = u8::try_from((i * 37) % 256).unwrap_or(0);
                        }
                        t.set_clip(Some(Arc::new(mask)));
                    }
                };
                let mut fast = Target::new(9, 7, peniko::Color::WHITE);
                seed(&mut fast);
                fast.composite_layer(&layer, mode);

                let mut slow = Target::new(9, 7, peniko::Color::WHITE);
                seed(&mut slow);
                composite_layer_by_pixels(&mut slow, &layer, mode);

                assert_eq!(
                    fast.pixels().data(),
                    slow.pixels().data(),
                    "mode={mode:?} clip={clip}"
                );
            }
        }
    }

    /// A zero-alpha source pixel is the identity, under every blend mode and
    /// over every destination — so [`Target::composite_layer`]'s skip of it is
    /// an optimisation and cannot be a behaviour.
    ///
    /// Checked rather than argued, because the argument ("the composite weights
    /// by the source alpha") is about `blend::composite_premultiplied`'s
    /// internals and this is the property the skip actually rests on. Every
    /// mode against a destination sweeping alpha and each channel, which is
    /// what makes it a statement about the mode set rather than about one
    /// pixel: a mode whose zero-source arithmetic escaped the alpha weighting
    /// would fail here, and the skip would then be a divergence rather than a
    /// hoist.
    #[test]
    fn a_transparent_source_pixel_is_the_identity_under_every_mode() {
        for mode in ALL_BLEND_MODES {
            for a in [0_u8, 1, 63, 128, 254, 255] {
                for c in [0_u8, 1, 127, 254] {
                    // Premultiplied, so no channel may exceed the alpha.
                    let dest = [c.min(a), a.saturating_sub(c).min(a), c.min(a), a];
                    let out = blend::composite_premultiplied(dest, [0, 0, 0, 0], 255, mode);
                    assert_eq!(out, dest, "mode={mode:?} dest={dest:?}");
                }
            }
        }
    }

    /// A clip **narrower than the target** clips the blit away entirely rather
    /// than reading the next mask row as though it were this one.
    ///
    /// `clip_span` answered this per row with a zero-filled `Owned` slice;
    /// `blit_image` decides it once, before the row walk, because `x1` does not
    /// vary with the row. A version that dropped the check would paint the
    /// wrong mask bytes; one that mistook it for "unclipped" would paint an
    /// unclipped blit, which is the one way this could have changed a pixel.
    #[test]
    fn a_clip_narrower_than_the_blit_paints_nothing() {
        let img = noisy(6, 2, 7);
        let mut t = Target::new(8, 4, peniko::Color::WHITE);
        t.set_clip(Some(Arc::new(AlphaMask::filled(4, 4, 255))));
        t.blit_image(&img, 0, 0, 255);
        assert!(
            t.pixels().data().iter().all(|&b| b == 255),
            "a clip that does not reach the span's last column paints nothing"
        );
    }

    /// Every row inside the band is painted, and the row's source is *its own*
    /// image row.
    ///
    /// The mutation this exists for is an off-by-one in the band or a row walk
    /// that reads a fixed source row: both leave a plausible-looking image, and
    /// only a per-row-distinct source catches them. Each image row here is a
    /// different solid colour, so a skipped, duplicated or shifted row is a
    /// visible mismatch at a named coordinate.
    #[test]
    fn each_row_of_a_blit_lands_on_its_own_row() {
        let mut img = Pixmap::new(3, 5);
        for y in 0..5_u32 {
            let v = u8::try_from(y + 1).unwrap_or(1) * 40;
            for x in 0..3_u32 {
                img.set_pixel(x, y, [v, v, v, 255]);
            }
        }
        let mut t = Target::new(6, 9, peniko::Color::TRANSPARENT);
        t.blit_image(&img, 2, 3, 255);
        for y in 0..5_u32 {
            let v = u8::try_from(y + 1).unwrap_or(1) * 40;
            for x in 0..3_u32 {
                assert_eq!(
                    t.pixels().pixel(x + 2, y + 3),
                    Some([v, v, v, 255]),
                    "row {y} of the image belongs at device row {}",
                    y + 3
                );
            }
        }
        // And nothing outside the footprint was touched.
        assert_eq!(t.pixels().pixel(1, 3), Some([0, 0, 0, 0]));
        assert_eq!(t.pixels().pixel(5, 3), Some([0, 0, 0, 0]));
        assert_eq!(t.pixels().pixel(2, 2), Some([0, 0, 0, 0]));
        assert_eq!(t.pixels().pixel(2, 8), Some([0, 0, 0, 0]));
    }

    fn opaque(r: u8, g: u8, b: u8) -> Source {
        Source::Straight([r, g, b], 255)
    }

    /// A mask whose bytes are all distinct modulo 251, so a slice taken from
    /// the wrong offset cannot coincidentally match the right one.
    fn awkward_mask(width: u32, height: u32) -> AlphaMask {
        let mut mask = AlphaMask::new(width, height);
        for (i, slot) in mask.data_mut().iter_mut().enumerate() {
            #[expect(clippy::cast_possible_truncation, reason = "the modulus is 251")]
            let byte = (i % 251) as u8;
            *slot = byte;
        }
        mask
    }

    #[test]
    fn clip_span_reproduces_clip_at_exactly() {
        // The M12 hoist replaced a per-pixel `clip_at` with a per-span slice.
        // That is only sound if the slice carries the same bytes the calls
        // would have returned, at every position — including the positions
        // where `clip_at` returns its out-of-range zero. Rather than reason
        // about the indexing, this walks it: for a target with a clip, every
        // row and every span within it, the two must agree.
        let mut target = Target::new(7, 5, peniko::Color::TRANSPARENT);
        target.set_clip(Some(Arc::new(awkward_mask(7, 5))));
        for row in 0..5 {
            for x0 in 0..7 {
                for x1 in (x0 + 1)..=7 {
                    let span = clip_span(target.clip().map(Arc::as_ref), x0, x1, row)
                        .expect("a clip is set, so this is Some");
                    for (i, &byte) in span.iter().enumerate() {
                        #[expect(clippy::cast_possible_truncation, reason = "i < 7")]
                        let col = x0 + i as u32;
                        assert_eq!(
                            byte,
                            target.clip_at(col, row),
                            "row={row} x0={x0} x1={x1} col={col}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn a_clip_narrower_than_the_target_reads_zero_past_its_edge() {
        // A mask narrower than the target, which is the one shape where the
        // span form and the per-pixel form could disagree — and, it turns out,
        // the one shape where the *per-pixel* form was wrong.
        //
        // `clip_at` computes `y * mask.width() + x` and reads it out of a flat
        // buffer. At `x >= mask.width()` that index is still inside the buffer:
        // it lands on the **next row** of the mask. So `clip_at(4, 0)` on a
        // 4-wide mask returns row 1's first byte rather than "outside the
        // clip", and a span running past the mask's right edge would have been
        // painted through whatever the row below happened to hold. `clip_span`
        // refuses the slice in that case and answers all-zero — outside the
        // clip is not painted.
        //
        // This is a latent bug the M12 restructuring removes rather than a
        // behaviour change with a visible effect: the engine sizes every clip
        // mask to the device (`a_layer_mask_must_be_device_sized` enforces it
        // on the layer path), so no mask reaching a real render is narrower
        // than its target and no corpus pixel moves. It is pinned here because
        // the invariant is now load-bearing for the fast path, and a future
        // change that relaxes the sizing rule must fail this test rather than
        // silently reintroduce the read.
        let mut target = Target::new(8, 3, peniko::Color::TRANSPARENT);
        target.set_clip(Some(Arc::new(awkward_mask(4, 3))));

        let span = clip_span(target.clip().map(Arc::as_ref), 0, 8, 0).expect("a clip is set");
        assert_eq!(span.len(), 8);
        assert!(span.iter().all(|&b| b == 0), "{span:?}");

        // The old per-pixel spelling, for the record: in range it agrees, and
        // past the mask's edge it reads the next row instead of zero.
        for col in 0..4 {
            #[expect(clippy::cast_possible_truncation, reason = "col < 4")]
            let expected = col as u8;
            assert_eq!(target.clip_at(col, 0), expected);
        }
        assert_eq!(target.clip_at(4, 0), 4, "this is row 1's first byte");

        // Painting through the narrow clip leaves the target alone.
        target.blend_span(0, 8, 0, 255, opaque(255, 0, 0), BlendMode::Normal);
        for col in 0..8 {
            assert_eq!(
                target.pixels().pixel(col, 0),
                Some([0, 0, 0, 0]),
                "col={col}"
            );
        }
    }

    #[test]
    fn an_absent_clip_is_full_coverage_not_an_empty_span() {
        // `None` from `clip_span` means "unclipped", and the two callers turn
        // it into full coverage. Confusing it with "clipped to nothing" would
        // make every unclipped draw a no-op, so it is worth one assertion.
        let target = Target::new(4, 1, peniko::Color::TRANSPARENT);
        assert!(clip_span(target.clip().map(Arc::as_ref), 0, 4, 0).is_none());
    }

    #[test]
    fn a_span_paints_its_columns_and_no_others() {
        let mut t = Target::new(8, 1, peniko::Color::TRANSPARENT);
        t.blend_span(2, 3, 0, 255, opaque(255, 0, 0), BlendMode::Normal);
        assert_eq!(t.pixels().pixel(1, 0).map(|p| p[3]), Some(0));
        assert_eq!(t.pixels().pixel(2, 0), Some([255, 0, 0, 255]));
        assert_eq!(t.pixels().pixel(4, 0), Some([255, 0, 0, 255]));
        assert_eq!(t.pixels().pixel(5, 0).map(|p| p[3]), Some(0));
    }

    #[test]
    fn a_span_running_off_both_edges_is_clipped_not_wrapped() {
        let mut t = Target::new(4, 2, peniko::Color::TRANSPARENT);
        t.blend_span(-10, 100, 1, 255, opaque(0, 255, 0), BlendMode::Normal);
        for col in 0..4 {
            assert_eq!(t.pixels().pixel(col, 1).map(|p| p[3]), Some(255));
            assert_eq!(
                t.pixels().pixel(col, 0).map(|p| p[3]),
                Some(0),
                "row 0 untouched"
            );
        }
    }

    #[test]
    fn a_span_off_the_target_paints_nothing() {
        let mut t = Target::new(4, 4, peniko::Color::TRANSPARENT);
        t.blend_span(0, 4, 9, 255, opaque(255, 0, 0), BlendMode::Normal);
        t.blend_span(0, 4, -1, 255, opaque(255, 0, 0), BlendMode::Normal);
        t.blend_span(10, 4, 0, 255, opaque(255, 0, 0), BlendMode::Normal);
        assert!(t.pixels().data().iter().all(|&b| b == 0));
    }

    #[test]
    fn the_clip_multiplies_coverage_truncating() {
        // `CFX_AggClipRgn::IntersectMask` is `a * b / 255`: a half clip over a
        // half coverage is 64, not 63 or 65.
        let mut t = Target::new(1, 1, peniko::Color::TRANSPARENT);
        t.set_clip(Some(Arc::new(AlphaMask::filled(1, 1, 128))));
        t.blend_span(0, 1, 0, 128, opaque(255, 255, 255), BlendMode::Normal);
        assert_eq!(t.pixels().pixel(0, 0).map(|p| p[3]), Some(64));
    }

    #[test]
    fn a_zero_clip_paints_nothing() {
        let mut t = Target::new(2, 2, peniko::Color::WHITE);
        let before = t.pixels().clone();
        t.set_clip(Some(Arc::new(AlphaMask::new(2, 2))));
        t.blend_span(0, 2, 0, 255, opaque(255, 0, 0), BlendMode::Normal);
        assert_eq!(t.pixels(), &before);
    }

    #[test]
    fn an_lcd_pixel_merges_each_stripe_into_its_own_channel() {
        // Black text on white with the three stripes fully, half and not
        // covered: `MergeGammaAdjustRgb` gives each channel its own alpha, so
        // the pixel comes out (0, 127, 255) — a colour fringe from a colour
        // that has none.
        let mut t = Target::new(1, 1, peniko::Color::WHITE);
        t.merge_lcd_pixel(0, 0, [0, 0, 0], 255, [255, 128, 0]);
        // R: merge(255, 0, 255) = 0. G: merge(255, 0, 128) = 127.
        // B: alpha 0, so the channel is left alone at 255.
        assert_eq!(t.pixels().pixel(0, 0), Some([0, 127, 255, 255]));
    }

    #[test]
    fn an_lcd_pixel_is_left_opaque_however_little_it_covered() {
        // `SetAlpha` writes 255 unconditionally: three alphas have no
        // single-alpha expression, so the oracle resolves them into the colour
        // and calls the pixel solid.
        let mut t = Target::new(1, 1, peniko::Color::WHITE);
        t.merge_lcd_pixel(0, 0, [0, 0, 0], 255, [1, 0, 0]);
        assert_eq!(t.pixels().pixel(0, 0).map(|p| p[3]), Some(255));
    }

    #[test]
    fn an_lcd_pixel_with_no_coverage_anywhere_leaves_the_pixel_alone() {
        let mut t = Target::new(1, 1, peniko::Color::WHITE);
        t.merge_lcd_pixel(0, 0, [0, 0, 0], 255, [0, 0, 0]);
        // Every channel's alpha is zero, so nothing is merged — and the
        // opacity write is the only thing that runs, on an already-opaque
        // pixel.
        assert_eq!(t.pixels().pixel(0, 0), Some([255, 255, 255, 255]));
    }

    #[test]
    fn an_lcd_pixel_folds_the_clip_into_every_channels_alpha() {
        // The clip is a coverage, multiplied in by the same truncating product
        // every other primitive uses. At clip 128 a fully covered black stripe
        // gives alpha `255*255/255 * 128/255 = 128`, and `merge(255, 0, 128)`
        // is `255*127/255 = 127` — the destination keeps rather than loses the
        // odd count, which is the truncating merge's own asymmetry.
        let mut t = Target::new(1, 1, peniko::Color::WHITE);
        t.set_clip(Some(Arc::new(AlphaMask::filled(1, 1, 128))));
        t.merge_lcd_pixel(0, 0, [0, 0, 0], 255, [255, 255, 255]);
        let px = t.pixels().pixel(0, 0).expect("a pixel");
        assert_eq!([px[0], px[1], px[2]], [127, 127, 127]);
        // A fully clipped-out pixel is untouched, alpha included.
        let mut t = Target::new(1, 1, peniko::Color::WHITE);
        t.set_clip(Some(Arc::new(AlphaMask::filled(1, 1, 0))));
        t.merge_lcd_pixel(0, 0, [0, 0, 0], 255, [255, 255, 255]);
        assert_eq!(t.pixels().pixel(0, 0), Some([255, 255, 255, 255]));
    }

    #[test]
    fn an_lcd_pixel_outside_the_target_is_a_no_op() {
        // A rasterizer must not panic on a crafted file, and a glyph's box can
        // run off any edge.
        let mut t = Target::new(2, 2, peniko::Color::WHITE);
        for (x, y) in [(-1, 0), (0, -1), (2, 0), (0, 2), (99, 99)] {
            t.merge_lcd_pixel(x, y, [0, 0, 0], 255, [255, 255, 255]);
        }
        for y in 0..2 {
            for x in 0..2 {
                assert_eq!(t.pixels().pixel(x, y), Some([255, 255, 255, 255]));
            }
        }
    }

    #[test]
    fn a_translucent_text_colour_scales_every_stripe() {
        // `CalcAlpha(gamma, bgra.alpha)` is applied per stripe, so the colour's
        // own alpha and the stripe's coverage compose exactly once each.
        let mut t = Target::new(1, 1, peniko::Color::WHITE);
        t.merge_lcd_pixel(0, 0, [0, 0, 0], 128, [255, 255, 255]);
        let px = t.pixels().pixel(0, 0).expect("a pixel");
        // 255*128/255 = 128, then merge(255, 0, 128) = 127.
        assert_eq!([px[0], px[1], px[2]], [127, 127, 127]);
    }

    #[test]
    fn a_varying_span_skips_where_the_source_has_nothing() {
        let mut t = Target::new(4, 1, peniko::Color::TRANSPARENT);
        t.blend_span_with(0, 4, 0, 255, BlendMode::Normal, |x, _| {
            (x % 2 == 0).then_some([0, 0, 255, 255])
        });
        assert_eq!(t.pixels().pixel(0, 0).map(|p| p[3]), Some(255));
        assert_eq!(t.pixels().pixel(1, 0).map(|p| p[3]), Some(0));
        assert_eq!(t.pixels().pixel(2, 0).map(|p| p[3]), Some(255));
    }
}
