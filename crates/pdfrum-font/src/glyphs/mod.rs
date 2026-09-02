//! Where a glyph index becomes an outline, an advance, or a bounding box.
//!
//! Two backends sit behind one enum: `skrifa` for everything with an SFNT or
//! CFF shape, and `pdfrum-type1` for Type 1 programs, which Fontations reads
//! only at the weight vector the file ships with — not enough for the
//! Multiple-Master fallback faces PDFium leans on
//! (`docs/design/pdfrum-font.md` §1.14, §3.6).

mod cache;
mod face;

pub use cache::{GlyphCache, GlyphKey};
pub use face::{Charmap, CharmapId, Face};

pub use crate::descriptor::em_adjust;
pub(crate) use crate::descriptor::normalize_font_metric;

use crate::{Gid, GlyphName};
use pdfrum_common::kurbo::{Affine, BezPath, Rect};
use pdfrum_common::{Diagnostics, Limits};
use std::sync::Arc;

/// Where glyphs come from.
///
/// `Fontations` covers TrueType, bare CFF, OpenType and everything else with a
/// table directory; `Type1` covers PFA/PFB programs and the two Multiple-Master
/// fallback faces; `None` is a Type3 font or a program nothing could read.
#[derive(Debug, Clone, Default)]
pub enum GlyphSource {
    /// A face read by `skrifa`, over bytes this value owns.
    Fontations(Face),
    /// A Type 1 program, optionally instantiated at design coordinates.
    Type1(Arc<pdfrum_type1::Type1Font>),
    /// No glyphs at all.
    #[default]
    None,
}

/// The parameters that change what a glyph *looks like*, beyond its index.
///
/// For an ordinary face these are all inert. For a Multiple-Master face they
/// are not: `dest_width` alone changes the outline, which is why the glyph
/// cache keys on them (SPEC §6's amended key).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub(crate) struct GlyphParams {
    /// The width the PDF declared for this character code, in 1000/em units.
    /// Zero means "whatever the face does naturally".
    pub dest_width: i32,
    /// The substitution weight, or 0 for the face's own.
    pub weight: i32,
}

impl GlyphSource {
    /// Open a TrueType, OpenType, bare-CFF, or Type 1 program from its bytes.
    ///
    /// `None` when no backend recognises the blob.
    #[must_use]
    pub fn from_bytes(bytes: impl Into<Arc<[u8]>>) -> Option<Self> {
        let bytes = bytes.into();
        if let Some(face) = Face::new(Arc::clone(&bytes), 0) {
            return Some(Self::Fontations(face));
        }
        let mut diags = Diagnostics::default();
        pdfrum_type1::Type1Font::parse(&bytes, &Limits::default(), &mut diags)
            .ok()
            .map(|font| Self::Type1(Arc::new(font)))
    }

    /// Is there a face at all?
    #[must_use]
    pub(crate) fn is_some(&self) -> bool {
        !matches!(self, Self::None)
    }

    /// Design units per em; 0 when there is no face.
    #[must_use]
    pub fn units_per_em(&self) -> u16 {
        match self {
            Self::Fontations(f) => f.units_per_em(),
            Self::Type1(f) => f.units_per_em(),
            Self::None => 0,
        }
    }

    /// How many glyphs the face declares.
    #[must_use]
    pub fn num_glyphs(&self) -> u32 {
        match self {
            Self::Fontations(f) => f.num_glyphs(),
            Self::Type1(f) => f.num_glyphs(),
            Self::None => 0,
        }
    }

    /// Is this a TrueType-shaped face? PDFium's per-glyph fallback and its
    /// `ShouldUseFont` test both branch on it.
    #[must_use]
    pub fn is_truetype(&self) -> bool {
        match self {
            Self::Fontations(f) => f.is_truetype(),
            Self::Type1(_) | Self::None => false,
        }
    }

    /// The glyph a character code selects through the face's *currently
    /// selected* charmap.
    ///
    /// Returns 0 rather than `None` on a miss, because every ladder in §1.8
    /// and §1.9 tests `!= 0` and 0 is `.notdef` either way.
    #[must_use]
    pub fn char_index(&self, charmap: Charmap, code: u32) -> u16 {
        match self {
            Self::Fontations(f) => f.char_index(charmap, code),
            // A Type 1 face's "charmap" is its built-in encoding vector for a
            // byte code, and the synthesized Unicode map otherwise.
            Self::Type1(f) => {
                let gid = match charmap {
                    Charmap::Unicode => char::from_u32(code).and_then(|c| f.unicode_to_gid(c)),
                    _ => u8::try_from(code).ok().and_then(|b| f.code_to_gid(b)),
                };
                gid.map_or(0, |g| g.0)
            }
            Self::None => 0,
        }
    }

    /// The glyph a *name* selects. Zero on a miss, as `FT_Get_Name_Index`
    /// leaves it.
    #[must_use]
    pub(crate) fn name_index(&self, name: &[u8]) -> u16 {
        let Ok(name) = std::str::from_utf8(name) else {
            return 0;
        };
        match self {
            Self::Fontations(f) => f.name_index(name),
            Self::Type1(f) => f.name_to_gid(name).map_or(0, |g| g.0),
            Self::None => 0,
        }
    }

    /// A glyph's own name, when the face has a name table.
    #[must_use]
    pub(crate) fn glyph_name(&self, gid: Gid) -> Option<GlyphName> {
        match self {
            Self::Fontations(f) => f.glyph_name(gid).map(|n| GlyphName::new(n.into_bytes())),
            Self::Type1(f) => f
                .glyph_name(gid.into())
                .map(|n| GlyphName::new(n.as_bytes().to_vec())),
            Self::None => None,
        }
    }

    /// Whether the face can name its glyphs at all.
    #[must_use]
    pub(crate) fn has_glyph_names(&self) -> bool {
        match self {
            Self::Fontations(f) => f.has_glyph_names(),
            Self::Type1(_) => true,
            Self::None => false,
        }
    }

    /// The charmaps the face declares, as `(platform, encoding)` pairs in
    /// table order.
    #[must_use]
    pub fn charmaps(&self) -> Vec<CharmapId> {
        match self {
            Self::Fontations(f) => f.charmaps(),
            // A Type 1 face exposes a synthesized Unicode charmap first and
            // its own encoding second — the shape `UseType1Charmap` expects.
            Self::Type1(_) => vec![CharmapId::UNICODE_SYNTHETIC, CharmapId::ADOBE_CUSTOM],
            Self::None => Vec::new(),
        }
    }

    /// A glyph's outline in **1000/em text space**.
    ///
    /// Three things happen here that a plain `draw` would not do:
    ///
    /// - The outline is requested **unscaled**, in font units, and then scaled
    ///   by `1000 / upem` — matching the Fontations path PDFium itself is
    ///   moving to, rather than its FreeType path's 64-pixel dance.
    /// - **Degenerate trailing contours are trimmed** (`Outline_CheckEmptyContour`),
    ///   because `kurbo` will happily hold a zero-area contour that changes
    ///   what a rasterizer produces.
    /// - An outline that trims to nothing yields `None`, not an empty path.
    #[must_use]
    pub(crate) fn outline(&self, gid: Gid, params: GlyphParams) -> Option<BezPath> {
        let upem = self.units_per_em();
        let raw = match self {
            Self::Fontations(f) => f.outline(gid)?,
            Self::Type1(f) => match Self::mm_instance(f, gid, params) {
                Some(inst) => inst.outline(gid.into())?.0,
                None => f.outline(gid.into())?.0,
            },
            Self::None => return None,
        };
        let trimmed = trim_empty_contours(raw)?;
        if upem == 0 || upem == 1000 {
            return Some(trimmed);
        }
        let scale = 1000.0 / f64::from(upem);
        Some(Affine::scale(scale) * trimmed)
    }

    /// A glyph's outline in 1000/em text space, **grid-fitted at 64 ppem**.
    ///
    /// The same space [`Self::outline`] returns, so the two are interchangeable
    /// at every call site and the renderer's glyph matrix does not change. The
    /// difference is what happened before the scaling: this one ran the face's
    /// own hinting programs against a 64-pixel grid, which is what the oracle
    /// does for every SFNT face it draws as a *bitmap*
    /// (`CFX_Face::RenderGlyph`, `cfx_face.cpp:841-843`).
    ///
    /// The conversion is a pure scale — `1000 / 64` — because a 64-ppem
    /// instance draws in 64ths of an em. That is exactly the composition the
    /// oracle performs by handing FreeType a matrix pre-divided by 64, and it
    /// is why grid-fitting at a pinned ppem is not the same thing as
    /// grid-fitting at the size the glyph is drawn at.
    ///
    /// `None` for every face the oracle would not hint, which is the caller's
    /// signal to fall back to [`Self::outline`] rather than to draw nothing:
    /// a face with no table directory (`!IsTtOt()` — every bare CFF, so every
    /// base-14 substitution, and every `Type1` program), and a face whose own
    /// programs the interpreter refuses, which is the case upstream handles by
    /// reloading the glyph unhinted (`cfx_face.cpp:849-857`).
    #[must_use]
    pub(crate) fn hinted_outline(&self, gid: Gid) -> Option<BezPath> {
        let Self::Fontations(f) = self else {
            return None;
        };
        let raw = f.hinted_outline(gid)?;
        let trimmed = trim_empty_contours(raw)?;
        Some(Affine::scale(1000.0 / f64::from(Face::HINT_PPEM)) * trimmed)
    }

    /// Advance in 1000/em units at the face's default location.
    #[must_use]
    pub fn default_advance(&self, gid: Gid) -> i32 {
        self.advance(gid, GlyphParams::default())
    }

    /// A glyph's advance width in 1000/em units.
    ///
    /// Uses the **truncating** normalizer, which is the one
    /// `CFX_Face::GetGlyphWidth` calls — its sibling rounds, and the two
    /// disagree for half the inputs (§1.3).
    #[must_use]
    pub(crate) fn advance(&self, gid: Gid, params: GlyphParams) -> i32 {
        let upem = self.units_per_em();
        let raw = match self {
            Self::Fontations(f) => f.advance(gid),
            Self::Type1(f) => match Self::mm_instance(f, gid, params) {
                Some(inst) => inst.advance(gid.into()),
                None => f.outline(gid.into()).map(|(_, a)| a),
            },
            Self::None => None,
        };
        let Some(raw) = raw else { return 0 };
        // The C++'s range guard: an advance that would overflow the ×1000
        // scaling reports zero rather than a wrapped value.
        let raw = raw as i64;
        if raw < i64::from(i32::MIN) / 1000 || raw > i64::from(i32::MAX) / 1000 {
            return 0;
        }
        em_adjust(raw as i32, upem)
    }

    /// A glyph's advance through the **rounding** normalizer, which is what
    /// `LoadCharMetrics` uses when filling in a width the PDF omitted (§1.3).
    #[must_use]
    pub(crate) fn advance_tt(&self, gid: Gid) -> i32 {
        let upem = self.units_per_em();
        let raw = match self {
            Self::Fontations(f) => f.advance(gid),
            Self::Type1(f) => f.outline(gid.into()).map(|(_, a)| a),
            Self::None => None,
        };
        raw.map_or(0, |a| normalize_font_metric(a as i64, upem))
    }

    /// A glyph's bounding box in 1000/em units, y-up.
    ///
    /// PDFium's own differential check maps skrifa's `(x_min, y_min, x_max,
    /// y_max)` to `(left, top, right, bottom)` in its y-down convention and
    /// asserts agreement within 2 units; we take that mapping and keep the
    /// result y-up, which is what `kurbo::Rect` means.
    #[must_use]
    pub(crate) fn glyph_bbox(&self, gid: Gid) -> Option<Rect> {
        let upem = self.units_per_em();
        let raw = match self {
            Self::Fontations(f) => f.glyph_bbox(gid)?,
            Self::Type1(f) => f.glyph_bounds(gid.into())?,
            Self::None => return None,
        };
        let n = |v: f64| f64::from(normalize_font_metric(v as i64, upem));
        Some(Rect::new(n(raw.x0), n(raw.y0), n(raw.x1), n(raw.y1)))
    }

    /// The design-space instance to draw a Multiple-Master glyph at, solving
    /// the width axis for `dest_width` (`AdjustVariationParams`, §1.14).
    ///
    /// Axis 0 is weight, taken **directly** as a design coordinate. Axis 1 is
    /// width, found by probing the advance at both ends of the axis and
    /// interpolating — **without clamping**, so an extreme `dest_width`
    /// deliberately extrapolates past the axis.
    fn mm_instance(
        font: &pdfrum_type1::Type1Font,
        gid: Gid,
        params: GlyphParams,
    ) -> Option<pdfrum_type1::Type1Instance<'_>> {
        let axes = font.mm_axes()?;
        let weight_axis = axes.first()?;
        let width_axis = axes.get(1)?;

        let weight = if params.weight == 0 {
            weight_axis.default
        } else {
            params.weight as f32
        };

        if params.dest_width == 0 {
            return font.instantiate(&[weight, width_axis.default]);
        }

        let upem = font.units_per_em();
        let probe = |coord: f32| -> Option<i32> {
            let inst = font.instantiate(&[weight, coord])?;
            let adv = inst.advance(gid.into())?;
            Some(em_adjust(adv as i32, upem))
        };
        let (lo, hi) = (width_axis.min, width_axis.max);
        let min_w = probe(lo)?;
        let max_w = probe(hi)?;
        if max_w == min_w {
            // Degenerate: the C++ leaves the coordinates at the max probe.
            return font.instantiate(&[weight, hi]);
        }
        let t = (params.dest_width - min_w) as f32 / (max_w - min_w) as f32;
        font.instantiate(&[weight, (hi - lo).mul_add(t, lo)])
    }

    /// PostScript name, or a family/style display name, when the face has one.
    #[must_use]
    pub fn postscript_name(&self) -> Option<String> {
        match self {
            Self::Fontations(f) => f.postscript_name(),
            Self::Type1(f) => f
                .postscript_name()
                .map(ToOwned::to_owned)
                .or_else(|| f.family_name().map(ToOwned::to_owned)),
            Self::None => None,
        }
    }

    /// Fixed pitch: `post.isFixedPitch`, or Type 1 `/isFixedPitch`.
    #[must_use]
    pub fn is_fixed_pitch(&self) -> bool {
        match self {
            Self::Fontations(f) => f.is_fixed_pitch(),
            Self::Type1(f) => f.is_fixed_pitch(),
            Self::None => false,
        }
    }

    /// Italic: OS/2 / `macStyle` / `post.italicAngle`, or a Type 1 `/ItalicAngle`.
    #[must_use]
    pub fn is_italic(&self) -> bool {
        match self {
            Self::Fontations(f) => f.is_italic(),
            Self::Type1(f) => f.italic_angle() != 0.0,
            Self::None => false,
        }
    }

    /// Bold: OS/2 / `macStyle`, or a Type 1 name containing `Bold` / `Black`.
    #[must_use]
    pub fn is_bold(&self) -> bool {
        match self {
            Self::Fontations(f) => f.is_bold(),
            Self::Type1(f) => {
                let name = f.postscript_name().or_else(|| f.full_name()).unwrap_or("");
                name.contains("Bold") || name.contains("Black")
            }
            Self::None => false,
        }
    }

    /// OS/2 `sCapHeight` in font units, when present.
    #[must_use]
    pub fn cap_height_unscaled(&self) -> Option<f32> {
        match self {
            Self::Fontations(f) => f.cap_height(),
            Self::Type1(_) | Self::None => None,
        }
    }

    /// Ascender in font units (`hhea`, or the Type 1 bbox top).
    #[must_use]
    pub fn unscaled_ascent(&self) -> Option<i32> {
        match self {
            Self::Fontations(f) => f.metrics().and_then(|m| i32::try_from(m.ascender).ok()),
            Self::Type1(f) => Some(f.bbox().y1 as i32),
            Self::None => None,
        }
    }

    /// Descender in font units (`hhea`, or the Type 1 bbox bottom).
    #[must_use]
    pub fn unscaled_descent(&self) -> Option<i32> {
        match self {
            Self::Fontations(f) => f.metrics().and_then(|m| i32::try_from(m.descender).ok()),
            Self::Type1(f) => Some(f.bbox().y0 as i32),
            Self::None => None,
        }
    }

    /// Font bounding box in font units, `(left, bottom, right, top)`.
    #[must_use]
    pub fn unscaled_bbox(&self) -> Option<(i32, i32, i32, i32)> {
        match self {
            Self::Fontations(f) => {
                let m = f.metrics()?;
                Some((
                    i32::try_from(m.bbox_left).ok()?,
                    i32::try_from(m.bbox_bottom).ok()?,
                    i32::try_from(m.bbox_right).ok()?,
                    i32::try_from(m.bbox_top).ok()?,
                ))
            }
            Self::Type1(f) => {
                let b = f.bbox();
                Some((b.x0 as i32, b.y0 as i32, b.x1 as i32, b.y1 as i32))
            }
            Self::None => None,
        }
    }

    /// Unicode → glyph mappings with `code <= max`, sorted by codepoint.
    ///
    /// A miss is omitted rather than recorded as glyph 0.
    #[must_use]
    pub fn unicode_mappings(&self, max: u32) -> Vec<(u32, u16)> {
        match self {
            Self::Fontations(f) => f.unicode_mappings(max),
            Self::Type1(f) => {
                let mut out: Vec<(u32, u16)> = f
                    .unicode_pairs()
                    .filter(|(ch, gid)| u32::from(*ch) <= max && gid.0 != 0)
                    .map(|(ch, gid)| (u32::from(ch), gid.0))
                    .collect();
                out.sort_unstable_by_key(|(cp, _)| *cp);
                out
            }
            Self::None => Vec::new(),
        }
    }

    /// Glyph for a Unicode codepoint through the Unicode cmap. Zero on a miss.
    #[must_use]
    pub fn gid_for_unicode(&self, code: u32) -> u16 {
        self.char_index(Charmap::Unicode, code)
    }
}

/// Drop degenerate trailing contours (`Outline_CheckEmptyContour`).
///
/// FreeType's decomposition leaves two shapes behind that draw nothing but do
/// change a rasterizer's output: a `MoveTo` followed by a line back to the
/// same point, and a `MoveTo` followed by three curves all landing on it. Both
/// are trimmed, repeatedly, and an outline that trims away entirely yields
/// `None` rather than an empty path.
fn trim_empty_contours(path: BezPath) -> Option<BezPath> {
    use pdfrum_common::kurbo::PathEl;

    let mut els: Vec<PathEl> = path.into_iter().collect();
    loop {
        // A `ClosePath` is not itself degenerate; look past it.
        let end = els
            .iter()
            .rposition(|e| !matches!(e, PathEl::ClosePath))
            .map_or(0, |i| i + 1);

        // `[MoveTo(p), LineTo(p)]`.
        if end >= 2
            && let (Some(PathEl::MoveTo(a)), Some(PathEl::LineTo(b))) =
                (els.get(end - 2), els.get(end - 1))
            && a == b
        {
            els.truncate(end - 2);
            continue;
        }
        // `[MoveTo(p), CurveTo(_,_,p) × 3]`.
        if end >= 4
            && let (
                Some(PathEl::MoveTo(a)),
                Some(PathEl::CurveTo(_, _, b)),
                Some(PathEl::CurveTo(_, _, c)),
                Some(PathEl::CurveTo(_, _, d)),
            ) = (
                els.get(end - 4),
                els.get(end - 3),
                els.get(end - 2),
                els.get(end - 1),
            )
            && a == b
            && b == c
            && c == d
        {
            els.truncate(end - 4);
            continue;
        }
        break;
    }
    if els.iter().all(|e| matches!(e, PathEl::ClosePath)) {
        return None;
    }
    let out = BezPath::from_vec(els);
    if out.elements().is_empty() {
        None
    } else {
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdfrum_common::kurbo::{PathEl, Point};

    #[test]
    fn a_move_and_a_line_back_to_it_is_trimmed() {
        let mut p = BezPath::new();
        p.move_to((10.0, 10.0));
        p.line_to((50.0, 10.0));
        p.line_to((50.0, 50.0));
        p.close_path();
        p.move_to((7.0, 7.0));
        p.line_to((7.0, 7.0));
        let trimmed = trim_empty_contours(p).expect("the real contour survives");
        assert_eq!(trimmed.elements().len(), 4);
        assert!(matches!(
            trimmed.elements().first(),
            Some(PathEl::MoveTo(_))
        ));
    }

    #[test]
    fn a_move_and_three_curves_to_it_is_trimmed() {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((10.0, 0.0));
        p.close_path();
        let q = Point::new(3.0, 3.0);
        p.move_to(q);
        for _ in 0..3 {
            p.curve_to(q, q, q);
        }
        let trimmed = trim_empty_contours(p).expect("the real contour survives");
        assert_eq!(trimmed.elements().len(), 3);
    }

    #[test]
    fn repeated_degenerate_contours_are_all_trimmed() {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((10.0, 0.0));
        p.close_path();
        for i in 0..3 {
            let q = Point::new(f64::from(i), f64::from(i));
            p.move_to(q);
            p.line_to(q);
        }
        let trimmed = trim_empty_contours(p).expect("the real contour survives");
        assert_eq!(trimmed.elements().len(), 3);
    }

    #[test]
    fn an_entirely_degenerate_outline_is_none_not_an_empty_path() {
        let mut p = BezPath::new();
        p.move_to((5.0, 5.0));
        p.line_to((5.0, 5.0));
        assert!(trim_empty_contours(p).is_none());
        assert!(trim_empty_contours(BezPath::new()).is_none());
    }

    #[test]
    fn a_healthy_outline_is_untouched() {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.curve_to((10.0, 0.0), (10.0, 10.0), (0.0, 10.0));
        p.close_path();
        let n = p.elements().len();
        assert_eq!(trim_empty_contours(p).map(|q| q.elements().len()), Some(n));
    }

    #[test]
    fn the_empty_source_answers_everything_with_nothing() {
        let s = GlyphSource::None;
        assert!(!s.is_some());
        assert_eq!(s.units_per_em(), 0);
        assert_eq!(s.num_glyphs(), 0);
        assert!(!s.is_truetype());
        assert_eq!(s.char_index(Charmap::Unicode, 0x41), 0);
        assert_eq!(s.name_index(b"A"), 0);
        assert!(s.glyph_name(Gid(0)).is_none());
        assert!(!s.has_glyph_names());
        assert!(s.charmaps().is_empty());
        assert!(s.outline(Gid(0), GlyphParams::default()).is_none());
        assert_eq!(s.advance(Gid(0), GlyphParams::default()), 0);
        assert_eq!(s.advance_tt(Gid(0)), 0);
        assert!(s.glyph_bbox(Gid(0)).is_none());
    }
}
