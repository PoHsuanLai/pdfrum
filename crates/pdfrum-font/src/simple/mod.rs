//! Simple fonts: one byte in, one glyph out.
//!
//! Type 1 and TrueType share everything except the glyph ladder itself, so
//! they share this module and differ only in which of [`type1`] and
//! [`truetype`] runs. The **order** of the load steps is behavior, because
//! each one reads state the previous wrote (`docs/design/pdfrum-font.md` §1.4).

mod truetype;
mod type1;

use crate::encoding::{FontEncoding, adobe_char_name, load_differences};
use crate::glyphs::{Charmap, Face, GlyphParams, GlyphSource};
use crate::subst::{
    self, CodePage, FontRequest, StandardFont, SubstFont, SubstitutionOptions, SystemFontDb,
    TestFontDb, strip_subset_prefix,
};
use crate::widths::{SimpleWidths, WIDTH_UNSET};
use crate::{
    CharCode, CharItem, FontCache, FontDescriptor, FontFlags, FontId, Gid, GlyphName, ToUnicode,
    descriptor, names, tounicode, widths,
};
use pdfrum_common::kurbo::Rect;
use pdfrum_common::{DiagKind, Diagnostics, Limits, Severity};
use pdfrum_object::{Dict, Resolve};
use smallvec::SmallVec;

/// Which of the two ladders a simple font runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimpleKind {
    /// Type 1, MMType1, or a font with no usable `/Subtype`.
    Type1 {
        /// The standard font this resolved to, if any. Note an *embedded*
        /// font is never "standard" even when it is named `Helvetica`.
        base14: Option<StandardFont>,
    },
    /// TrueType.
    TrueType,
}

/// A one-byte-per-code font.
///
/// The four parallel 256-entry tables are the shape PDFium works in, and they
/// stay: the ladders write into them in an order that matters, and collapsing
/// them into one array of records would hide which step wrote what.
#[derive(Debug)]
pub struct SimpleFont {
    /// This font's identity, for glyph-cache keys.
    pub id: FontId,
    /// Where glyphs come from.
    pub glyphs: GlyphSource,
    /// The `/Differences` overlay, empty when the font declared none.
    pub encoding: [Option<GlyphName>; 256],
    /// Which predefined set the encoding resolved to.
    pub encoding_kind: FontEncoding,
    /// The Unicode each code stands for, as the ladder computed it. **Not** a
    /// `/ToUnicode` substitute: this is the ladder's own working table, which
    /// several branches write into and later branches read back.
    pub unicodes: [u16; 256],
    /// The glyph each code selects. [`WIDTH_UNSET`] means "no glyph", which is
    /// distinct from glyph 0.
    pub glyph_index: [u16; 256],
    /// The declared widths.
    pub widths: SimpleWidths,
    /// The `/ToUnicode` CMap.
    pub to_unicode: Option<ToUnicode>,
    /// The `/FontDescriptor`'s contents, after repair.
    pub descriptor: FontDescriptor,
    /// What substitution decided, when the font was not embedded.
    pub subst: Option<SubstFont>,
    /// Which ladder ran.
    pub kind: SimpleKind,
    /// Whether a usable font program was embedded. A program that failed to
    /// parse counts as **not** embedded, which is what routes it to
    /// substitution.
    pub embedded: bool,
    /// The base font name, subset prefix stripped.
    pub base_font_name: Vec<u8>,
    /// Per-code bounding boxes, filled lazily by the metric derivation.
    char_bbox: [Rect; 256],
}

impl SimpleFont {
    /// The glyph a character code selects, or `None` for "draw nothing"
    /// (`CPDF_FaceBasedSimpleFont::GlyphFromCharCode`).
    ///
    /// All the work happened at load time; this is a table read. **Glyph 0 is
    /// a legitimate result** and is distinct from `None`.
    #[must_use]
    pub fn glyph_from_charcode(&self, code: CharCode) -> Option<Gid> {
        let index = usize::try_from(code.0).ok()?;
        match self.glyph_index.get(index) {
            Some(&WIDTH_UNSET) | None => None,
            Some(&g) => Some(Gid(g)),
        }
    }

    /// The advance width for a code, in 1000/em units.
    ///
    /// A code above 255 reads code **0**, not a miss — PDFium's own clamp, and
    /// the reason a stray wide code draws a space-ish advance rather than
    /// nothing.
    #[must_use]
    pub fn char_width(&self, code: CharCode) -> f32 {
        let code = if code.0 > 0xff { 0 } else { code.0 as u8 };
        if let Some(w) = self.widths.get(code) {
            return w;
        }
        // Nothing declared: ask the face.
        match self.glyph_from_charcode(CharCode(u32::from(code))) {
            Some(gid) => f32::from(self.glyphs.advance_tt(gid) as i16),
            None => 0.0,
        }
    }

    /// The Unicode a code stands for, `/ToUnicode` first and the ladder's own
    /// table second.
    #[must_use]
    pub fn unicode_from_charcode(&self, code: CharCode) -> SmallVec<[char; 2]> {
        if let Some(tu) = &self.to_unicode {
            let chars = tu.lookup(code);
            if !chars.is_empty() {
                return chars;
            }
        }
        let Ok(index) = usize::try_from(code.0) else {
            return SmallVec::new();
        };
        match self.unicodes.get(index) {
            Some(&0) | None => SmallVec::new(),
            Some(&u) => char::from_u32(u32::from(u))
                .map(|c| SmallVec::from_slice(&[c]))
                .unwrap_or_default(),
        }
    }

    /// The character code that produces `unicode`, or `None`.
    ///
    /// `/ToUnicode`'s reverse map first, then a scan of the ladder's own
    /// Unicode table. Appearance generation needs this to *write* text with a
    /// font the document already carries, which is the opposite direction from
    /// everything else here.
    #[must_use]
    pub fn char_code_from_unicode(&self, unicode: char) -> Option<CharCode> {
        if let Some(tu) = &self.to_unicode {
            let code = tu.reverse(unicode);
            if code.0 != 0 {
                return Some(code);
            }
        }
        let target = u16::try_from(u32::from(unicode)).ok()?;
        if target == 0 {
            return None;
        }
        // The ladder's table is the same one `unicode_from_charcode` reads, so
        // a code found here round-trips by construction.
        self.unicodes
            .iter()
            .position(|&u| u == target)
            .and_then(|i| u32::try_from(i).ok())
            .map(CharCode)
    }

    /// The bounding box for a code, in 1000/em units.
    #[must_use]
    pub fn char_bbox(&self, code: CharCode) -> Rect {
        let code = if code.0 > 0xff { 0 } else { code.0 as usize };
        self.char_bbox.get(code).copied().unwrap_or(Rect::ZERO)
    }

    /// Build one [`CharItem`].
    pub(crate) fn char_item(&self, code: CharCode) -> CharItem {
        let gid = self.glyph_from_charcode(code);
        CharItem {
            code,
            cid: None,
            gid: gid.unwrap_or_default(),
            unicode: self.unicode_from_charcode(code),
            width: self.char_width(code),
            vertical_glyph: false,
            has_glyph: gid.is_some(),
        }
    }

    /// Whether the PDF declared widths, which gates the glyph-spacing
    /// heuristic (`HasFontWidths`).
    #[must_use]
    pub fn has_font_widths(&self) -> bool {
        self.widths.has_declared_widths()
    }

    /// Whether this font resolved to one of the standard fourteen **and** is
    /// not embedded — an embedded font named `Helvetica` is not standard
    /// (`IsStandardFont`).
    #[must_use]
    pub fn is_standard_font(&self) -> bool {
        matches!(self.kind, SimpleKind::Type1 { base14: Some(_) }) && !self.embedded
    }
}

/// Load a simple font (`LoadCommon`).
///
/// **Cannot fail.** Every path returns a font, even one with no program, no
/// encoding and no glyphs at all — which is why the public entry point's
/// `Option` is about Type0 fonts only.
// One ordered sequence: every step reads state the steps above it left in
// `flags`, `encoding` and `base_font_name`, and *when* each write happens is
// the behavior. Helpers would move those writes behind call sites and hide the
// order, so the sequence stays whole.
#[allow(clippy::too_many_lines)]
pub(crate) fn load(
    dict: &Dict,
    r: &impl Resolve,
    cache: &FontCache,
    opts: &SubstitutionOptions,
    limits: &Limits,
    diags: &mut Diagnostics,
    is_truetype: bool,
) -> SimpleFont {
    let mut base_font_name = dict
        .name(names::BASE_FONT)
        .map(|n| n.as_bytes().to_vec())
        .unwrap_or_default();

    // The base-14 detection runs *before* the descriptor, and what it writes
    // to `flags` survives only when there is no descriptor at all.
    let base14 = if is_truetype {
        None
    } else {
        subst::standard_font_index(&base_font_name)
    };
    let mut flags = FontFlags::DEFAULT;
    let mut encoding_kind = FontEncoding::Builtin;
    let mut widths_table = SimpleWidths::default();
    if let Some(f) = base14 {
        base_font_name = subst::canonical_font_name(f).as_bytes().to_vec();
        flags = if f.is_symbolic() {
            FontFlags(FontFlags::SYMBOLIC)
        } else {
            FontFlags(FontFlags::NON_SYMBOLIC)
        };
        if f.is_fixed() {
            // The four Couriers: every glyph 600 units wide.
            widths_table = SimpleWidths {
                raw: [600; 256],
                use_face_widths: false,
            };
        }
        encoding_kind = match f {
            StandardFont::Symbol => FontEncoding::AdobeSymbol,
            StandardFont::Dingbats => FontEncoding::ZapfDingbats,
            _ if flags.is_non_symbolic() => FontEncoding::Standard,
            _ => encoding_kind,
        };
    }

    // Step 1 — the descriptor, which overwrites `flags` when it exists.
    let desc = dict.dict(names::FONT_DESCRIPTOR, r);
    let mut descriptor = FontDescriptor {
        flags,
        ..FontDescriptor::default()
    };
    if let Some(d) = &desc {
        descriptor = descriptor::load(d, r);
    }

    // The font program, whichever key carries it — the `/FontFile3` subtype is
    // never read, so the three keys are interchangeable.
    let (mut glyphs, mut embedded) = load_font_program(desc.as_ref(), r, limits, diags);

    // Step 2 — widths. A base-14 Courier's fixed widths are only kept when the
    // PDF declared none of its own.
    let declared = widths::load_simple(dict, desc.as_ref(), r);
    if declared.has_declared_widths() || !widths_table.has_declared_widths() {
        widths_table = declared;
    }

    // Step 3 — strip a subset prefix, or substitute.
    let mut subst_font = None;
    if embedded {
        base_font_name = strip_subset_prefix(&base_font_name).to_vec();
    } else {
        let request = FontRequest {
            name: base_font_name.clone(),
            is_truetype,
            flags: descriptor.flags,
            weight: descriptor.subst_weight(),
            italic_angle: descriptor.italic_angle,
            code_page: CodePage::DefAnsi,
            vertical: false,
        };
        let s = substitute(&request, opts, diags);
        glyphs = s.glyphs;
        subst_font = Some(s.subst);
    }

    // Step 4 — a *reset*, not a default: a non-symbolic font's encoding is
    // overwritten with Standard even when step 0 chose something else.
    if !descriptor.flags.is_symbolic() {
        encoding_kind = FontEncoding::Standard;
    }

    // Step 5 — the PDF's own encoding.
    let mut differences: [Option<GlyphName>; 256] = [const { None }; 256];
    let has_differences = load_pdf_encoding(
        dict,
        r,
        &base_font_name,
        descriptor.flags,
        embedded,
        is_truetype,
        &mut encoding_kind,
        &mut differences,
    );

    let to_unicode = load_to_unicode(dict, r, limits, diags);

    // Step 6 — the ladder.
    let mut unicodes = [0u16; 256];
    let mut glyph_index = [WIDTH_UNSET; 256];
    if glyphs.is_some() {
        let ctx = LadderContext {
            glyphs: &glyphs,
            encoding: encoding_kind,
            differences: &differences,
            flags: descriptor.flags,
            embedded,
            base14,
            to_unicode: to_unicode.as_ref(),
            first_char: dict.int(names::FIRST_CHAR, r).unwrap_or(0),
        };
        if is_truetype {
            truetype::load_glyph_map(&ctx, &mut unicodes, &mut glyph_index);
        } else {
            type1::load_glyph_map(&ctx, &mut unicodes, &mut glyph_index);
        }
    }

    // Step 9 — the all-caps aliasing, which for a **non-embedded** font
    // replaces lowercase glyphs *even when they mapped successfully*.
    if descriptor.flags.is_all_cap() {
        apply_all_caps(&mut glyph_index, &mut widths_table, embedded);
    }

    // Step 10 — derive whatever metrics the PDF failed to declare.
    let mut char_bbox = [Rect::ZERO; 256];
    for (code, slot) in char_bbox.iter_mut().enumerate() {
        let Some(&g) = glyph_index.get(code) else {
            continue;
        };
        if g == WIDTH_UNSET {
            continue;
        }
        if let Some(b) = glyphs.glyph_bbox(Gid(g)) {
            *slot = b;
        }
    }
    let metrics = match &glyphs {
        GlyphSource::Fontations(f) => f.metrics(),
        GlyphSource::Type1(f) => Some(descriptor::FaceMetrics {
            upem: f.units_per_em(),
            bbox_left: f.bbox().x0 as i64,
            bbox_top: f.bbox().y1 as i64,
            bbox_right: f.bbox().x1 as i64,
            bbox_bottom: f.bbox().y0 as i64,
            ascender: f.bbox().y1 as i64,
            descender: f.bbox().y0 as i64,
        }),
        GlyphSource::None => None,
    };
    descriptor::check_font_metrics(&mut descriptor, metrics, |c| {
        char_bbox.get(usize::from(c)).copied().unwrap_or(Rect::ZERO)
    });

    if !embedded && !glyphs.is_some() {
        diags.record(Severity::Suspicious, DiagKind::FontSubstitutionFailed, None);
    }
    if !glyphs.is_some() {
        embedded = false;
    }

    SimpleFont {
        id: cache.next_id(),
        glyphs,
        encoding: if has_differences {
            differences
        } else {
            [const { None }; 256]
        },
        encoding_kind,
        unicodes,
        glyph_index,
        widths: widths_table,
        to_unicode,
        descriptor,
        subst: subst_font,
        kind: if is_truetype {
            SimpleKind::TrueType
        } else {
            SimpleKind::Type1 { base14 }
        },
        embedded,
        base_font_name,
        char_bbox,
    }
}

/// What a ladder needs to decide a glyph.
pub(crate) struct LadderContext<'a> {
    pub glyphs: &'a GlyphSource,
    pub encoding: FontEncoding,
    pub differences: &'a [Option<GlyphName>; 256],
    pub flags: FontFlags,
    pub embedded: bool,
    pub base14: Option<StandardFont>,
    pub to_unicode: Option<&'a ToUnicode>,
    pub first_char: i64,
}

impl LadderContext<'_> {
    /// The merged glyph name for a code.
    pub(crate) fn char_name(&self, code: u8) -> Option<&[u8]> {
        adobe_char_name(self.encoding, self.differences, u32::from(code))
    }

    /// Whether `/Differences` supplied any names at all, which changes what
    /// `char_name` can return for a `Builtin` encoding.
    pub(crate) fn has_differences(&self) -> bool {
        self.differences.iter().any(Option::is_some)
    }
}

/// Read a font program from whichever of the three keys carries one.
///
/// The keys are tried in order and **the first present wins**; `/FontFile3`'s
/// own `/Subtype` is never consulted, so a CFF under `/FontFile2` loads fine
/// and so does a TrueType program under `/FontFile`. Format detection is
/// entirely the backend's job.
pub(crate) fn load_font_program(
    desc: Option<&Dict>,
    r: &impl Resolve,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> (GlyphSource, bool) {
    let Some(desc) = desc else {
        return (GlyphSource::None, false);
    };
    let stream = [names::FONT_FILE, names::FONT_FILE2, names::FONT_FILE3]
        .into_iter()
        .find_map(|k| desc.stream(k, r));
    let Some(stream) = stream else {
        return (GlyphSource::None, false);
    };

    // `/Length1`, `/Length2` and `/Length3` are a buffer hint only — PDFium
    // sums them for sizing and then discards them, and never uses them to
    // split a PFB. Trusting them loses fonts, because they are often wrong.
    let bytes = pdfrum_filters::decode_chain(&stream, 0, r, limits, diags).data;
    if bytes.is_empty() {
        diags.record(Severity::Suspicious, DiagKind::FontProgramUnreadable, None);
        return (GlyphSource::None, false);
    }
    let shared: std::sync::Arc<[u8]> = std::sync::Arc::from(bytes.as_slice());

    if let Some(face) = Face::new(shared.clone(), 0) {
        return (GlyphSource::Fontations(face), true);
    }
    // Not a table-directory font: try Type 1, which is the one format
    // Fontations does not read end to end.
    if let Ok(f) = pdfrum_type1::Type1Font::parse(&shared, limits, diags) {
        (GlyphSource::Type1(std::sync::Arc::new(f)), true)
    } else {
        // A program nothing can read nulls the font file, which makes
        // `IsEmbedded()` false and routes the font to substitution.
        diags.record(Severity::Suspicious, DiagKind::FontProgramUnreadable, None);
        (GlyphSource::None, false)
    }
}

/// Run substitution against whichever database the options select.
fn substitute(
    request: &FontRequest,
    opts: &SubstitutionOptions,
    diags: &mut Diagnostics,
) -> subst::Substitution {
    // Scanning the system's fonts is expensive and most callers do not want
    // it, so an empty `font_dirs` with no system scan requested means "the
    // built-in faces only" — which is what makes tests hermetic by default.
    if opts.font_dirs.is_empty() {
        return subst::resolve(request, &TestFontDb::new(), opts, diags);
    }
    let db = SystemFontDb::scan(&opts.font_dirs);
    subst::resolve(request, &db, opts, diags)
}

/// `/Encoding` resolution (`LoadPDFEncoding`).
///
/// Returns whether `/Differences` supplied anything. Three rewrites in here
/// look arbitrary and are not: `/MacExpertEncoding` named directly becomes
/// WinAnsi **unconditionally**, while through `/BaseEncoding` it becomes
/// WinAnsi only for a TrueType font — so `MacExpert` is reachable only through
/// a non-TrueType font's `/BaseEncoding`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn load_pdf_encoding(
    dict: &Dict,
    r: &impl Resolve,
    base_font_name: &[u8],
    flags: FontFlags,
    embedded: bool,
    is_truetype: bool,
    encoding: &mut FontEncoding,
    differences: &mut [Option<GlyphName>; 256],
) -> bool {
    let Some(enc) = dict.get(names::ENCODING, r) else {
        if base_font_name == b"Symbol" {
            *encoding = if is_truetype {
                FontEncoding::MsSymbol
            } else {
                FontEncoding::AdobeSymbol
            };
        } else if !embedded && *encoding == FontEncoding::Builtin {
            *encoding = FontEncoding::WinAnsi;
        }
        return false;
    };

    if let Some(name) = enc.as_name() {
        // A symbolic set already chosen is never overridden by a name.
        if matches!(
            *encoding,
            FontEncoding::AdobeSymbol | FontEncoding::ZapfDingbats
        ) {
            return false;
        }
        if flags.is_symbolic() && base_font_name == b"Symbol" {
            if !is_truetype {
                *encoding = FontEncoding::AdobeSymbol;
            }
            return false;
        }
        let mut spelling = name.as_bytes();
        if spelling == b"MacExpertEncoding" {
            spelling = b"WinAnsiEncoding";
        }
        if let Some(e) = FontEncoding::from_pdf_name(spelling) {
            *encoding = e;
        }
        return false;
    }

    let Some(enc_dict) = enc.as_dict() else {
        // An array, a number, anything else: nothing happens at all.
        return false;
    };
    if !matches!(
        *encoding,
        FontEncoding::AdobeSymbol | FontEncoding::ZapfDingbats
    ) && let Some(base) = enc_dict.name(names::BASE_ENCODING)
    {
        let mut spelling = base.as_bytes();
        if is_truetype && spelling == b"MacExpertEncoding" {
            spelling = b"WinAnsiEncoding";
        }
        if let Some(e) = FontEncoding::from_pdf_name(spelling) {
            *encoding = e;
        }
    }
    if (!embedded || is_truetype) && *encoding == FontEncoding::Builtin {
        *encoding = FontEncoding::Standard;
    }
    match enc_dict.array(names::DIFFERENCES, r) {
        Some(diffs) => load_differences(&diffs, r, differences),
        None => false,
    }
}

/// Read and parse `/ToUnicode`.
pub(crate) fn load_to_unicode(
    dict: &Dict,
    r: &impl Resolve,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> Option<ToUnicode> {
    let stream = dict.stream(names::TO_UNICODE, r)?;
    let bytes = pdfrum_filters::decode_chain(&stream, 0, r, limits, diags).data;
    let map = tounicode::parse(&bytes, limits, diags);
    if map.is_empty() { None } else { Some(map) }
}

/// The all-caps glyph aliasing.
///
/// For each of three ranges, a lowercase code borrows the glyph 32 codes
/// below it. The guard is the surprising part: an **embedded** font keeps a
/// glyph it already mapped, while a **non-embedded** one has its lowercase
/// glyphs replaced even when they mapped perfectly well.
fn apply_all_caps(glyph_index: &mut [u16; 256], widths: &mut SimpleWidths, embedded: bool) {
    for (lo, hi) in [(b'a', b'z'), (0xE0u8, 0xF6u8), (0xF8, 0xFD)] {
        for i in lo..=hi {
            let idx = usize::from(i);
            if glyph_index.get(idx) != Some(&WIDTH_UNSET) && embedded {
                continue;
            }
            let Some(j) = idx.checked_sub(32) else {
                continue;
            };
            let (Some(&src_glyph), Some(&src_width)) = (glyph_index.get(j), widths.raw.get(j))
            else {
                continue;
            };
            if let Some(slot) = glyph_index.get_mut(idx) {
                *slot = src_glyph;
            }
            // Note `!= 0`, not `!= WIDTH_UNSET`: an *unset* width is nonzero
            // and therefore propagates.
            if src_width != 0
                && let Some(slot) = widths.raw.get_mut(idx)
            {
                *slot = src_width;
            }
        }
    }
}

/// Look a glyph up by name in a face, for the ladders.
pub(crate) fn name_index(glyphs: &GlyphSource, name: &[u8]) -> u16 {
    glyphs.name_index(name)
}

/// Look a code up through a charmap, for the ladders.
pub(crate) fn char_index(glyphs: &GlyphSource, charmap: Charmap, code: u32) -> u16 {
    glyphs.char_index(charmap, code)
}

/// Drawn-outline access, so a caller need not reach into `glyphs`.
impl SimpleFont {
    /// The outline for a glyph, in 1000/em text space.
    #[must_use]
    pub fn glyph_path(&self, gid: Gid) -> Option<pdfrum_common::kurbo::BezPath> {
        self.glyphs.outline(gid, &GlyphParams::default())
    }
}

/// A resolved `/Encoding` value, exposed for tests of the decision table.
#[cfg(test)]
pub(crate) fn resolve_encoding_for_test(
    dict: &Dict,
    r: &impl Resolve,
    base_font_name: &[u8],
    flags: FontFlags,
    embedded: bool,
    is_truetype: bool,
    prior: FontEncoding,
) -> (FontEncoding, bool) {
    let mut e = prior;
    let mut diffs: [Option<GlyphName>; 256] = [const { None }; 256];
    let had = load_pdf_encoding(
        dict,
        r,
        base_font_name,
        flags,
        embedded,
        is_truetype,
        &mut e,
        &mut diffs,
    );
    (e, had)
}

#[cfg(test)]
use pdfrum_object::Object;

#[cfg(test)]
#[path = "simple_tests.rs"]
mod tests;
