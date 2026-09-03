//! Glyphs as alpha bitmaps, rendered the way the oracle's FreeType renders
//! them.
//!
//! **Part of the backend seam,** for exactly one type: [`SubpixelBitmap`] is
//! what [`RenderDevice::draw_glyph_lcd`](crate::RenderDevice::draw_glyph_lcd)
//! hands a backend, so the trait cannot be implemented without naming it.
//! Everything else here — the gray rasterizer, the LCD filter, the session's
//! bitmap cache — is the engine's own and is private.
//!
//! Not a general glyph rasterizer: above a size threshold the outline is
//! filled instead, and `RenderOptions::subpixel_text_positioning` is how a
//! caller asks for true fractional placement at every size.

// The oracle asks FreeType for a **bitmap** below
// `|char2device.a| + |char2device.b| > 50` and blits it rather than filling
// the outline (`CFX_Face::RenderGlyph` -> `FT_Render_Glyph` ->
// `DrawNormalTextHelper`). Four stages of producing that bitmap each move
// pixels by more than the coverage integral that would otherwise decide
// them, so none can be dropped or applied in isolation:
//
// 1. The outline is grid-fitted at a pinned 64 ppem, never at the size it is
//    drawn at: `FT_Set_Pixel_Sizes(rec, 64, 64)` runs once at face
//    construction and the real size arrives afterwards through
//    `FT_Set_Transform`, which FreeType applies *after* hinting. On a 6 pt
//    stem that is worth up to 10 counts per pixel, because a 3x-wide grid
//    resolves a third of the horizontal displacement an ordinary one does.
// 2. It is rasterized three times as wide: `FT_RENDER_MODE_LCD` multiplies
//    every x coordinate by 3, and the bitmap is padded by 43/64 of a
//    subpixel on each side so the filter's tails have somewhere to land.
// 3. Every span is spread across five subpixel columns by the FIR5 weights
//    `{8, 77, 86, 77, 8}`, summing to 256, applied as
//    `(coverage * w + 85) >> 8` and *accumulated*. This is the stage that
//    puts ink in columns an outline fill leaves white.
// 4. The triples are averaged back to gray through `kTextGammaAdjust`:
//    `(r + g + b) / 3`, then a table lookup.
//
// The gamma table's input is the average of three FIR5-filtered subpixel
// coverages, not a pixel's coverage, and the FIR5 filter's input is a
// 3x-wide rasterization of a *hinted* outline. Applying any one stage to a
// plain outline coverage is not a weaker version of the pipeline; it is a
// different function.

use kurbo::{Affine, BezPath, Shape};

use crate::scanline::{Coverage, FillRule, Rasterizer};

/// FreeType's `FT_LCD_FILTER_DEFAULT` five-tap weights
/// (`ftlcdfil.c`'s `default_weights`).
///
/// They sum to exactly 256, which is what lets the filter be applied as a
/// shift and what bounds the accumulated result at a byte.
pub(crate) const LCD_FIR5: [i32; 5] = [0x08, 0x4d, 0x56, 0x4d, 0x08];

/// The horizontal padding `ft_lcd_padding` adds to an LCD glyph's control box,
/// in 26.6 units — "2/3 of a pixel", as its comment says.
///
/// It is what gives the FIR5 filter's outer taps somewhere to write, and
/// omitting it clips the two columns of ink that are the whole reason the LCD
/// path differs from an outline fill.
pub(crate) const LCD_PADDING_26_6: i64 = 43;

/// The text gamma table, transcribed verbatim.
///
/// Applied to the *average of the three subpixel coverages*, not to a pixel's
/// own coverage. It is not a power curve: close to `x^(1/1.05)` in the middle,
/// pinned at both ends, with 24 repeated values. The transcription is the
/// authority.
pub(crate) const TEXT_GAMMA_ADJUST: [u8; 256] = [
    0, 2, 3, 4, 6, 7, 8, 10, 11, 12, 13, 15, 16, 17, 18, 19, 21, 22, 23, 24, 25, 26, 27, 29, 30,
    31, 32, 33, 34, 35, 36, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 51, 52, 53, 54, 55, 56,
    57, 58, 59, 60, 61, 62, 63, 64, 65, 66, 67, 68, 69, 71, 72, 73, 74, 75, 76, 77, 78, 79, 80, 81,
    82, 83, 84, 85, 86, 87, 88, 89, 90, 91, 92, 93, 94, 95, 96, 97, 98, 99, 100, 101, 102, 103,
    104, 105, 106, 107, 108, 109, 110, 111, 112, 113, 114, 115, 116, 117, 118, 119, 120, 121, 122,
    123, 124, 125, 126, 127, 128, 129, 129, 130, 131, 132, 133, 134, 135, 136, 137, 138, 139, 140,
    141, 142, 143, 144, 145, 146, 147, 148, 149, 150, 151, 152, 153, 154, 155, 156, 156, 157, 158,
    159, 160, 161, 162, 163, 164, 165, 166, 167, 168, 169, 170, 171, 172, 173, 174, 174, 175, 176,
    177, 178, 179, 180, 181, 182, 183, 184, 185, 186, 187, 188, 189, 190, 190, 191, 192, 193, 194,
    195, 196, 197, 198, 199, 200, 201, 202, 203, 204, 204, 205, 206, 207, 208, 209, 210, 211, 212,
    213, 214, 215, 216, 217, 217, 218, 219, 220, 221, 222, 223, 224, 225, 226, 227, 228, 228, 229,
    230, 231, 232, 233, 234, 235, 236, 237, 238, 239, 239, 240, 241, 242, 243, 244, 245, 246, 247,
    248, 249, 250, 250, 251, 252, 253, 254, 255,
];

/// The largest glyph bitmap either axis may reach, in device pixels.
///
/// A glyph past it renders as nothing at all rather than as a huge
/// allocation, which is upstream's answer and not a clamp: `RenderGlyph`
/// returns `nullptr` and `DrawNormalText` skips the glyph.
pub(crate) const MAX_GLYPH_DIMENSION: i32 = 2048;

/// One glyph rasterized to **three** coverages per pixel — one per LCD stripe.
///
/// The same bitmap `GlyphBitmap` holds, with the 3× subpixel triples kept
/// apart instead of averaged. It is produced for exactly one kind of text on
/// a page: a live edit's, which is drawn with `ClearType` on while the rest
/// of the page is not.
///
/// The three bytes are the destination's **red, green and blue** coverages in
/// that order — the oracle assumes RGB-ordered stripes, mapping the leftmost
/// subpixel to red. Each is gamma-adjusted and merged into its own destination
/// channel independently, which is what puts colour on the fringes of a glyph
/// drawn in a single colour.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubpixelBitmap {
    /// The device x of column 0, relative to the glyph's snapped origin.
    pub left: i32,
    /// The device y of row 0, relative to the glyph's snapped origin.
    pub top: i32,
    /// Columns.
    pub width: i32,
    /// Rows.
    pub height: i32,
    /// `height * width * 3` gamma-adjusted coverages, row-major, `[r, g, b]`
    /// per pixel.
    pub channels: Vec<u8>,
}

impl SubpixelBitmap {
    /// The `[r, g, b]` coverages at `(x, y)`, or zeroes outside the bitmap.
    #[must_use]
    pub fn at(&self, x: i32, y: i32) -> [u8; 3] {
        if x < 0 || y < 0 || x >= self.width || y >= self.height {
            return [0; 3];
        }
        let Ok(i) = usize::try_from((y * self.width + x) * 3) else {
            return [0; 3];
        };
        let get = |k: usize| self.channels.get(i + k).copied().unwrap_or(0);
        [get(0), get(1), get(2)]
    }

    /// Whether the bitmap has no pixels at all, which a blank glyph produces.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.width <= 0 || self.height <= 0
    }
}

/// One glyph rasterized to gray coverage, positioned by its top-left corner.
///
/// The coverages are what [`recolour`] multiplies a colour's alpha by; they are
/// not premultiplied and carry no colour of their own, exactly like the
/// oracle's `k8bppMask`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GlyphBitmap {
    /// The device x of column 0, relative to the glyph's snapped origin.
    pub left: i32,
    /// The device y of row 0, relative to the glyph's snapped origin.
    pub top: i32,
    /// Columns.
    pub width: i32,
    /// Rows.
    pub height: i32,
    /// `height * width` coverage bytes, row-major.
    pub coverage: Vec<u8>,
}

impl GlyphBitmap {
    /// The coverage at `(x, y)` in the bitmap's own coordinates, or zero
    /// outside it.
    #[cfg(test)]
    #[must_use]
    pub fn at(&self, x: i32, y: i32) -> u8 {
        if x < 0 || y < 0 || x >= self.width || y >= self.height {
            return 0;
        }
        let Ok(i) = usize::try_from(y * self.width + x) else {
            return 0;
        };
        self.coverage.get(i).copied().unwrap_or(0)
    }
}

/// Which third of a pixel a glyph's true origin sits in.
///
/// The oracle's `origin_.x` is `floor(device_origin.x)` and the fraction is
/// recovered inside the blit loop as `(int)(device_origin.x * 3) % 3`, which
/// shifts `DrawNormalTextHelper`'s sampling window into the 3×-wide bitmap by
/// that many subpixels. Two glyphs at the same integer column and different
/// thirds therefore get *different gray*, which is why the phase is part of the
/// bitmap cache's key rather than something the blit can apply afterwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum SubpixelPhase {
    /// The origin is on the pixel boundary: the window is aligned.
    Zero,
    /// One third of a pixel in.
    One,
    /// Two thirds of a pixel in.
    Two,
}

impl SubpixelPhase {
    /// The phase of a device x, computed the way the C++ computes it.
    ///
    /// `static_cast<int>(x * 3) % 3` truncates toward zero and keeps the sign,
    /// so a negative origin yields a negative remainder; `-1` and `-2` land on
    /// [`Self::Two`] and [`Self::One`] respectively, which is the arm the C++'s
    /// `x_subpixel == 1` / else-branch reaches for them. Spelling it as
    /// `floor(3x) % 3` instead would differ there.
    #[must_use]
    pub fn of(x: f64) -> Self {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "the C++ is `static_cast<int>(x * 3) % 3`; a device origin \
                      beyond i32 has already been clamped by the ±32000 rule"
        )]
        let n = (x * 3.0) as i32 % 3;
        match n {
            1 | -2 => Self::One,
            2 | -1 => Self::Two,
            _ => Self::Zero,
        }
    }

    /// How many subpixels the sampling window shifts left, 0..=2.
    #[must_use]
    pub fn shift(self) -> usize {
        match self {
            Self::Zero => 0,
            Self::One => 1,
            Self::Two => 2,
        }
    }
}

/// Rasterize one glyph outline into an alpha bitmap the oracle's way, at one
/// subpixel phase.
///
/// `outline` is in **device pixels**, already positioned so that the glyph's
/// origin is at `(0, 0)`: the `crate::text::snap_origin` snap is applied by
/// translating the *bitmap* rather than the outline, which is what lets one
/// bitmap serve every glyph of the same shape wherever it lands.
///
/// A caller drawing more than one glyph should build the [`LcdBitmap`] once
/// with [`render_lcd`] and call [`LcdBitmap::to_gray`] per phase — that is the
/// split the oracle's own cache makes, and it is why its key has no phase in
/// it. This function is the one-shot spelling.
///
/// Returns `None` when the glyph would exceed [`MAX_GLYPH_DIMENSION`], which is
/// what `RenderGlyph` does, or when it has no area at all.
#[cfg(test)]
#[must_use]
pub(crate) fn rasterize(outline: &BezPath, phase: SubpixelPhase) -> Option<GlyphBitmap> {
    Some(render_lcd(outline)?.to_gray(phase))
}

/// The 3×-wide LCD coverage bitmap FreeType's `FT_RENDER_MODE_LCD` produces.
///
/// This is what is cached, and the reason the cache key carries no subpixel
/// phase: the phase is a *window shift into this buffer*, applied by the blit
/// loop, so one bitmap serves all three thirds of a pixel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LcdBitmap {
    /// The device x of column 0, relative to the glyph's snapped origin.
    pub left: i32,
    /// The device y of row 0, relative to the glyph's snapped origin.
    pub top: i32,
    /// Whole *pixels* across; [`Self::subpixels`] is three times this wide.
    pub width: i32,
    /// Rows.
    pub height: i32,
    /// `height * width * 3` subpixel coverages, row-major.
    pub subpixels: Vec<u8>,
}

impl LcdBitmap {
    /// Roughly how many bytes this occupies, for the cache's budget.
    #[must_use]
    pub fn byte_size(&self) -> usize {
        self.subpixels.len() + std::mem::size_of::<Self>()
    }
}

/// The gamma-adjusted coverage for a window's tap sum.
///
/// Three bytes summed and divided by three is a byte, so the table index needs
/// neither a clamp nor a fallible narrowing — which is what the per-subpixel
/// spelling paid, once per pixel, for a value that cannot leave `0..=255`.
fn gamma_of_mean(sum: u32) -> u8 {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "at most `3 * 255 / 3`, which is 255"
    )]
    let mean = (sum / 3) as u8;
    TEXT_GAMMA_ADJUST
        .get(usize::from(mean))
        .copied()
        .unwrap_or(0)
}

/// Stages 1–3: pad, implode, rasterize, filter.
///
/// Independent of where the glyph lands and of the colour it will be drawn in,
/// which is exactly the part worth caching.
#[must_use]
pub(crate) fn render_lcd(outline: &BezPath) -> Option<LcdBitmap> {
    let bbox = outline.bounding_box();
    if !bbox.x0.is_finite() || !bbox.y0.is_finite() || !bbox.x1.is_finite() || !bbox.y1.is_finite()
    {
        return None;
    }
    // `ft_glyphslot_preset_bitmap` works in 26.6 and takes the whole-pixel
    // floor of the padded control box's minimum and the ceiling of its
    // maximum (`pbox.min += cbox.min >> 6; pbox.max += (cbox.max + 63) >> 6`,
    // `ftobjs.c`). Only x is padded: the filter is horizontal.
    //
    // Rounding to 26.6 first rather than working in `f64` throughout is not
    // pedantry — FreeType's control box *is* a 26.6 quantity, and a glyph edge
    // a millionth of a pixel past a 64th falls on the other side of the ceiling
    // there and would give a bitmap one column wider here.
    // The rounding is outward on both sides, so a box that is not exactly on a
    // 26.6 step never loses a subpixel of the glyph — which truncating toward
    // zero would do on a negative coordinate and only there, making a glyph
    // left of the origin render differently from the same glyph right of it.
    let quantise = |v: f64, outward: fn(f64) -> f64| -> Option<i64> {
        // The bitmap is bounded by MAX_GLYPH_DIMENSION anyway, but the range
        // check has to happen before the cast rather than after it.
        let steps = outward(v * 64.0);
        (steps.abs() < 1e15).then(|| {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "guarded above: finite, integral, and within i64"
            )]
            let n = steps as i64;
            n
        })
    };
    let low = |v: f64| quantise(v, f64::floor);
    let high = |v: f64| quantise(v, f64::ceil);
    let left = div_floor(low(bbox.x0)? - LCD_PADDING_26_6, 64);
    let right = div_ceil(high(bbox.x1)? + LCD_PADDING_26_6, 64);
    let top = div_floor(low(bbox.y0)?, 64);
    let bottom = div_ceil(high(bbox.y1)?, 64);

    let width = i32::try_from(right - left).ok()?;
    let height = i32::try_from(bottom - top).ok()?;
    if width <= 0 || height <= 0 || width > MAX_GLYPH_DIMENSION || height > MAX_GLYPH_DIMENSION {
        return None;
    }
    let sub_width = width.checked_mul(3)?;
    let cells = usize::try_from(sub_width.checked_mul(height)?).ok()?;
    let mut subpixels = vec![0u8; cells];

    // "Implode": translate the glyph's box to the bitmap's origin, then scale
    // x by three. Both are exact in binary, so the imploded outline is the
    // outline FreeType hands ftgrays and not an approximation of it.
    #[expect(
        clippy::cast_precision_loss,
        reason = "a bitmap origin is bounded by MAX_GLYPH_DIMENSION"
    )]
    let placed = Affine::scale_non_uniform(3.0, 1.0)
        * Affine::translate((-(left as f64), -(top as f64)))
        * outline.clone();

    let mut ras = Rasterizer::new();
    // The bitmap's rows are the only ones written below; the box was derived
    // from this very outline, so nothing outside them is expected — saying so
    // makes the callback's own bound the second of two rather than the only
    // one, and costs a comparison per cell.
    ras.keep_rows(0..height);
    ras.add_path(&placed, FLATTEN_TOLERANCE);
    ras.sweep(FillRule::NonZero, Coverage::Exact, |x, len, y, alpha| {
        if y < 0 || y >= height {
            return;
        }
        let row = y * sub_width;
        for i in 0..len {
            // `ft_smooth_lcd_spans` writes five subpixels starting two to the
            // left of the span's own, accumulating into whatever is there.
            // The accumulation is what makes two adjacent spans' tails add up
            // rather than overwrite, and it is why the filter is applied per
            // span rather than as a post-pass over the finished bitmap.
            for (k, weight) in LCD_FIR5.iter().enumerate() {
                let Ok(k) = i32::try_from(k) else { continue };
                let dx = x + i + k - 2;
                if dx < 0 || dx >= sub_width {
                    continue;
                }
                let Ok(idx) = usize::try_from(row + dx) else {
                    continue;
                };
                // `(coverage * weight + 85) >> 8`, the C's own rounding, whose
                // 85 is a third of 256: it biases each tap up by a third of a
                // count so that five of them recover the coverage rather than
                // losing it to five truncations.
                //
                // The C stores this straight into a `uint8_t` with no clamp,
                // and it does not need one: the largest tap is
                // `(255 · 86 + 85) >> 8 == 85`, and the five sum to at most
                // 255 because the weights sum to 256. `saturating_add` is
                // therefore never reached in practice, and is how a
                // *rounding* difference between the two rasterizers stays a
                // count rather than a wraparound.
                let add =
                    u8::try_from(((i32::from(alpha) * weight + 85) >> 8).max(0)).unwrap_or(u8::MAX);
                if let Some(cell) = subpixels.get_mut(idx) {
                    *cell = cell.saturating_add(add);
                }
            }
        }
    });

    Some(LcdBitmap {
        left: i32::try_from(left).ok()?,
        top: i32::try_from(top).ok()?,
        width,
        height,
        subpixels,
    })
}

/// How finely the glyph outline is flattened, in device pixels **of the
/// 3×-wide grid**.
///
/// A third of what the page rasterizer uses, because the grid it is measured
/// against is three times finer horizontally: keeping the flattening error the
/// same fraction of an output byte is the property that matters, not the
/// absolute number.
const FLATTEN_TOLERANCE: f64 = 0.0333;

impl LcdBitmap {
    /// Stage 4: `DrawNormalTextHelper` with `normalize = true`, at one phase.
    ///
    /// The window shift is the whole of the phase's effect. At phase zero a
    /// pixel's three subpixels are its own; at phase one it borrows one
    /// subpixel from its left neighbour; at phase two, two. The first column
    /// has no left neighbour, and the C++ handles that by averaging fewer terms
    /// *over the same divisor* — `(src[0] + src[1]) / 3` and `src[0] / 3` —
    /// which darkens it rather than brightening it, and is reproduced rather
    /// than corrected.
    #[cfg(test)]
    #[must_use]
    pub fn to_gray(&self, phase: SubpixelPhase) -> GlyphBitmap {
        let mut coverage = Vec::new();
        self.gray_coverage_into(phase, &mut coverage);
        GlyphBitmap {
            left: self.left,
            top: self.top,
            width: self.width,
            height: self.height,
            coverage,
        }
    }

    /// [`Self::to_gray`]'s coverage bytes, into a buffer the caller owns.
    ///
    /// The same arithmetic and the same bytes — this is where the body lives
    /// and [`Self::to_gray`] is this with a fresh `Vec` — split out so that the
    /// glyph blit, which runs once per glyph *occurrence* rather than once per
    /// distinct glyph, reuses one buffer instead of allocating per occurrence.
    /// `out` is cleared and refilled, so nothing carries over from the previous
    /// glyph.
    pub(crate) fn gray_coverage_into(&self, phase: SubpixelPhase, out: &mut Vec<u8>) {
        let shift = phase.shift();
        let (Ok(width), Ok(height)) = (usize::try_from(self.width), usize::try_from(self.height))
        else {
            out.clear();
            return;
        };
        let sub_width = width * 3;
        // A zero-width bitmap has no columns to average and would make both
        // `chunks_exact` below panic on a zero chunk size. It draws nothing;
        // an empty buffer is what the per-subpixel spelling's `0..0` column
        // loop left behind for it.
        if sub_width == 0 {
            out.clear();
            return;
        }
        // Sized to the pixel count, and **every byte is written below**: the
        // row loop covers every row and the column walk every column of it,
        // so the `resize`'s fill value never survives. `subpixels.len() / 3`
        // said the same thing the long way round.
        out.clear();
        out.resize(width * height, 0);

        // **The window's shift, applied to the row rather than to every
        // subpixel index.** Pixel `x` reads subpixels `[3x - shift, 3x -
        // shift + 3)` of its row, and a shift is the same for every pixel of
        // every row — so instead of re-deriving that index three times per
        // pixel and bounds-checking each, the row itself is shifted once:
        // `shift` zero taps are prepended and the row's last `shift`
        // subpixels fall off the end, which is exactly the window walking
        // left.
        //
        // The zeros are what the old spelling's `if idx < row { return 0 }`
        // produced. Only the first pixel of a row could ever reach left of
        // it, and only by fewer than three subpixels — the C++ averages the
        // surviving taps *over the same divisor of three*, which **darkens**
        // that column rather than brightening it, and prepending zeros is
        // that. Clamping the window to the row start instead would brighten
        // it, and `the_first_column_darkens_at_a_shifted_phase` is the test
        // that says which.
        //
        // The row's last `shift` subpixels are then the tail of no complete
        // window, and `chunks_exact` drops them — which is right, because
        // once every window has moved left by `shift` there is no pixel whose
        // window reaches them.
        for (dst_row, src_row) in out
            .chunks_exact_mut(width)
            .zip(self.subpixels.chunks_exact(sub_width))
        {
            // `shift` is 0, 1 or 2, so the taps a pixel reads are the row's
            // own except for the first pixel's, which reads `shift` fewer.
            // Splitting the row at `sub_width - shift` puts every complete
            // window in `body` — walked three at a time with no index
            // arithmetic and no bounds check — and leaves the leading partial
            // window, if there is one, to the arm below.
            // Column zero's window is the one that can reach left of the
            // row, and `head` is the part of it that survives: `3 - shift`
            // taps at a non-zero phase, all three at phase zero. Splitting
            // there leaves `body` holding every *complete* window, one after
            // another, which is what makes the loop below a `chunks_exact(3)`
            // with no index arithmetic and no bounds check.
            let (head, body) = src_row.split_at(3 - shift);
            let mut cells = dst_row.iter_mut();
            if let Some(cell) = cells.next() {
                // The C++ averages column zero's survivors over the same
                // divisor of three, which **darkens** it rather than
                // brightening it; summing `head` alone and dividing by three
                // is exactly that. Clamping the window to the row start —
                // reading three real taps — would brighten it, and
                // `the_first_column_darkens_at_a_shifted_phase` is the test
                // that says which.
                *cell = gamma_of_mean(head.iter().map(|v| u32::from(*v)).sum());
            }
            for (cell, taps) in cells.zip(body.chunks_exact(3)) {
                *cell = gamma_of_mean(taps.iter().map(|v| u32::from(*v)).sum());
            }
        }
    }

    /// Stage 4 with `normalize = false`: the three subpixels kept apart.
    ///
    /// The window is [`Self::to_gray`]'s, shifted by the same phase; what
    /// differs is that the triple is *not* averaged. Each subpixel is
    /// gamma-adjusted on its own and becomes one destination channel's
    /// coverage — leftmost to red, then green, then blue.
    ///
    /// The left edge is reproduced rather than corrected, and it is not the
    /// same rule as the gray path's. Where `to_gray` averages the surviving
    /// samples *over the same divisor of three* — darkening the first column —
    /// the oracle's per-channel arm simply **does not write** the channels
    /// whose subpixel would come from before the bitmap
    /// (`if (start_col > left)` guards them), leaving the destination's own
    /// value there. A zero coverage is how that is expressed here: the merge
    /// below leaves a channel untouched at coverage zero, which is the same
    /// destination byte the oracle's skipped write leaves.
    #[must_use]
    pub fn to_subpixel(&self, phase: SubpixelPhase) -> SubpixelBitmap {
        let shift = i32::try_from(phase.shift()).unwrap_or(0);
        let sub_width = self.width * 3;
        let mut channels = vec![0u8; self.subpixels.len()];
        for y in 0..self.height {
            let row = y * sub_width;
            for x in 0..self.width {
                let start = row + x * 3 - shift;
                for k in 0..3 {
                    let idx = start + k;
                    // Only the first column can reach left of its row, and the
                    // oracle leaves that channel unwritten rather than clamping.
                    if idx < row {
                        continue;
                    }
                    let raw = usize::try_from(idx)
                        .ok()
                        .and_then(|i| self.subpixels.get(i))
                        .copied()
                        .unwrap_or(0);
                    let gamma = TEXT_GAMMA_ADJUST
                        .get(usize::from(raw))
                        .copied()
                        .unwrap_or(0);
                    if let Ok(i) = usize::try_from((y * self.width + x) * 3 + k)
                        && let Some(cell) = channels.get_mut(i)
                    {
                        *cell = gamma;
                    }
                }
            }
        }
        SubpixelBitmap {
            left: self.left,
            top: self.top,
            width: self.width,
            height: self.height,
            channels,
        }
    }
}

/// Floor division for a positive divisor.
fn div_floor(a: i64, b: i64) -> i64 {
    let q = a / b;
    if a % b != 0 && (a < 0) != (b < 0) {
        q - 1
    } else {
        q
    }
}

/// Ceiling division for a positive divisor.
fn div_ceil(a: i64, b: i64) -> i64 {
    let q = a / b;
    if a % b != 0 && (a < 0) == (b < 0) {
        q + 1
    } else {
        q
    }
}

/// What identifies one cached [`LcdBitmap`].
///
/// It is [`pdfrum_font::GlyphKey`] — which already carries the font, the glyph
/// and the four substitution parameters that change an outline — plus the
/// **shape of the device matrix**, because a bitmap is rasterized at a size and
/// an outline is not.
///
/// The matrix is reduced to four integers by `(int)(m · 10000)` on each of
/// `a`, `b`, `c`, `d`, truncating toward zero rather than rounding. That
/// coarseness is visible in pixels: two glyph matrices closer together than
/// one part in ten thousand share a *bitmap*, so a page whose text matrix
/// drifts by rounding error draws identical glyphs there. A finer key would
/// draw very slightly different ones.
///
/// The translation is deliberately absent, as it is upstream: the glyph is
/// rasterized about its own origin and *placed* by the blit.
///
/// So is the subpixel phase, and for a sharper reason — the cached bitmap is
/// three times as wide as the glyph, and the phase is a window shift into it
/// that [`LcdBitmap::to_gray`] applies at blit time. Keying on the phase would
/// store the same rasterization three times.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct BitmapKey {
    /// Which glyph, and at which substitution parameters.
    pub glyph: pdfrum_font::GlyphKey,
    /// `(int)(a · 10000)`, and likewise `b`, `c`, `d`.
    pub matrix: [i32; 4],
}

impl BitmapKey {
    /// The key for a glyph drawn under `matrix`.
    #[must_use]
    pub fn new(glyph: pdfrum_font::GlyphKey, matrix: Affine) -> Self {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "the C++ is `static_cast<int>(m * 10000)`; a matrix \
                      coefficient past i32 belongs to a glyph the ±32000 \
                      coordinate rule has already rejected"
        )]
        fn ten_thousandths(v: f64) -> i32 {
            (v * 10_000.0) as i32
        }
        let [xx, yx, xy, yy, _, _] = matrix.as_coeffs();
        Self {
            glyph,
            matrix: [
                ten_thousandths(xx),
                ten_thousandths(yx),
                ten_thousandths(xy),
                ten_thousandths(yy),
            ],
        }
    }
}

/// How many bytes of glyph bitmaps one render session keeps.
///
/// A budget rather than an entry count, because glyph bitmaps differ in size by
/// four orders of magnitude: 24 bytes for a 6 pt comma and megabytes for
/// display type just under the outline threshold. Sixteen megabytes holds every
/// glyph of every font on any page in the corpus several times over, and bounds
/// what a hostile file can make a session allocate.
pub(crate) const BITMAP_CACHE_BUDGET: usize = 16 * 1024 * 1024;

/// Memoized glyph bitmaps for one render session.
///
/// The cache PDFium keeps per face; ours is keyed across faces because the key
/// already names the font, which keeps one map rather than a map of maps.
///
/// A miss is memoized as `None`, exactly as [`pdfrum_font::GlyphCache`] does:
/// a glyph too large to rasterize, or with no area, must not be re-attempted
/// once per occurrence on a page of a thousand of them.
///
/// When the budget is exhausted the cache **stops inserting** rather than
/// evicting, and the caller still gets its bitmap — the cache degrades to no
/// cache, never to no glyph. Eviction would need a recency order, and the
/// access pattern here makes one worth very little: a page's glyph repertoire
/// is small and is touched over and over, so the entries that would be evicted
/// are the ones about to be wanted again. Refusing to grow keeps the bound with
/// no policy.
#[derive(Debug, Default)]
pub(crate) struct BitmapCache {
    /// Keyed with [`pdfrum_common::FxBuildHasher`], not `std`'s `SipHash`.
    ///
    /// [`BitmapKey`] is a glyph id and four `i32` matrix coefficients — sixteen
    /// bytes this crate computes, none of which a file supplies directly — and
    /// it is looked up once per glyph *occurrence*, tens of thousands of times
    /// on a dense page. `SipHash`'s collision resistance buys nothing against a
    /// key an attacker cannot choose, and its mixing is several times the cost
    /// of the lookup it guards. See `pdfrum_common::FxBuildHasher`'s docs for
    /// which keys may and may not use it.
    entries: std::collections::HashMap<BitmapKey, Option<LcdBitmap>, pdfrum_common::FxBuildHasher>,
    bytes: usize,
}

/// A cached bitmap, or one rendered past a full cache.
///
/// The two are the same value to a caller; the distinction exists so that the
/// borrow of a cached entry and the ownership of an uncached one can share one
/// return type without cloning the cached case, which is the case that matters.
#[derive(Debug)]
pub(crate) enum Cached<'a> {
    /// Held by the cache, borrowed for this draw.
    Hit(&'a LcdBitmap),
    /// Rendered past the budget and dropped after this draw.
    Uncached(LcdBitmap),
}

impl std::ops::Deref for Cached<'_> {
    type Target = LcdBitmap;

    fn deref(&self) -> &LcdBitmap {
        match self {
            Self::Hit(b) => b,
            Self::Uncached(b) => b,
        }
    }
}

impl BitmapCache {
    /// The bitmap for `key`, rasterizing it through `render` on a miss.
    ///
    /// `render` is a closure rather than an outline because producing the
    /// outline is itself the expensive half — a hinted glyph runs the face's
    /// bytecode — and a hit must not pay for it.
    pub fn get_or_insert(
        &mut self,
        key: BitmapKey,
        render: impl FnOnce() -> Option<LcdBitmap>,
    ) -> Option<Cached<'_>> {
        // Two probes on the hit path, and it is not for want of trying to
        // make it one. A hit is what every call after the first is — on
        // `benches/corpus/vector_font_size14.pdf` this runs 8775 times per
        // render and misses zero times — so the obvious fix is to return the
        // occupied entry's borrow directly. It does not compile: returning a
        // borrow taken from `self.entries` in one arm and re-borrowing it
        // mutably in the other is NLL problem case 3, which the current
        // borrow checker rejects and Polonius accepts. The `Entry` API does
        // not rescue it either, because the budget check needs `render()`'s
        // result and `render()` cannot run while a `Vacant` entry holds the
        // borrow.
        //
        // It was measured before being left: the pair costs about 85 ns of a
        // 1000 ns per-glyph chain (`docs/status/M13-perf-baseline.md` §16.2),
        // and the second probe is at most half of that on a table that is
        // already in cache from the first.
        if !self.entries.contains_key(&key) {
            let bitmap = render();
            let size = bitmap.as_ref().map_or(0, LcdBitmap::byte_size);
            if self.bytes.saturating_add(size) > BITMAP_CACHE_BUDGET && !self.entries.is_empty() {
                // Full: draw this glyph without keeping it.
                return bitmap.map(Cached::Uncached);
            }
            self.bytes = self.bytes.saturating_add(size);
            self.entries.insert(key, bitmap);
        }
        self.entries
            .get(&key)
            .and_then(Option::as_ref)
            .map(Cached::Hit)
    }
}

/// Collapse a subpixel bitmap's three channels back to one gray coverage.
///
/// The fallback `crate::device::RenderDevice::draw_glyph_lcd`'s default takes
/// for a backend that cannot address channels separately. It is **not** the
/// oracle's own gray path and must not be mistaken for it: `to_gray` averages
/// the *raw* subpixels and gamma-adjusts the average once, where this averages
/// three values the gamma table has already been applied to. The two differ by
/// a few counts on a partially covered pixel, because the table is not linear.
///
/// Reaching for it means the LCD render is already not happening; this makes
/// the result grey and legible rather than absent, and the honest description
/// of it is an approximation of the wrong branch, not a second opinion on the
/// right one.
///
/// Returns `None` for a bitmap with no pixels.
#[must_use]
pub(crate) fn average_to_gray(bitmap: &SubpixelBitmap) -> Option<GlyphBitmap> {
    if bitmap.is_empty() {
        return None;
    }
    let mut coverage = Vec::with_capacity(bitmap.channels.len() / 3);
    for triple in bitmap.channels.chunks_exact(3) {
        let sum: u32 = triple.iter().map(|&v| u32::from(v)).sum();
        #[expect(
            clippy::cast_possible_truncation,
            reason = "three bytes divided by three is at most 255"
        )]
        let byte = (sum / 3) as u8;
        coverage.push(byte);
    }
    Some(GlyphBitmap {
        left: bitmap.left,
        top: bitmap.top,
        width: bitmap.width,
        height: bitmap.height,
        coverage,
    })
}

/// One glyph's coverage recoloured into a premultiplied pixmap, ready to blit.
///
/// The oracle keeps the mask and the colour apart all the way to the
/// destination: `DrawNormalTextHelper` merges `bgra` into each pixel at the
/// glyph's own alpha (`ApplyAlpha` → `AlphaMerge`). Premultiplying here and
/// compositing source-over is the same arithmetic — `dest·(255−a)/255 +
/// colour·a/255` either way — expressed in the vocabulary
/// `crate::device::RenderDevice::draw_image` already speaks, which is what
/// lets the glyph path use the existing device seam rather than growing a
/// seventh primitive that every backend would have to reimplement.
///
/// The glyph's own coverage is scaled by the colour's alpha first
/// (`CalcAlpha(gamma, bgra.alpha)` is `gamma · alpha / 255`), so a translucent
/// fill colour and a partial coverage compose exactly once.
///
/// Returns `None` for a bitmap with no pixels, and for a colour that is fully
/// transparent — both of which draw nothing.
#[must_use]
pub(crate) fn recolour(bitmap: &GlyphBitmap, colour: peniko::Color) -> Option<crate::Pixmap> {
    let mut pixmap = crate::Pixmap::new(0, 0);
    let by_ref = GlyphBitmapRef {
        width: bitmap.width,
        height: bitmap.height,
        coverage: &bitmap.coverage,
    };
    recolour_ref_into(by_ref, colour, &mut pixmap).then_some(pixmap)
}

/// One glyph occurrence, from the cached LCD bitmap to a pixmap ready to blit.
///
/// The two halves the blit runs per occurrence — [`LcdBitmap::to_gray`] and
/// [`recolour`] — writing into `scratch`'s buffers instead of allocating a
/// `Vec` and a `Pixmap` each. Both are rewritten in full, so nothing of the
/// previous glyph survives into this one; what is reused is the memory alone.
///
/// The arithmetic is the two functions' own, unchanged: the same gamma table
/// over the same window shift, the same truncating `CalcAlpha` product, the
/// same premultiplied bytes. `false` means the glyph draws nothing, which is
/// the `None` the two spellings return between them.
pub(crate) fn recolour_glyph_into(
    lcd: &LcdBitmap,
    phase: SubpixelPhase,
    colour: peniko::Color,
    scratch: &mut crate::ctx::GlyphBlitScratch,
) -> bool {
    lcd.gray_coverage_into(phase, &mut scratch.coverage);
    let bitmap = GlyphBitmapRef {
        width: lcd.width,
        height: lcd.height,
        coverage: &scratch.coverage,
    };
    recolour_ref_into(bitmap, colour, &mut scratch.pixels)
}

/// A [`GlyphBitmap`]'s pixels without owning them.
///
/// The blit's coverage lives in a session-owned buffer, so the recolour reads
/// it by reference; `left` and `top` are absent because only the caller places
/// the bitmap and this half never looks at them.
#[derive(Clone, Copy)]
struct GlyphBitmapRef<'a> {
    width: i32,
    height: i32,
    coverage: &'a [u8],
}

/// [`recolour`] into a pixmap the caller owns, reporting whether it drew.
///
/// The same arithmetic and the same bytes — this is where the body lives and
/// [`recolour`] is this with a fresh pixmap — split out for the reason
/// [`LcdBitmap::gray_coverage_into`] is: the blit runs once per glyph
/// *occurrence*, and a page of ten thousand glyphs otherwise allocates a
/// pixmap ten thousand times. `out` is reshaped to this glyph and **every one
/// of its bytes is written**, which is what makes reusing it sound: the
/// coverage-zero and alpha-zero pixels, which the allocating spelling got for
/// free from a freshly zeroed buffer, are written as transparent here rather
/// than skipped. Skipping them blits the previous glyph's ink through this
/// one's gaps, and
/// `a_reused_scratch_carries_none_of_the_glyph_before_it` fails when they are.
///
/// `false` means nothing was drawn — an empty bitmap or a fully transparent
/// colour — and leaves `out` in an unspecified state, exactly as the `None`
/// it replaces gave the caller no pixmap at all.
fn recolour_ref_into(
    bitmap: GlyphBitmapRef<'_>,
    colour: peniko::Color,
    out: &mut crate::Pixmap,
) -> bool {
    if bitmap.width <= 0 || bitmap.height <= 0 {
        return false;
    }
    let [r, g, b, alpha] = colour.to_rgba8().to_u8_array();
    if alpha == 0 {
        return false;
    }
    let (Ok(width), Ok(height)) = (u32::try_from(bitmap.width), u32::try_from(bitmap.height))
    else {
        return false;
    };
    out.reshape_keeping_pixels(width, height);
    let stride = width as usize;
    let data = out.data_mut();
    // The colour's three channels and the alpha, lifted out of the loop:
    // they are the text object's and do not vary with the pixel. That is what
    // the `let [r, g, b, alpha]` above already does; what is new below is the
    // *index*.
    //
    // The coverage buffer is exactly `height * width` bytes — `render_lcd`
    // sizes it and `gray_coverage_into` refills it to that — so zipping the
    // two row-wise is the index arithmetic and the bounds check the old
    // spelling paid per pixel (`coverage.get(y * stride + x)`), done once per
    // row by the iterator instead. A shorter coverage buffer simply yields
    // fewer rows here, where the old `get` would have painted the missing
    // ones transparent; the two are produced together and cannot differ, and
    // `reshape_keeping_pixels` has already sized `out` to this glyph.
    //
    // **A coverage-to-pixel lookup table was tried here and is not this.** It
    // is the obvious hoist — 256 entries derived once per occurrence turn the
    // four `mul255`es into one array read — and it was measured 3.9x *slower*
    // on both vector fixtures, because a glyph is about fifty pixels and a
    // 256-entry table is five times more arithmetic than the pixels it
    // serves. The quantity to amortize over here is the glyph, not the page.
    for (dst_row, cov_row) in data
        .chunks_exact_mut(stride * 4)
        .zip(bitmap.coverage.chunks_exact(stride))
    {
        for (dest, coverage) in dst_row.chunks_exact_mut(4).zip(cov_row) {
            // `CalcAlpha(TextGammaAdjust(src), bgra.alpha)`, whose product is
            // the truncating one the whole engine uses.
            let a = crate::pixmap::mul255(*coverage, alpha);
            // Zero coverage and zero alpha both wrote nothing before, into a
            // buffer that was already zero; writing the zero explicitly is the
            // same pixel and is what makes a reused buffer sound.
            dest.copy_from_slice(&[
                crate::pixmap::mul255(r, a),
                crate::pixmap::mul255(g, a),
                crate::pixmap::mul255(b, a),
                a,
            ]);
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unit square filled at the origin, as a device-space outline.
    /// Two boxes with a clear column between them, so the middle columns of
    /// the glyph's own box have genuinely zero coverage.
    fn split_box(w: f64, h: f64) -> BezPath {
        let mut p = square(w, h);
        let x = w + 3.0;
        p.move_to((x, 0.0));
        p.line_to((x + w, 0.0));
        p.line_to((x + w, h));
        p.line_to((x, h));
        p.close_path();
        p
    }

    fn square(w: f64, h: f64) -> BezPath {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((w, 0.0));
        p.line_to((w, h));
        p.line_to((0.0, h));
        p.close_path();
        p
    }

    #[test]
    fn the_gamma_table_is_the_oracles() {
        // Endpoints, the three plateaus a reader might "fix", and the length.
        assert_eq!(TEXT_GAMMA_ADJUST.len(), 256);
        assert_eq!(TEXT_GAMMA_ADJUST[0], 0);
        assert_eq!(TEXT_GAMMA_ADJUST[255], 255);
        // It is monotone non-decreasing, and it repeats rather than descends —
        // unlike `kWeightPow11`, whose three descents are transcription errors
        // frozen into the oracle's output.
        for w in TEXT_GAMMA_ADJUST.windows(2) {
            let (Some(a), Some(b)) = (w.first(), w.last()) else {
                continue;
            };
            assert!(a <= b, "the table never descends: {a} then {b}");
        }
        // It brightens: every entry is at or above the identity below the top.
        assert!(TEXT_GAMMA_ADJUST[1] > 1);
        assert!(TEXT_GAMMA_ADJUST[128] > 128);
    }

    #[test]
    fn the_filter_weights_sum_to_a_whole_scale() {
        // 256, not 255 — which is what lets `(cov * w + 85) >> 8` be exact and
        // what bounds the accumulated subpixel at a byte.
        assert_eq!(LCD_FIR5.iter().sum::<i32>(), 256);
        // Symmetric, so the filter does not shift the glyph.
        assert_eq!(LCD_FIR5[0], LCD_FIR5[4]);
        assert_eq!(LCD_FIR5[1], LCD_FIR5[3]);
    }

    #[test]
    fn the_bitmap_is_wider_than_the_outline_by_the_filters_reach() {
        // The whole reason an LCD glyph inks columns an outline fill leaves
        // white: `ft_lcd_padding` widens the box by 43/64 of a pixel on each
        // side so the FIR5 tails have somewhere to land.
        let bmp = rasterize(&square(2.0, 2.0), SubpixelPhase::Zero).expect("a square rasterizes");
        assert_eq!(bmp.top, 0);
        assert_eq!(bmp.height, 2);
        // 43/64 each side rounds out to one whole pixel each side.
        assert_eq!(bmp.left, -1, "one pixel of padding on the left");
        assert_eq!(bmp.width, 4, "two pixels of glyph plus one each side");
    }

    #[test]
    fn the_padding_columns_carry_the_filters_tails() {
        let bmp = rasterize(&square(2.0, 2.0), SubpixelPhase::Zero).expect("a square rasterizes");
        // Column 0 is entirely outside the outline, and it is not blank: it
        // holds the two outermost taps of the leftmost span's filter. An
        // outline fill writes zero here, and that is the difference the whole
        // module exists to reproduce.
        assert!(
            bmp.at(0, 0) > 0,
            "the padding column must carry ink, not zero"
        );
        // The interior is darker than the padding, but it is NOT opaque: the
        // filter takes ink out of a boundary pixel as well as putting it into
        // its neighbour, and a two-pixel-wide glyph has no column far enough
        // from an edge to keep all of its own. That redistribution is the
        // whole effect, and asserting 255 here would assert it away.
        assert!(bmp.at(0, 0) < bmp.at(1, 0));
        assert!(bmp.at(1, 0) > 200, "the interior is nearly opaque");
        // Symmetric about the glyph.
        assert_eq!(bmp.at(0, 0), bmp.at(3, 0));
    }

    #[test]
    fn a_phase_shifts_the_window_and_changes_the_gray() {
        // Two glyphs at the same integer column and different thirds get
        // different gray, which is why the phase keys the cache.
        let outline = square(1.5, 2.0);
        let zero = rasterize(&outline, SubpixelPhase::Zero).expect("rasterizes");
        let one = rasterize(&outline, SubpixelPhase::One).expect("rasterizes");
        let two = rasterize(&outline, SubpixelPhase::Two).expect("rasterizes");
        // Same geometry, so the same box.
        assert_eq!((zero.left, zero.width), (one.left, one.width));
        assert_eq!((zero.left, zero.width), (two.left, two.width));
        assert_ne!(zero.coverage, one.coverage);
        assert_ne!(one.coverage, two.coverage);
    }

    #[test]
    fn the_phase_of_an_x_is_the_c_remainder() {
        // The thirds of one pixel, at their boundaries and inside them.
        assert_eq!(SubpixelPhase::of(10.0), SubpixelPhase::Zero);
        assert_eq!(SubpixelPhase::of(10.32), SubpixelPhase::Zero);
        assert_eq!(SubpixelPhase::of(10.34), SubpixelPhase::One);
        assert_eq!(SubpixelPhase::of(10.5), SubpixelPhase::One);
        assert_eq!(SubpixelPhase::of(10.66), SubpixelPhase::One);
        assert_eq!(SubpixelPhase::of(10.7), SubpixelPhase::Two);
        assert_eq!(SubpixelPhase::of(10.99), SubpixelPhase::Two);
        assert_eq!(SubpixelPhase::of(11.0), SubpixelPhase::Zero);
        // A negative origin: `(int)(x * 3) % 3` truncates toward zero and
        // keeps the sign, so -10.4 gives -31 % 3 == -1, which is the C++'s
        // else-branch — two thirds, not one.
        assert_eq!(SubpixelPhase::of(-10.4), SubpixelPhase::Two);
        assert_eq!(SubpixelPhase::of(-10.7), SubpixelPhase::One);
        assert_eq!(SubpixelPhase::of(-10.0), SubpixelPhase::Zero);
    }

    #[test]
    fn an_enormous_glyph_renders_as_nothing() {
        // `RenderGlyph` returns nullptr past `kMaxGlyphDimension` and the blit
        // loop skips the glyph; it does not clamp it to the limit.
        let huge = square(f64::from(MAX_GLYPH_DIMENSION) + 10.0, 4.0);
        assert!(rasterize(&huge, SubpixelPhase::Zero).is_none());
    }

    #[test]
    fn a_degenerate_outline_yields_nothing_rather_than_an_empty_bitmap() {
        let mut zero_height = BezPath::new();
        zero_height.move_to((0.0, 0.0));
        zero_height.line_to((4.0, 0.0));
        zero_height.close_path();
        // Zero rows: the box collapses and there is no bitmap to blit.
        assert!(rasterize(&zero_height, SubpixelPhase::Zero).is_none());
    }

    #[test]
    fn the_first_column_averages_over_three_however_few_it_has() {
        // The C++ divides by three even when it summed two terms or one, which
        // darkens the leftmost column of a phase-shifted glyph rather than
        // brightening it. Reproduced, not corrected.
        let outline = square(2.0, 1.0);
        let two = rasterize(&outline, SubpixelPhase::Two).expect("rasterizes");
        let zero = rasterize(&outline, SubpixelPhase::Zero).expect("rasterizes");
        assert!(
            two.at(0, 0) <= zero.at(0, 0),
            "borrowing from off the left edge cannot brighten the column"
        );
    }

    #[test]
    fn coverage_outside_the_bitmap_reads_as_zero() {
        let bmp = rasterize(&square(1.0, 1.0), SubpixelPhase::Zero).expect("rasterizes");
        assert_eq!(bmp.at(-1, 0), 0);
        assert_eq!(bmp.at(0, -1), 0);
        assert_eq!(bmp.at(bmp.width, 0), 0);
        assert_eq!(bmp.at(0, bmp.height), 0);
    }

    #[test]
    fn the_subpixel_bitmap_reads_the_same_window_as_the_gray_one() {
        // `to_subpixel` and `to_gray` differ in one thing only: whether the
        // triple is averaged. So the gamma of the average must sit between the
        // smallest and largest of the three gammas the subpixel form keeps —
        // which it cannot if the two are reading different windows, which is
        // the mistake a phase shift invites.
        let outline = square(3.0, 2.0);
        let lcd = render_lcd(&outline).expect("rasterizes");
        for phase in [SubpixelPhase::Zero, SubpixelPhase::One, SubpixelPhase::Two] {
            let gray = lcd.to_gray(phase);
            let sub = lcd.to_subpixel(phase);
            assert_eq!((sub.width, sub.height), (gray.width, gray.height));
            assert_eq!((sub.left, sub.top), (gray.left, gray.top));
            for y in 0..gray.height {
                for x in 0..gray.width {
                    let three = sub.at(x, y);
                    let (lo, hi) = (
                        three.iter().copied().min().unwrap_or(0),
                        three.iter().copied().max().unwrap_or(0),
                    );
                    let g = gray.at(x, y);
                    assert!(
                        g >= lo.saturating_sub(2) && g <= hi.saturating_add(2),
                        "{phase:?} at ({x},{y}): gray {g} outside subpixels {three:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_fully_covered_pixel_has_no_fringe_and_an_edge_does() {
        // The whole point of the subpixel form. Inside a solid glyph all three
        // stripes are saturated, so there is nothing to tell apart and the
        // pixel is neutral; at the glyph's vertical edge they differ, which is
        // the colour fringe `bClearType` exists to produce. A form that
        // averaged internally would show neither.
        let lcd = render_lcd(&square(6.0, 2.0)).expect("rasterizes");
        let sub = lcd.to_subpixel(SubpixelPhase::Zero);
        let interior = sub.at(sub.width / 2, 0);
        assert_eq!(
            interior[0], interior[2],
            "a fully covered pixel must have no fringe, got {interior:?}"
        );
        let edges: Vec<_> = (0..sub.width)
            .map(|x| sub.at(x, 0))
            .filter(|c| c[0] != c[2])
            .collect();
        assert!(
            !edges.is_empty(),
            "no pixel had unequal stripes; the triples are being averaged \
             somewhere they should not be"
        );
    }

    #[test]
    fn the_gamma_table_is_applied_once_per_stripe_not_once_per_pixel() {
        // `MergeGammaAdjustRgb` calls `TextGammaAdjust` on each of the three
        // subpixels separately (`cfx_renderdevice.cpp:132-140`); the gray path
        // calls it once on their average. Every byte the subpixel form emits
        // must therefore be a value the table can produce.
        let lcd = render_lcd(&square(4.0, 1.0)).expect("rasterizes");
        let sub = lcd.to_subpixel(SubpixelPhase::One);
        for &byte in &sub.channels {
            assert!(
                TEXT_GAMMA_ADJUST.contains(&byte),
                "{byte} is not in the gamma table's range"
            );
        }
    }

    #[test]
    fn averaging_back_to_gray_keeps_the_bitmaps_shape() {
        // The fallback a backend without per-channel addressing takes. It must
        // preserve position and size exactly — a glyph that moved or resized
        // when a backend declined the LCD path would be a worse failure than
        // the missing fringes it is standing in for.
        let lcd = render_lcd(&square(3.0, 2.0)).expect("rasterizes");
        let sub = lcd.to_subpixel(SubpixelPhase::Zero);
        let gray = average_to_gray(&sub).expect("has pixels");
        assert_eq!((gray.width, gray.height), (sub.width, sub.height));
        assert_eq!((gray.left, gray.top), (sub.left, sub.top));
        assert_eq!(gray.coverage.len(), sub.channels.len() / 3);
    }

    /// A big glyph then a small one, through one reused scratch.
    ///
    /// This is the way the reuse could be wrong: the buffers are sized to the
    /// *previous* glyph, so a smaller one that only resized the pixmap without
    /// rewriting every byte would blit the tail of the glyph before it. Both
    /// spellings must produce exactly what the allocating one does.
    #[test]
    fn a_reused_scratch_carries_none_of_the_glyph_before_it() {
        let mut scratch = crate::ctx::GlyphBlitScratch::default();
        let colour = peniko::Color::from_rgba8(200, 100, 50, 255);
        let big = render_lcd(&square(20.0, 9.0)).expect("rasterizes");
        // Two boxes with a gap, not one filled box: the gap's pixels have
        // zero coverage, so a spelling that skipped writing them would leave
        // the big glyph's ink showing through exactly there. A filled square
        // covers every pixel of its own box and cannot detect the leak.
        let small = render_lcd(&split_box(3.0, 5.0)).expect("rasterizes");

        // Prime the scratch with the large glyph, then draw the small one.
        assert!(recolour_glyph_into(
            &big,
            SubpixelPhase::Zero,
            colour,
            &mut scratch
        ));
        assert!(recolour_glyph_into(
            &small,
            SubpixelPhase::Zero,
            colour,
            &mut scratch
        ));

        // The allocating spelling, which starts from a zeroed buffer.
        let fresh = recolour(&small.to_gray(SubpixelPhase::Zero), colour).expect("draws");
        assert_eq!(
            (scratch.pixels.width(), scratch.pixels.height()),
            (fresh.width(), fresh.height()),
            "the reused pixmap must be resized to this glyph"
        );
        assert_eq!(
            scratch.pixels.data(),
            fresh.data(),
            "a reused buffer must not leak the previous glyph's pixels"
        );
    }

    /// The fused chain equals the two functions it replaces, at every phase.
    ///
    /// `recolour_glyph_into` is `to_gray` followed by `recolour`; the split
    /// exists to reuse memory and must not have changed a byte. The phase is
    /// swept because it is the one input that reaches both halves.
    #[test]
    fn the_fused_glyph_blit_is_the_two_functions_it_replaces() {
        let mut scratch = crate::ctx::GlyphBlitScratch::default();
        let colour = peniko::Color::from_rgba8(17, 200, 99, 255);
        let lcd = render_lcd(&square(5.0, 3.0)).expect("rasterizes");
        for phase in [SubpixelPhase::Zero, SubpixelPhase::One, SubpixelPhase::Two] {
            let expected = recolour(&lcd.to_gray(phase), colour).expect("draws");
            assert!(recolour_glyph_into(&lcd, phase, colour, &mut scratch));
            assert_eq!(
                scratch.pixels.data(),
                expected.data(),
                "the fused blit must be byte-identical at phase {phase:?}"
            );
        }
    }

    /// A fully transparent fill draws nothing and says so.
    ///
    /// The `false` arm stands in for the `None` the allocating spelling
    /// returned, and a caller that blitted anyway would paint the previous
    /// glyph still sitting in the scratch.
    #[test]
    fn a_transparent_colour_draws_no_glyph() {
        let mut scratch = crate::ctx::GlyphBlitScratch::default();
        let lcd = render_lcd(&square(4.0, 2.0)).expect("rasterizes");
        assert!(!recolour_glyph_into(
            &lcd,
            SubpixelPhase::Zero,
            peniko::Color::from_rgba8(1, 2, 3, 0),
            &mut scratch
        ));
    }

    /// The per-subpixel spelling `gray_coverage_into` replaced, kept in the
    /// tests as the *specification*.
    ///
    /// The same relationship `clip_at` has to `clip_span` and the span loop
    /// has to `blit_image`: the reference is the code that was there, written
    /// the slow obvious way, so that a divergence in the fast one is a failed
    /// comparison rather than an argument about which is right.
    fn gray_coverage_per_subpixel(lcd: &LcdBitmap, phase: SubpixelPhase) -> Vec<u8> {
        let shift = i32::try_from(phase.shift()).unwrap_or(0);
        let sub_width = lcd.width * 3;
        let mut coverage = vec![0u8; lcd.subpixels.len() / 3];
        for y in 0..lcd.height {
            let row = y * sub_width;
            for x in 0..lcd.width {
                let start = row + x * 3 - shift;
                let sum: i32 = (0..3)
                    .map(|k| {
                        let idx = start + k;
                        if idx < row {
                            return 0;
                        }
                        usize::try_from(idx)
                            .ok()
                            .and_then(|i| lcd.subpixels.get(i))
                            .map_or(0, |v| i32::from(*v))
                    })
                    .sum();
                let average = (sum / 3).clamp(0, 255);
                let Ok(average) = usize::try_from(average) else {
                    continue;
                };
                let gamma = TEXT_GAMMA_ADJUST.get(average).copied().unwrap_or(0);
                if let Ok(i) = usize::try_from(y * lcd.width + x)
                    && let Some(cell) = coverage.get_mut(i)
                {
                    *cell = gamma;
                }
            }
        }
        coverage
    }

    /// The row-sliced coverage walk equals the per-subpixel one it replaced,
    /// on every glyph shape and at every phase.
    ///
    /// The shapes are chosen for the two things the hoist trades on: the
    /// **row band**, so a one-row and a many-row glyph both appear, and the
    /// **window's left edge**, which is the only index the old spelling's
    /// bounds check could ever have caught — column zero at a non-zero phase.
    /// A single-column glyph is in the list because for it *every* column is
    /// column zero.
    #[test]
    fn the_sliced_coverage_walk_matches_the_per_subpixel_one() {
        let shapes: [BezPath; 6] = [
            square(1.0, 1.0),
            square(1.0, 7.0),
            square(9.0, 1.0),
            square(5.0, 3.0),
            split_box(3.0, 5.0),
            split_box(1.0, 4.0),
        ];
        let mut compared = 0;
        for shape in &shapes {
            let lcd = render_lcd(shape).expect("rasterizes");
            for phase in [SubpixelPhase::Zero, SubpixelPhase::One, SubpixelPhase::Two] {
                let expected = gray_coverage_per_subpixel(&lcd, phase);
                let mut got = Vec::new();
                lcd.gray_coverage_into(phase, &mut got);
                assert_eq!(
                    got, expected,
                    "coverage differs at phase {phase:?} on a {}x{} bitmap",
                    lcd.width, lcd.height
                );
                compared += 1;
            }
        }
        assert_eq!(compared, 18, "every shape must have been compared");
    }

    /// Column zero at a non-zero phase reads fewer taps, and darkens.
    ///
    /// The one index the old bounds check existed for, isolated: at phase two
    /// the first pixel's window starts two subpixels before the row, so only
    /// one of its three taps is real and the average is over three anyway.
    /// A hoist that clamped the window to the row start instead — reading
    /// subpixels 0, 1, 2 rather than dropping the two missing taps — would
    /// *brighten* that column, which is the opposite of what the C++ does.
    #[test]
    fn the_first_column_darkens_at_a_shifted_phase() {
        let lcd = render_lcd(&square(4.0, 2.0)).expect("rasterizes");
        let mut zero = Vec::new();
        lcd.gray_coverage_into(SubpixelPhase::Zero, &mut zero);
        let mut two = Vec::new();
        lcd.gray_coverage_into(SubpixelPhase::Two, &mut two);
        let width = usize::try_from(lcd.width).expect("positive");
        let first = |v: &[u8]| v.first().copied().expect("a non-empty bitmap");
        assert!(first(&zero) > 0, "the glyph must cover its first column");
        assert!(
            first(&two) < first(&zero),
            "phase two drops two of the first column's three taps: {} vs {}",
            first(&two),
            first(&zero)
        );
        // And the rest of the row is the same window moved, not a clamp: it
        // still reads three real taps.
        assert_eq!(
            two,
            gray_coverage_per_subpixel(&lcd, SubpixelPhase::Two),
            "only the first column of each row may differ from an unshifted read"
        );
        assert_eq!(
            two.len(),
            width * usize::try_from(lcd.height).expect("positive")
        );
    }

    /// The row-zipped recolour is the indexed arithmetic it replaced, over
    /// **every** coverage byte and a spread of colours.
    ///
    /// The hoist changed how a coverage byte is *reached* — a zipped row slice
    /// rather than `coverage.get(y * stride + x)` — and not what is done with
    /// it. So the comparison is against the four `mul255`es spelled out here,
    /// swept over all 256 coverages including zero, which is the transparent
    /// pixel a reused buffer depends on being written rather than skipped.
    #[test]
    fn the_row_zipped_recolour_is_the_indexed_arithmetic() {
        for colour in [
            peniko::Color::from_rgba8(255, 255, 255, 255),
            peniko::Color::from_rgba8(0, 0, 0, 255),
            peniko::Color::from_rgba8(200, 100, 50, 255),
            peniko::Color::from_rgba8(3, 251, 128, 137),
            peniko::Color::from_rgba8(17, 200, 99, 1),
        ] {
            let [r, g, b, alpha] = colour.to_rgba8().to_u8_array();
            // Every coverage byte a glyph can carry, as a one-row bitmap.
            let coverage: Vec<u8> = (0..=255).collect();
            let bitmap = GlyphBitmapRef {
                width: 256,
                height: 1,
                coverage: &coverage,
            };
            let mut out = crate::Pixmap::new(0, 0);
            assert!(recolour_ref_into(bitmap, colour, &mut out));
            for (i, dest) in out.data().chunks_exact(4).enumerate() {
                #[expect(clippy::cast_possible_truncation, reason = "the index runs 0..256")]
                let cov = i as u8;
                let a = crate::pixmap::mul255(cov, alpha);
                assert_eq!(
                    dest,
                    [
                        crate::pixmap::mul255(r, a),
                        crate::pixmap::mul255(g, a),
                        crate::pixmap::mul255(b, a),
                        a,
                    ],
                    "coverage {cov} under {colour:?}"
                );
            }
        }
    }

    /// A zero-width bitmap yields no coverage rather than panicking.
    ///
    /// The row walk chunks by `width` and by `width * 3`, and `chunks_exact`
    /// panics on a chunk size of zero — so the degenerate bitmap the old
    /// spelling's `0..0` column loop simply skipped needs saying out loud
    /// here. `render_lcd` does not produce one, and this is the guard for a
    /// caller that constructs an `LcdBitmap` some other way.
    #[test]
    fn a_zero_width_bitmap_yields_no_coverage() {
        for (width, height) in [(0, 4), (0, 0), (4, 0)] {
            let lcd = LcdBitmap {
                left: 0,
                top: 0,
                width,
                height,
                subpixels: Vec::new(),
            };
            for phase in [SubpixelPhase::Zero, SubpixelPhase::One, SubpixelPhase::Two] {
                // Primed with a previous glyph's bytes, so "left empty" is a
                // claim about this call and not about the buffer's history.
                let mut out = vec![7u8; 12];
                lcd.gray_coverage_into(phase, &mut out);
                assert!(
                    out.is_empty(),
                    "a {width}x{height} bitmap has no coverage at phase {phase:?}"
                );
            }
        }
    }

    #[test]
    fn an_empty_subpixel_bitmap_averages_to_nothing() {
        let empty = SubpixelBitmap {
            left: 0,
            top: 0,
            width: 0,
            height: 0,
            channels: Vec::new(),
        };
        assert!(empty.is_empty());
        assert!(average_to_gray(&empty).is_none());
        assert_eq!(empty.at(0, 0), [0; 3]);
    }
}
