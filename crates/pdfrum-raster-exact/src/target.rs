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
        for col in x0..x1 {
            let cov = pixmap::mul255(coverage, self.clip_at(col, row));
            if cov == 0 {
                continue;
            }
            self.blend_pixel(col, row, src, cov, mode);
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
        for col in x0..x1 {
            let cov = pixmap::mul255(coverage, self.clip_at(col, row));
            if cov == 0 {
                continue;
            }
            let Some(src) = sample(col, row) else {
                continue;
            };
            self.blend_pixel(col, row, src, cov, mode);
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

    /// Composite one premultiplied source pixel at full generality.
    fn blend_pixel(&mut self, x: u32, y: u32, src: [u8; 4], coverage: u8, mode: BlendMode) {
        let Some(dest) = self.pixels.pixel(x, y) else {
            return;
        };
        // The blend arithmetic is `pdfrum-render`'s, not this crate's: a pixel
        // this backend composites and a pixel the engine composites in its own
        // offscreen buffers must agree exactly, and one authority is how that
        // is guaranteed rather than hoped for.
        let out = blend::composite_premultiplied(dest, src, coverage, mode);
        self.pixels.set_pixel(x, y, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opaque(r: u8, g: u8, b: u8) -> [u8; 4] {
        [r, g, b, 255]
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
