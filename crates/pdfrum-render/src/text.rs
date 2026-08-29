//! Text as filled glyph outlines (`ProcessText`,
//! `cpdf_renderstatus.cpp:824-930`, and `CFX_RenderDevice::DrawTextPath`).
//!
//! # Two paths, and which one a run takes
//!
//! The oracle draws small text and large text differently, and so does this.
//! Below `|char2device.a| + |char2device.b| > 50` it rasterizes a *glyph
//! bitmap* and blits it at a snapped origin; above, it fills the outline at
//! its true position through `DrawTextPath`. [`takes_bitmap_path`] is that
//! threshold and [`snaps_origins`] the three gates around it.
//!
//! This module lays glyphs out and decides which path each run takes. It does
//! not rasterize: the bitmap pipeline is [`crate::glyph`], which reproduces
//! all four of the oracle's stages — the 64-ppem grid fit, the 3×-wide LCD
//! rasterization, FreeType's FIR5 filter, and `kTextGammaAdjust` over the
//! averaged triples. A run that takes it carries a [`BitmapPlacement`]; one
//! that does not is drawn by filling [`PlacedGlyph::outline`].
//!
//! [`RenderOptions::subpixel_text_positioning`](crate::options::RenderOptions::subpixel_text_positioning)
//! turns the bitmap path off for a caller who wants text where the PDF puts it
//! rather than where a golden expects it.

use kurbo::{Affine, BezPath, Vec2};

use crate::options::{RenderOptions, TextAa};
use pdfrum_font::{CharItem, Font, GlyphCache, GlyphKey, cid_transform_to_float};
use pdfrum_page::{TextObject, TextRenderMode};

/// Which of fill, stroke and clip a text render mode asks for
/// (`cpdf_renderstatus.cpp:752-761`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextPaintKinds {
    /// Whether the glyphs are filled.
    pub fill: bool,
    /// Whether they are stroked.
    pub stroke: bool,
    /// Whether they contribute to the clip.
    pub clip: bool,
}

/// Resolve a text render mode into what it paints.
///
/// `has_face` is whether the font has real outlines: a stroke-only mode on a
/// font without them **falls back to a fill**, which is upstream's own
/// substitution and not a rounding of intent.
#[must_use]
pub fn paint_kinds(mode: TextRenderMode, has_face: bool) -> Option<TextPaintKinds> {
    let none = TextPaintKinds {
        fill: false,
        stroke: false,
        clip: false,
    };
    match mode {
        // Tr 3: nothing at all, not even a clip contribution.
        TextRenderMode::Invisible => None,
        // Tr 7: clip only, no paint. Returns early in the C++ *before* the
        // clip accumulation, so it paints nothing here either.
        TextRenderMode::Clip => Some(TextPaintKinds { clip: true, ..none }),
        TextRenderMode::Fill => Some(TextPaintKinds { fill: true, ..none }),
        TextRenderMode::FillClip => Some(TextPaintKinds {
            fill: true,
            clip: true,
            ..none
        }),
        TextRenderMode::Stroke => Some(if has_face {
            TextPaintKinds {
                stroke: true,
                ..none
            }
        } else {
            TextPaintKinds { fill: true, ..none }
        }),
        TextRenderMode::StrokeClip => Some(if has_face {
            TextPaintKinds {
                stroke: true,
                clip: true,
                ..none
            }
        } else {
            TextPaintKinds {
                fill: true,
                clip: true,
                ..none
            }
        }),
        TextRenderMode::FillStroke => Some(TextPaintKinds {
            fill: true,
            stroke: has_face,
            ..none
        }),
        TextRenderMode::FillStrokeClip => Some(TextPaintKinds {
            fill: true,
            stroke: has_face,
            clip: true,
        }),
    }
}

/// Where a snapped glyph's bitmap goes, and which of its three phases to
/// average.
///
/// Present exactly when the run takes the oracle's glyph-*bitmap* path, which
/// is what [`snaps_origins`] decides. A run that does not — display type, a
/// stroke, a caller who asked for fractional placement — carries `None` and is
/// drawn by filling [`PlacedGlyph::outline`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BitmapPlacement {
    /// The whole-pixel device origin the bitmap's own origin lands on.
    ///
    /// `floor(x)` and `round(y)`: the two halves of the oracle's snap, kept
    /// separate from the third-of-a-pixel remainder rather than folded into one
    /// number, because the bitmap is blitted at the integer and the remainder
    /// selects a *different bitmap* rather than moving this one.
    pub origin: kurbo::Point,
    /// Which third of a pixel the true origin sat in.
    pub phase: crate::glyph::SubpixelPhase,
}

/// One glyph, placed.
#[derive(Debug, Clone)]
pub struct PlacedGlyph {
    /// The outline in 1000-unit text space, straight from the cache.
    pub outline: BezPath,
    /// Text space to device space for this glyph, font size included.
    ///
    /// For a snapped glyph this is the matrix *before* the snap: the snap is
    /// [`Self::bitmap`]'s integer origin, and folding it in here as well would
    /// apply it twice. For an unsnapped one it is simply where the glyph goes.
    pub matrix: Affine,
    /// The cache key the outline came back under, which the bitmap path needs
    /// to key its own cache by.
    pub key: GlyphKey,
    /// Where the bitmap goes, when this run takes the bitmap path.
    pub bitmap: Option<BitmapPlacement>,
}

impl PlacedGlyph {
    /// The outline in device space.
    #[must_use]
    pub fn device_path(&self) -> BezPath {
        self.matrix * self.outline.clone()
    }
}

/// The matrix one glyph is drawn under.
///
/// Three spaces compose. Outlines arrive scaled to **1000 units per em**, so
/// the font size divides by a thousand to reach text space. `pen` is the
/// glyph's origin in that same text space, where the pen advances left to
/// right and y grows *upward*. And `text_to_device` — which is
/// `pdfrum-page`'s `TextObject::matrix`, already carrying the CTM, the text
/// matrix and the horizontal scale, composed with the page-to-device
/// transform — takes it the rest of the way.
///
/// There is deliberately **no y flip here**: PDF text space and PDF user
/// space share their orientation, and the single flip that turns y-up into a
/// y-down device lives in the page matrix, where every object kind sees it.
/// Flipping again per glyph mirrors every letter about its own baseline.
#[must_use]
pub fn glyph_matrix(font_size: f32, pen: kurbo::Point, text_to_device: Affine) -> Affine {
    let s = f64::from(font_size) / 1000.0;
    text_to_device * Affine::translate((pen.x, pen.y)) * Affine::scale(s)
}

/// What the Adobe-Japan1 per-CID transform does to one glyph.
///
/// Two separate things, which is why it is not simply a matrix: a shift of
/// the drawing **origin** in text space, and a reshaping **matrix** applied
/// to the outline inside its em box.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Japan1Adjust {
    /// Added to the pen for this glyph only — never to the running pen.
    pub origin: Vec2,
    /// Post-multiplied onto the glyph matrix, so it acts in em space.
    pub matrix: Affine,
}

impl Japan1Adjust {
    /// No adjustment: the identity matrix and no shift.
    const NONE: Self = Self {
        origin: Vec2::new(0.0, 0.0),
        matrix: Affine::IDENTITY,
    };
}

/// The Japan1 adjustment one decoded character takes
/// (`CPDF_Font::GetCharPosList`, `cpdf_font.cpp:482-498`).
///
/// A non-embedded Japanese font is substituted onto a face that has only
/// upright glyphs, so PDFium carries a hand-tuned 154-row table of per-CID
/// transforms and applies them itself. It is *not* only for vertical writing:
/// the gate is the charset and the absence of a font program
/// (`CPDF_CIDFont::GetCIDTransform`, `cpdf_cidfont.cpp:878-887`), and a
/// horizontal CMap such as `/90pv-RKSJ-H` reaches the listed CIDs perfectly
/// well. `bug_1402.pdf` is exactly that file, and without this its three
/// ideographic full stops land 22 device pixels left and 27 up — right shape,
/// right ink, wrong side of the em box.
///
/// **The advance is untouched.** Upstream mutates a per-glyph `origin_`, not
/// the running pen, so a run's spacing is the same with the transform as
/// without it and only each glyph's own placement moves.
///
/// `is_vertical_glyph` suppresses it: a `GSUB` `vert` substitution has already
/// produced a rotated form, and rotating it again is the one way to make this
/// worse than not applying it.
///
/// The C++ additionally scales `a` and `b` by the glyph-spacing heuristic's
/// `scaling_factor` (`cpdf_font.cpp:449-467`). That heuristic is not ported —
/// it fires only when a PDF's declared width is narrower than the face's own
/// glyph — and where it does not fire the factor is exactly 1, which is the
/// case every corpus file with a Japan1 transform is in.
#[must_use]
pub fn japan1_adjust(font: &Font, item: &CharItem, font_size: f32) -> Japan1Adjust {
    if item.vertical_glyph {
        return Japan1Adjust::NONE;
    }
    let Some(t) = font.japan1_transform(item.code) else {
        return Japan1Adjust::NONE;
    };
    let f = |b: u8| f64::from(cid_transform_to_float(b));
    Japan1Adjust {
        origin: Vec2::new(f(t.e) * f64::from(font_size), f(t.f) * f64::from(font_size)),
        // The packed order is the PDF matrix's own: `a b c d`.
        matrix: Affine::new([f(t.a), f(t.b), f(t.c), f(t.d), 0.0, 0.0]),
    }
}

/// The size threshold above which the oracle abandons glyph bitmaps for
/// outline fills (`cfx_renderdevice.cpp:1240`).
///
/// `char2device` is the text-to-device matrix scaled by `(font_size,
/// -font_size)`, so `|a| + |b|` is roughly the em's device width. Above 50
/// device units `DrawNormalText` hands the run to `DrawTextPath`, which
/// places every glyph at its true fractional origin — so the integer snap is
/// a *small-text* rule and large display type is unaffected by it either way.
pub const BITMAP_PATH_MAX_EM: f64 = 50.0;

/// Whether a run of this size takes the oracle's glyph-*bitmap* path, and so
/// gets its origins snapped (`cfx_renderdevice.cpp:1240-1246`).
///
/// The C++ spelling is `fabs(char2device.a) + fabs(char2device.b) > 50 * 1.0f
/// || is_printer`, with the `> 50` arm *leaving* the bitmap path. `is_printer`
/// is false for every `pdfium_test --png` render, and the `font->HasFace()`
/// guard inside it only matters for a face with no outlines at all — which in
/// this engine is a type-3 font, and type-3 text never reaches here.
#[must_use]
pub fn takes_bitmap_path(font_size: f32, text_to_device: Affine) -> bool {
    // char2device = text2device * Scale(font_size, -font_size); a column-major
    // `Affine` holds [a, b, c, d, e, f], and scaling post-multiplies, so
    // a' = a * font_size and b' = b * font_size.
    let [a, b, ..] = text_to_device.as_coeffs();
    let size = f64::from(font_size);
    (a * size).abs() + (b * size).abs() <= BITMAP_PATH_MAX_EM
}

/// Snap one glyph's device origin to the grid the oracle blits its bitmap
/// on (`cfx_renderdevice.cpp:1254-1257` **and** `1352`).
///
/// # The x grid is thirds of a pixel, not whole pixels
///
/// Reading only the snap itself is misleading, and burn-down wave 4 read only
/// the snap:
///
/// ```cpp
/// glyph.origin_.x = anti_alias_is_lcd ? static_cast<int>(floor(x))
///                                     : FXSYS_roundf(x);
/// glyph.origin_.y = FXSYS_roundf(y);
/// ```
///
/// Under `kLcd` the integer `origin_.x` is only *half* of the horizontal
/// placement. The blit loop recovers the rest:
///
/// ```cpp
/// int x_subpixel = static_cast<int>(glyph.device_origin_.x * 3) % 3;
/// ```
///
/// and `DrawNormalTextHelper` shifts its window into the 3×-wide LCD bitmap
/// by that many subpixels before averaging the triples back down. So the
/// effective origin is `floor(x) + x_subpixel/3`, which for a non-negative x
/// is exactly `floor(3x)/3`: **x is quantised downward to a third of a
/// pixel.** Only y is quantised to a whole pixel, and that asymmetry is the
/// whole of the placement divergence.
///
/// Wave 4's measurement stands — the residual really is positional, and it
/// really is dominated by the baseline — because y is where the whole-pixel
/// quantisation lives, and a horizontal stem edge is what y moves.
///
/// # Which rounding runs is `FontAntiAliasingMode`, not `bClearType`
///
/// `DrawNormalText` derives that mode itself
/// (`cfx_renderdevice.cpp:1165-1206`): with a smooth aliasing type on a
/// display device at 32 bpp it is always `kLcd`, whatever the flag word said,
/// so the conformance configuration takes the thirds. `--no-smoothtext` sets
/// `aliasing_type = kAliasing`, `IsSmooth()` is then false, the whole
/// derivation is skipped and the mode stays at its `kMono` initialiser — one
/// bit per pixel, no LCD triple to shift into, so x snaps to a whole pixel
/// through `FXSYS_roundf` like y. Hence the argument here is [`TextAa`]
/// rather than a bare "is LCD" boolean: the two are the same decision.
///
/// `round` is `FXSYS_roundf`, which is C `round`: half away from zero, unlike
/// Rust's `round_ties_even`.
#[must_use]
pub fn snap_origin(origin: kurbo::Point, text_aa: TextAa) -> kurbo::Point {
    let x = match text_aa {
        // kLcd: `floor(x)` plus `(int)(x * 3) % 3` thirds. The C++ `(int)`
        // truncates toward zero and `%` keeps the sign, so this is written
        // the way the C++ computes it rather than as `(3x).floor() / 3`,
        // which differs on a negative origin — where upstream's negative
        // `x_subpixel` falls into the `x_subpixel == 2` arm.
        TextAa::Grayscale => {
            let whole = origin.x.floor();
            #[expect(
                clippy::cast_possible_truncation,
                reason = "the C++ is `static_cast<int>(x * 3) % 3`; a device \
                          origin beyond i32 has already been clamped by the \
                          ±32000 coordinate rule"
            )]
            let subpixel = f64::from((origin.x * 3.0) as i32 % 3);
            whole + subpixel / 3.0
        }
        // kMono: nearest, ties away from zero.
        TextAa::None => origin.x.round(),
    };
    kurbo::Point::new(x, origin.y.round())
}

/// Pull one glyph's origins back together after snapping, when consecutive
/// integer origins have drifted more than half a pixel from the fractional
/// spacing they came from (`AdjustGlyphSpace`,
/// `cfx_renderdevice.cpp:57-98`).
///
/// It runs **only when the mode is not LCD and the run has more than one
/// glyph** (`cfx_renderdevice.cpp:1265-1267`), which under the conformance
/// flags means it never runs at all — only `--no-smoothtext` reaches it. The
/// rule is deliberately conservative: it gives up entirely unless the run is
/// axis-aligned (every origin sharing an x, or every origin sharing a y after
/// the snap), and it never touches the first or last glyph.
///
/// Note the loop bound. The C++ walks `i` from `size - 1` down to `2`
/// exclusive and edits `glyphs[i - 1]`, so glyph 0 is never adjusted and the
/// *last* glyph is only ever read. That asymmetry is upstream's, not a
/// transcription slip, and it is why a two-glyph run is a no-op even though
/// the size guard admits it.
#[expect(
    clippy::float_cmp,
    reason = "the C++ compares snapped origins, which are whole pixels there \
              and exact integers in this f64 after `snap_origin` — an epsilon \
              would admit a run the oracle rejects as non-axis-aligned"
)]
pub fn adjust_glyph_space(origins: &mut [kurbo::Point], device: &[kurbo::Point]) {
    debug_assert_eq!(origins.len(), device.len());
    let (Some(first), Some(last)) = (origins.first().copied(), origins.last().copied()) else {
        return;
    };
    if origins.len() <= 1 {
        return;
    }
    let vertical = last.x == first.x;
    if !vertical && last.y != first.y {
        return;
    }
    // Reading one axis of a point, chosen once for the whole run.
    let axis = |p: kurbo::Point| if vertical { p.y } else { p.x };

    for i in (2..origins.len()).rev() {
        let (Some(next_origin), Some(next_f)) = (origins.get(i), device.get(i)) else {
            continue;
        };
        let (Some(cur_origin), Some(cur_f)) = (origins.get(i - 1), device.get(i - 1)) else {
            continue;
        };
        let space = axis(*next_origin) - axis(*cur_origin);
        let space_f = axis(*next_f) - axis(*cur_f);
        // The fractional spacing exceeds the integer one by more than half a
        // pixel, so the snap has stretched this gap: close it by a pixel.
        if space_f.abs() - space.abs() <= 0.5 {
            continue;
        }
        let nudge = if space > 0.0 { -1.0 } else { 1.0 };
        if let Some(target) = origins.get_mut(i - 1) {
            if vertical {
                target.y += nudge;
            } else {
                target.x += nudge;
            }
        }
    }
}

/// The stroked-text CTM un-transform (`cpdf_renderstatus.cpp:772-777`).
///
/// A stroke's width must be measured in *text* space, so when the text
/// state's CTM carries a non-unit x or y scale the text matrix is pre-divided
/// by it and the scale is folded into the device matrix instead. Returns the
/// adjusted `(text_matrix, device_matrix)` pair.
#[must_use]
#[expect(
    clippy::float_cmp,
    reason = "the exact `a == 1 && d == 1` is upstream's unit-scale short \
              circuit; with a tolerance a slightly-off-unit CTM would skip \
              the split and stroke at the wrong width, which is the whole \
              point of the function"
)]
pub fn stroke_ctm_split(text_matrix: Affine, to_device: Affine, ctm: [f64; 4]) -> (Affine, Affine) {
    let [a, b, c, d] = ctm;
    if a == 1.0 && d == 1.0 {
        return (text_matrix, to_device);
    }
    let scale = Affine::new([a, b, c, d, 0.0, 0.0]);
    let det = scale.determinant();
    if det == 0.0 || !det.is_finite() {
        return (text_matrix, to_device);
    }
    (text_matrix * scale.inverse(), scale * to_device)
}

/// Lay out one text object's glyphs.
///
/// # The two coordinate systems this has to keep straight
///
/// `pdfrum-page` hands a text object *two* pieces of placement, and they are
/// in different spaces. `TextObject::matrix` is `ctm * text_matrix *
/// horizontal_scale` — **text space to page space**, with no font size —
/// while `TextObject::position` is `ctm * text_matrix` already applied to
/// the pen, i.e. a **page-space** point.
///
/// Composing the two naively applies the matrix twice, which shifts the run
/// wherever the text matrix has a translation and is invisible wherever it
/// does not — so it survives every fixture whose `Tm` is the identity. The
/// run's origin is therefore recovered by pulling `position` *back* through
/// the matrix, and the pen then advances in text space where the advances
/// are actually defined.
///
/// Advances follow the same rules `pdfrum-page` used to build the object:
/// each code's width scaled by the font size, plus the character spacing,
/// plus the word spacing on a single-byte space only, plus any kerning
/// between segments. The horizontal scale is *not* applied again, because
/// `matrix` already carries it.
///
/// # The glyph origins are then snapped
///
/// Unless [`RenderOptions::subpixel_text_positioning`] asks otherwise, a run
/// the oracle would draw through `DrawNormalText` has every glyph's device
/// origin pushed onto the blit grid by [`snap_origin`], per glyph and
/// independently. The snap is applied as a *device translation* on top of the
/// glyph's matrix, so the outline keeps its own shape and orientation and
/// only its placement moves — which is what blitting a bitmap at a fixed
/// origin amounts to.
///
/// **Three conditions gate it, all of them upstream's**
/// (`ProcessText`, `cpdf_renderstatus.cpp:905-928`, and
/// `cfx_renderdevice.cpp:1240-1246`), and each of them is a run the oracle
/// itself places fractionally:
///
/// - the run is **not stroked** — `if (is_clip || is_stroke)` takes
///   `DrawTextPath`, which places every glyph at its true origin. A
///   pattern-coloured one takes `DrawTextPathWithPattern` and never reaches
///   here at all, so `render_text` has already excluded it. Note that
///   `is_clip` is **not** a text render mode: `ProcessText` is called twice,
///   once from `ProcessClipPath` with a `clipping_path` and once from
///   `ProcessObjectNoClip` with `nullptr`, and only the first sets it. So a
///   `Tr 4` fill-and-clip run still snaps on its painting pass, and it is the
///   clip *accumulation* — which this engine builds in `pdfrum-page`, not
///   here — that does not.
/// - the run is **small**, `|char2device.a| + |char2device.b| <= 50`
///   ([`takes_bitmap_path`]); above that `DrawNormalText` itself defers to
///   `DrawTextPath`.
/// - the caller has not asked for [`RenderOptions::subpixel_text_positioning`].
///
/// `AdjustGlyphSpace` then runs over the whole run, but only in the non-LCD
/// mode, which is `--no-smoothtext` and not conformance. See
/// [`adjust_glyph_space`].
#[must_use]
pub fn place_glyphs(
    object: &TextObject,
    state: &pdfrum_page::GraphicsState,
    cache: &mut GlyphCache,
    to_device: Affine,
    opts: &RenderOptions,
    kinds: TextPaintKinds,
) -> Vec<PlacedGlyph> {
    let Some((font, size)) = &object.font else {
        return Vec::new();
    };
    let text_to_device = to_device * object.matrix;
    // Pull the page-space start back into the text space the advances live
    // in. A singular text matrix has no text space to speak of, and the
    // object would not have been drawable anyway.
    let det = object.matrix.determinant();
    if det == 0.0 || !det.is_finite() {
        return Vec::new();
    }
    let mut pen = object.matrix.inverse() * object.position;
    let mut out = Vec::new();
    let subst_weight = font.subst().map_or(0, pdfrum_font::SubstFont::raw_weight);
    let subst_italic = font.subst().map_or(0, |s| s.italic_angle);
    let widths_drive_the_design = width_drives_the_design_space(font);

    for segment in &object.segments {
        // A kerning adjustment shifts the pen before the segment it precedes,
        // and is negated: a positive `TJ` number moves text *left*.
        pen.x -= f64::from(segment.kerning) / 1000.0 * f64::from(*size);
        for item in font.decode(&segment.codes) {
            let advance = f64::from(item.width) / 1000.0 * f64::from(*size)
                + f64::from(state.text.char_space);
            // Word spacing applies to a single-byte space only, which is why
            // a CID-keyed code of 0x20 does not earn it.
            let word = if item.code.0 == 0x20 && item.cid.is_none() {
                f64::from(state.text.word_space)
            } else {
                0.0
            };
            if let Some(gid) = item.glyph() {
                let key = GlyphKey {
                    font: font.id(),
                    gid,
                    dest_width: if widths_drive_the_design {
                        #[expect(
                            clippy::cast_possible_truncation,
                            reason = "`GetCharWidth` is an int on the C++ side \
                                      and a /Widths entry is a small number; \
                                      the f32 is this crate's own carrier"
                        )]
                        {
                            item.width as i32
                        }
                    } else {
                        0
                    },
                    weight: subst_weight,
                    italic_angle: subst_italic,
                    vertical: item.vertical_glyph,
                };
                if let Some(outline) = cache.path(font, key) {
                    // The Japan1 per-CID transform moves and reshapes the
                    // glyph *within* its em box without touching the advance,
                    // so it applies to this glyph's origin and matrix and the
                    // pen walks on as if it were not there.
                    let adjust = japan1_adjust(font, &item, *size);
                    out.push(PlacedGlyph {
                        outline: outline.clone(),
                        matrix: glyph_matrix(*size, pen + adjust.origin, text_to_device)
                            * adjust.matrix,
                        key,
                        bitmap: None,
                    });
                }
            }
            pen.x += advance + word;
        }
    }
    if snaps_origins(opts, kinds, *size, text_to_device) {
        snap_run(&mut out, opts.text_aa);
    }
    out
}

/// Whether the PDF's own `/Widths` reach the glyph's *outline* rather than
/// only its advance (`CPDF_Font::GetCharPosList`, `cpdf_font.cpp:440-444`).
///
/// ```cpp
/// if (!IsEmbedded() && !IsCIDFont()) {
///   text_char_pos.font_char_width_ = GetCharWidth(char_code);
/// } else {
///   text_char_pos.font_char_width_ = 0;
/// }
/// ```
///
/// `font_char_width_` becomes `dest_width` at the face, and there
/// `AdjustVariationParams` (`cfx_face.cpp:941-943, 1561-1605`) solves a
/// **Multiple-Master** face's width axis until the glyph's own advance equals
/// it. The two internal generics the substitution ladder terminates on —
/// Chrome Sans and Chrome Serif — *are* MM Type 1 faces, so this is not a
/// corner: it is how every non-embedded font in the corpus gets drawn at the
/// width its PDF declares rather than at the fallback face's own.
///
/// Skipping it drew every substituted glyph at the face's default design
/// position. On `5.5_simple_font.pdf` — whose whole point is a `/Widths`
/// array with values like `a = 800, b = 100, c = 400` against a face whose
/// own are 452, 470 and 480 — the glyphs overran their advances and piled
/// into each other, which read as dropped characters and as an MM band drawn
/// "too narrow". Both were the same defect: the advances were always right
/// and the outlines were always wrong.
///
/// The gate is exactly upstream's, and both halves matter. An **embedded**
/// font is drawn from its own program, where the PDF's width is metadata and
/// not a design parameter. A **CID** font's `dest_width` is zeroed even when
/// substituted, because a CID font's widths are keyed by CID rather than by
/// character code and the C++ declines to reconcile the two.
#[must_use]
pub fn width_drives_the_design_space(font: &Font) -> bool {
    !font.is_embedded() && !matches!(font, Font::Type0(_))
}

/// Whether this run's origins are snapped: the three gates named on
/// [`place_glyphs`].
#[must_use]
pub fn snaps_origins(
    opts: &RenderOptions,
    kinds: TextPaintKinds,
    font_size: f32,
    text_to_device: Affine,
) -> bool {
    !opts.subpixel_text_positioning && !kinds.stroke && takes_bitmap_path(font_size, text_to_device)
}

/// Snap a whole laid-out run onto the oracle's blit grid.
///
/// Split out from [`place_glyphs`] because the two halves are separable and
/// only this one is the oracle's placement rule: the layout above is where
/// the glyphs *are*, and this is where the oracle *draws* them.
///
/// Two things come out of it, and keeping them apart is the point.
/// [`PlacedGlyph::matrix`] gets the snap folded in as a device translation, so
/// that filling the outline lands where the oracle blits — which is what the
/// pre-wave-7 engine did and what still runs whenever a bitmap cannot be
/// produced. And [`PlacedGlyph::bitmap`] gets the *integer* origin together
/// with the third-of-a-pixel remainder as a phase, because the bitmap path does
/// not translate by a third of a pixel: it averages a different window of the
/// same 3×-wide bitmap, which is a different set of bytes rather than the same
/// bytes moved.
fn snap_run(glyphs: &mut [PlacedGlyph], text_aa: TextAa) {
    if glyphs.is_empty() {
        return;
    }
    // Each glyph's matrix maps the outline's own origin to the device, so the
    // device origin is simply the matrix applied to the origin point.
    let device: Vec<kurbo::Point> = glyphs
        .iter()
        .map(|g| g.matrix * kurbo::Point::ZERO)
        .collect();
    let mut snapped: Vec<kurbo::Point> = device.iter().map(|p| snap_origin(*p, text_aa)).collect();
    // `AdjustGlyphSpace` is guarded on the mode *not* being LCD, so it is
    // reachable only under `--no-smoothtext`.
    if text_aa == TextAa::None {
        adjust_glyph_space(&mut snapped, &device);
    }
    for ((glyph, from), to) in glyphs.iter_mut().zip(&device).zip(&snapped) {
        let delta = Vec2::new(to.x - from.x, to.y - from.y);
        // Pre-multiplying translates in *device* space, which is where the
        // snap happens. Folding it into the glyph matrix instead would scale
        // and rotate the nudge by the text matrix.
        glyph.matrix = Affine::translate(delta) * glyph.matrix;
        glyph.bitmap = bitmap_placement(*from, *to, text_aa);
    }
}

/// The bitmap origin and phase for one glyph, or `None` in the mono mode,
/// which has no LCD bitmap to shift a window into.
///
/// `snapped.x` is `floor(x) + phase/3` under `kLcd`, so the integer origin is
/// its own floor — recovered from the snapped value rather than recomputed from
/// the true one, because `AdjustGlyphSpace` may have moved it and the two must
/// not disagree.
fn bitmap_placement(
    device: kurbo::Point,
    snapped: kurbo::Point,
    text_aa: TextAa,
) -> Option<BitmapPlacement> {
    if text_aa != TextAa::Grayscale {
        // `kMono` renders a 1-bit mask through `CompositeOneBPPMask`, not an
        // LCD triple, and it is reachable only under `--no-smoothtext`. Left on
        // the outline path, where the whole-pixel snap above already places it.
        return None;
    }
    Some(BitmapPlacement {
        origin: kurbo::Point::new(snapped.x.floor(), snapped.y),
        phase: crate::glyph::SubpixelPhase::of(device.x),
    })
}

/// Whether a font has real outlines, which decides the stroke-to-fill
/// fallback above.
#[must_use]
pub fn has_face(font: &Font) -> bool {
    !matches!(font, Font::Type3(_))
}

/// One Type 3 character, placed.
///
/// A Type 3 glyph is a *content stream*, not an outline, so what a placement
/// yields is the character's code — with which the caller looks up the
/// procedure's objects — and the matrix taking glyph space to device space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlacedType3Char {
    /// The character code, keying `TextObject::type3_metrics`.
    pub code: u32,
    /// Glyph space to device space, `font_matrix * font_size` composed with
    /// the pen position and the text-to-device transform.
    pub matrix: Affine,
}

/// Lay out one Type 3 text object's characters.
///
/// The matrix differs from an ordinary glyph's in one way that matters: an
/// outline arrives pre-scaled to 1000 units per em, so [`glyph_matrix`]
/// divides the font size by a thousand. A Type 3 procedure's coordinates are
/// in **glyph space**, whose relationship to text space is stated by the
/// font's own `/FontMatrix` and is not a thousandth in general — a font may
/// declare any matrix at all, and several in the corpus do. So the font
/// matrix is composed in explicitly and the font size scales it, which is
/// `char_matrix = font_matrix scaled by (font_size, font_size)`.
///
/// Advances still come from `/Widths` on the thousandth convention, matching
/// how `pdfrum-page` computed the object's own advance; making the two
/// disagree would slide a Type 3 run relative to the pen the page recorded.
#[must_use]
pub fn place_type3_chars(
    object: &TextObject,
    state: &pdfrum_page::GraphicsState,
    to_device: Affine,
) -> Vec<PlacedType3Char> {
    let Some((font, size)) = &object.font else {
        return Vec::new();
    };
    let Some(type3) = font.type3() else {
        return Vec::new();
    };
    let text_to_device = to_device * object.matrix;
    let det = object.matrix.determinant();
    if det == 0.0 || !det.is_finite() {
        return Vec::new();
    }
    let char_matrix = type3.font_matrix * Affine::scale(f64::from(*size));
    let mut pen = object.matrix.inverse() * object.position;
    let mut out = Vec::new();

    for segment in &object.segments {
        pen.x -= f64::from(segment.kerning) / 1000.0 * f64::from(*size);
        for item in font.decode(&segment.codes) {
            let advance = f64::from(item.width) / 1000.0 * f64::from(*size)
                + f64::from(state.text.char_space);
            let word = if item.code.0 == 0x20 && item.cid.is_none() {
                f64::from(state.text.word_space)
            } else {
                0.0
            };
            out.push(PlacedType3Char {
                code: item.code.0,
                matrix: text_to_device * Affine::translate((pen.x, pen.y)) * char_matrix,
            });
            pen.x += advance + word;
        }
    }
    out
}

/// A text run's bounding rectangle in page space, or `None` when it is empty.
///
/// `CPDF_TextObject::CalcPositionDataInternal`'s bounding half
/// (`cpdf_textobject.cpp:288-357`): the pen walks the run exactly as
/// [`place_glyphs`] does, and each character's **glyph box** — not its
/// outline — grows the extent. Horizontal writing accumulates x from the pen
/// and y from the raw box; vertical writing swaps the two roles and offsets
/// each box by the character's vertical origin first. The finished box is then
/// scaled by the font size on the axis that was left in 1000/em units, and
/// mapped through the run's own matrix.
///
/// Only [`crate::walk`]'s pattern-text path wants this, and it wants it
/// because upstream fills that rectangle rather than the glyphs. The stroke
/// inflation `CalcPositionDataInternal` applies is deliberately absent: that
/// arm of `DrawTextPathWithPattern` draws glyph outlines instead and never
/// reads the rectangle at all.
#[must_use]
pub fn run_rect(object: &TextObject, state: &pdfrum_page::GraphicsState) -> Option<kurbo::Rect> {
    let (font, size) = object.font.as_ref()?;
    let vertical = font.is_vertical();
    let (mut min_x, mut max_x) = (f64::MAX, f64::MIN);
    let (mut min_y, mut max_y) = (f64::MAX, f64::MIN);
    let det = object.matrix.determinant();
    if det == 0.0 || !det.is_finite() {
        return None;
    }
    // The same start [`place_glyphs`] uses: the run's page-space origin pulled
    // back into the text space the advances live in. Upstream keeps the two
    // apart — `CalcPositionDataInternal` walks from zero and `GetTextMatrix`
    // carries the origin — and composing them here is the same arithmetic.
    let start = object.matrix.inverse() * object.position;
    let mut pen = start.x;
    let size = f64::from(*size);

    for segment in &object.segments {
        pen -= f64::from(segment.kerning) / 1000.0 * size;
        for item in font.decode(&segment.codes) {
            let bbox = font.char_bbox(item.code);
            if vertical {
                let (ox, oy) = font.vert_origin(item.code).unwrap_or((0.0, 880.0));
                let (left, right) = (bbox.x0 - f64::from(ox), bbox.x1 - f64::from(ox));
                let (top, bottom) = (bbox.y1 - f64::from(oy), bbox.y0 - f64::from(oy));
                min_x = min_x.min(left).min(right);
                max_x = max_x.max(left).max(right);
                for edge in [pen + top * size / 1000.0, pen + bottom * size / 1000.0] {
                    min_y = min_y.min(edge);
                    max_y = max_y.max(edge);
                }
            } else {
                min_y = min_y.min(bbox.y0).min(bbox.y1);
                max_y = max_y.max(bbox.y0).max(bbox.y1);
                for edge in [pen + bbox.x0 * size / 1000.0, pen + bbox.x1 * size / 1000.0] {
                    min_x = min_x.min(edge);
                    max_x = max_x.max(edge);
                }
            }
            pen += f64::from(item.width) / 1000.0 * size;
            // Word spacing on a single-byte space only, as everywhere else.
            if item.code.0 == 0x20 && item.cid.is_none() {
                pen += f64::from(state.text.word_space);
            }
            pen += f64::from(state.text.char_space);
        }
    }
    if min_x > max_x || min_y > max_y {
        return None;
    }
    // The axis still in 1000/em units takes the font size; the other already
    // has it, because the pen carried it.
    let (min_x, max_x, min_y, max_y) = if vertical {
        (
            start.x + min_x * size / 1000.0,
            start.x + max_x * size / 1000.0,
            min_y,
            max_y,
        )
    } else {
        (
            min_x,
            max_x,
            start.y + min_y * size / 1000.0,
            start.y + max_y * size / 1000.0,
        )
    };
    let rect = kurbo::Rect::new(min_x, min_y, max_x, max_y);
    let corners = [
        (rect.x0, rect.y0),
        (rect.x1, rect.y0),
        (rect.x1, rect.y1),
        (rect.x0, rect.y1),
    ]
    .map(|(x, y)| object.matrix * kurbo::Point::new(x, y));
    let mut out = kurbo::Rect::new(f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for p in corners {
        out.x0 = out.x0.min(p.x);
        out.y0 = out.y0.min(p.y);
        out.x1 = out.x1.max(p.x);
        out.y1 = out.y1.max(p.y);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    // The snapping tests assert *exact* placements — that is what the rule
    // being pinned is — and index fixtures whose length the fixture fixes.
    #![allow(
        clippy::float_cmp,
        clippy::indexing_slicing,
        reason = "a snapped origin is an exact value, and a tolerance here \
                  would let a wrong rounding pass"
    )]

    use kurbo::Point;

    use super::*;

    #[test]
    fn invisible_paints_nothing() {
        assert_eq!(paint_kinds(TextRenderMode::Invisible, true), None);
    }

    /// A run showing `text` at page-space `(x, y)`, in Helvetica at 20 pt.
    fn run(text: &[u8], x: f64, y: f64) -> TextObject {
        let font = std::sync::Arc::new(pdfrum_font::Font::load_standard(
            pdfrum_font::StandardFont::Helvetica,
            &pdfrum_font::FontCache::default(),
        ));
        TextObject {
            segments: Box::new([pdfrum_page::TextSegment {
                codes: text.to_vec().into_boxed_slice(),
                kerning: 0.0,
            }]),
            position: Point::new(x, y),
            matrix: Affine::IDENTITY,
            font: Some((font, 20.0)),
            render_mode: TextRenderMode::Fill,
            type3_metrics: std::collections::BTreeMap::new(),
        }
    }

    /// The rectangle `DrawTextPathWithPattern` fills is the run's own extent:
    /// it starts at the run's origin, grows rightward with the advances, and
    /// its height is the glyph boxes rather than the font size.
    #[test]
    fn a_runs_rect_starts_at_its_origin_and_spans_its_advances() {
        let state = pdfrum_page::GraphicsState::default();
        let short = run_rect(&run(b"H", 100.0, 50.0), &state).expect("a box");
        let long = run_rect(&run(b"HHHH", 100.0, 50.0), &state).expect("a box");
        assert!(
            (short.x0 - 100.0).abs() < 2.0,
            "starts at the origin: {short:?}"
        );
        assert!(short.y0 > 49.0 && short.y0 < 51.0, "sits on the baseline");
        assert!(short.y1 > 60.0, "rises to the cap height: {short:?}");
        assert!(
            long.width() > short.width() * 3.0,
            "four glyphs span four advances: {long:?} vs {short:?}"
        );
        // The origin moves the box and nothing else.
        let moved = run_rect(&run(b"H", 200.0, 50.0), &state).expect("a box");
        assert!((moved.x0 - short.x0 - 100.0).abs() < 1e-6);
        assert!((moved.width() - short.width()).abs() < 1e-6);
    }

    /// Character spacing is graphics state, and it widens the box the same way
    /// it widens the run — `CalcPositionDataInternal` adds it to the pen.
    #[test]
    fn character_spacing_widens_the_rect() {
        let plain = pdfrum_page::GraphicsState::default();
        let spaced = pdfrum_page::GraphicsState {
            text: pdfrum_page::TextState {
                char_space: 10.0,
                ..pdfrum_page::TextState::default()
            },
            ..pdfrum_page::GraphicsState::default()
        };
        let a = run_rect(&run(b"HH", 0.0, 0.0), &plain).expect("a box");
        let b = run_rect(&run(b"HH", 0.0, 0.0), &spaced).expect("a box");
        assert!(b.width() > a.width() + 9.0, "{a:?} vs {b:?}");
    }

    #[test]
    fn a_run_with_no_characters_has_no_rect() {
        let state = pdfrum_page::GraphicsState::default();
        assert!(run_rect(&run(b"", 0.0, 0.0), &state).is_none());
    }

    #[test]
    fn clip_only_mode_paints_nothing_but_clips() {
        let k = paint_kinds(TextRenderMode::Clip, true).expect("Tr 7 is not invisible");
        assert!(!k.fill && !k.stroke && k.clip);
    }

    #[test]
    fn stroke_without_a_face_falls_back_to_fill() {
        let with = paint_kinds(TextRenderMode::Stroke, true).expect("some");
        assert!(with.stroke && !with.fill);
        let without = paint_kinds(TextRenderMode::Stroke, false).expect("some");
        assert!(
            without.fill && !without.stroke,
            "no outlines means fill instead"
        );
    }

    #[test]
    fn fill_stroke_keeps_the_fill_when_there_is_no_face() {
        let k = paint_kinds(TextRenderMode::FillStroke, false).expect("some");
        assert!(k.fill);
        assert!(!k.stroke, "only the stroke half is dropped");
    }

    #[test]
    fn every_clip_mode_contributes_to_the_clip() {
        for mode in [
            TextRenderMode::FillClip,
            TextRenderMode::StrokeClip,
            TextRenderMode::FillStrokeClip,
            TextRenderMode::Clip,
        ] {
            assert!(paint_kinds(mode, true).is_some_and(|k| k.clip), "{mode:?}");
        }
        for mode in [
            TextRenderMode::Fill,
            TextRenderMode::Stroke,
            TextRenderMode::FillStroke,
        ] {
            assert!(paint_kinds(mode, true).is_some_and(|k| !k.clip), "{mode:?}");
        }
    }

    #[test]
    fn glyph_matrix_scales_by_size_over_1000_without_flipping() {
        // Outlines are 1000 units per em, so a full em at size 1000 is a
        // whole unit of text space, unmirrored: the single y flip lives in
        // the page matrix, where every object kind sees it. Flipping here
        // too would mirror each letter about its own baseline.
        let m = glyph_matrix(1000.0, Point::ZERO, Affine::IDENTITY);
        let p = m * Point::new(0.0, 1000.0);
        assert!(
            (p.y - 1000.0).abs() < 1e-9,
            "y is not flipped here: {}",
            p.y
        );
        assert!((p.x - 0.0).abs() < 1e-9);

        let half = glyph_matrix(500.0, Point::ZERO, Affine::IDENTITY);
        let p = half * Point::new(1000.0, 0.0);
        assert!((p.x - 500.0).abs() < 1e-9, "half size halves the advance");
    }

    #[test]
    fn the_pen_translates_in_text_space_before_the_size_scale() {
        // A pen at x = 40 with a size of 12 puts the glyph's own origin at
        // 40 text units, not at 40 * 12 / 1000.
        let m = glyph_matrix(12.0, Point::new(40.0, 0.0), Affine::IDENTITY);
        let origin = m * Point::ZERO;
        assert!((origin.x - 40.0).abs() < 1e-9, "origin at {}", origin.x);
    }

    #[test]
    fn stroke_ctm_split_is_identity_at_unit_scale() {
        let text = Affine::translate((3.0, 4.0));
        let device = Affine::scale(2.0);
        let (t, d) = stroke_ctm_split(text, device, [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(t, text);
        assert_eq!(d, device);
    }

    #[test]
    fn stroke_ctm_split_moves_the_scale_into_the_device_matrix() {
        let text = Affine::IDENTITY;
        let device = Affine::IDENTITY;
        let (t, d) = stroke_ctm_split(text, device, [2.0, 0.0, 0.0, 3.0]);
        // The product is unchanged — the split only moves where the scale
        // lives, so the glyph lands in the same place but the stroke width is
        // measured in text space.
        let composed = t * d;
        for (a, b) in composed
            .as_coeffs()
            .iter()
            .zip(Affine::IDENTITY.as_coeffs().iter())
        {
            assert!((a - b).abs() < 1e-9, "{composed:?}");
        }
        assert!(
            (d.as_coeffs()[0] - 2.0).abs() < 1e-9,
            "the x scale moved to the device matrix"
        );
    }

    #[test]
    fn the_conformance_snap_quantises_x_to_thirds_and_y_to_whole_pixels() {
        // The mode resolves to `kLcd` (bClearType only decides `normalize`),
        // where `origin_.x = floor(x)` and the blit adds
        // `(int)(x * 3) % 3` thirds back (`cfx_renderdevice.cpp:1254, 1352`).
        // So x lands on a third and y on a whole pixel.
        let near = |p: Point, x: f64, y: f64| {
            assert!(
                (p.x - x).abs() < 1e-9 && (p.y - y).abs() < 1e-9,
                "{p:?} is not ({x}, {y})"
            );
        };
        near(
            snap_origin(Point::new(10.9, 100.4), TextAa::Grayscale),
            10.0 + 2.0 / 3.0,
            100.0,
        );
        near(
            snap_origin(Point::new(10.1, 100.6), TextAa::Grayscale),
            10.0,
            101.0,
        );
        near(
            snap_origin(Point::new(10.5, 0.0), TextAa::Grayscale),
            10.0 + 1.0 / 3.0,
            0.0,
        );
        // A third is not a whole pixel: the x quantum is small enough that a
        // 9-pixel glyph's stems barely move, which is why the whole-pixel
        // read of this rule cost 58 files when it was tried.
        near(
            snap_origin(Point::new(10.99, 0.0), TextAa::Grayscale),
            10.0 + 2.0 / 3.0,
            0.0,
        );
    }

    #[test]
    fn only_y_is_quantised_to_a_whole_pixel_under_lcd() {
        // The asymmetry is the placement divergence: a baseline at 100.4 is
        // drawn at 100, which moves every horizontal stem edge, while an x of
        // 10.4 moves by at most a third.
        for tenth in 0..10 {
            let x = 10.0 + f64::from(tenth) / 10.0;
            let p = snap_origin(Point::new(x, 100.4), TextAa::Grayscale);
            assert!((p.x - x).abs() <= 1.0 / 3.0, "x moved {} at {x}", p.x - x);
            assert_eq!(p.y, 100.0);
        }
    }

    #[test]
    fn no_smoothtext_rounds_x_instead_of_flooring_it() {
        // `--no-smoothtext` leaves `anti_alias` at its `kMono` initialiser,
        // so `anti_alias_is_lcd` is false and x takes `FXSYS_roundf` too.
        assert_eq!(
            snap_origin(Point::new(10.9, 5.0), TextAa::None),
            Point::new(11.0, 5.0)
        );
        assert_eq!(
            snap_origin(Point::new(10.1, 5.0), TextAa::None),
            Point::new(10.0, 5.0)
        );
        // Half away from zero, which is C `round` and not `round_ties_even`.
        assert_eq!(
            snap_origin(Point::new(10.5, -2.5), TextAa::None),
            Point::new(11.0, -3.0)
        );
    }

    #[test]
    fn y_always_rounds_whatever_the_mode_is() {
        for aa in [TextAa::Grayscale, TextAa::None] {
            assert_eq!(snap_origin(Point::new(0.0, 7.6), aa).y, 8.0, "{aa:?}");
            assert_eq!(snap_origin(Point::new(0.0, 7.4), aa).y, 7.0, "{aa:?}");
        }
    }

    #[test]
    fn the_bitmap_path_is_a_small_text_rule() {
        // char2device = text2device * Scale(size, -size), so |a| + |b| is the
        // em's device extent. 12 pt at unit scale is well under 50.
        assert!(takes_bitmap_path(12.0, Affine::IDENTITY));
        assert!(
            takes_bitmap_path(50.0, Affine::IDENTITY),
            "the `> 50` is strict"
        );
        assert!(!takes_bitmap_path(51.0, Affine::IDENTITY));
        // A device scale counts: 12 pt at 5x is 60 device units.
        assert!(!takes_bitmap_path(12.0, Affine::scale(5.0)));
        // And so does a rotation, through `b`.
        assert!(!takes_bitmap_path(
            40.0,
            Affine::rotate(std::f64::consts::FRAC_PI_4)
        ));
    }

    #[test]
    fn a_stroked_run_does_not_snap_but_a_clipping_one_does() {
        let opts = RenderOptions::default();
        let fill = TextPaintKinds {
            fill: true,
            stroke: false,
            clip: false,
        };
        assert!(snaps_origins(&opts, fill, 12.0, Affine::IDENTITY));
        // `if (is_clip || is_stroke)` sends the run to `DrawTextPath`, which
        // places every glyph at its true fractional origin.
        assert!(!snaps_origins(
            &opts,
            TextPaintKinds {
                stroke: true,
                ..fill
            },
            12.0,
            Affine::IDENTITY
        ));
        // But `is_clip` is the *caller*, not the render mode: the painting
        // pass always passes `clipping_path = nullptr`
        // (`cpdf_renderstatus.cpp:312`), so a `Tr 4` run still snaps when it
        // paints. Reading `is_clip` as "the mode has a clip bit" costs six
        // files, which is how this was found.
        assert!(snaps_origins(
            &opts,
            TextPaintKinds { clip: true, ..fill },
            12.0,
            Affine::IDENTITY
        ));
        // And large text never snaps, whatever it paints.
        assert!(!snaps_origins(&opts, fill, 80.0, Affine::IDENTITY));
    }

    #[test]
    fn the_knob_turns_the_snap_off() {
        let fill = TextPaintKinds {
            fill: true,
            stroke: false,
            clip: false,
        };
        let subpixel = RenderOptions {
            subpixel_text_positioning: true,
            ..RenderOptions::default()
        };
        assert!(!snaps_origins(&subpixel, fill, 12.0, Affine::IDENTITY));
        assert!(snaps_origins(
            &RenderOptions::default(),
            fill,
            12.0,
            Affine::IDENTITY
        ));
    }

    #[test]
    fn adjust_glyph_space_never_moves_the_first_or_last_glyph() {
        // The C++ loop is `for (i = size - 1; i > 1; --i)` editing `[i - 1]`,
        // so index 0 and index size-1 are read-only.
        let device: Vec<Point> = (0..4)
            .map(|i| Point::new(f64::from(i) * 9.9, 0.0))
            .collect();
        let mut origins: Vec<Point> = device
            .iter()
            .map(|p| snap_origin(*p, TextAa::None))
            .collect();
        let (first, last) = (origins[0], origins[3]);
        adjust_glyph_space(&mut origins, &device);
        assert_eq!(origins[0], first);
        assert_eq!(origins[3], last);
    }

    #[test]
    fn adjust_glyph_space_closes_a_gap_the_snap_stretched() {
        // Spacing of 10.6 px snaps to 11, 11, 11 while the true gaps are
        // 10.6 — an error of 0.6 > 0.5, so the middle origins pull back one
        // pixel each. Round to 0, 11, 21, 32; the walk fixes index 2 then 1.
        let device: Vec<Point> = (0..4)
            .map(|i| Point::new(f64::from(i) * 10.6, 0.0))
            .collect();
        let mut origins: Vec<Point> = device
            .iter()
            .map(|p| snap_origin(*p, TextAa::None))
            .collect();
        assert_eq!(
            origins.iter().map(|p| p.x).collect::<Vec<_>>(),
            vec![0.0, 11.0, 21.0, 32.0]
        );
        adjust_glyph_space(&mut origins, &device);
        // Gap 3->2 is 32-21 = 11 against 10.6: error 0.0, left alone. Gap
        // 2->1 is 21-11 = 10 against 10.6: error 0.6, so index 1 pulls to 10.
        assert_eq!(
            origins.iter().map(|p| p.x).collect::<Vec<_>>(),
            vec![0.0, 10.0, 21.0, 32.0]
        );
    }

    #[test]
    fn adjust_glyph_space_declines_a_run_that_is_not_axis_aligned() {
        let device = vec![
            Point::new(0.0, 0.0),
            Point::new(10.0, 5.0),
            Point::new(20.0, 10.0),
        ];
        let mut origins = device.clone();
        adjust_glyph_space(&mut origins, &device);
        assert_eq!(origins, device, "a diagonal run is left entirely alone");
    }

    #[test]
    fn the_japan1_transform_shifts_the_origin_without_touching_the_advance() {
        // CID 7888 — U+3002 reached through `/90pv-RKSJ-H`, the row
        // `bug_1402.pdf` lands on. `{7888, 127, 0, 0, 127, 79, 94}` unpacks to
        // the identity matrix and a translation of (79/127, 94/127) em, which
        // is why that file's glyphs had the right shape and the wrong place.
        let t = pdfrum_font::CidTransform {
            cid: 7888,
            a: 127,
            b: 0,
            c: 0,
            d: 127,
            e: 79,
            f: 94,
        };
        let unpack = |b: u8| f64::from(pdfrum_font::cid_transform_to_float(b));
        let size = 36.0_f64;
        let dx = unpack(t.e) * size;
        let dy = unpack(t.f) * size;
        // Roughly (+22.4, +26.7) in text space, which after the page matrix's
        // single y flip is the (-22, +27) device displacement the file showed
        // when the transform was not applied at all.
        assert!((dx - 22.394).abs() < 0.01, "e * font_size is {dx}");
        assert!((dy - 26.646).abs() < 0.01, "f * font_size is {dy}");

        // Byte 127 is exactly 1 and byte 0 exactly 0, so this row's matrix is
        // the identity: the glyph is moved, never reshaped.
        assert_eq!(unpack(t.a), 1.0);
        assert_eq!(unpack(t.b), 0.0);
        assert_eq!(unpack(t.c), 0.0);
        assert_eq!(unpack(t.d), 1.0);

        // And the shift reaches the *matrix* rather than the pen: two glyphs
        // one advance apart stay one advance apart.
        let origin = Vec2::new(dx, dy);
        let a = glyph_matrix(36.0, Point::new(0.0, 0.0) + origin, Affine::IDENTITY);
        let b = glyph_matrix(36.0, Point::new(50.0, 0.0) + origin, Affine::IDENTITY);
        let step = b.translation() - a.translation();
        assert!(
            (step.x - 50.0).abs() < 1e-9 && step.y.abs() < 1e-9,
            "the advance is unchanged by the adjustment, got {step:?}"
        );
    }

    #[test]
    fn a_degenerate_ctm_leaves_the_matrices_alone() {
        let (t, d) = stroke_ctm_split(Affine::IDENTITY, Affine::IDENTITY, [0.0, 0.0, 0.0, 0.0]);
        assert_eq!(t, Affine::IDENTITY);
        assert_eq!(d, Affine::IDENTITY);
    }
}
