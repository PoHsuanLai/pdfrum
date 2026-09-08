//! The `skrifa` / `read-fonts` adapter.
//!
//! PDFium drives FreeType, which carries a *selected charmap* as face state
//! and mutates it as the glyph ladders walk. Selecting a charmap on a shared
//! face is exactly the kind of hidden mutation this crate avoids, so the
//! selection becomes a value — [`Charmap`] — that the ladders pass to every
//! lookup. The ladders' sequence of "select this, try that" reads the same;
//! nothing is hidden in the face.

use crate::Gid;
use pdfrum_common::kurbo::{BezPath, Rect};
use read_fonts::TableProvider;
use read_fonts::tables::cmap::PlatformId;
use skrifa::MetadataProvider;
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{
    DrawSettings, Engine as HintingEngine, HintingInstance, HintingOptions, OutlinePen,
    Target as HintingTarget,
};
use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, OnceLock, RwLock};

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
    pub(crate) fn face_encoding(self) -> crate::encoding::FaceEncoding {
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
    /// The 64-ppem hinting instance, built on first use.
    ///
    /// The one piece of state this type keeps, and it earns the exception by
    /// measurement rather than by principle: building the instance runs the
    /// face's `fpgm` and `prep`
    /// programs and costs about **50 µs**, against 4 µs to rasterize a glyph
    /// bitmap and 0.4 µs to blit one. Rebuilding it per glyph made a
    /// text-heavy page 30% slower than filling outlines; keeping it makes the
    /// same page faster.
    ///
    /// It cannot be a borrowed `HintingInstance<'_>` because there is no such
    /// type — `skrifa`'s is owned, which is precisely what lets this sit beside
    /// the bytes without the self-reference the doc above rules out.
    ///
    /// `None` inside the lock is a face that cannot be hinted at all, cached so
    /// that a bare CFF does not re-attempt it once per glyph. The `Arc` shares
    /// the lock across clones, so two fonts substituted onto one face pay for
    /// the interpreter once between them.
    hinting: Arc<OnceLock<Option<HintingInstance>>>,
    /// Glyph name → the first glyph id carrying it, built on the first name
    /// lookup. A simple font with `/Differences` looks up hundreds of names
    /// against one face; scanning the `post` table per name was quadratic.
    names: Arc<OnceLock<HashMap<Box<[u8]>, u16>>>,
    /// Glyph id → the advance [`advance`](Self::advance) reported for it.
    ///
    /// The second measured exception, and for the same reason as
    /// [`hinting`](Self::hinting): a **bare CFF** carries no `hmtx`, so the
    /// only place an advance exists is inside the charstring, and reading it
    /// means running the Type 2 interpreter over the glyph's whole outline
    /// and discarding the path. Text extraction asks for a width once per
    /// shown character — `Font::char_width` for every glyph the page draws,
    /// then again through the extractor's own fallback ladder — so the same
    /// handful of glyphs are drawn hundreds of times each. On
    /// `text_tcpdf_063` that interpreter was **84% of the whole text run**.
    ///
    /// A map rather than a `num_glyphs`-long table because a CID font has
    /// tens of thousands of glyphs and a page shows tens of them; a
    /// `RwLock` rather than a `Mutex` because after the first few characters
    /// every access is a read, and `TextPage` is `Send + Sync` precisely so
    /// that pages extract in parallel. The `Arc` shares the cache across
    /// clones, so two fonts substituted onto one face pay once between them.
    advances: Arc<RwLock<HashMap<Gid, Option<f32>>>>,
    /// Glyph id → the box [`glyph_bbox`](Self::glyph_bbox) reported for it,
    /// cached for the same reason as [`advances`](Self::advances) and asked
    /// for just as often -- once per shown character through
    /// `TextRun::glyph_bbox`, and again by the width ladder's last rung.
    ///
    /// Each miss rebuilds a `skrifa::FontRef` and a whole `GlyphMetrics`
    /// (`hmtx`, `loca`, `glyf`, the variation tables) to read one box, or,
    /// on a CFF-flavoured face, draws the outline and measures it. On
    /// `text_foxit_products` that was **28% of the whole text run**, over
    /// half of it inside `GlyphMetrics::new`.
    boxes: Arc<RwLock<HashMap<Gid, Option<Rect>>>>,
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
            .field("hinting", &self.hinting.get().map(Option::is_some))
            .field("names", &self.names.get().map(HashMap::len))
            .field(
                "advances",
                &self.advances.read().map(|cache| cache.len()).ok(),
            )
            .field("boxes", &self.boxes.read().map(|cache| cache.len()).ok())
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
        Self::from_sfnt(&bytes, index).or_else(|| Self::from_bare_cff(bytes, index))
    }

    /// Read a table-directory font.
    fn from_sfnt(bytes: &Arc<[u8]>, index: u32) -> Option<Self> {
        let font = skrifa::FontRef::from_index(bytes.as_ref(), index).ok()?;
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
        Some(Self {
            bytes: Arc::clone(bytes),
            index,
            backend: Backend::Sfnt,
            upem,
            num_glyphs,
            is_truetype,
            charmaps,
            hinting: Arc::default(),
            names: Arc::default(),
            advances: Arc::default(),
            boxes: Arc::default(),
        })
    }

    /// Read a bare CFF font program.
    ///
    /// It reports exactly one charmap — its built-in encoding, which FreeType
    /// surfaces as `ADOBE_CUSTOM` — plus a synthesized Unicode one from its
    /// glyph names, matching the shape a Type 1 face presents. That is what
    /// makes the Type 1 ladder's `UseType1Charmap` step behave the same for a
    /// bare CFF as for a PFB, which is what PDFium's FreeType backend does.
    fn from_bare_cff(bytes: Arc<[u8]>, index: u32) -> Option<Self> {
        let cff = read_fonts::ps::cff::CffFontRef::new(bytes.as_ref(), 0, None).ok()?;
        let num_glyphs = cff.num_glyphs();
        let upem = u16::try_from(cff.upem()).unwrap_or(1000);
        Some(Self {
            bytes,
            index,
            backend: Backend::BareCff,
            upem,
            num_glyphs,
            // CFF outlines are cubic charstrings, never `glyf` splines.
            is_truetype: false,
            charmaps: vec![CharmapId::UNICODE_SYNTHETIC, CharmapId::ADOBE_CUSTOM],
            hinting: Arc::default(),
            names: Arc::default(),
            advances: Arc::default(),
            boxes: Arc::default(),
        })
    }

    /// Open the bare-CFF reader, when that is this face's backend.
    fn cff(&self) -> Option<read_fonts::ps::cff::CffFontRef<'_>> {
        if self.backend != Backend::BareCff {
            return None;
        }
        read_fonts::ps::cff::CffFontRef::new(&self.bytes, 0, None).ok()
    }

    /// The index a bare CFF actually stores a glyph under.
    ///
    /// For an ordinary CFF this is the number it was handed. For a **CID-keyed**
    /// one it is not: the composite-font layer above hands down a CID, because
    /// that is what PDFium hands FreeType, and FreeType silently maps it
    /// through the font's charset. A subsetted CID-keyed program holds a
    /// handful of glyphs numbered from zero while its CIDs are wherever the
    /// original collection put them, so skipping the mapping asks for a glyph
    /// number that does not exist and the font draws nothing at all.
    fn cff_glyph_id(
        cff: &read_fonts::ps::cff::CffFontRef<'_>,
        gid: Gid,
    ) -> read_fonts::types::GlyphId {
        let raw = read_fonts::types::GlyphId::new(u32::from(gid.0));
        if !cff.is_cid() {
            return raw;
        }
        // In a CID-keyed font the charset's string identifiers *are* CIDs.
        cff.charset()
            .and_then(|charset| {
                charset
                    .glyph_id(read_fonts::ps::string::Sid::new(gid.0))
                    .ok()
            })
            .unwrap_or(raw)
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
                Charmap::Unicode => self.cff_unicode_to_gid(code),
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
    fn cff_unicode_to_gid(&self, code: u32) -> Option<read_fonts::types::GlyphId> {
        let ch = char::from_u32(code)?;
        let mut buf = [0u8; read_fonts::ps::agl::MAX_NAME_LEN];
        let name = read_fonts::ps::agl::char_to_name(u32::from(ch), &mut buf)?;
        let gid = self.name_index(name);
        (gid != 0).then(|| read_fonts::types::GlyphId::new(u32::from(gid)))
    }

    /// Scan a bare CFF's charset for a glyph name.
    /// One-pass twin of the per-name scan this replaced: every name the
    /// face carries, mapped to the **first** glyph id that has it.
    fn build_name_map(&self) -> HashMap<Box<[u8]>, u16> {
        if let Some(cff) = self.cff() {
            let Some(charset) = cff.charset() else {
                return HashMap::new();
            };
            let mut map = HashMap::new();
            for gid in 0..self.num_glyphs {
                let Ok(g) = u16::try_from(gid) else { break };
                let Ok(sid) = charset.string_id(read_fonts::types::GlyphId::new(gid)) else {
                    continue;
                };
                if let Some(bytes) = cff.string(sid) {
                    map.entry(bytes.into()).or_insert(g);
                }
            }
            return map;
        }
        let Ok(font) = skrifa::FontRef::from_index(&self.bytes, self.index) else {
            return HashMap::new();
        };
        let Ok(post) = font.post() else {
            return HashMap::new();
        };
        let default_names = &read_fonts::tables::post::DEFAULT_GLYPH_NAMES;
        let mut map = HashMap::new();
        if post.version() == read_fonts::types::Version16Dot16::VERSION_1_0 {
            for (gid, name) in default_names
                .iter()
                .enumerate()
                .take(self.num_glyphs as usize)
            {
                let Ok(g) = u16::try_from(gid) else { break };
                map.entry(name.as_bytes().into()).or_insert(g);
            }
            return map;
        }
        if post.version() != read_fonts::types::Version16Dot16::VERSION_2_0 {
            return map;
        }
        let Some(index) = post.glyph_name_index() else {
            return map;
        };
        // The custom strings are a Pascal-string array: read it once,
        // sequentially, instead of walking it from the start per glyph.
        let strings: Vec<&str> = post
            .string_data()
            .map(|d| d.iter().map_while(Result::ok).map(|s| s.as_str()).collect())
            .unwrap_or_default();
        for gid in 0..self.num_glyphs {
            let Ok(g) = u16::try_from(gid) else { break };
            let Some(idx) = index.get(gid as usize) else {
                break;
            };
            let idx = usize::from(idx.get());
            let name = if idx < default_names.len() {
                default_names.get(idx).copied()
            } else {
                strings.get(idx - default_names.len()).copied()
            };
            if let Some(name) = name {
                map.entry(name.as_bytes().into()).or_insert(g);
            }
        }
        map
    }

    /// The glyph a name selects. Zero on a miss.
    #[must_use]
    pub fn name_index(&self, name: &str) -> u16 {
        self.names
            .get_or_init(|| self.build_name_map())
            .get(name.as_bytes())
            .copied()
            .unwrap_or(0)
    }

    /// The scan [`Face::build_name_map`] replaced, kept as the test oracle
    /// for it: the first glyph whose name matches, zero on a miss.
    #[cfg(test)]
    pub(crate) fn name_index_by_scan(&self, name: &str) -> u16 {
        if let Some(cff) = self.cff() {
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
            return 0;
        }
        let Ok(font) = skrifa::FontRef::from_index(&self.bytes, self.index) else {
            return 0;
        };
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

    /// The pixels-per-em every hinted glyph is grid-fitted at.
    ///
    /// A pinned constant rather than the glyph's real size: hinting always
    /// fits to a 64-pixel grid, and the size the glyph is actually drawn at
    /// is applied afterwards as a plain scale. Grid-fitting at a pinned ppem
    /// is therefore *not* the same thing as grid-fitting at the drawn size.
    ///
    /// The measured consequence: this moves outline points by about 1/25 of a
    /// device pixel at 9 pt, which is up to 10 counts per pixel on a 6 pt stem
    /// once the glyph is rasterized.
    // Where the 64 comes from: `CFX_Face::New` calls
    // `FT_Set_Pixel_Sizes(rec, 64, 64)` once (`cfx_face.cpp:376`) and nothing
    // ever changes it; the real size reaches FreeType through
    // `FT_Set_Transform` with the matrix pre-divided by 64
    // (`cfx_face.cpp:822-825`). FreeType applies a transform *after* hinting,
    // so the interpreter fits to a 64-pixel grid whose alignment is then
    // scaled away.
    pub(crate) const HINT_PPEM: f32 = 64.0;

    /// A glyph's outline grid-fitted at [`Self::HINT_PPEM`], in **64ths of an
    /// em** — the units a 64-ppem instance draws in.
    ///
    /// `None` for every face that is not hinted, and that is exactly the
    /// faces with **no table directory**: a bare CFF is never hinted, which
    /// matters because all fourteen base-14 blobs are bare CFF.
    ///
    /// It is also `None` when the interpreter refuses the face's own
    /// programs, in which case the caller falls back to the unhinted
    /// [`Self::outline`] rather than drawing nothing.
    ///
    /// Building the instance costs about 50 µs — the face's `fpgm` and `prep`
    /// programs run — so it is memoized per face rather than per glyph. See
    /// [`Self::hinting`].
    // The two `None` arms restate one upstream rule each.
    // `CFX_Face::RenderGlyph` adds `FT_LOAD_NO_HINTING` exactly when
    // `!IsTtOt()` — no `FT_FACE_FLAG_SFNT`, i.e. no table directory
    // (`cfx_face.cpp:841-843`). And a glyph is loaded `FT_LOAD_PEDANTIC`; on
    // an error `cfx_face.cpp:849-857` reloads it *unhinted* rather than
    // failing, which is the same place our second `None` sends the caller.
    #[must_use]
    pub(crate) fn hinted_outline(&self, gid: Gid) -> Option<BezPath> {
        let instance = self.hinting_instance()?;
        let font = skrifa::FontRef::from_index(&self.bytes, self.index).ok()?;
        let glyph = font
            .outline_glyphs()
            .get(skrifa::GlyphId::new(u32::from(gid.0)))?;
        let mut pen = PathPen::default();
        glyph
            .draw(DrawSettings::hinted(instance, false), &mut pen)
            .ok()?;
        Some(pen.path)
    }

    /// The memoized 64-ppem hinting instance, built on first use.
    fn hinting_instance(&self) -> Option<&HintingInstance> {
        self.hinting
            .get_or_init(|| {
                if self.backend != Backend::Sfnt {
                    return None;
                }
                let font = skrifa::FontRef::from_index(&self.bytes, self.index).ok()?;
                // `Engine::Interpreter` rather than the default
                // `AutoFallback`: the autofitter is compiled out of the
                // oracle's FreeType (`ftmodule.h`), so a face with no
                // `fpgm`/`prep` gets no hinting there at all, and falling back
                // to an autohinter here would invent grid-fitting the oracle
                // never applies. `Target::Smooth`'s default `Normal` mode is
                // `FT_RENDER_MODE_NORMAL`, which is what `RenderGlyph` selects
                // by passing no `FT_LOAD_TARGET_*` at all.
                HintingInstance::new(
                    &font.outline_glyphs(),
                    Size::new(Self::HINT_PPEM),
                    LocationRef::default(),
                    HintingOptions {
                        engine: HintingEngine::Interpreter,
                        target: HintingTarget::default(),
                    },
                )
                .ok()
            })
            .as_ref()
    }

    /// A glyph's outline in **font units**, unhinted.
    ///
    /// Unhinted at every size, for every face — this is the *path* side of
    /// text, which is never grid-fitted. The glyph-*bitmap* side is a
    /// different rule and a different function: see [`Self::hinted_outline`].
    // Unconditionally unhinted is not a simplification. `CFX_Face::LoadGlyphPath`
    // hints only a face that is both SFNT and on FreeType's ~20-font "tricky"
    // list (`cfx_face.cpp:948-951`); `skrifa` does not model that list and no
    // corpus font is on it, so the hinted arm is unreachable either way.
    #[must_use]
    pub(crate) fn outline(&self, gid: Gid) -> Option<BezPath> {
        let mut pen = PathPen::default();
        if let Some(cff) = self.cff() {
            let id = Self::cff_glyph_id(&cff, gid);
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
    pub(crate) fn advance(&self, gid: Gid) -> Option<f32> {
        if let Ok(cache) = self.advances.read()
            && let Some(hit) = cache.get(&gid)
        {
            return *hit;
        }
        let computed = self.advance_uncached(gid);
        if let Ok(mut cache) = self.advances.write() {
            cache.insert(gid, computed);
        }
        computed
    }

    /// [`advance`](Self::advance) with the cache bypassed.
    #[must_use]
    fn advance_uncached(&self, gid: Gid) -> Option<f32> {
        if let Some(cff) = self.cff() {
            let id = Self::cff_glyph_id(&cff, gid);
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
    pub(crate) fn glyph_bbox(&self, gid: Gid) -> Option<Rect> {
        if let Ok(cache) = self.boxes.read()
            && let Some(hit) = cache.get(&gid)
        {
            return *hit;
        }
        let computed = self.glyph_bbox_uncached(gid);
        if let Ok(mut cache) = self.boxes.write() {
            cache.insert(gid, computed);
        }
        computed
    }

    /// [`glyph_bbox`](Self::glyph_bbox) with the cache bypassed.
    #[must_use]
    fn glyph_bbox_uncached(&self, gid: Gid) -> Option<Rect> {
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
    pub(crate) fn metrics(&self) -> Option<crate::descriptor::FaceMetrics> {
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
    pub(crate) fn bytes(&self) -> &Arc<[u8]> {
        &self.bytes
    }

    /// The face index within a collection.
    #[must_use]
    pub(crate) fn index(&self) -> u32 {
        self.index
    }

    /// The family and style names, joined as PDFium's `GetFontNameFromFace`
    /// joins them: family, then a space and the style unless the style is
    /// empty or `Regular`.
    #[must_use]
    pub(crate) fn display_name(&self) -> Option<String> {
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

    /// The PostScript name (name ID 6), falling back to the family name.
    #[must_use]
    pub fn postscript_name(&self) -> Option<String> {
        if let Some(cff) = self.cff() {
            let meta = cff.metadata()?;
            return meta
                .name()
                .or_else(|| meta.family_name())
                .map(ToOwned::to_owned);
        }
        let font = skrifa::FontRef::from_index(&self.bytes, self.index).ok()?;
        let ps: String = font
            .localized_strings(skrifa::string::StringId::POSTSCRIPT_NAME)
            .english_or_first()
            .map(|s| s.chars().collect())
            .unwrap_or_default();
        if !ps.is_empty() {
            return Some(ps);
        }
        self.display_name()
    }

    /// `post.isFixedPitch`, or false when the table is missing.
    #[must_use]
    pub fn is_fixed_pitch(&self) -> bool {
        let Ok(font) = skrifa::FontRef::from_index(&self.bytes, self.index) else {
            return false;
        };
        font.post().is_ok_and(|p| p.is_fixed_pitch() != 0)
    }

    /// Italic from OS/2 `fsSelection`, `head.macStyle`, or a non-zero `post.italicAngle`.
    #[must_use]
    pub fn is_italic(&self) -> bool {
        let Ok(font) = skrifa::FontRef::from_index(&self.bytes, self.index) else {
            return false;
        };
        if let Ok(os2) = font.os2() {
            let sel = os2.fs_selection();
            if sel.contains(read_fonts::tables::os2::SelectionFlags::ITALIC)
                || sel.contains(read_fonts::tables::os2::SelectionFlags::OBLIQUE)
            {
                return true;
            }
        }
        if font.head().is_ok_and(|h| {
            h.mac_style()
                .contains(read_fonts::tables::head::MacStyle::ITALIC)
        }) {
            return true;
        }
        font.post().is_ok_and(|p| p.italic_angle().to_f64() != 0.0)
    }

    /// Bold from OS/2 `fsSelection` / `usWeightClass >= 700`, or `head.macStyle`.
    #[must_use]
    pub fn is_bold(&self) -> bool {
        let Ok(font) = skrifa::FontRef::from_index(&self.bytes, self.index) else {
            return false;
        };
        if let Ok(os2) = font.os2() {
            if os2
                .fs_selection()
                .contains(read_fonts::tables::os2::SelectionFlags::BOLD)
            {
                return true;
            }
            if os2.us_weight_class() >= 700 {
                return true;
            }
        }
        font.head().is_ok_and(|h| {
            h.mac_style()
                .contains(read_fonts::tables::head::MacStyle::BOLD)
        })
    }

    /// OS/2 `sCapHeight` in font units, when the table is version 2 or later.
    #[must_use]
    pub fn cap_height(&self) -> Option<f32> {
        let font = skrifa::FontRef::from_index(&self.bytes, self.index).ok()?;
        font.os2().ok()?.s_cap_height().map(f32::from)
    }

    /// Unicode codepoint → glyph mappings of the Unicode cmap, with `code <= max`.
    ///
    /// Sorted by codepoint. Glyph 0 (`.notdef`) is omitted, matching
    /// `FT_Get_Next_Char`'s `glyph_index == 0` stop.
    #[must_use]
    pub fn unicode_mappings(&self, max: u32) -> Vec<(u32, u16)> {
        if self.backend == Backend::BareCff {
            return (0..=max)
                .filter_map(|cp| {
                    let gid = self.char_index(Charmap::Unicode, cp);
                    (gid != 0).then_some((cp, gid))
                })
                .collect();
        }
        let Ok(font) = skrifa::FontRef::from_index(&self.bytes, self.index) else {
            return Vec::new();
        };
        let mut out: Vec<(u32, u16)> = font
            .charmap()
            .mappings()
            .filter_map(|(cp, gid)| {
                if cp > max {
                    return None;
                }
                let g = u16::try_from(gid.to_u32()).ok()?;
                (g != 0).then_some((cp, g))
            })
            .collect();
        out.sort_unstable_by_key(|(cp, _)| *cp);
        out.dedup_by_key(|(cp, _)| *cp);
        out
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
    /// The one-pass map answers exactly what the per-name scan answered, for
    /// every name every fixture face carries, and zero for a name none has.
    #[test]
    fn the_name_map_answers_what_the_scan_answered() {
        let fixtures = [
            "tt_custom_40.ttf",
            "tt_macroman_10.ttf",
            "tt_macroman_empty.ttf",
            "tt_named_no_cmap.ttf",
            "tt_sjis_and_unicode.ttf",
            "tt_symbol_30.ttf",
            "tt_symbol_and_macroman.ttf",
            "tt_symbol_empty.ttf",
            "tt_unicode_03_and_symbol.ttf",
            "tt_unicode_03.ttf",
            "tt_unicode_31_and_symbol.ttf",
            "tt_unicode_31.ttf",
        ];
        let mut named_faces = 0;
        for fixture in fixtures {
            let bytes: Arc<[u8]> = crate::testfonts::load(fixture).into();
            let face = Face::new(bytes, 0).unwrap();
            let map = face.build_name_map();
            named_faces += usize::from(!map.is_empty());
            for (name, gid) in &map {
                let name = std::str::from_utf8(name).unwrap();
                let scanned = face.name_index_by_scan(name);
                assert_eq!(*gid, scanned, "{fixture}: {name}");
                assert_eq!(face.name_index(name), scanned, "{fixture}: {name}");
            }
            assert_eq!(face.name_index("nonesuch"), 0, "{fixture}");
            assert_eq!(face.name_index_by_scan("nonesuch"), 0, "{fixture}");
        }
        assert!(
            named_faces > 0,
            "no fixture carries glyph names; the pin proves nothing"
        );
    }

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
