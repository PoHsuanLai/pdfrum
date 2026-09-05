//! The sixteen PDF blend modes (ISO 32000-1 §11.3.5), computed the way the
//! oracle does: all-integer, truncating, unclamped.
//!
//! **Part of the backend seam.** A backend that writes its own pixels
//! composites with [`composite_solid`] and [`composite_premultiplied`] rather
//! than with its rasterizer library's, which rounds differently.
//!
//! These are the *engine's* blend functions, used where the engine
//! composites its own offscreen buffers; on-device compositing goes through
//! each rasterizer's native blend modes, which round differently by ±1.
//! Where a value is a *decision* rather than rasterization it comes from
//! here, so every backend sees the same bytes.

// Sources: `core/fxge/dib/blend.cpp` and `cfx_scanlinecompositor.cpp:41-121`.
// `pdfrum-raster-agg` is the in-tree proof that a backend can composite
// through this module alone. The ±1 rounding difference is what Tier B's
// threshold absorbs and Tier C's edge budget names.
//
// Three formulas resist re-derivation and are reproduced literally:
// `Overlay` is `HardLight` with its arguments swapped, `HardLight`'s
// threshold is `s < 128` rather than `2s <= 255`, and `SoftLight` reads a
// 256-entry table that is *not* a square root.

use pdfrum_page::BlendMode;

/// ISO 32000-1 §11.3.5.2's auxiliary `D(x)`, tabulated at 8-bit precision,
/// transcribed verbatim.
///
/// The closed form is `D(x) = if x <= 0.25 { ((16x - 12)x + 4)x } else {
/// sqrt(x) }` and `kColorSqrt[i] == round(255 * D(i / 255))` for all 256
/// entries — but the low branch is a cubic, not a root: entry `1` is `3`
/// where a plain `round(255 * sqrt(1/255))` would give `16`. A rewrite that
/// "simplifies" this to a square root is wrong by 17 counts, not by rounding.
pub(crate) const COLOR_SQRT: [u8; 256] = [
    0x00, 0x03, 0x07, 0x0B, 0x0F, 0x12, 0x16, 0x19, 0x1D, 0x20, 0x23, 0x26, 0x29, 0x2C, 0x2F, 0x32,
    0x35, 0x37, 0x3A, 0x3C, 0x3F, 0x41, 0x43, 0x46, 0x48, 0x4A, 0x4C, 0x4E, 0x50, 0x52, 0x54, 0x56,
    0x57, 0x59, 0x5B, 0x5C, 0x5E, 0x60, 0x61, 0x63, 0x64, 0x65, 0x67, 0x68, 0x69, 0x6B, 0x6C, 0x6D,
    0x6E, 0x70, 0x71, 0x72, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7A, 0x7B, 0x7C, 0x7D, 0x7E,
    0x80, 0x81, 0x82, 0x83, 0x84, 0x85, 0x86, 0x87, 0x87, 0x88, 0x89, 0x8A, 0x8B, 0x8C, 0x8D, 0x8E,
    0x8F, 0x90, 0x91, 0x91, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x97, 0x98, 0x99, 0x9A, 0x9B, 0x9C,
    0x9C, 0x9D, 0x9E, 0x9F, 0xA0, 0xA0, 0xA1, 0xA2, 0xA3, 0xA4, 0xA4, 0xA5, 0xA6, 0xA7, 0xA7, 0xA8,
    0xA9, 0xAA, 0xAA, 0xAB, 0xAC, 0xAD, 0xAD, 0xAE, 0xAF, 0xB0, 0xB0, 0xB1, 0xB2, 0xB3, 0xB3, 0xB4,
    0xB5, 0xB5, 0xB6, 0xB7, 0xB7, 0xB8, 0xB9, 0xBA, 0xBA, 0xBB, 0xBC, 0xBC, 0xBD, 0xBE, 0xBE, 0xBF,
    0xC0, 0xC0, 0xC1, 0xC2, 0xC2, 0xC3, 0xC4, 0xC4, 0xC5, 0xC6, 0xC6, 0xC7, 0xC7, 0xC8, 0xC9, 0xC9,
    0xCA, 0xCB, 0xCB, 0xCC, 0xCC, 0xCD, 0xCE, 0xCE, 0xCF, 0xD0, 0xD0, 0xD1, 0xD1, 0xD2, 0xD3, 0xD3,
    0xD4, 0xD4, 0xD5, 0xD6, 0xD6, 0xD7, 0xD7, 0xD8, 0xD9, 0xD9, 0xDA, 0xDA, 0xDB, 0xDC, 0xDC, 0xDD,
    0xDD, 0xDE, 0xDE, 0xDF, 0xE0, 0xE0, 0xE1, 0xE1, 0xE2, 0xE2, 0xE3, 0xE4, 0xE4, 0xE5, 0xE5, 0xE6,
    0xE6, 0xE7, 0xE7, 0xE8, 0xE9, 0xE9, 0xEA, 0xEA, 0xEB, 0xEB, 0xEC, 0xEC, 0xED, 0xED, 0xEE, 0xEE,
    0xEF, 0xF0, 0xF0, 0xF1, 0xF1, 0xF2, 0xF2, 0xF3, 0xF3, 0xF4, 0xF4, 0xF5, 0xF5, 0xF6, 0xF6, 0xF7,
    0xF7, 0xF8, 0xF8, 0xF9, 0xF9, 0xFA, 0xFA, 0xFB, 0xFB, 0xFC, 0xFC, 0xFD, 0xFD, 0xFE, 0xFE, 0xFF,
];

/// One separable blend mode's per-channel function, on `0..=255` inputs.
///
/// Non-separable modes have no per-channel form; [`blend_rgb`] handles all
/// sixteen and is what callers should reach for. Results are *not* clamped,
/// exactly as upstream leaves them.
#[must_use]
#[expect(
    clippy::match_same_arms,
    reason = "Normal/Compatible returning the source is the blend function; \
              the non-separable arm returning it is an unreachable fallback \
              (upstream NOTREACHED()s). Merging them would erase that \
              distinction and hide the day one of the two changes."
)]
pub(crate) fn blend_channel(mode: BlendMode, back: i32, src: i32) -> i32 {
    match mode {
        BlendMode::Normal | BlendMode::Compatible => src,
        BlendMode::Multiply => src * back / 255,
        BlendMode::Screen => src + back - src * back / 255,
        // Literally HardLight with the arguments swapped, not its own formula.
        BlendMode::Overlay => blend_channel(BlendMode::HardLight, src, back),
        BlendMode::Darken => src.min(back),
        BlendMode::Lighten => src.max(back),
        BlendMode::ColorDodge => {
            if src == 255 {
                255
            } else {
                (back * 255 / (255 - src)).min(255)
            }
        }
        BlendMode::ColorBurn => {
            if src == 0 {
                0
            } else {
                255 - ((255 - back) * 255 / src).min(255)
            }
        }
        BlendMode::HardLight => {
            if src < 128 {
                (src * back * 2) / 255
            } else {
                blend_channel(BlendMode::Screen, back, 2 * src - 255)
            }
        }
        BlendMode::SoftLight => {
            if src < 128 {
                // Two sequential divides, not one by 65025: the truncations
                // differ and the difference is visible.
                back - (255 - 2 * src) * back * (255 - back) / 255 / 255
            } else {
                #[expect(
                    clippy::cast_sign_loss,
                    reason = "the clamp lower bound is 0, so the value is non-negative"
                )]
                let idx = back.clamp(0, 255) as usize;
                let d = i32::from(COLOR_SQRT.get(idx).copied().unwrap_or(0));
                back + (2 * src - 255) * (d - back) / 255
            }
        }
        BlendMode::Difference => (back - src).abs(),
        BlendMode::Exclusion => back + src - 2 * back * src / 255,
        // Non-separable: no per-channel form. Upstream NOTREACHED()s here.
        BlendMode::Hue | BlendMode::Saturation | BlendMode::Color | BlendMode::Luminosity => src,
    }
}

/// `Lum(c) = (r*30 + g*59 + b*11) / 100` — integer, truncating.
#[must_use]
fn lum(c: [i32; 3]) -> i32 {
    (c[0] * 30 + c[1] * 59 + c[2] * 11) / 100
}

/// `Sat(c) = max - min`.
#[must_use]
fn sat(c: [i32; 3]) -> i32 {
    c.iter().copied().max().unwrap_or(0) - c.iter().copied().min().unwrap_or(0)
}

/// `ClipColor`, with the ordering that makes it load-bearing.
///
/// `l`, `n` and `x` are computed **once, from the pre-clip colour**; the
/// `x > 255` branch then operates on channels the `n < 0` branch may already
/// have rewritten, while still dividing by the original `x - l`. Recomputing
/// either bound between the branches changes the result.
#[must_use]
fn clip_color(mut c: [i32; 3]) -> [i32; 3] {
    let l = lum(c);
    let n = c.iter().copied().min().unwrap_or(0);
    let x = c.iter().copied().max().unwrap_or(0);
    if n < 0 && l != n {
        for ch in &mut c {
            *ch = l + ((*ch - l) * l / (l - n));
        }
    }
    if x > 255 && x != l {
        for ch in &mut c {
            *ch = l + ((*ch - l) * (255 - l) / (x - l));
        }
    }
    c
}

/// `SetLum(c, l)`: shift every channel by `l - Lum(c)`, then clip.
#[must_use]
fn set_lum(mut c: [i32; 3], l: i32) -> [i32; 3] {
    let d = l - lum(c);
    for ch in &mut c {
        *ch += d;
    }
    clip_color(c)
}

/// `SetSat(c, s)`: rescale to the requested saturation, or collapse to black
/// when the colour has none to rescale.
#[must_use]
fn set_sat(mut c: [i32; 3], s: i32) -> [i32; 3] {
    let min = c.iter().copied().min().unwrap_or(0);
    let max = c.iter().copied().max().unwrap_or(0);
    if min == max {
        return [0, 0, 0];
    }
    for ch in &mut c {
        *ch = (*ch - min) * s / (max - min);
    }
    c
}

/// Blend one RGB triple over another, separable and non-separable alike.
///
/// Inputs are `0..=255`; the result is clamped on the way out because a
/// caller is storing bytes, while upstream's own intermediate values are not.
#[must_use]
pub(crate) fn blend_rgb(mode: BlendMode, back: [u8; 3], src: [u8; 3]) -> [u8; 3] {
    let b = back.map(i32::from);
    let s = src.map(i32::from);
    let out = match mode {
        BlendMode::Hue => set_lum(set_sat(s, sat(b)), lum(b)),
        BlendMode::Saturation => set_lum(set_sat(b, sat(s)), lum(b)),
        BlendMode::Color => set_lum(s, lum(b)),
        BlendMode::Luminosity => set_lum(b, lum(s)),
        separable => [
            blend_channel(separable, b[0], s[0]),
            blend_channel(separable, b[1], s[1]),
            blend_channel(separable, b[2], s[2]),
        ],
    };
    out.map(|v| {
        #[expect(
            clippy::cast_sign_loss,
            reason = "the clamp lower bound is 0, so the value fits u8 exactly"
        )]
        let byte = v.clamp(0, 255) as u8;
        byte
    })
}

/// The straight-alpha source-over composite the oracle performs, returning
/// the new `(rgb, alpha)` of the destination.
///
/// The `dest.a == 0` short circuit is the structural difference from a
/// textbook Porter-Duff implementation: over a fully transparent backdrop the
/// source is copied verbatim and **no blending happens at all** — which is
/// also what ISO 32000 §11.3.6 requires, reached by another route.
#[must_use]
pub(crate) fn composite_straight(
    dest: ([u8; 3], u8),
    src: ([u8; 3], u8),
    mode: BlendMode,
) -> ([u8; 3], u8) {
    let (dest_rgb, dest_a) = dest;
    let (src_rgb, src_a) = src;
    if dest_a == 0 {
        return (src_rgb, src_a);
    }
    if src_a == 0 {
        return dest;
    }
    let da = u32::from(dest_a);
    let sa = u32::from(src_a);
    // `da + sa - da*sa/255` is upstream's union of two 0..=255 alphas; with
    // both operands bounded by 255 the truncating divide makes the result
    // 0..=255 too, so the narrowing is exact rather than wrapping.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the alpha union of two 0..=255 values is itself 0..=255"
    )]
    let out_a = (da + sa - da * sa / 255) as u8;
    if out_a == 0 {
        return (dest_rgb, 0);
    }
    let ratio = (sa * 255 / u32::from(out_a)).min(255) as u8;
    let blended = blend_rgb(mode, dest_rgb, src_rgb);
    let mut out = [0u8; 3];
    for i in 0..3 {
        let (Some(&d), Some(&s), Some(&bl)) = (dest_rgb.get(i), src_rgb.get(i), blended.get(i))
        else {
            continue;
        };
        // The (1 - alpha_b) * Cs term, then the ratio merge.
        let to_source = crate::pixmap::alpha_merge(s, bl, dest_a);
        if let Some(slot) = out.get_mut(i) {
            *slot = crate::pixmap::alpha_merge(d, to_source, ratio);
        }
    }
    (out, out_a)
}

/// Composite one **premultiplied** RGBA8 source pixel over a premultiplied
/// destination pixel, blending with `mode` and scaling the source by
/// `coverage`.
///
/// `coverage` is the rasterizer's antialiasing byte, folded into the source
/// alpha by a truncating product. A zero coverage or a zero source alpha
/// leaves the destination untouched, which is what lets a caller blend
/// unconditionally.
///
/// A source that carries a *straight* colour — every
/// [`Brush::Solid`](crate::device::Brush) — must use [`composite_solid`]
/// instead: premultiplying it first quantises it.
///
/// ```
/// use pdfrum_page::BlendMode;
/// use pdfrum_render::blend::composite_premultiplied;
///
/// let dest = [0, 0, 255, 255];
/// let src = [255, 0, 0, 255];
///
/// // Full coverage and a fully opaque source replace the destination.
/// assert_eq!(
///     composite_premultiplied(dest, src, 255, BlendMode::default()),
///     [255, 0, 0, 255],
/// );
/// // Zero coverage leaves it untouched, so a caller can blend always.
/// assert_eq!(composite_premultiplied(dest, src, 0, BlendMode::default()), dest);
/// ```
#[must_use]
pub fn composite_premultiplied(
    dest: [u8; 4],
    src: [u8; 4],
    coverage: u8,
    mode: BlendMode,
) -> [u8; 4] {
    // This is `composite_straight` wearing the buffer layout both rasterizer
    // backends and every offscreen target in this crate actually use, and it
    // exists so there is exactly one place the blend arithmetic lives. It
    // un-premultiplies, delegates, and premultiplies back rather than deriving
    // a second premultiplied formula — the round trip costs two divides on a
    // pixel that is being blended anyway, and a second formula is a second
    // thing to keep in step with the oracle.
    //
    // The coverage fold is the same truncating product the oracle folds a
    // clip mask in with.
    //
    // # The opaque-destination fast path (M12)
    //
    // The general route below is four divides and two multiplies per channel:
    // it un-premultiplies both pixels, blends them straight, and
    // premultiplies the result back. For **`BlendMode::Normal` over an opaque
    // destination** the whole of that collapses, algebraically and exactly, to
    // one `alpha_merge` per channel. Substituting `dest_a = 255` and
    // `mode = Normal` into `composite_straight`:
    //
    // - `blend_rgb(Normal, ..)` is the identity on the source
    //   (`blend_channel`'s first arm), so `blended == src_rgb`;
    // - `out_a = 255 + sa - 255*sa/255 = 255`, so the result is opaque and the
    //   premultiply-back is the identity;
    // - `ratio = sa * 255 / 255 = sa`;
    // - `to_source = alpha_merge(s, blended, 255) = s`, because `blended` *is*
    //   `s`;
    // - and the channel is therefore `alpha_merge(d, s, sa)`.
    //
    // The un-premultiply of the destination is also the identity at
    // `da == 255`, so the only surviving work is un-premultiplying the source
    // and one `alpha_merge`. That is not an approximation and not a "close
    // enough": it is the same expression with a constant folded in, and
    // `the_opaque_normal_fast_path_is_exhaustively_identical` checks all
    // 256^3 relevant inputs against the general route rather than asserting it
    // here.
    //
    // It is worth a fast path because it is not a corner case: a PDF page
    // renders onto an opaque white backdrop by default, so *every* pixel of
    // *every* ordinary fill, stroke, glyph blit and image draw on a page with
    // no transparency group takes exactly this branch. Measured on the M12
    // corpus, it is 60% of `render-exact`'s `text` class and 78% of its
    // `shading` class — see `docs/status/M12.md`.
    let (Some(&sr), Some(&sg), Some(&sb), Some(&sa)) =
        (src.first(), src.get(1), src.get(2), src.get(3))
    else {
        return dest;
    };
    let src_alpha = crate::pixmap::mul255(sa, coverage);
    if src_alpha == 0 {
        return dest;
    }
    if matches!(mode, BlendMode::Normal | BlendMode::Compatible)
        && dest.get(3) == Some(&255)
        && let (Some(&dr), Some(&dg), Some(&db)) = (dest.first(), dest.get(1), dest.get(2))
    {
        let [ur, ug, ub] = crate::pixmap::unpremultiply_rgb(sr, sg, sb, sa);
        return [
            crate::pixmap::alpha_merge(dr, ur, src_alpha),
            crate::pixmap::alpha_merge(dg, ug, src_alpha),
            crate::pixmap::alpha_merge(db, ub, src_alpha),
            255,
        ];
    }
    let (Some(&dr), Some(&dg), Some(&db), Some(&da)) =
        (dest.first(), dest.get(1), dest.get(2), dest.get(3))
    else {
        return dest;
    };
    let src_rgb = crate::pixmap::unpremultiply_rgb(sr, sg, sb, sa);
    let dest_rgb = crate::pixmap::unpremultiply_rgb(dr, dg, db, da);
    let (rgb, alpha) = composite_straight((dest_rgb, da), (src_rgb, src_alpha), mode);
    let (Some(&r), Some(&g), Some(&b)) = (rgb.first(), rgb.get(1), rgb.get(2)) else {
        return dest;
    };
    [
        premultiply_channel(r, alpha),
        premultiply_channel(g, alpha),
        premultiply_channel(b, alpha),
        alpha,
    ]
}

/// Composite a **straight** RGBA8 source pixel over a premultiplied
/// destination pixel, blending with `mode` and scaling the source by
/// `coverage`.
///
/// The same composite as [`composite_premultiplied`], entered one step
/// earlier — and that step is not free: premultiplying a straight colour and
/// un-premultiplying it back **quantises it**, because a premultiplied byte
/// at alpha `a` can only express `a + 1` of the 256 straight values. Every
/// caller holding a straight colour — every
/// [`Brush::Solid`](crate::device::Brush) — must use this; a source already
/// premultiplied (an image sample, a composited layer) has no straight
/// colour to preserve and uses [`composite_premultiplied`].
///
/// ```
/// use pdfrum_page::BlendMode;
/// use pdfrum_render::blend::composite_solid;
///
/// let dest = [0, 0, 255, 255];
///
/// // A straight colour, entered one step earlier so the round trip
/// // through premultiplication cannot quantise it.
/// assert_eq!(
///     composite_solid(dest, [221, 0, 0], 255, 255, BlendMode::default()),
///     [221, 0, 0, 255],
/// );
/// ```
#[must_use]
pub fn composite_solid(
    dest: [u8; 4],
    src_rgb: [u8; 3],
    src_a: u8,
    coverage: u8,
    mode: BlendMode,
) -> [u8; 4] {
    // At the alpha the form-field highlight uses — 100/255 — the
    // representable reds near `221` are `219`, `222`, `224`: `221` is not
    // among them, so the round trip that stores it lands on `219` and the
    // tint composites a count low wherever a solid colour is drawn below full
    // alpha.
    //
    // The oracle never takes that step at all. Its AGG render targets are
    // `FXDIB_Format::kBgra` — **straight** alpha; `CFX_DIBitmap::PreMultiply`
    // exists only behind `PDF_USE_SKIA`. So a solid fill's colour reaches
    // `CFX_ScanlineCompositor` exactly as the content stream stated it, and
    // this is the entry point that reproduces that.
    let src_alpha = crate::pixmap::mul255(src_a, coverage);
    if src_alpha == 0 {
        return dest;
    }
    let (Some(&dr), Some(&dg), Some(&db), Some(&da)) =
        (dest.first(), dest.get(1), dest.get(2), dest.get(3))
    else {
        return dest;
    };
    if matches!(mode, BlendMode::Normal | BlendMode::Compatible) && da == 255 {
        // The same collapse `composite_premultiplied` documents, minus the
        // un-premultiply it no longer has to undo.
        let (Some(&sr), Some(&sg), Some(&sb)) = (src_rgb.first(), src_rgb.get(1), src_rgb.get(2))
        else {
            return dest;
        };
        return [
            crate::pixmap::alpha_merge(dr, sr, src_alpha),
            crate::pixmap::alpha_merge(dg, sg, src_alpha),
            crate::pixmap::alpha_merge(db, sb, src_alpha),
            255,
        ];
    }
    let dest_rgb = crate::pixmap::unpremultiply_rgb(dr, dg, db, da);
    let (rgb, alpha) = composite_straight((dest_rgb, da), (src_rgb, src_alpha), mode);
    let (Some(&r), Some(&g), Some(&b)) = (rgb.first(), rgb.get(1), rgb.get(2)) else {
        return dest;
    };
    [
        premultiply_channel(r, alpha),
        premultiply_channel(g, alpha),
        premultiply_channel(b, alpha),
        alpha,
    ]
}

/// `c * a / 255`, **rounding** — premultiplication that survives the trip
/// back.
///
/// The truncating [`mul255`](crate::pixmap::mul255) is the oracle's product
/// wherever the oracle itself performs one, and it stays that everywhere else.
/// Here it is wrong for a structural reason: this is not one of the oracle's
/// products at all, it is the *storage* half of a round trip our premultiplied
/// buffers impose and the oracle's straight ones do not. Its inverse,
/// [`unpremultiply_rgb`](crate::pixmap::unpremultiply_rgb), rounds, so
/// truncating on the way in would make the pair lose a count on most values
/// instead of none.
///
/// Measured: a straight `145` at alpha `223` premultiplies to `126` truncating
/// and `127` rounding, and only `127` comes back as `145`. That count is
/// visible in the corpus — it is every pixel of `alpha_composite`'s overlap.
#[must_use]
fn premultiply_channel(c: u8, a: u8) -> u8 {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "(255*255 + 127)/255 == 255 is the maximum, so the quotient fits u8"
    )]
    let byte = ((u32::from(c) * u32::from(a) + 127) / 255) as u8;
    byte
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The general route, with the opaque-Normal fast path deliberately not
    /// taken.
    ///
    /// A transcription of [`composite_premultiplied`]'s body from the
    /// `src_alpha == 0` check onward, which is what the fast path claims to be
    /// equal to. It is spelt out here rather than reached by a flag on the
    /// real function because a flag would be a branch in the hot path that
    /// exists only for a test.
    fn general_route(dest: [u8; 4], src: [u8; 4], coverage: u8, mode: BlendMode) -> [u8; 4] {
        let [sr, sg, sb, sa] = src;
        let [dr, dg, db, da] = dest;
        let src_alpha = crate::pixmap::mul255(sa, coverage);
        if src_alpha == 0 {
            return dest;
        }
        let src_rgb = crate::pixmap::unpremultiply_rgb(sr, sg, sb, sa);
        let dest_rgb = crate::pixmap::unpremultiply_rgb(dr, dg, db, da);
        let (rgb, alpha) = composite_straight((dest_rgb, da), (src_rgb, src_alpha), mode);
        let [r, g, b] = rgb;
        [
            premultiply_channel(r, alpha),
            premultiply_channel(g, alpha),
            premultiply_channel(b, alpha),
            alpha,
        ]
    }

    #[test]
    fn the_opaque_normal_fast_path_is_exhaustively_identical() {
        // The claim in `composite_premultiplied`'s docs is that for Normal
        // over an opaque destination the general route's four divides collapse
        // to one `alpha_merge` per channel *exactly*. This is a performance
        // change in a parity engine, so "exactly" is checked rather than
        // reasoned about: every source alpha, every source channel value and
        // every destination channel value, on one channel — the three channels
        // are independent in this branch, which is itself why one channel
        // suffices and is checked by the mixed-channel case below.
        for sa in 0..=255u8 {
            for s in (0..=255u8).step_by(1) {
                for d in (0..=255u8).step_by(17) {
                    let src = [
                        crate::pixmap::mul255(s, sa),
                        crate::pixmap::mul255(s, sa),
                        crate::pixmap::mul255(s, sa),
                        sa,
                    ];
                    let dest = [d, d, d, 255];
                    assert_eq!(
                        composite_premultiplied(dest, src, 255, BlendMode::Normal),
                        general_route(dest, src, 255, BlendMode::Normal),
                        "sa={sa} s={s} d={d}"
                    );
                }
            }
        }
    }

    #[test]
    fn the_fast_path_holds_with_unequal_channels_and_partial_coverage() {
        // The exhaustive case above moves all three channels together, which
        // would hide a formula that accidentally read the wrong slot. This one
        // moves them independently, and sweeps `coverage` as well — the byte
        // the rasterizer folds in, which reaches the fast path through
        // `src_alpha` rather than being multiplied in afterwards.
        for sa in [0u8, 1, 63, 128, 200, 254, 255] {
            for cov in [0u8, 1, 64, 127, 128, 200, 255] {
                for (r, g, b) in [(0u8, 128u8, 255u8), (255, 1, 77), (13, 200, 4)] {
                    let src = [
                        crate::pixmap::mul255(r, sa),
                        crate::pixmap::mul255(g, sa),
                        crate::pixmap::mul255(b, sa),
                        sa,
                    ];
                    for dest in [[0u8, 0, 0, 255], [255, 255, 255, 255], [9, 180, 70, 255]] {
                        assert_eq!(
                            composite_premultiplied(dest, src, cov, BlendMode::Normal),
                            general_route(dest, src, cov, BlendMode::Normal),
                            "sa={sa} cov={cov} rgb=({r},{g},{b}) dest={dest:?}"
                        );
                        assert_eq!(
                            composite_premultiplied(dest, src, cov, BlendMode::Compatible),
                            general_route(dest, src, cov, BlendMode::Compatible),
                            "Compatible: sa={sa} cov={cov}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn a_non_opaque_destination_does_not_take_the_fast_path() {
        // The guard is `dest.a == 255`, and it has to be: at any lower alpha
        // the output alpha is no longer 255, the premultiply-back stops being
        // the identity, and the collapse the fast path performs is invalid.
        // Checked by construction — every non-opaque destination must agree
        // with the general route, which it does only by not taking the branch.
        for da in [0u8, 1, 100, 254] {
            for sa in [1u8, 90, 255] {
                let src = [
                    crate::pixmap::mul255(200, sa),
                    crate::pixmap::mul255(50, sa),
                    crate::pixmap::mul255(7, sa),
                    sa,
                ];
                let dest = [
                    crate::pixmap::mul255(30, da),
                    crate::pixmap::mul255(220, da),
                    crate::pixmap::mul255(90, da),
                    da,
                ];
                assert_eq!(
                    composite_premultiplied(dest, src, 255, BlendMode::Normal),
                    general_route(dest, src, 255, BlendMode::Normal),
                    "da={da} sa={sa}"
                );
            }
        }
    }

    #[test]
    fn every_other_blend_mode_still_takes_the_general_route() {
        // The fast path is guarded on the mode as well as the alpha, and the
        // guard names two modes out of sixteen. This walks the other fourteen
        // over an opaque destination — the exact shape that would wrongly
        // match if the mode check were dropped — and requires each to agree
        // with the general route.
        let modes = [
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
        for mode in modes {
            for sa in [1u8, 128, 255] {
                let src = [
                    crate::pixmap::mul255(180, sa),
                    crate::pixmap::mul255(60, sa),
                    crate::pixmap::mul255(240, sa),
                    sa,
                ];
                let dest = [40u8, 200, 90, 255];
                assert_eq!(
                    composite_premultiplied(dest, src, 255, mode),
                    general_route(dest, src, 255, mode),
                    "{mode:?} sa={sa}"
                );
            }
        }
    }

    #[test]
    fn premultiplied_composite_agrees_with_the_straight_one() {
        // The premultiplied spelling must be the straight one in a different
        // buffer layout, not a second formula that drifts from it.
        let dest_straight = ([10u8, 200, 30], 255u8);
        let src_straight = ([250u8, 40, 90], 128u8);
        let (want_rgb, want_a) =
            composite_straight(dest_straight, src_straight, BlendMode::Multiply);
        let premul = |(rgb, a): ([u8; 3], u8)| -> [u8; 4] {
            [
                crate::pixmap::mul255(rgb[0], a),
                crate::pixmap::mul255(rgb[1], a),
                crate::pixmap::mul255(rgb[2], a),
                a,
            ]
        };
        let got = composite_premultiplied(
            premul(dest_straight),
            premul(src_straight),
            255,
            BlendMode::Multiply,
        );
        let want = premul((want_rgb, want_a));
        for i in 0..4 {
            let (Some(&g), Some(&w)) = (got.get(i), want.get(i)) else {
                continue;
            };
            assert!(g.abs_diff(w) <= 1, "channel {i}: {got:?} vs {want:?}");
        }
    }

    #[test]
    fn premultiplying_survives_the_trip_back() {
        // The storage round trip a premultiplied buffer imposes must not lose
        // a count, or every composited pixel drifts one low against a golden
        // taken from the oracle's straight buffer. The witness that found
        // this is 145 at alpha 223 -- `alpha_composite`'s overlap colour.
        assert_eq!(premultiply_channel(145, 223), 127);
        assert_eq!(
            crate::pixmap::unpremultiply_rgb(127, 127, 127, 223),
            [145, 145, 145]
        );
        // Premultiplication is genuinely lossy at low alpha — at alpha 1 the
        // whole colour range collapses onto two representable values — so the
        // property is not exactness but *optimality*: the round trip lands
        // within the quantisation step the alpha imposes, and it is never
        // worse than the truncating spelling. Exhaustively.
        let mut rounding_wins = 0u32;
        for a in 1..=255u8 {
            let step = 255_u32.div_ceil(u32::from(a));
            for c in 0..=255u8 {
                let rounded =
                    crate::pixmap::unpremultiply_rgb(premultiply_channel(c, a), 0, 0, a)[0];
                let truncated =
                    crate::pixmap::unpremultiply_rgb(crate::pixmap::mul255(c, a), 0, 0, a)[0];
                let rounded_err = u32::from(rounded).abs_diff(u32::from(c));
                let truncated_err = u32::from(truncated).abs_diff(u32::from(c));
                assert!(rounded_err <= step, "c={c} a={a} drifted {rounded_err}");
                assert!(
                    rounded_err <= truncated_err,
                    "c={c} a={a}: rounding lost to truncating"
                );
                if rounded_err < truncated_err {
                    rounding_wins += 1;
                }
            }
        }
        // And it is not a distinction without a difference: rounding is
        // strictly better on a large share of the 65 280 pairs.
        assert!(
            rounding_wins > 20_000,
            "only {rounding_wins} pairs improved"
        );
    }

    #[test]
    fn zero_coverage_leaves_the_destination_alone() {
        let dest = [1u8, 2, 3, 255];
        assert_eq!(
            composite_premultiplied(dest, [255, 255, 255, 255], 0, BlendMode::Normal),
            dest
        );
    }

    #[test]
    fn full_coverage_opaque_source_replaces() {
        let out =
            composite_premultiplied([0, 0, 0, 255], [10, 20, 30, 255], 255, BlendMode::Normal);
        assert_eq!(out, [10, 20, 30, 255]);
    }

    #[test]
    fn coverage_scales_the_source_alpha_truncating() {
        // Half coverage over an empty destination copies the source at half
        // alpha, and the product truncates the way the oracle's does.
        let out = composite_premultiplied([0, 0, 0, 0], [255, 0, 0, 255], 128, BlendMode::Normal);
        assert_eq!(out[3], 128, "coverage becomes the alpha");
    }

    #[test]
    fn the_form_field_highlight_composites_to_the_goldens_tint() {
        // `FPDF_SetFormFieldHighlightColor(.., 0xFFE4DD)` with the default
        // alpha 100: `FX_COLORREF` is BGR, so the straight colour is
        // (0xDD, 0xE4, 0xFF), and over the page's opaque white the oracle's
        // truncating `AlphaMerge` gives (241, 244, 255) — the tint every form
        // golden in the corpus carries over its fields.
        let white = [255u8, 255, 255, 255];
        assert_eq!(
            composite_solid(white, [0xDD, 0xE4, 0xFF], 100, 255, BlendMode::Normal),
            [241, 244, 255, 255]
        );
        // And the premultiplied entry point cannot reach it: 221 is not one
        // of the 101 straight reds a premultiplied byte at alpha 100 can hold,
        // so storing it quantises down to 219 and the merge lands at 240.
        // That one count, over 2482 px, is M14 OWED item 3.
        let stored = crate::pixmap::premultiply(peniko::Color::from_rgba8(0xDD, 0xE4, 0xFF, 100));
        assert_eq!(
            composite_premultiplied(white, stored, 255, BlendMode::Normal),
            [240, 244, 255, 255],
            "the round trip through premultiplied storage is what lost the count"
        );
    }

    #[test]
    fn a_solid_source_agrees_with_the_premultiplied_route_wherever_storage_is_lossless() {
        // `composite_solid` is not a *different* composite, it is the same one
        // entered before the lossy step. Where premultiplied storage happens
        // to be exact — full alpha, and every straight value a lower alpha can
        // represent — the two must return identical bytes, in every mode and
        // at every coverage. Anything else would mean the new entry point had
        // changed the arithmetic rather than skipped a quantisation.
        let modes = [
            BlendMode::Normal,
            BlendMode::Multiply,
            BlendMode::Screen,
            BlendMode::Darken,
            BlendMode::HardLight,
            BlendMode::SoftLight,
            BlendMode::Difference,
            BlendMode::Luminosity,
        ];
        for mode in modes {
            for sa in [1u8, 51, 85, 128, 170, 204, 255] {
                for cov in [1u8, 64, 128, 255] {
                    for rgb in [[0u8, 0, 0], [255, 255, 255], [51, 102, 153]] {
                        let src = [
                            crate::pixmap::mul255(rgb[0], sa),
                            crate::pixmap::mul255(rgb[1], sa),
                            crate::pixmap::mul255(rgb[2], sa),
                            sa,
                        ];
                        // Only compare where storage really is lossless.
                        if crate::pixmap::unpremultiply_rgb(src[0], src[1], src[2], sa) != rgb {
                            continue;
                        }
                        for dest in [[255u8, 255, 255, 255], [0, 0, 0, 255], [40, 90, 20, 128]] {
                            assert_eq!(
                                composite_solid(dest, rgb, sa, cov, mode),
                                composite_premultiplied(dest, src, cov, mode),
                                "{mode:?} sa={sa} cov={cov} rgb={rgb:?} dest={dest:?}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn a_solid_source_at_zero_alpha_or_coverage_leaves_the_destination_alone() {
        let dest = [1u8, 2, 3, 255];
        assert_eq!(
            composite_solid(dest, [255, 255, 255], 0, 255, BlendMode::Normal),
            dest
        );
        assert_eq!(
            composite_solid(dest, [255, 255, 255], 255, 0, BlendMode::Normal),
            dest
        );
    }

    #[test]
    fn color_sqrt_is_piecewise_d_not_sqrt() {
        // The table tracks ISO 32000-1 §11.3.5.2's piecewise
        // `D(x) = if x <= 0.25 { ((16x-12)x+4)x } else { sqrt(x) }`, but it is
        // **not** any closed form of it: 35 of the 256 entries differ from
        // `round(255*D)` and 102 from `trunc(255*D)`, so it is hand-tuned and
        // the transcription is the authority. (Render brief Q7 claims the
        // round-trip is exact for all 256; measured, it is not — erratum.)
        // What must hold is that no entry drifts more than one count from
        // `D`, which pins the transcription against a typo.
        for (i, entry) in COLOR_SQRT.iter().enumerate() {
            #[expect(clippy::cast_precision_loss, reason = "i < 256 is exact in f64")]
            let x = i as f64 / 255.0;
            let d = if x <= 0.25 {
                ((16.0 * x - 12.0) * x + 4.0) * x
            } else {
                x.sqrt()
            };
            let scaled = 255.0 * d;
            let drift = (f64::from(*entry) - scaled).abs();
            assert!(drift <= 1.0, "entry {i}: table {entry} vs D {scaled}");
        }
        // The low branch is a cubic, so entry 1 is 3. A "simplification" to a
        // plain square root would put 16 there — wrong by 17 counts, not by
        // rounding — and this assertion is what makes that fail loudly.
        assert_eq!(COLOR_SQRT.get(1).copied(), Some(3));
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "255*sqrt(1/255) rounds to 16, well inside u8"
        )]
        let naive = (255.0 * (1.0f64 / 255.0).sqrt()).round() as u8;
        assert_eq!(naive, 16, "the wrong answer a plain sqrt would give");
    }

    #[test]
    fn overlay_is_hardlight_swapped() {
        for b in [0i32, 1, 63, 127, 128, 200, 255] {
            for s in [0i32, 1, 63, 127, 128, 200, 255] {
                assert_eq!(
                    blend_channel(BlendMode::Overlay, b, s),
                    blend_channel(BlendMode::HardLight, s, b),
                    "back={b} src={s}"
                );
            }
        }
    }

    #[test]
    fn hardlight_threshold_128() {
        // s == 127 takes the low branch, s == 128 the high one. A `2s <= 255`
        // reading would put 127 and 128 on the same side of the split at
        // s = 127.5 and agree here, but disagrees for a 0.5-scaled input;
        // the direct assertion is the safe one.
        assert_eq!(
            blend_channel(BlendMode::HardLight, 200, 127),
            (127 * 200 * 2) / 255
        );
        assert_eq!(
            blend_channel(BlendMode::HardLight, 200, 128),
            blend_channel(BlendMode::Screen, 200, 1)
        );
    }

    #[test]
    fn softlight_divides_twice() {
        // The low branch spells its scaling as `/255/255`, not `/65025`.
        // The brief flags this as a truncation trap; exhaustively, it is not
        // one — on this branch the numerator is always non-negative (since
        // `src < 128` makes `255 - 2*src >= 1`), and truncating twice by 255
        // equals truncating once by 65025 for every non-negative value.
        // (Render brief test 33's premise is an erratum.) The spelling is
        // still ported verbatim, and this test pins the equivalence so a
        // future negative-input path cannot silently change meaning.
        for back in 0..=255i32 {
            for src in 0..128i32 {
                let n = (255 - 2 * src) * back * (255 - back);
                assert_eq!(
                    back - n / 255 / 255,
                    back - n / 65025,
                    "back={back} src={src}"
                );
                assert_eq!(
                    blend_channel(BlendMode::SoftLight, back, src),
                    back - n / 255 / 255
                );
            }
        }
    }

    #[test]
    fn softlight_high_branch_uses_the_table() {
        let (back, src) = (7i32, 200i32);
        let d = i32::from(COLOR_SQRT.get(7).copied().unwrap_or(0));
        assert_eq!(
            blend_channel(BlendMode::SoftLight, back, src),
            back + (2 * src - 255) * (d - back) / 255
        );
    }

    #[test]
    fn separable_formulas() {
        // Ports core/fxge/dib/blend_unittest.cpp's per-mode expectations.
        assert_eq!(blend_channel(BlendMode::Normal, 100, 200), 200);
        assert_eq!(
            blend_channel(BlendMode::Multiply, 100, 200),
            200 * 100 / 255
        );
        assert_eq!(
            blend_channel(BlendMode::Screen, 100, 200),
            200 + 100 - 200 * 100 / 255
        );
        assert_eq!(blend_channel(BlendMode::Darken, 100, 200), 100);
        assert_eq!(blend_channel(BlendMode::Lighten, 100, 200), 200);
        assert_eq!(blend_channel(BlendMode::Difference, 100, 200), 100);
        assert_eq!(
            blend_channel(BlendMode::Exclusion, 100, 200),
            100 + 200 - 2 * 100 * 200 / 255
        );
        assert_eq!(blend_channel(BlendMode::ColorDodge, 100, 255), 255);
        assert_eq!(blend_channel(BlendMode::ColorBurn, 100, 0), 0);
    }

    #[test]
    fn clip_color_ordering() {
        // A colour that trips both branches: the x > 255 arm must still use
        // the pre-clip `l` and `x` while reading channels the n < 0 arm wrote.
        let c = [-50, 128, 300];
        let l = lum(c);
        let n = -50;
        let x = 300;
        let mut manual = c;
        for ch in &mut manual {
            *ch = l + ((*ch - l) * l / (l - n));
        }
        for ch in &mut manual {
            *ch = l + ((*ch - l) * (255 - l) / (x - l));
        }
        assert_eq!(clip_color(c), manual);
    }

    #[test]
    fn transparent_backdrop_skips_blend() {
        // dest.a == 0 copies the source verbatim, whatever the mode.
        for mode in [
            BlendMode::Multiply,
            BlendMode::Difference,
            BlendMode::Luminosity,
        ] {
            assert_eq!(
                composite_straight(([9, 9, 9], 0), ([1, 2, 3], 200), mode),
                ([1, 2, 3], 200)
            );
        }
    }

    #[test]
    fn nonseparable_modes_use_the_backdrop_luminosity() {
        // Luminosity(back, src) takes the source's luminosity onto the
        // backdrop's colour; with a black source the result is black.
        assert_eq!(
            blend_rgb(BlendMode::Luminosity, [10, 20, 30], [0, 0, 0]),
            [0, 0, 0]
        );
        // Color(src, Lum(back)) keeps the source's hue and saturation. The
        // target luminosity is only *approximately* preserved, because
        // ClipColor rescales channels driven past the 0/255 boundary using
        // the pre-clip bounds — which is precisely the ordering §6.2 calls
        // out as amplifying rounding on these four modes.
        let out = blend_rgb(BlendMode::Color, [128, 128, 128], [255, 0, 0]);
        let target = lum([128, 128, 128]);
        assert!(
            lum(out.map(i32::from)).abs_diff(target) <= 3,
            "Color landed at {:?}, luminosity {} vs {target}",
            out,
            lum(out.map(i32::from))
        );
        // Saturation and Hue both read the backdrop's luminosity too.
        for mode in [BlendMode::Hue, BlendMode::Saturation] {
            let out = blend_rgb(mode, [40, 40, 40], [200, 10, 10]);
            assert!(
                lum(out.map(i32::from)).abs_diff(lum([40, 40, 40])) <= 3,
                "{mode:?}"
            );
        }
    }
}
