//! Composite (Type0) fonts: a multi-byte code, through a CMap, to a CID, to a
//! glyph.
//!
//! This is the one font kind whose load can genuinely **fail** — four ways,
//! all of which mean "the resource is not there" and cause the content-stream
//! interpreter to skip text using the font
//! (`docs/design/pdfrum-font.md` §1.10).

mod glyph;
mod gsub;
mod transform;

pub use transform::{CidTransform, japan1_transform};

use crate::glyphs::{Charmap, Face, GlyphSource};
use crate::subst::{
    self, CodePage, FontRequest, SubstFont, SubstitutionOptions, SystemFontDb, TestFontDb,
};
use crate::widths::CidWidths;
use crate::{
    CharCode, CharItem, Cid, Error, FontCache, FontDescriptor, FontId, Gid, ToUnicode, descriptor,
    names, widths,
};
use pdfrum_cmap::{CMap, CidCoding, CidSet};
use pdfrum_common::kurbo::Rect;
use pdfrum_common::{DiagKind, Diagnostics, Limits, Severity};
use pdfrum_object::{Dict, Object, Resolve};
use smallvec::SmallVec;

pub use crate::widths::VerticalMetrics;

/// How a CID becomes a glyph index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CidToGid {
    /// `/CIDToGIDMap /Identity`, or the absence of the key for a font where
    /// CID and GID coincide.
    Identity,
    /// A `/CIDToGIDMap` stream: a big-endian `u16` table indexed by CID.
    Stream(Box<[u8]>),
    /// Neither: the glyph is found through a charmap instead.
    ViaCharmap,
}

/// Which flavour of descendant font.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CidFontKind {
    /// `CIDFontType0` — CFF outlines, where CID *is* GID for an embedded font.
    Type1,
    /// `CIDFontType2` — TrueType outlines.
    TrueType,
}

/// A composite font.
#[derive(Debug)]
pub struct Type0Font {
    /// This font's identity, for glyph-cache keys.
    pub id: FontId,
    /// The CMap that splits bytes into codes and maps them to CIDs.
    pub cmap: CMap,
    /// Where glyphs come from.
    pub glyphs: GlyphSource,
    /// The CID collection, from the CMap or from `/CIDSystemInfo`.
    pub charset: CidSet,
    /// How a CID becomes a glyph.
    pub cid_to_gid: CidToGid,
    /// `/W` and `/DW`.
    pub widths: CidWidths,
    /// `/W2` and `/DW2`, only for a vertical font.
    pub vertical: Option<VerticalMetrics>,
    /// The `/ToUnicode` CMap.
    pub to_unicode: Option<ToUnicode>,
    /// The descendant's `/FontDescriptor`.
    pub descriptor: FontDescriptor,
    /// What substitution decided.
    pub subst: Option<SubstFont>,
    /// Which flavour of descendant.
    pub kind: CidFontKind,
    /// Whether a usable font program was embedded.
    pub embedded: bool,
    /// The descendant's `/BaseFont`.
    pub base_font_name: Vec<u8>,
    /// Adobe's CourierStd, whose CIDs are offset from the standard encoding by
    /// 31 — a hard-coded rescue with no general rule behind it.
    pub adobe_courier_std: bool,
    /// The GB2312 rescue path, which fixes ASCII widths.
    pub ansi_widths_fixed: bool,
    /// Vertical substitution, parsed from `GSUB` on first use.
    gsub: gsub::VerticalSubst,
}

impl Type0Font {
    /// The CID a character code maps to.
    #[must_use]
    pub fn cid_from_charcode(&self, code: CharCode) -> Cid {
        self.cmap.cid(code)
    }

    /// The glyph a character code selects, or `None` for "draw nothing".
    ///
    /// Also reports whether a vertical form was substituted, which suppresses
    /// the Japan1 transform downstream.
    #[must_use]
    pub fn glyph_from_charcode(&self, code: CharCode) -> (Option<Gid>, bool) {
        glyph::resolve(self, code)
    }

    /// The advance width for a character code, in 1000/em units.
    #[must_use]
    pub fn char_width(&self, code: CharCode) -> f32 {
        self.widths.width(code, self.cid_from_charcode(code))
    }

    /// The vertical advance for a character code.
    #[must_use]
    pub fn vert_width(&self, code: CharCode) -> f32 {
        match &self.vertical {
            Some(v) => v.width(self.cid_from_charcode(code)),
            None => -1000.0,
        }
    }

    /// The vertical origin for a character code, in 1000/em units.
    ///
    /// Absent a `/W2` record this is **half the horizontal width** and
    /// `DW2[0]`, which is why the horizontal table is consulted here.
    #[must_use]
    pub fn vert_origin(&self, code: CharCode) -> (f32, f32) {
        let cid = self.cid_from_charcode(code);
        match &self.vertical {
            Some(v) => v.origin(cid, &self.widths),
            None => ((self.widths.width(code, cid) / 2.0).trunc(), 880.0),
        }
    }

    /// The Unicode a code stands for, `/ToUnicode` first
    /// (`CPDF_CIDFont::UnicodeFromCharCode`).
    #[must_use]
    pub fn unicode_from_charcode(&self, code: CharCode) -> SmallVec<[char; 2]> {
        if let Some(tu) = &self.to_unicode {
            let chars = tu.lookup(code);
            if !chars.is_empty() {
                return chars;
            }
        }
        match self.scalar_unicode(code) {
            0 => SmallVec::new(),
            u => char::from_u32(u32::from(u))
                .map(|c| SmallVec::from_slice(&[c]))
                .unwrap_or_default(),
        }
    }

    /// The *scalar* Unicode derivation, which does **not** consult
    /// `/ToUnicode` (`CPDF_CIDFont::GetUnicodeFromCharCode`).
    ///
    /// The CMap's coding scheme decides: a UCS-2 or UTF-16 CMap means the
    /// character code simply *is* the Unicode, while a CID-coded one goes
    /// through the collection's table.
    #[must_use]
    pub fn scalar_unicode(&self, code: CharCode) -> u16 {
        match self.cmap.coding() {
            CidCoding::Ucs2 | CidCoding::Utf16 => return (code.0 & 0xffff) as u16,
            CidCoding::Cid => {
                if !pdfrum_cmap::has_cid2unicode(self.charset) {
                    return 0;
                }
                let cid = Cid((code.0 & 0xffff) as u16);
                return pdfrum_cmap::unicode_from_cid(self.charset, cid).map_or(0, |c| c as u16);
            }
            _ => {}
        }
        if pdfrum_cmap::has_cid2unicode(self.charset) && self.cmap.is_loaded() {
            let cid = self.cid_from_charcode(code);
            return pdfrum_cmap::unicode_from_cid(self.charset, cid).map_or(0, |c| c as u16);
        }
        // The non-Windows tail: only the four CJK registries have a static
        // map to walk backwards through.
        if !self.cmap.has_static_map() {
            return 0;
        }
        let cid = self.cid_from_charcode(code);
        if cid.0 == 0 {
            return 0;
        }
        pdfrum_cmap::unicode_from_cid(self.charset, cid).map_or(0, |c| c as u16)
    }

    /// The character code for a Unicode, or 0 (`CharCodeFromUnicode`).
    #[must_use]
    pub fn charcode_from_unicode(&self, unicode: char) -> CharCode {
        if let Some(tu) = &self.to_unicode {
            let c = tu.reverse(unicode);
            if c.0 != 0 {
                return c;
            }
        }
        match self.cmap.coding() {
            CidCoding::Unknown => return CharCode(0),
            CidCoding::Ucs2 | CidCoding::Utf16 => return CharCode(unicode as u32),
            CidCoding::Cid => {
                if !pdfrum_cmap::has_cid2unicode(self.charset) {
                    return CharCode(0);
                }
                // The C++ scans all 65 536 CIDs linearly; the cmap crate has
                // the same scan behind a name.
                for cid in 0..=u16::MAX {
                    if pdfrum_cmap::unicode_from_cid(self.charset, Cid(cid)) == Some(unicode) {
                        return CharCode(u32::from(cid));
                    }
                }
            }
            _ => {}
        }
        if (unicode as u32) < 0x80 {
            return CharCode(unicode as u32);
        }
        if self.cmap.coding() == CidCoding::Cid {
            return CharCode(0);
        }
        pdfrum_cmap::charcode_from_unicode(&self.cmap, unicode)
    }

    /// Whether codes can be turned into Unicode at all (`IsUnicodeCompatible`).
    #[must_use]
    pub fn is_unicode_compatible(&self) -> bool {
        if pdfrum_cmap::has_cid2unicode(self.charset) && self.cmap.is_loaded() {
            return true;
        }
        self.cmap.coding() != CidCoding::Unknown
    }

    /// Whether the font writes vertically.
    #[must_use]
    pub fn is_vertical(&self) -> bool {
        self.cmap.is_vertical()
    }

    /// The bounding box for a code, in 1000/em units, with the Japan1
    /// transform applied when it applies.
    #[must_use]
    pub fn char_bbox(&self, code: CharCode) -> Rect {
        let (gid, vertical) = self.glyph_from_charcode(code);
        let Some(gid) = gid else { return Rect::ZERO };
        let Some(bbox) = self.glyphs.glyph_bbox(gid) else {
            return Rect::ZERO;
        };
        // The transform rotates an upright glyph into a vertical one — so a
        // glyph GSUB *already* substituted must not be rotated again.
        if vertical {
            return bbox;
        }
        match self.japan1_transform(code) {
            Some(t) => transform::apply(t, bbox),
            None => bbox,
        }
    }

    /// The Japan1 vertical transform for a code, when one applies.
    ///
    /// Only for a **non-embedded** Adobe-Japan1 font: an embedded one is
    /// expected to carry its own vertical forms.
    #[must_use]
    pub fn japan1_transform(&self, code: CharCode) -> Option<CidTransform> {
        if self.charset != CidSet::Japan1 || self.embedded {
            return None;
        }
        japan1_transform(self.cid_from_charcode(code))
    }

    /// Build one [`CharItem`].
    pub(crate) fn char_item(&self, code: CharCode) -> CharItem {
        let (gid, vertical_glyph) = self.glyph_from_charcode(code);
        CharItem {
            code,
            cid: Some(self.cid_from_charcode(code)),
            gid: gid.unwrap_or_default(),
            unicode: self.unicode_from_charcode(code),
            width: if self.is_vertical() {
                self.vert_width(code)
            } else {
                self.char_width(code)
            },
            vertical_glyph,
            has_glyph: gid.is_some(),
        }
    }

    fn gsub(&self) -> &gsub::VerticalSubst {
        &self.gsub
    }
}

/// Load a Type0 font (`CPDF_CIDFont::Load`).
///
/// # Errors
///
/// The four cases PDFium treats as "this resource does not exist": a
/// `/DescendantFonts` that is missing or does not hold exactly one element, a
/// first element that is not a dictionary, a missing `/Encoding`, and an
/// `/Encoding` that is neither a name nor a stream.
pub(crate) fn load(
    dict: &Dict,
    r: &impl Resolve,
    cache: &FontCache,
    opts: &SubstitutionOptions,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> Result<Type0Font, Error> {
    let descendants = dict
        .array(names::DESCENDANT_FONTS, r)
        .ok_or(Error::BadDescendantFonts)?;
    if descendants.len() != 1 {
        return Err(Error::BadDescendantFonts);
    }
    let cid_dict = descendants.dict_at(0, r).ok_or(Error::BadDescendantFonts)?;

    let base_font_name = cid_dict
        .name(names::BASE_FONT)
        .map(|n| n.as_bytes().to_vec())
        .unwrap_or_default();

    let encoding = dict.raw(names::ENCODING).ok_or(Error::BadCidEncoding)?;
    let kind = match cid_dict.name(names::SUBTYPE).map(|n| n.as_bytes().to_vec()) {
        Some(s) if s == b"CIDFontType0" => CidFontKind::Type1,
        _ => CidFontKind::TrueType,
    };

    // An `/Encoding` must be a name or a stream. A dictionary, an array or a
    // number fails the load outright.
    let cmap = match encoding {
        Object::Name(name) => pdfrum_cmap::from_encoding_name(name, diags),
        Object::Stream(_) | Object::Ref(_) => {
            if let Some(stream) = dict.stream(names::ENCODING, r) {
                let bytes = pdfrum_filters::decode_chain(&stream, 0, r, limits, diags).data;
                pdfrum_cmap::parse_embedded(&bytes, limits, diags)
            } else {
                // A reference is only usable if it resolves to a stream or a
                // name; anything else is one of the four fatal cases.
                let resolved = dict.get(names::ENCODING, r);
                match resolved.as_ref().and_then(|o| o.as_name()) {
                    Some(name) => pdfrum_cmap::from_encoding_name(name, diags),
                    None => return Err(Error::BadCidEncoding),
                }
            }
        }
        _ => return Err(Error::BadCidEncoding),
    };

    Ok(build(
        &cid_dict,
        dict,
        cmap,
        kind,
        base_font_name,
        r,
        cache,
        opts,
        limits,
        diags,
        false,
    ))
}

/// The GB2312 rescue path, reached only through the Chinese-name special case
/// of the font-type dispatch.
///
/// Charset GB1 with the predefined `GBK-EUC-H` CMap, and fixed ASCII widths.
pub(crate) fn load_gb2312(
    dict: &Dict,
    r: &impl Resolve,
    cache: &FontCache,
    opts: &SubstitutionOptions,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> Result<Type0Font, Error> {
    let cmap = pdfrum_cmap::predefined(&pdfrum_object::Name::from("GBK-EUC-H"))
        .ok_or(Error::BadCidEncoding)?;
    let base_font_name = dict
        .name(names::BASE_FONT)
        .map(|n| n.as_bytes().to_vec())
        .unwrap_or_default();
    Ok(build(
        dict,
        dict,
        cmap,
        CidFontKind::TrueType,
        base_font_name,
        r,
        cache,
        opts,
        limits,
        diags,
        true,
    ))
}

// The ladder below reads as one sequence — each step's inputs come from the
// step above it — and splitting it into pieces would hide the order the
// decisions have to be taken in.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn build(
    cid_dict: &Dict,
    font_dict: &Dict,
    cmap: CMap,
    kind: CidFontKind,
    base_font_name: Vec<u8>,
    r: &impl Resolve,
    cache: &FontCache,
    opts: &SubstitutionOptions,
    limits: &Limits,
    diags: &mut Diagnostics,
    gb2312: bool,
) -> Type0Font {
    // Adobe's CourierStd, whose CIDs sit 31 codes below the standard
    // encoding. There is no general rule here — four literal names.
    let adobe_courier_std = matches!(
        base_font_name.as_slice(),
        b"CourierStd" | b"CourierStd-Bold" | b"CourierStd-BoldOblique" | b"CourierStd-Oblique"
    );

    let desc = cid_dict.dict(names::FONT_DESCRIPTOR, r);
    let mut descriptor = FontDescriptor::default();
    if let Some(d) = &desc {
        descriptor = descriptor::load(d, r);
    }
    let (mut glyphs, mut embedded) =
        crate::simple::load_font_program(desc.as_ref(), r, limits, diags);

    // The collection: what the CMap says, or what `/CIDSystemInfo` says.
    let mut charset = if gb2312 { CidSet::Gb1 } else { cmap.charset() };
    if charset == CidSet::Unknown
        && let Some(info) = cid_dict.dict(names::CID_SYSTEM_INFO, r)
        && let Some(ordering) = info.byte_string(names::ORDERING, r)
    {
        charset = pdfrum_cmap::charset_from_ordering(&ordering);
    }

    let mut widths_table = CidWidths::load(cid_dict, r, diags);
    if gb2312 {
        widths_table.set_ansi_widths_fixed();
    }

    let mut subst_font = None;
    if !embedded {
        let request = FontRequest {
            name: base_font_name.clone(),
            is_truetype: kind == CidFontKind::TrueType,
            flags: descriptor.flags,
            // The C++ multiplies the stem by five here rather than using the
            // descriptor's own weight estimate, saturating to 400.
            weight: descriptor
                .stem_v
                .checked_mul(5)
                .filter(|w| *w > 0)
                .unwrap_or(400),
            italic_angle: descriptor.italic_angle,
            code_page: CodePage::for_cid_set(charset),
            vertical: cmap.is_vertical(),
        };
        let s = substitute(&request, opts, diags);
        glyphs = s.glyphs;
        subst_font = Some(s.subst);
    }
    if !glyphs.is_some() {
        embedded = false;
    }

    // `/CIDToGIDMap` is read non-resolving for the name form: PDFium checks
    // the direct object's type before resolving, so an indirect `/Identity`
    // reads as absent.
    let cid_to_gid = match cid_dict.raw(names::CID_TO_GID_MAP) {
        Some(Object::Name(n)) if n.as_bytes() == b"Identity" && embedded => CidToGid::Identity,
        Some(Object::Stream(_) | Object::Ref(_)) => {
            match cid_dict.stream(names::CID_TO_GID_MAP, r) {
                Some(s) => {
                    let bytes = pdfrum_filters::decode_chain(&s, 0, r, limits, diags).data;
                    // Two bytes per CID: a table shorter than the face has
                    // glyphs leaves the tail unmapped rather than erroring.
                    if bytes.len() < glyphs.num_glyphs() as usize * 2 {
                        diags.record(Severity::Suspicious, DiagKind::CidToGidStreamShort, None);
                    }
                    CidToGid::Stream(bytes.into_boxed_slice())
                }
                None => CidToGid::ViaCharmap,
            }
        }
        _ => CidToGid::ViaCharmap,
    };

    let vertical = if cmap.is_vertical() {
        Some(widths::VerticalMetrics::load(cid_dict, r, diags))
    } else {
        None
    };

    let to_unicode = crate::simple::load_to_unicode(font_dict, r, limits, diags);

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
    descriptor::check_font_metrics(&mut descriptor, metrics, |_| Rect::ZERO);

    let gsub = if cmap.is_vertical() {
        gsub::VerticalSubst::parse(&glyphs, diags)
    } else {
        gsub::VerticalSubst::none()
    };

    Type0Font {
        id: cache.next_id(),
        cmap,
        glyphs,
        charset,
        cid_to_gid,
        widths: widths_table,
        vertical,
        to_unicode,
        descriptor,
        subst: subst_font,
        kind,
        embedded,
        base_font_name,
        adobe_courier_std,
        ansi_widths_fixed: gb2312,
        gsub,
    }
}

fn substitute(
    request: &FontRequest,
    opts: &SubstitutionOptions,
    diags: &mut Diagnostics,
) -> subst::Substitution {
    if opts.font_dirs.is_empty() {
        return subst::resolve(request, &TestFontDb::new(), opts, diags);
    }
    let db = SystemFontDb::scan(&opts.font_dirs);
    subst::resolve(request, &db, opts, diags)
}

/// Choose the charmap a CID font drives (`UseCIDCharmap`).
///
/// Three rungs: the **legacy** charmap the coding scheme names, then Unicode,
/// then whatever charmap comes first. A CJK font that carries its national
/// encoding's own subtable is driven through *that*, with character codes
/// passed straight in — which is why the embedded glyph ladder branches on
/// whether the chosen charmap is Unicode before deciding what to look up.
///
/// Note **Korea asks for Johab**, encoding id 6, not Wansung's 5. A font
/// carrying only a Wansung subtable therefore falls through to Unicode, which
/// looks like an oversight and is what the oracle does.
pub(crate) fn cid_charmap(glyphs: &GlyphSource, coding: CidCoding) -> Charmap {
    let charmaps = glyphs.charmaps();

    // Rung 1 — the national encoding, as a Windows-platform subtable.
    if let Some(wanted) = legacy_encoding_id(coding)
        && let Some(i) = charmaps
            .iter()
            .position(|c| c.platform == 3 && c.encoding == wanted)
    {
        return Charmap::Index(i);
    }
    // Rung 2 — Unicode.
    if let Some(i) = charmaps.iter().position(|c| c.is_unicode()) {
        return Charmap::Index(i);
    }
    // Rung 3 — anything at all.
    if charmaps.is_empty() {
        Charmap::None
    } else {
        Charmap::Index(0)
    }
}

/// The `cmap` encoding id a CID coding scheme's legacy charmap carries, on the
/// Windows platform.
///
/// The ids are the `TT_MS_ID_*` values FreeType maps its `FT_ENCODING_*`
/// constants onto: Shift-JIS 2, GB2312 3, Big5 4, Johab 6.
fn legacy_encoding_id(coding: CidCoding) -> Option<u16> {
    Some(match coding {
        CidCoding::Gb => 3,
        CidCoding::Big5 => 4,
        CidCoding::Jis => 2,
        CidCoding::Korea => 6,
        // Every other scheme asks for Unicode outright, which rung 2 covers.
        CidCoding::Unknown | CidCoding::Ucs2 | CidCoding::Cid | CidCoding::Utf16 => return None,
    })
}

/// Read a `Face` out of a glyph source, for the GSUB reader.
pub(crate) fn face_of(glyphs: &GlyphSource) -> Option<&Face> {
    match glyphs {
        GlyphSource::Fontations(f) => Some(f),
        GlyphSource::Type1(_) | GlyphSource::None => None,
    }
}

#[cfg(test)]
#[path = "cid_tests.rs"]
mod tests;
