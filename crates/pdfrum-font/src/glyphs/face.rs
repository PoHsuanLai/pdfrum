//! The `skrifa` / `read-fonts` adapter.
//!
//! PDFium drives FreeType, which carries a *selected charmap* as face state
//! and mutates it as the glyph ladders walk. Selecting a charmap on a shared
//! face is exactly the kind of hidden mutation STYLE.md §1 forbids, so the
//! selection becomes a value — [`Charmap`] — that the ladders pass to every
//! lookup. The ladders' sequence of "select this, try that" reads the same;
//! nothing is hidden in the face.

use crate::Gid;
use pdfrum_common::kurbo::{BezPath, Rect};
use read_fonts::TableProvider;
use read_fonts::tables::cmap::PlatformId;
use skrifa::MetadataProvider;
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use std::fmt;
use std::sync::Arc;

/// A charmap's `(platform, encoding)` identity, as the `cmap` table declares
/// it.
///
/// PDFium compares these pairs literally — `(3,1)` for Windows Unicode,
/// `(3,0)` for Windows Symbol, `(1,0)` for Mac Roman — and the *order* it
/// prefers them in flips with the symbolic flag, so the pairs have to survive
/// as data rather than being collapsed into a "best charmap".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CharmapId {
    /// The `cmap` platform ID.
    pub platform: u16,
    /// The `cmap` encoding ID, whose meaning depends on the platform.
    pub encoding: u16,
}

impl CharmapId {
    /// Windows Unicode BMP — the charmap `UseTTCharmapUnicode` accepts outright.
    pub const WINDOWS_UNICODE: Self = Self {
        platform: 3,
        encoding: 1,
    };
    /// Windows Symbol, the `0xF0xx` private-use charmap.
    pub const WINDOWS_SYMBOL: Self = Self {
        platform: 3,
        encoding: 0,
    };
    /// Mac Roman.
    pub const MAC_ROMAN: Self = Self {
        platform: 1,
        encoding: 0,
    };
    /// The synthesized Unicode charmap a Type 1 face exposes first.
    pub const UNICODE_SYNTHETIC: Self = Self {
        platform: 0,
        encoding: 3,
    };
    /// A Type 1 face's own encoding vector, which FreeType reports as
    /// `ADOBE_CUSTOM`.
    pub const ADOBE_CUSTOM: Self = Self {
        platform: 4,
        encoding: 0,
    };

    /// Does this charmap map Unicode?
    ///
    /// Platform 0 is Unicode by definition and `(3,1)`/`(3,10)` are Windows'
    /// Unicode encodings. This is FreeType's `FT_ENCODING_UNICODE` test, which
    /// `UseTTCharmapUnicode` reads for any charmap that is not `(3,0)`.
    #[must_use]
    pub fn is_unicode(self) -> bool {
        self.platform == 0 || (self.platform == 3 && (self.encoding == 1 || self.encoding == 10))
    }

    /// The `fxge`-level encoding this charmap reports, for the reverse lookups
    /// of §1.7.
    #[must_use]
    pub fn face_encoding(self) -> crate::encoding::FaceEncoding {
        use crate::encoding::FaceEncoding as E;
        match (self.platform, self.encoding) {
            (0, _) | (3, 1 | 10) => E::Unicode,
            (3, 0) => E::Symbol,
            (1, 0) => E::AppleRoman,
            (4, _) => E::AdobeCustom,
            _ => E::Other,
        }
    }
}

/// Which charmap a lookup reads.
///
/// A value rather than face state: PDFium's `FT_Set_Charmap` mutates the face,
/// which would make every ladder order-dependent on a shared value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Charmap {
    /// The face's best Unicode charmap, chosen by `skrifa`.
    #[default]
    Unicode,
    /// A specific subtable, by index into the `cmap` encoding records.
    Index(usize),
    /// No charmap: every lookup yields 0.
    None,
}

/// Which reader answers for a face's bytes.
///
/// **A bare CFF has no table directory**, so `skrifa::FontRef` cannot open one
/// — and all fourteen Foxit base-14 blobs are bare CFF, which makes this a
/// requirement rather than a nicety. PDFium's own Rust bridge splits the same
/// way (`Sfnt::new ?? CffFontRef::new ?? Type1Font::new`), so this is the
/// shape upstream arrived at too.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Backend {
    /// A table-directory font: TrueType, OpenType/CFF, a collection member.
    Sfnt,
    /// A bare CFF font program, read through `read_fonts::ps::cff`.
    BareCff,
}

/// A font face, owning its bytes.
///
/// The bytes are `Arc`'d and the reader is rebuilt per use rather than stored.
/// Opening only validates a header, so this is a handful of bounds checks —
/// cheap next to drawing a glyph, and it keeps the type free of the
/// self-reference a borrowed `FontRef<'static>` would need.
#[derive(Clone)]
pub struct Face {
    bytes: Arc<[u8]>,
    index: u32,
    backend: Backend,
    upem: u16,
    num_glyphs: u32,
    is_truetype: bool,
    charmaps: Vec<CharmapId>,
}

impl fmt::Debug for Face {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Face")
            .field("bytes", &format_args!("{} bytes", self.bytes.len()))
            .field("index", &self.index)
            .field("backend", &self.backend)
            .field("upem", &self.upem)
            .field("num_glyphs", &self.num_glyphs)
            .field("is_truetype", &self.is_truetype)
            .field("charmaps", &self.charmaps)
            .finish()
    }
}

impl Face {
    /// Read a face from a font program.
    ///
    /// Accepts anything with a table directory — TrueType, bare CFF,
    /// OpenType/CFF, and a TrueType Collection member by `index`. Returns
    /// `None` for a blob no backend recognises, which is the signal to fall
    /// back to [`pdfrum_type1`] and then to substitution.
    #[must_use]
    pub fn new(bytes: Arc<[u8]>, index: u32) -> Option<Self> {
        // A table directory first, then a bare CFF. The order matters only in
        // that an SFNT is unambiguous while a bare CFF is identified by a very
        // short header, so trying the specific format first avoids a false
        // positive on a truncated SFNT.
        Self::new_sfnt(&bytes, index)
            .or_else(|| Self::new_bare_cff(&bytes))
            .map(|f| f(bytes, index))
    }

    /// Read a table-directory font.
    #[allow(clippy::type_complexity)]
    fn new_sfnt(bytes: &[u8], index: u32) -> Option<Box<dyn FnOnce(Arc<[u8]>, u32) -> Self>> {
        let font = skrifa::FontRef::from_index(bytes, index).ok()?;
        let upem = font.head().map_or(0, |h| h.units_per_em());
        let num_glyphs = u32::from(font.maxp().ok()?.num_glyphs());
        // `glyf` present means outlines are quadratic TrueType splines; a
        // bare or wrapped CFF has none. PDFium asks FreeType the same question
        // through `FT_IS_SFNT` plus the driver name.
        let is_truetype = font.glyf().is_ok();
        let charmaps: Vec<CharmapId> = font
            .cmap()
            .map(|cmap| {
                cmap.encoding_records()
                    .iter()
                    .map(|rec| CharmapId {
                        platform: platform_ordinal(rec.platform_id()),
                        encoding: rec.encoding_id(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        Some(Box::new(move |bytes, index| Self {
            bytes,
            index,
            backend: Backend::Sfnt,
            upem,
            num_glyphs,
            is_truetype,
            charmaps,
        }))
    }

    /// Read a bare CFF font program.
    ///
    /// It reports exactly one charmap — its built-in encoding, which FreeType
    /// surfaces as `ADOBE_CUSTOM` — plus a synthesized Unicode one from its
    /// glyph names, matching the shape a Type 1 face presents. That is what
    /// makes the Type 1 ladder's `UseType1Charmap` step behave the same for a
    /// bare CFF as for a PFB, which is what PDFium's FreeType backend does.
    #[allow(clippy::type_complexity)]
    fn new_bare_cff(bytes: &[u8]) -> Option<Box<dyn FnOnce(Arc<[u8]>, u32) -> Self>> {
        let cff = read_fonts::ps::cff::CffFontRef::new(bytes, 0, None).ok()?;
        let num_glyphs = cff.num_glyphs();
        let upem = u16::try_from(cff.upem()).unwrap_or(1000);
        Some(Box::new(move |bytes, index| Self {
            bytes,
            index,
            backend: Backend::BareCff,
            upem,
            num_glyphs,
            // CFF outlines are cubic charstrings, never `glyf` splines.
            is_truetype: false,
            charmaps: vec![CharmapId::UNICODE_SYNTHETIC, CharmapId::ADOBE_CUSTOM],
        }))
    }

    /// Open the bare-CFF reader, when that is this face's backend.
    fn cff(&self) -> Option<read_fonts::ps::cff::CffFontRef<'_>> {
        if self.backend != Backend::BareCff {
            return None;
        }
        read_fonts::ps::cff::CffFontRef::new(&self.bytes, 0, None).ok()
    }

    /// Design units per em.
    #[must_use]
    pub fn units_per_em(&self) -> u16 {
        self.upem
    }

    /// How many glyphs the face declares.
    #[must_use]
    pub fn num_glyphs(&self) -> u32 {
        self.num_glyphs
    }

    /// Whether outlines come from a `glyf` table.
    #[must_use]
    pub fn is_truetype(&self) -> bool {
        self.is_truetype
    }

    /// The `(platform, encoding)` pairs the `cmap` table declares, in table
    /// order — which is the order every ladder scans them in.
    #[must_use]
    pub fn charmaps(&self) -> Vec<CharmapId> {
        self.charmaps.clone()
    }

    /// The glyph a code selects through `charmap`. Zero on any miss.
    #[must_use]
    pub fn char_index(&self, charmap: Charmap, code: u32) -> u16 {
        if let Some(cff) = self.cff() {
            // A bare CFF has two routes: its built-in encoding for a byte
            // code, and its glyph names through the Adobe Glyph List for a
            // Unicode. Which one applies is exactly the distinction
            // `UseType1Charmap` draws.
            let gid = match charmap {
                Charmap::None => None,
                Charmap::Unicode => self.cff_unicode_to_gid(&cff, code),
                Charmap::Index(_) => u8::try_from(code).ok().and_then(|b| cff.encoding()?.map(b)),
            };
            return gid
                .and_then(|g| u16::try_from(g.to_u32()).ok())
                .unwrap_or(0);
        }

        let Ok(font) = skrifa::FontRef::from_index(&self.bytes, self.index) else {
            return 0;
        };
        let gid = match charmap {
            Charmap::None => None,
            Charmap::Unicode => font.charmap().map(code),
            Charmap::Index(i) => font
                .cmap()
                .ok()
                .and_then(|cmap| {
                    let rec = cmap.encoding_records().get(i)?;
                    rec.subtable(cmap.offset_data()).ok()
                })
                .and_then(|sub| sub.map_codepoint(code)),
        };
        gid.and_then(|g| u16::try_from(g.to_u32()).ok())
            .unwrap_or(0)
    }

    /// A bare CFF's synthesized Unicode charmap: glyph names through the AGL.
    fn cff_unicode_to_gid(
        &self,
        cff: &read_fonts::ps::cff::CffFontRef<'_>,
        code: u32,
    ) -> Option<read_fonts::types::GlyphId> {
        let ch = char::from_u32(code)?;
        let mut buf = [0u8; read_fonts::ps::agl::MAX_NAME_LEN];
        let name = read_fonts::ps::agl::char_to_name(u32::from(ch), &mut buf)?;
        let gid = self.cff_name_index(cff, name);
        (gid != 0).then(|| read_fonts::types::GlyphId::new(u32::from(gid)))
    }

    /// Scan a bare CFF's charset for a glyph name.
    fn cff_name_index(&self, cff: &read_fonts::ps::cff::CffFontRef<'_>, name: &str) -> u16 {
        let Some(charset) = cff.charset() else {
            return 0;
        };
        for gid in 0..self.num_glyphs {
            let Ok(g) = u16::try_from(gid) else { break };
            let id = read_fonts::types::GlyphId::new(gid);
            let Ok(sid) = charset.string_id(id) else {
                continue;
            };
            if cff.string(sid) == Some(name.as_bytes()) {
                return g;
            }
        }
        0
    }

    /// The glyph a name selects. Zero on a miss.
    #[must_use]
    pub fn name_index(&self, name: &str) -> u16 {
        if let Some(cff) = self.cff() {
            return self.cff_name_index(&cff, name);
        }
        let Ok(font) = skrifa::FontRef::from_index(&self.bytes, self.index) else {
            return 0;
        };
        // `post` version 2.0 is the only format carrying per-glyph names.
        let Ok(post) = font.post() else { return 0 };
        for gid in 0..self.num_glyphs {
            let Ok(g) = u16::try_from(gid) else { break };
            if post.glyph_name(read_fonts::types::GlyphId16::new(g)) == Some(name) {
                return g;
            }
        }
        0
    }

    /// A glyph's own name.
    #[must_use]
    pub fn glyph_name(&self, gid: Gid) -> Option<String> {
        if let Some(cff) = self.cff() {
            let sid = cff
                .charset()?
                .string_id(read_fonts::types::GlyphId::new(u32::from(gid.0)))
                .ok()?;
            return cff
                .string(sid)
                .and_then(|b| std::str::from_utf8(b).ok())
                .map(ToOwned::to_owned);
        }
        let font = skrifa::FontRef::from_index(&self.bytes, self.index).ok()?;
        font.post()
            .ok()?
            .glyph_name(read_fonts::types::GlyphId16::new(gid.0))
            .map(ToOwned::to_owned)
    }

    /// Whether the face carries glyph names at all.
    #[must_use]
    pub fn has_glyph_names(&self) -> bool {
        if self.backend == Backend::BareCff {
            // A CFF charset always names its glyphs.
            return self.cff().and_then(|c| c.charset()).is_some();
        }
        skrifa::FontRef::from_index(&self.bytes, self.index)
            .ok()
            .and_then(|f| f.post().ok())
            .is_some_and(|p| p.glyph_name(read_fonts::types::GlyphId16::new(0)).is_some())
    }

    /// A glyph's outline in **font units**, unhinted.
    ///
    /// Unhinted unconditionally, per the brief's OQ-4: we render text as
    /// filled outlines, and the only path that would hint requires a face that
    /// is both SFNT and on FreeType's ~20-font "tricky" list, which `skrifa`
    /// does not model and which no corpus font needs when filling.
    #[must_use]
    pub fn outline(&self, gid: Gid) -> Option<BezPath> {
        let mut pen = PathPen::default();
        if let Some(cff) = self.cff() {
            let id = read_fonts::types::GlyphId::new(u32::from(gid.0));
            let subfont_index = cff.subfont_index(id)?;
            let subfont = cff.subfont(subfont_index, &[]).ok()?;
            // `ppem: None` means unscaled font units, which is the same
            // request the SFNT path makes through `Size::unscaled`.
            cff.draw(&subfont, id, &[], None, &mut pen).ok()?;
            return Some(pen.path);
        }
        let font = skrifa::FontRef::from_index(&self.bytes, self.index).ok()?;
        let glyph = font
            .outline_glyphs()
            .get(skrifa::GlyphId::new(u32::from(gid.0)))?;
        glyph
            .draw(
                DrawSettings::unhinted(Size::unscaled(), LocationRef::default()),
                &mut pen,
            )
            .ok()?;
        Some(pen.path)
    }

    /// A glyph's advance in font units.
    ///
    /// A bare CFF carries no `hmtx`: the advance comes out of the charstring
    /// itself, which is why drawing is how it is read.
    #[must_use]
    pub fn advance(&self, gid: Gid) -> Option<f32> {
        if let Some(cff) = self.cff() {
            let id = read_fonts::types::GlyphId::new(u32::from(gid.0));
            let subfont_index = cff.subfont_index(id)?;
            let subfont = cff.subfont(subfont_index, &[]).ok()?;
            let mut pen = PathPen::default();
            return cff.draw(&subfont, id, &[], None, &mut pen).ok().flatten();
        }
        let font = skrifa::FontRef::from_index(&self.bytes, self.index).ok()?;
        font.glyph_metrics(Size::unscaled(), LocationRef::default())
            .advance_width(skrifa::GlyphId::new(u32::from(gid.0)))
    }

    /// A glyph's bounding box in font units, y-up.
    ///
    /// The fast path is the `glyf` table's per-glyph bounds, which only a
    /// TrueType-outlined face has. A **CFF-flavoured** OpenType face has no
    /// such table — its bounds live inside each charstring — so it falls
    /// through to measuring the outline, exactly as the C++'s FreeType
    /// backend does by loading the glyph and reading its control box. Getting
    /// this wrong makes every glyph of a CFF font report a zero box, which
    /// text extraction reads as a degenerate text object and drops whole.
    #[must_use]
    pub fn glyph_bbox(&self, gid: Gid) -> Option<Rect> {
        if self.backend != Backend::BareCff {
            let font = skrifa::FontRef::from_index(&self.bytes, self.index).ok()?;
            if let Some(b) = font
                .glyph_metrics(Size::unscaled(), LocationRef::default())
                .bounds(skrifa::GlyphId::new(u32::from(gid.0)))
            {
                return Some(Rect::new(
                    f64::from(b.x_min),
                    f64::from(b.y_min),
                    f64::from(b.x_max),
                    f64::from(b.y_max),
                ));
            }
        }
        let path = self.outline(gid)?;
        let b = pdfrum_common::kurbo::Shape::bounding_box(&path);
        (b.width() > 0.0 || b.height() > 0.0).then_some(b)
    }

    /// The raw metrics `CheckFontMetrics` derives a bounding box from.
    #[must_use]
    pub fn metrics(&self) -> Option<crate::descriptor::FaceMetrics> {
        if self.backend == Backend::BareCff {
            // A bare CFF declares no `head` or `hhea`; PDFium's FreeType
            // backend synthesizes the same nothing, and the caller's
            // per-code union then supplies the box.
            return None;
        }
        let font = skrifa::FontRef::from_index(&self.bytes, self.index).ok()?;
        let head = font.head().ok()?;
        let hhea = font.hhea().ok()?;
        Some(crate::descriptor::FaceMetrics {
            upem: head.units_per_em(),
            bbox_left: i64::from(head.x_min()),
            bbox_top: i64::from(head.y_max()),
            bbox_right: i64::from(head.x_max()),
            bbox_bottom: i64::from(head.y_min()),
            ascender: i64::from(hhea.ascender().to_i16()),
            descender: i64::from(hhea.descender().to_i16()),
        })
    }

    /// The face's own bytes, for the `GSUB` reader.
    #[must_use]
    pub fn bytes(&self) -> &Arc<[u8]> {
        &self.bytes
    }

    /// The face index within a collection.
    #[must_use]
    pub fn index(&self) -> u32 {
        self.index
    }

    /// The family and style names, joined as PDFium's `GetFontNameFromFace`
    /// joins them: family, then a space and the style unless the style is
    /// empty or `Regular`.
    #[must_use]
    pub fn display_name(&self) -> Option<String> {
        if let Some(cff) = self.cff() {
            let meta = cff.metadata()?;
            return meta
                .family_name()
                .or_else(|| meta.name())
                .map(ToOwned::to_owned);
        }
        let font = skrifa::FontRef::from_index(&self.bytes, self.index).ok()?;
        let strings = font.localized_strings(skrifa::string::StringId::FAMILY_NAME);
        let family: String = strings.english_or_first()?.chars().collect();
        if family.is_empty() {
            return None;
        }
        let style: String = font
            .localized_strings(skrifa::string::StringId::SUBFAMILY_NAME)
            .english_or_first()
            .map(|s| s.chars().collect())
            .unwrap_or_default();
        if style.is_empty() || style == "Regular" {
            Some(family)
        } else {
            Some(format!("{family} {style}"))
        }
    }
}

fn platform_ordinal(p: PlatformId) -> u16 {
    match p {
        PlatformId::Unicode => 0,
        PlatformId::Macintosh => 1,
        PlatformId::ISO => 2,
        PlatformId::Windows => 3,
        PlatformId::Custom => 4,
        // A malformed platform id must not collide with a real one.
        PlatformId::Unknown => u16::MAX,
    }
}

/// Collects `skrifa`'s outline verbs into a `kurbo` path.
///
/// Quadratics are elevated to cubics rather than kept, matching the
/// `ConvertOutline` step PDFium's own Fontations bridge performs so FreeType's
/// decomposition and this one agree.
#[derive(Default)]
struct PathPen {
    path: BezPath,
    current: (f32, f32),
    open: bool,
}

impl OutlinePen for PathPen {
    fn move_to(&mut self, x: f32, y: f32) {
        if self.open {
            self.path.close_path();
        }
        self.path.move_to((f64::from(x), f64::from(y)));
        self.current = (x, y);
        self.open = true;
    }

    fn line_to(&mut self, x: f32, y: f32) {
        if self.open {
            self.path.line_to((f64::from(x), f64::from(y)));
            self.current = (x, y);
        }
    }

    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        if !self.open {
            return;
        }
        // The standard quadratic-to-cubic elevation: each cubic control point
        // sits two thirds of the way from an endpoint to the quadratic's.
        let (px, py) = self.current;
        let c1 = (
            f64::from(px) + 2.0 / 3.0 * f64::from(cx0 - px),
            f64::from(py) + 2.0 / 3.0 * f64::from(cy0 - py),
        );
        let c2 = (
            f64::from(cx0) + f64::from(x - cx0) / 3.0,
            f64::from(cy0) + f64::from(y - cy0) / 3.0,
        );
        self.path.curve_to(c1, c2, (f64::from(x), f64::from(y)));
        self.current = (x, y);
    }

    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        if !self.open {
            return;
        }
        self.path.curve_to(
            (f64::from(cx0), f64::from(cy0)),
            (f64::from(cx1), f64::from(cy1)),
            (f64::from(x), f64::from(y)),
        );
        self.current = (x, y);
    }

    fn close(&mut self) {
        if self.open {
            self.path.close_path();
            self.open = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn charmap_ids_classify_unicode_correctly() {
        assert!(CharmapId::WINDOWS_UNICODE.is_unicode());
        assert!(CharmapId::UNICODE_SYNTHETIC.is_unicode());
        assert!(
            CharmapId {
                platform: 3,
                encoding: 10
            }
            .is_unicode()
        );
        // `(3,0)` is Windows *Symbol*, deliberately not Unicode — the whole
        // `UseTTCharmapUnicode` rule turns on that distinction.
        assert!(!CharmapId::WINDOWS_SYMBOL.is_unicode());
        assert!(!CharmapId::MAC_ROMAN.is_unicode());
    }

    #[test]
    fn charmap_ids_map_to_face_encodings() {
        use crate::encoding::FaceEncoding as E;
        assert_eq!(CharmapId::WINDOWS_UNICODE.face_encoding(), E::Unicode);
        assert_eq!(CharmapId::WINDOWS_SYMBOL.face_encoding(), E::Symbol);
        assert_eq!(CharmapId::MAC_ROMAN.face_encoding(), E::AppleRoman);
        assert_eq!(CharmapId::ADOBE_CUSTOM.face_encoding(), E::AdobeCustom);
        assert_eq!(
            CharmapId {
                platform: 2,
                encoding: 7
            }
            .face_encoding(),
            E::Other
        );
    }

    #[test]
    fn garbage_bytes_yield_no_face() {
        assert!(Face::new(Arc::from(&b""[..]), 0).is_none());
        assert!(Face::new(Arc::from(&b"not a font at all"[..]), 0).is_none());
        assert!(Face::new(Arc::from(vec![0u8; 4096].as_slice()), 0).is_none());
    }

    #[test]
    fn a_foxit_base14_blob_reads_as_a_non_truetype_face() {
        let bytes: Arc<[u8]> = Arc::from(crate::subst::standard_font_data(
            crate::StandardFont::Helvetica,
        ));
        let face = Face::new(bytes, 0).expect("bare CFF is readable");
        assert!(!face.is_truetype(), "a bare CFF has no glyf table");
        assert!(face.num_glyphs() > 100);
        assert_eq!(face.units_per_em(), 1000);
    }

    #[test]
    fn the_pen_elevates_quadratics_to_cubics() {
        let mut pen = PathPen::default();
        pen.move_to(0.0, 0.0);
        pen.quad_to(30.0, 60.0, 60.0, 0.0);
        pen.close();
        let els: Vec<_> = pen.path.into_iter().collect();
        assert_eq!(els.len(), 3);
        assert!(matches!(
            els.get(1),
            Some(pdfrum_common::kurbo::PathEl::CurveTo(..))
        ));
    }

    #[test]
    fn the_pen_ignores_segments_before_any_move() {
        let mut pen = PathPen::default();
        pen.line_to(10.0, 10.0);
        pen.curve_to(1.0, 1.0, 2.0, 2.0, 3.0, 3.0);
        pen.close();
        assert!(pen.path.elements().is_empty());
    }
}
