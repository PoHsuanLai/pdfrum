//! The pixel buffer a device draws into, and the two things that modulate a
//! draw: the clip stack and the span blitter.
//!
//! A [`Target`] is a premultiplied RGBA8 buffer plus the clip in force. Every
//! primitive reaches it as *spans* — a row, a start column, a length and a
//! coverage byte — because that is what the analytic rasterizer produces and
//! because it lets the clip apply as one multiply per pixel rather than as a
//! second rasterization pass.

use pdfrum_page::BlendMode;
use pdfrum_render::{AlphaMask, Pixmap, blend, pixmap};

/// A premultiplied RGBA8 render target with a clip.
#[derive(Debug, Clone)]
pub struct Target {
    pixels: Pixmap,
    /// The clip in force, or `None` for "everything is visible".
    ///
    /// A clip is a coverage plane the size of the target, and intersecting two
    /// is `a * b / 255` — the same truncating product `CFX_AggClipRgn` uses,
    /// which is what makes a clipped edge here land where the oracle's does.
    clip: Option<AlphaMask>,
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
    #[must_use]
    pub fn clip(&self) -> Option<&AlphaMask> {
        self.clip.as_ref()
    }

    /// Replace the clip.
    pub fn set_clip(&mut self, clip: Option<AlphaMask>) {
        self.clip = clip;
    }

    /// The clip's coverage at a pixel: `255` where there is no clip.
    ///
    /// **Not on the hot path any more.** M12 replaced the per-pixel call with
    /// [`clip_span`], which takes the whole span's bytes at once. This is kept
    /// as that function's *specification*: it is the simple, obviously correct
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
        src: [u8; 4],
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
        let clip = clip_span(clip.as_ref(), x0, x1, row);
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
        let clip = clip_span(clip.as_ref(), x0, x1, row);
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
            blend_into(dest, src, cov, mode);
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

/// Composite one premultiplied source pixel into a four-byte destination slot.
///
/// A free function over `&mut [u8]` rather than a method taking `(x, y)`,
/// because the span loops above already hold the destination row as a slice
/// and re-deriving an index from coordinates inside the loop was the thing
/// M12 removed. It is a no-op on a slot that is not exactly four bytes, which
/// `chunks_exact_mut(4)` guarantees it always is — the check is there because
/// `unsafe_code = "forbid"` means the alternative is an index that could
/// panic, and a rasterizer must not panic on a crafted file.
///
/// The blend arithmetic is `pdfrum-render`'s, not this crate's: a pixel this
/// backend composites and a pixel the engine composites in its own offscreen
/// buffers must agree exactly, and one authority is how that is guaranteed
/// rather than hoped for.
fn blend_into(dest: &mut [u8], src: [u8; 4], coverage: u8, mode: BlendMode) {
    let (Some(&r), Some(&g), Some(&b), Some(&a)) =
        (dest.first(), dest.get(1), dest.get(2), dest.get(3))
    else {
        return;
    };
    let out = blend::composite_premultiplied([r, g, b, a], src, coverage, mode);
    if let Some(slot) = dest.get_mut(..4) {
        slot.copy_from_slice(&out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opaque(r: u8, g: u8, b: u8) -> [u8; 4] {
        [r, g, b, 255]
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
        target.set_clip(Some(awkward_mask(7, 5)));
        for row in 0..5 {
            for x0 in 0..7 {
                for x1 in (x0 + 1)..=7 {
                    let span = clip_span(target.clip(), x0, x1, row)
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
        target.set_clip(Some(awkward_mask(4, 3)));

        let span = clip_span(target.clip(), 0, 8, 0).expect("a clip is set");
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
        assert!(clip_span(target.clip(), 0, 4, 0).is_none());
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
        t.set_clip(Some(AlphaMask::filled(1, 1, 128)));
        t.blend_span(0, 1, 0, 128, opaque(255, 255, 255), BlendMode::Normal);
        assert_eq!(t.pixels().pixel(0, 0).map(|p| p[3]), Some(64));
    }

    #[test]
    fn a_zero_clip_paints_nothing() {
        let mut t = Target::new(2, 2, peniko::Color::WHITE);
        let before = t.pixels().clone();
        t.set_clip(Some(AlphaMask::new(2, 2)));
        t.blend_span(0, 2, 0, 255, opaque(255, 0, 0), BlendMode::Normal);
        assert_eq!(t.pixels(), &before);
    }

    #[test]
    fn a_varying_span_skips_where_the_source_has_nothing() {
        let mut t = Target::new(4, 1, peniko::Color::TRANSPARENT);
        t.blend_span_with(0, 4, 0, 255, BlendMode::Normal, |x, _| {
            (x % 2 == 0).then_some(opaque(0, 0, 255))
        });
        assert_eq!(t.pixels().pixel(0, 0).map(|p| p[3]), Some(255));
        assert_eq!(t.pixels().pixel(1, 0).map(|p| p[3]), Some(0));
        assert_eq!(t.pixels().pixel(2, 0).map(|p| p[3]), Some(255));
    }
}
