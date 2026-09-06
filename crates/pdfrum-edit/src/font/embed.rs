//! Embed a font program, or one of the standard 14, as PDF font objects.
//!
//! A caller hands over bytes (or a [`StandardFont`]); this allocates the
//! `/Font` dictionary chain [`super::collect::admit`] already recognises.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;

use pdfrum_font::{
    FaceEncoding, FontFlags, GlyphSource, StandardFont, canonical_font_name, em_adjust,
};
use pdfrum_object::{Array, ByteSpan, Dict, Name, ObjRef, Object, PdfString, Stream};

use crate::doc::EditDoc;
use crate::error::Error;
use crate::font::is_opentype_cff;
use crate::names;

/// How character codes in a content stream select glyphs of an embedded font.
///
/// The oracle's API spells this choice as a boolean; here it is an enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FontEncoding {
    /// One-byte codes, `/FirstChar` `/LastChar` `/Widths` (ISO 32000-1 §9.6.2).
    Simple,
    /// Identity-H, two-byte GIDs as CIDs, `/W` and `/ToUnicode` (§9.7).
    Composite,
}

/// A font dictionary this session added, ready to name from a content stream.
///
/// [`EmbeddedFont::object`] is the `/Font` resource [`crate::content::ResourceTable::realize`]
/// will allocate a name for. [`EmbeddedFont::encode`] turns Unicode into the
/// character codes a `Tj`/`TJ` of that font expects.
#[derive(Debug, Clone)]
pub struct EmbeddedFont {
    font: ObjRef,
    kind: EncodeKind,
}

/// How [`EmbeddedFont::encode`] turns a character into codes.
#[derive(Debug, Clone)]
enum EncodeKind {
    /// Identity-H: two-byte big-endian GIDs. Missing cmap entries become 0.
    Identity { unicode_to_gid: HashMap<u32, u16> },
    /// One-byte codes from the Unicode cmap, clipped to `0xFF`.
    Simple { unicode_to_code: HashMap<u32, u8> },
    /// `/WinAnsiEncoding`, used by the standard 14.
    WinAnsi,
    /// A composite font whose `/ToUnicode` the caller wrote: two-byte
    /// big-endian CIDs, taken by inverting that CMap rather than the
    /// program's own cmap.
    CustomCid { unicode_to_cid: HashMap<char, u32> },
}

impl EmbeddedFont {
    /// The `/Font` dictionary to name from a page resource.
    #[must_use]
    pub fn object(&self) -> ObjRef {
        self.font
    }

    /// Character codes for `text` in this font's encoding.
    ///
    /// Unmappable characters become `.notdef` (code 0). A composite font
    /// writes two-byte big-endian GIDs (Identity-H); a simple or standard
    /// font writes one byte.
    ///
    /// A font from [`EditDoc::embed_cid_font`] writes two-byte big-endian
    /// **CIDs**, and finds them by inverting the caller's `/ToUnicode` CMap
    /// rather than by consulting the program's cmap. That is not a
    /// convenience: under a caller-supplied CMap the program's cmap does not
    /// describe the file's code space, and the CMap is the document's only
    /// statement of what its codes mean. A character the CMap does not reach
    /// is code 0, as everywhere else.
    //
    // Both rules are the oracle's too: `CPDF_Font::CharCodeFromUnicode`
    // (`core/fpdfapi/font/cpdf_font.cpp:110-115`) falls back to 0, and for a
    // font with a caller-supplied CMap it *is*
    // `to_unicode_map_->ReverseLookup` over that same `/ToUnicode`, reached
    // by `FPDFText_SetText`.
    #[must_use]
    pub fn encode(&self, text: &str) -> Vec<u8> {
        let mut out = Vec::with_capacity(text.len());
        let notdef = self.kind.notdef();
        for ch in text.chars() {
            push_code(&mut out, self.code_of(ch).unwrap_or(notdef));
        }
        out
    }

    /// Character codes for `text`, refusing a character this font cannot
    /// draw.
    ///
    /// The same encoding as [`EmbeddedFont::encode`], with the `.notdef`
    /// fallback replaced by an error. This is what a caller who is *placing*
    /// text wants: a watermark whose degree sign silently became a blank is
    /// worse than one that refused to be written.
    ///
    /// ```
    /// use pdfrum_edit::{EditDoc, StandardFont};
    /// use pdfrum_parser::{LoadOptions, load};
    /// use std::sync::Arc;
    ///
    /// let bytes: Arc<[u8]> = Arc::from(&include_bytes!("../../tests/files/hello.pdf")[..]);
    /// let doc = load(bytes, &LoadOptions::default())?;
    /// let mut edit = EditDoc::new(&doc);
    /// let font = edit.standard_font(StandardFont::Helvetica)?;
    ///
    /// assert!(font.encode_checked("Hi").is_ok());
    /// // WinAnsi has no Han: refused rather than drawn blank.
    /// assert_eq!(font.encode_checked("\u{4e00}").unwrap_err().character, '\u{4e00}');
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// # Errors
    ///
    /// [`MissingGlyph`] naming the first character with no glyph, and its
    /// byte offset in `text`.
    pub fn encode_checked(&self, text: &str) -> Result<Vec<u8>, MissingGlyph> {
        let mut out = Vec::with_capacity(text.len());
        for (offset, ch) in text.char_indices() {
            let code = self.code_of(ch).ok_or(MissingGlyph {
                character: ch,
                offset,
            })?;
            push_code(&mut out, code);
        }
        Ok(out)
    }

    /// The code `ch` is drawn through, or `None` when this font has no glyph
    /// for it.
    fn code_of(&self, ch: char) -> Option<Code> {
        let missing = |code: u32| (code != 0).then_some(code);
        match &self.kind {
            EncodeKind::Identity { unicode_to_gid } => unicode_to_gid
                .get(&u32::from(ch))
                .copied()
                .and_then(|gid| (gid != 0).then_some(gid))
                .map(Code::Two),
            EncodeKind::Simple { unicode_to_code } => unicode_to_code
                .get(&u32::from(ch))
                .copied()
                .and_then(|code| (code != 0).then_some(code))
                .map(Code::One),
            EncodeKind::CustomCid { unicode_to_cid } => unicode_to_cid
                .get(&ch)
                .copied()
                .and_then(missing)
                .and_then(|cid| u16::try_from(cid).ok())
                .map(Code::Two),
            EncodeKind::WinAnsi => u16::try_from(u32::from(ch))
                .ok()
                .map(|u| FaceEncoding::Latin1.charcode_from_unicode(u))
                .and_then(missing)
                .and_then(|code| u8::try_from(code).ok())
                .map(Code::One),
        }
    }
}

/// One character's code in an embedded font's encoding: one byte for a simple
/// font, two big-endian for a composite one.
///
/// A tiny enum rather than a `(bytes, width)` pair so the two widths cannot be
/// mixed up at the push.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Code {
    /// A one-byte code (simple, or one of the standard 14).
    One(u8),
    /// A two-byte big-endian CID or GID (composite).
    Two(u16),
}

impl EncodeKind {
    /// This encoding's `.notdef`, **in its own code width**.
    ///
    /// A composite font's codes are two bytes, so its `.notdef` is two zero
    /// bytes and not one: writing a single zero would shift every code after
    /// it by a byte and turn the rest of the string into noise.
    fn notdef(&self) -> Code {
        match self {
            Self::Identity { .. } | Self::CustomCid { .. } => Code::Two(0),
            Self::Simple { .. } | Self::WinAnsi => Code::One(0),
        }
    }
}

/// Append `code`'s bytes.
fn push_code(out: &mut Vec<u8>, code: Code) {
    match code {
        Code::One(byte) => out.push(byte),
        Code::Two(pair) => out.extend_from_slice(&pair.to_be_bytes()),
    }
}

/// A character an embedded font has no glyph for.
///
/// What [`EmbeddedFont::encode_checked`] returns instead of writing
/// `.notdef`.
///
/// ```
/// use pdfrum_edit::MissingGlyph;
///
/// let missing = MissingGlyph { character: '\u{4e00}', offset: 3 };
/// assert_eq!(missing.to_string(), "the font has no glyph for '\u{4e00}' at byte 3");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the font has no glyph for {character:?} at byte {offset}")]
pub struct MissingGlyph {
    /// The character with no glyph.
    pub character: char,
    /// Its byte offset in the string that was encoded.
    pub offset: usize,
}

/// Kind of program, sniffed from the leading bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProgramKind {
    TrueType,
    OpenTypeCff,
    Type1,
}

impl EditDoc<'_> {
    /// Embed `program` as a new `/Font` in this document.
    ///
    /// The program kind is detected from its bytes: an `OTTO` tag is
    /// OpenType/CFF, `0x00010000` / `true` / `typ1` is TrueType, and a PFB
    /// marker or `%!PS` banner is Type 1.
    ///
    /// # Errors
    ///
    /// [`Error::UnrecognisedFontProgram`] when no backend can read the bytes,
    /// [`Error::EmptyFontProgram`] when the face declares no glyphs.
    pub fn embed_font(
        &mut self,
        program: &[u8],
        encoding: FontEncoding,
    ) -> Result<EmbeddedFont, Error> {
        let kind = sniff_kind(program).ok_or(Error::UnrecognisedFontProgram)?;
        let glyphs = GlyphSource::from_bytes(program).ok_or(Error::UnrecognisedFontProgram)?;
        if glyphs.num_glyphs() == 0 {
            return Err(Error::EmptyFontProgram);
        }
        match encoding {
            FontEncoding::Simple => embed_simple(self, program, kind, &glyphs),
            FontEncoding::Composite => embed_composite(self, program, kind, &glyphs),
        }
    }

    /// Embed `program` as a `/Type0` + `/CIDFontType2` font whose
    /// `/ToUnicode` and `/CIDToGIDMap` are the **caller's**, not generated
    /// from the program's own cmap.
    ///
    /// The dictionary chain is the one [`Self::embed_font`] builds for
    /// [`FontEncoding::Composite`], with three differences, all of them the
    /// point of the call:
    ///
    /// - **`/CIDToGIDMap`** is a stream holding `cid_to_gid` verbatim: one
    ///   big-endian `u16` glyph index per CID, indexed by CID (ISO 32000-1
    ///   §9.7.4.2). [`Self::embed_font`] writes no `/CIDToGIDMap` at all,
    ///   which means `/Identity` — CID *is* GID.
    /// - **`/W`** is computed per CID from that map rather than per GID from
    ///   the cmap: each two-byte entry names the glyph whose advance that CID
    ///   gets, so the array is dense from CID 0 and holds one width per map
    ///   entry.
    /// - **`/ToUnicode`** is a stream holding `to_unicode` verbatim.
    ///
    /// The descendant is always a `/CIDFontType2`, because that is what
    /// `/CIDToGIDMap` means: a `/CIDFontType0` reaches glyphs with the CID as
    /// the glyph index and never consults the map.
    ///
    /// [`EmbeddedFont::encode`] on the result writes CIDs found by inverting
    /// `to_unicode`, not GIDs found in the program.
    ///
    /// # Errors
    ///
    /// [`Error::UnrecognisedFontProgram`] when no backend can read the bytes,
    /// [`Error::EmptyFontProgram`] when the face declares no glyphs,
    /// [`Error::EmptyToUnicodeCMap`] for an empty `to_unicode`, and
    /// [`Error::BadCidToGidMap`] when `cid_to_gid` is empty or is not a whole
    /// number of two-byte entries.
    pub fn embed_cid_font(
        &mut self,
        program: &[u8],
        to_unicode: &str,
        cid_to_gid: &[u8],
    ) -> Result<EmbeddedFont, Error> {
        // `FPDFText_LoadCidType2Font` (`fpdfsdk/fpdf_edittext.cpp:481-516` →
        // `LoadCustomCompositeFont` `:281-334`) is where this shape comes
        // from. The per-CID `/W` walk is `:307-315`, and the "always
        // TrueType" choice is `FPDF_FONT_TRUETYPE` at `:296-303`; a
        // `/CIDFontType0` ignoring `/CIDToGIDMap` is
        // `core/fpdfapi/font/cpdf_cidfont.cpp:508-518`.
        if to_unicode.is_empty() {
            return Err(Error::EmptyToUnicodeCMap);
        }
        if cid_to_gid.is_empty() || !cid_to_gid.len().is_multiple_of(2) {
            return Err(Error::BadCidToGidMap(cid_to_gid.len()));
        }
        let glyphs = GlyphSource::from_bytes(program).ok_or(Error::UnrecognisedFontProgram)?;
        if glyphs.num_glyphs() == 0 {
            return Err(Error::EmptyFontProgram);
        }
        Ok(embed_custom_composite(
            self, program, to_unicode, cid_to_gid, &glyphs,
        ))
    }

    /// Add a non-embedded standard-14 Type 1 font (`/BaseFont`, `/Encoding
    /// /WinAnsiEncoding`, no `/Widths`).
    ///
    /// # Errors
    ///
    /// This path does not fail today; the [`Result`] is for symmetry with
    /// [`Self::embed_font`] and for a caller that wants to handle both the
    /// same way.
    pub fn standard_font(&mut self, which: StandardFont) -> Result<EmbeddedFont, Error> {
        let name = canonical_font_name(which);
        let dict = Dict::from_pairs([
            (names::TYPE.clone(), Object::Name(names::FONT.clone())),
            (names::SUBTYPE.clone(), Object::Name(names::TYPE1.clone())),
            (
                names::BASE_FONT.clone(),
                Object::Name(Name::from(name.as_bytes())),
            ),
            (
                names::ENCODING.clone(),
                Object::Name(names::WIN_ANSI_ENCODING.clone()),
            ),
        ]);
        Ok(EmbeddedFont {
            font: self.add(Object::Dict(dict)),
            kind: EncodeKind::WinAnsi,
        })
    }
}

fn sniff_kind(bytes: &[u8]) -> Option<ProgramKind> {
    if is_opentype_cff(bytes) {
        return Some(ProgramKind::OpenTypeCff);
    }
    if matches!(
        bytes.get(..4),
        Some(&[0x00, 0x01, 0x00, 0x00] | b"true" | b"typ1")
    ) {
        return Some(ProgramKind::TrueType);
    }
    if bytes.first() == Some(&0x80) {
        return Some(ProgramKind::Type1);
    }
    let start = bytes
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    let rest = bytes.get(start..).unwrap_or_default();
    if rest.starts_with(b"%!PS-AdobeFont") || rest.starts_with(b"%!FontType1") {
        return Some(ProgramKind::Type1);
    }
    // Ambiguous header: trust the parser.
    let glyphs = GlyphSource::from_bytes(bytes)?;
    if matches!(glyphs, GlyphSource::Type1(_)) {
        Some(ProgramKind::Type1)
    } else if glyphs.is_truetype() {
        Some(ProgramKind::TrueType)
    } else {
        Some(ProgramKind::OpenTypeCff)
    }
}

fn embed_simple(
    doc: &mut EditDoc<'_>,
    program: &[u8],
    kind: ProgramKind,
    glyphs: &GlyphSource,
) -> Result<EmbeddedFont, Error> {
    let pairs = char_maps(glyphs, 0xFF);
    if pairs.is_empty() {
        return Err(Error::EmptyFontProgram);
    }
    let name = font_name(glyphs);
    let first = pairs.first().map_or(0, |p| p.0);
    let last = pairs.last().map_or(first, |p| p.0);
    let by_code: BTreeMap<u32, u16> = pairs.iter().copied().collect();
    let mut widths = Vec::new();
    let mut code = first;
    while code <= last {
        let w = by_code
            .get(&code)
            .map_or(0, |gid| glyphs.default_advance(pdfrum_font::Gid(*gid)));
        widths.push(Object::Int(i64::from(w)));
        code = code.saturating_add(1);
        if code == 0 && last == 0 {
            break;
        }
    }
    let widths_ref = doc.add(Object::Array(Array::of(widths)));
    let descriptor = load_font_desc(doc, &name, program, kind, glyphs);
    let subtype = match kind {
        ProgramKind::Type1 => names::TYPE1.clone(),
        ProgramKind::TrueType | ProgramKind::OpenTypeCff => names::TRUE_TYPE.clone(),
    };
    let dict = Dict::from_pairs([
        (names::TYPE.clone(), Object::Name(names::FONT.clone())),
        (names::SUBTYPE.clone(), Object::Name(subtype)),
        (
            names::BASE_FONT.clone(),
            Object::Name(Name::from(name.as_slice())),
        ),
        (names::FIRST_CHAR.clone(), Object::Int(i64::from(first))),
        (names::LAST_CHAR.clone(), Object::Int(i64::from(last))),
        (names::WIDTHS.clone(), Object::Ref(widths_ref)),
        (names::FONT_DESCRIPTOR.clone(), Object::Ref(descriptor)),
    ]);
    let unicode_to_code: HashMap<u32, u8> = pairs
        .into_iter()
        .filter_map(|(cp, _)| u8::try_from(cp).ok().map(|b| (cp, b)))
        .collect();
    Ok(EmbeddedFont {
        font: doc.add(Object::Dict(dict)),
        kind: EncodeKind::Simple { unicode_to_code },
    })
}

fn embed_composite(
    doc: &mut EditDoc<'_>,
    program: &[u8],
    kind: ProgramKind,
    glyphs: &GlyphSource,
) -> Result<EmbeddedFont, Error> {
    let pairs = char_maps(glyphs, 0x0010_FFFF);
    if pairs.is_empty() {
        return Err(Error::EmptyFontProgram);
    }
    let name = font_name(glyphs);
    let base = match kind {
        ProgramKind::Type1 => {
            let mut n = name.clone();
            n.extend_from_slice(b"-Identity-H");
            n
        }
        ProgramKind::TrueType | ProgramKind::OpenTypeCff => name.clone(),
    };
    let descriptor = load_font_desc(doc, &name, program, kind, glyphs);

    let mut widths_map: BTreeMap<u32, u32> = BTreeMap::new();
    let mut to_unicode: BTreeMap<u32, u32> = BTreeMap::new();
    let mut unicode_to_gid = HashMap::new();
    for (cp, gid) in &pairs {
        let w = glyphs.default_advance(pdfrum_font::Gid(*gid));
        widths_map
            .entry(u32::from(*gid))
            .or_insert(u32::try_from(w).unwrap_or(0));
        to_unicode.entry(u32::from(*gid)).or_insert(*cp);
        unicode_to_gid.insert(*cp, *gid);
    }
    let w_ref = doc.add(Object::Array(create_widths_array(&widths_map)));
    let tounicode_ref = doc.add(Object::Stream(Box::new(Stream::new(
        Dict::new(),
        ByteSpan::from(load_unicode(&to_unicode)),
    ))));

    let cid_subtype = match kind {
        ProgramKind::TrueType => names::CID_FONT_TYPE2.clone(),
        ProgramKind::Type1 | ProgramKind::OpenTypeCff => names::CID_FONT_TYPE0.clone(),
    };
    let dict = cid_font_dict(doc, &name, cid_subtype, descriptor, w_ref);
    let cid_font = doc.add(Object::Dict(dict));
    Ok(EmbeddedFont {
        font: doc.add(Object::Dict(type0_font_dict(
            &base,
            cid_font,
            tounicode_ref,
        ))),
        kind: EncodeKind::Identity { unicode_to_gid },
    })
}

/// A composite font whose `/ToUnicode` and `/CIDToGIDMap` the caller supplied.
///
/// The one place the two composite paths genuinely differ is `/W`: here it is
/// keyed by **CID**, walked out of the caller's map, so the array runs dense
/// from CID 0 and is exactly as long as the map has entries. `embed_composite`
/// keys it by GID out of the program's cmap instead, which is only the same
/// array when `/CIDToGIDMap` is the identity.
fn embed_custom_composite(
    doc: &mut EditDoc<'_>,
    program: &[u8],
    to_unicode: &str,
    cid_to_gid: &[u8],
    glyphs: &GlyphSource,
) -> EmbeddedFont {
    let name = font_name(glyphs);
    // `LoadCustomCompositeFont` is `fpdfsdk/fpdf_edittext.cpp:281-334`, and
    // `FPDF_FONT_TRUETYPE` at `:296-303`: /CIDToGIDMap is what makes this a
    // /CIDFontType2, and a /CIDFontType0 would ignore it outright.
    let descriptor = load_font_desc(doc, &name, program, ProgramKind::TrueType, glyphs);

    let mut widths: BTreeMap<u32, u32> = BTreeMap::new();
    for (cid, entry) in cid_to_gid.chunks_exact(2).enumerate() {
        let gid = u16::from_be_bytes([
            entry.first().copied().unwrap_or(0),
            entry.get(1).copied().unwrap_or(0),
        ]);
        let advance = glyphs.default_advance(pdfrum_font::Gid(gid));
        widths.insert(
            u32::try_from(cid).unwrap_or(u32::MAX),
            u32::try_from(advance).unwrap_or(0),
        );
    }
    let w_ref = doc.add(Object::Array(create_widths_array(&widths)));

    let map_ref = doc.add(Object::Stream(Box::new(Stream::new(
        Dict::new(),
        ByteSpan::from(cid_to_gid.to_vec()),
    ))));
    let tounicode_ref = doc.add(Object::Stream(Box::new(Stream::new(
        Dict::new(),
        ByteSpan::from(to_unicode.as_bytes().to_vec()),
    ))));

    let mut dict = cid_font_dict(doc, &name, names::CID_FONT_TYPE2.clone(), descriptor, w_ref);
    dict.push(names::CID_TO_GID_MAP.clone(), Object::Ref(map_ref));
    let cid_font = doc.add(Object::Dict(dict));

    // The caller's CMap is the file's statement of what its codes mean, so it
    // is what `encode` inverts — `CPDF_Font::CharCodeFromUnicode` is
    // `to_unicode_map_->ReverseLookup` over this same stream
    // (`core/fpdfapi/font/cpdf_font.cpp:110-115`).
    let unicode_to_cid = pdfrum_font::invert_to_unicode(
        to_unicode.as_bytes(),
        &pdfrum_common::Limits::default(),
        &mut pdfrum_common::Diagnostics::default(),
    );
    EmbeddedFont {
        font: doc.add(Object::Dict(type0_font_dict(
            &name,
            cid_font,
            tounicode_ref,
        ))),
        kind: EncodeKind::CustomCid { unicode_to_cid },
    }
}

/// The descendant `/CIDFontType0` / `/CIDFontType2` dictionary and its
/// `/CIDSystemInfo`.
///
/// `Adobe`/`Identity`/`0` because the root's `/Encoding` is `Identity-H`: the
/// CID *is* the code, so no registry ordering applies.
fn cid_font_dict(
    doc: &mut EditDoc<'_>,
    name: &[u8],
    subtype: Name,
    descriptor: ObjRef,
    widths: ObjRef,
) -> Dict {
    let system_info = doc.add(Object::Dict(Dict::from_pairs([
        (
            names::REGISTRY.clone(),
            Object::Str(PdfString::literal(b"Adobe")),
        ),
        (
            names::ORDERING.clone(),
            Object::Str(PdfString::literal(b"Identity")),
        ),
        (names::SUPPLEMENT.clone(), Object::Int(0)),
    ])));
    Dict::from_pairs([
        (names::TYPE.clone(), Object::Name(names::FONT.clone())),
        (names::SUBTYPE.clone(), Object::Name(subtype)),
        (names::BASE_FONT.clone(), Object::Name(Name::from(name))),
        (names::CID_SYSTEM_INFO.clone(), Object::Ref(system_info)),
        (names::FONT_DESCRIPTOR.clone(), Object::Ref(descriptor)),
        (names::W.clone(), Object::Ref(widths)),
    ])
}

/// The `/Type0` root naming `Identity-H`, one descendant and a `/ToUnicode`.
fn type0_font_dict(base: &[u8], cid_font: ObjRef, to_unicode: ObjRef) -> Dict {
    Dict::from_pairs([
        (names::TYPE.clone(), Object::Name(names::FONT.clone())),
        (names::SUBTYPE.clone(), Object::Name(names::TYPE0.clone())),
        (
            names::ENCODING.clone(),
            Object::Name(names::IDENTITY_H.clone()),
        ),
        (names::BASE_FONT.clone(), Object::Name(Name::from(base))),
        (
            names::DESCENDANT_FONTS.clone(),
            Object::Array(Array::of([Object::Ref(cid_font)])),
        ),
        (names::TO_UNICODE.clone(), Object::Ref(to_unicode)),
    ])
}

/// `/FontDescriptor` plus the program stream.
///
/// Flags, bbox, italic angle, ascent/descent and `/StemV` are read off the
/// face. Three keys are `[oracle-bug]` sites, each fixed here to what
/// ISO 32000-1 asks for; the `//` comments on the body carry the oracle's
/// defect and the pdf.js reading for each.
///
/// - **OTTO** goes to `/FontFile3` with `/Subtype /OpenType` (table 126),
///   not `/FontFile2`. `is_opentype_cff` and the subsetter already agree.
/// - **Type 1** is unwrapped out of its PFB container and stored raw, with
///   `/Length1` `/Length2` `/Length3` partitioning what was stored
///   (table 127).
/// - **`/CapHeight`** is OS/2 `sCapHeight` when the face has it, which is the
///   capital-letter height ISO 32000-1 table 122 asks for; the ascent is only
///   the fallback.
fn load_font_desc(
    doc: &mut EditDoc<'_>,
    font_name: &[u8],
    program: &[u8],
    kind: ProgramKind,
    glyphs: &GlyphSource,
) -> ObjRef {
    let mut flags = FontFlags::NON_SYMBOLIC;
    if glyphs.is_fixed_pitch() {
        flags = flags.with(FontFlags::FIXED_PITCH);
    }
    if font_name.windows(5).any(|w| w == b"Serif") {
        flags = flags.with(FontFlags::SERIF);
    }
    if glyphs.is_italic() {
        flags = flags.with(FontFlags::ITALIC);
    }
    if glyphs.is_bold() {
        flags = flags.with(FontFlags::FORCE_BOLD);
    }

    let upem = glyphs.units_per_em();
    let ascent = glyphs.unscaled_ascent().map_or(0, |v| em_adjust(v, upem));
    let descent = glyphs.unscaled_descent().map_or(0, |v| em_adjust(v, upem));
    let (x0, y0, x1, y1) = glyphs.unscaled_bbox().map_or((0, 0, 0, 0), |(l, b, r, t)| {
        (
            em_adjust(l, upem),
            em_adjust(b, upem),
            em_adjust(r, upem),
            em_adjust(t, upem),
        )
    });
    // [oracle-bug] fpdfsdk/fpdf_edittext.cpp:160-161 always writes
    // CapHeight = GetAscent(); ISO 32000-1 table 122 has CapHeight as the
    // capital-letter height. OS/2 sCapHeight is that value when present.
    // pdf.js is a reader, not a writer: `translateFont` *depends* on the
    // descriptor value — `let capHeight = descriptor.get("CapHeight")`
    // (src/core/evaluator.js:4731) — and `Font` stores
    // `this.capHeight = properties.capHeight / PDF_GLYPH_SPACE_UNITS`
    // (src/core/fonts.js:1123).
    let cap_height = glyphs.cap_height_unscaled().map_or(ascent, |v| {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "font units rounded to the integer the PDF descriptor stores"
        )]
        let n = v.round() as i32;
        em_adjust(n, upem)
    });
    let italic_angle = if glyphs.is_italic() { -12 } else { 0 };
    let stem_v = if glyphs.is_bold() { 120 } else { 70 };

    let file = embed_program(doc, program, kind);
    let mut desc = Dict::from_pairs([
        (
            names::TYPE.clone(),
            Object::Name(Name::from("FontDescriptor")),
        ),
        (
            names::FONT_NAME.clone(),
            Object::Name(Name::from(font_name)),
        ),
        (names::FLAGS.clone(), Object::Int(i64::from(flags.bits()))),
        (
            names::FONT_BBOX.clone(),
            Object::Array(Array::of(
                [x0, y0, x1, y1].map(|v| Object::Int(i64::from(v))),
            )),
        ),
        (names::ITALIC_ANGLE.clone(), Object::Int(italic_angle)),
        (names::ASCENT.clone(), Object::Int(i64::from(ascent))),
        (names::DESCENT.clone(), Object::Int(i64::from(descent))),
        (
            names::CAP_HEIGHT.clone(),
            Object::Int(i64::from(cap_height)),
        ),
        (names::STEM_V.clone(), Object::Int(stem_v)),
    ]);
    let file_key = match kind {
        ProgramKind::Type1 => names::FONT_FILE.clone(),
        ProgramKind::TrueType => names::FONT_FILE2.clone(),
        // [oracle-bug] fpdfsdk/fpdf_edittext.cpp:171-174 writes /FontFile2
        // for every non-Type1 program, including OTTO. ISO 32000-1 §9.9
        // table 126 puts a CFF-in-OpenType program in /FontFile3 with
        // /Subtype /OpenType; `is_opentype_cff` and the subsetter already
        // follow that. pdf.js is a reader: `isOpenTypeFile` sniffs `OTTO`
        // (src/core/fonts.js:319) and `getFontFileType` classifies it
        // `"OpenType"` (`:357`); `checkAndRepair` then requires
        // `fontFileN === "FontFile3"` for an OTTO CFF CID (`:2761-2763`).
        // `translateFont` walks FontFile/2/3 (src/core/evaluator.js:4633)
        // and reads the stream dict `/Subtype` (`:4668-4670`).
        ProgramKind::OpenTypeCff => names::FONT_FILE3.clone(),
    };
    desc.push(file_key, Object::Ref(file));
    doc.add(Object::Dict(desc))
}

/// The `/FontFile*` stream: the program bytes, and the length keys that
/// describe them.
///
/// A TrueType or OpenType program is stored exactly as handed over. A **Type 1
/// program is unwrapped first**: a PFB is a container whose `[0x80, type,
/// len:u32le]` record headers and `80 03` end marker are framing, not font
/// data, and ISO 32000-1 §9.9 table 127 defines `/Length1` `/Length2`
/// `/Length3` as a partition of the *stored* stream. Concatenating the PFB's
/// record bodies gives the raw PFA-shaped program those three lengths measure.
fn embed_program(doc: &mut EditDoc<'_>, program: &[u8], kind: ProgramKind) -> ObjRef {
    let mut dict = Dict::new();
    let bytes = match kind {
        ProgramKind::TrueType => {
            dict.push(
                names::LENGTH1.clone(),
                Object::Int(i64::try_from(program.len()).unwrap_or(i64::MAX)),
            );
            program.to_vec()
        }
        ProgramKind::OpenTypeCff => {
            dict.push(
                names::SUBTYPE.clone(),
                Object::Name(names::OPEN_TYPE.clone()),
            );
            program.to_vec()
        }
        ProgramKind::Type1 => {
            // [oracle-bug] fpdfsdk/fpdf_edittext.cpp:166-174 stores the
            // caller's bytes verbatim under `TODO(npm): Lengths for Type1
            // fonts.` and writes none of /Length1 /Length2 /Length3 — so a PFB
            // reaches /FontFile with its segment framing intact and nothing
            // describing it. ISO 32000-1 §9.9 table 127 requires all three,
            // and requires them to partition the stream: clear-text portion,
            // encrypted portion, fixed-content (`cleartomark`) portion of a
            // *raw* Type 1 program. So the PFB is unwrapped and the raw
            // program is what we store. pdf.js is a reader: `translateFont`
            // pulls the three lengths off the stream dict
            // (src/core/evaluator.js:4672-4674) and `Type1Font.#parseType1`
            // splits the header and eexec blocks with `properties.length1` /
            // `properties.length2` (src/core/type1_font.js:195-201) — given
            // the oracle's output it would slice PFB headers as font data.
            let file = pdfrum_font::type1_font_file(program);
            dict.push(names::LENGTH1.clone(), Object::Int(i64::from(file.length1)));
            dict.push(names::LENGTH2.clone(), Object::Int(i64::from(file.length2)));
            dict.push(names::LENGTH3.clone(), Object::Int(i64::from(file.length3)));
            file.program
        }
    };
    doc.add(Object::Stream(Box::new(Stream::new(
        dict,
        ByteSpan::from(bytes),
    ))))
}

fn font_name(glyphs: &GlyphSource) -> Vec<u8> {
    match glyphs.postscript_name().filter(|s| !s.is_empty()) {
        Some(name) => name.into_bytes(),
        None => b"Untitled".to_vec(),
    }
}

fn char_maps(glyphs: &GlyphSource, max: u32) -> Vec<(u32, u16)> {
    let mut pairs = glyphs.unicode_mappings(max);
    if pairs.is_empty() {
        let n = glyphs.num_glyphs().min(max.saturating_add(1));
        pairs = (0..n)
            .filter_map(|i| u16::try_from(i).ok().map(|g| (u32::from(g), g)))
            .collect();
    }
    pairs
}

/// The `/W` array: a run of consecutive CIDs sharing one width goes out as
/// `first last width`, any other consecutive block as `first [w w …]`.
fn create_widths_array(widths: &BTreeMap<u32, u32>) -> Array {
    let mut out = Array::new();
    let keys: Vec<u32> = widths.keys().copied().collect();
    let mut i = 0;
    while i < keys.len() {
        let Some(&cid) = keys.get(i) else {
            break;
        };
        let width = widths.get(&cid).copied().unwrap_or(0);
        let mut j = i + 1;
        let same_run = keys.get(j).copied() == Some(cid.saturating_add(1))
            && widths.get(&cid.saturating_add(1)).copied() == Some(width);
        if same_run {
            let mut last = cid;
            while let Some(&next) = keys.get(j) {
                if next != last.saturating_add(1) || widths.get(&next).copied() != Some(width) {
                    break;
                }
                last = next;
                j += 1;
            }
            out.push(Object::Int(i64::from(cid)));
            out.push(Object::Int(i64::from(last)));
            out.push(Object::Int(i64::from(width)));
            i = j;
            continue;
        }
        out.push(Object::Int(i64::from(cid)));
        let mut inner = Array::new();
        inner.push(Object::Int(i64::from(width)));
        let mut last = cid;
        while let Some(&next) = keys.get(j) {
            if next != last.saturating_add(1) {
                break;
            }
            inner.push(Object::Int(i64::from(
                widths.get(&next).copied().unwrap_or(0),
            )));
            last = next;
            j += 1;
        }
        out.push(Object::Array(inner));
        i = j;
    }
    out
}

const TO_UNICODE_START: &str = "/CIDInit /ProcSet findresource begin\n\
12 dict begin\n\
begincmap\n\
/CIDSystemInfo\n\
<</Registry (Adobe)\n\
/Ordering (Identity)\n\
/Supplement 0\n\
>> def\n\
/CMapName /Adobe-Identity-H def\n\
/CMapType 2 def\n\
1 begincodespacerange\n\
<0000> <FFFF>\n\
endcodespacerange\n";

const TO_UNICODE_END: &str = "endcmap\n\
CMapName currentdict /CMap defineresource pop\n\
end\n\
end\n";

const MAX_BF_ENTRIES: usize = 100;

/// The generated `/ToUnicode` CMap: `bfchar` for isolated codes, `bfrange`
/// for consecutive runs — with the destination as a list when the Unicode
/// values are not themselves consecutive, and as a single start value when
/// they are.
///
/// Every range is confined to one 256-code block: a `bfrange` may not span a
/// change of high byte, so a run crossing that boundary is cut at it.
fn load_unicode(to_unicode: &BTreeMap<u32, u32>) -> Vec<u8> {
    // A faithful port of `LoadUnicode`
    // (`core/fpdfapi/edit/cpdf_font_util.cpp:120-273`), including the
    // `max_extra = 255 - (code % 256)` cap and the `code % 256 == 0` case
    // that falls back to two singles rather than opening a range.
    let entries: Vec<(u32, u32)> = to_unicode.iter().map(|(&c, &u)| (c, u)).collect();
    let mut singles: BTreeMap<u32, u32> = BTreeMap::new();
    let mut range_list: BTreeMap<(u32, u32), Vec<u32>> = BTreeMap::new();
    let mut range_consec: BTreeMap<(u32, u32), u32> = BTreeMap::new();

    let mut i = 0;
    while i < entries.len() {
        let Some(&(first_code, first_uni)) = entries.get(i) else {
            break;
        };
        let next = entries.get(i + 1).copied();
        if next.is_none_or(|(c, _)| c != first_code.saturating_add(1)) {
            singles.insert(first_code, first_uni);
            i += 1;
            continue;
        }
        let Some((current_code, current_uni)) = next else {
            break;
        };
        i += 1;
        if current_code % 256 == 0 {
            singles.insert(first_code, first_uni);
            singles.insert(current_code, current_uni);
            i += 1;
            continue;
        }
        let max_extra = 255 - (current_code % 256);
        if first_uni.saturating_add(1) != current_uni {
            let mut unicodes = vec![first_uni, current_uni];
            let mut last_code = current_code;
            let mut extra = 0;
            while extra < max_extra {
                let Some(&(ncode, nuni)) = entries.get(i + 1) else {
                    break;
                };
                if ncode != last_code.saturating_add(1) {
                    break;
                }
                i += 1;
                last_code = ncode;
                unicodes.push(nuni);
                extra += 1;
            }
            range_list.insert((first_code, last_code), unicodes);
            i += 1;
            continue;
        }
        let mut last_code = current_code;
        let mut last_uni = current_uni;
        let mut extra = 0;
        while extra < max_extra {
            let Some(&(ncode, nuni)) = entries.get(i + 1) else {
                break;
            };
            if ncode != last_code.saturating_add(1) || nuni != last_uni.saturating_add(1) {
                break;
            }
            i += 1;
            last_code = ncode;
            last_uni = nuni;
            extra += 1;
        }
        range_consec.insert((first_code, last_code), first_uni);
        i += 1;
    }

    let mut buf = String::from(TO_UNICODE_START);
    write_bfchar(&mut buf, &singles);
    write_bfrange_list(&mut buf, &range_list);
    write_bfrange_consec(&mut buf, &range_consec);
    buf.push_str(TO_UNICODE_END);
    buf.into_bytes()
}

fn write_bfchar(buf: &mut String, map: &BTreeMap<u32, u32>) {
    let items: Vec<(u32, u32)> = map.iter().map(|(&c, &u)| (c, u)).collect();
    for chunk in items.chunks(MAX_BF_ENTRIES) {
        let _ = writeln!(buf, "{} beginbfchar", chunk.len());
        for &(code, uni) in chunk {
            add_charcode(buf, code);
            buf.push(' ');
            add_unicode(buf, uni);
            buf.push('\n');
        }
        buf.push_str("endbfchar\n");
    }
}

fn write_bfrange_list(buf: &mut String, map: &BTreeMap<(u32, u32), Vec<u32>>) {
    let items: Vec<(&(u32, u32), &Vec<u32>)> = map.iter().collect();
    for chunk in items.chunks(MAX_BF_ENTRIES) {
        let _ = writeln!(buf, "{} beginbfrange", chunk.len());
        for ((start, end), unicodes) in chunk {
            add_charcode(buf, *start);
            buf.push(' ');
            add_charcode(buf, *end);
            buf.push_str(" [");
            for (i, u) in unicodes.iter().enumerate() {
                if i > 0 {
                    buf.push(' ');
                }
                add_unicode(buf, *u);
            }
            buf.push_str("]\n");
        }
        buf.push_str("endbfrange\n");
    }
}

fn write_bfrange_consec(buf: &mut String, map: &BTreeMap<(u32, u32), u32>) {
    let items: Vec<((u32, u32), u32)> = map.iter().map(|(&k, &v)| (k, v)).collect();
    for chunk in items.chunks(MAX_BF_ENTRIES) {
        let _ = writeln!(buf, "{} beginbfrange", chunk.len());
        for &((start, end), uni) in chunk {
            add_charcode(buf, start);
            buf.push(' ');
            add_charcode(buf, end);
            buf.push(' ');
            add_unicode(buf, uni);
            buf.push('\n');
        }
        buf.push_str("endbfrange\n");
    }
}

fn add_charcode(buf: &mut String, number: u32) {
    let _ = std::fmt::Write::write_fmt(buf, format_args!("<{number:04X}>"));
}

fn add_unicode(buf: &mut String, unicode: u32) {
    let u = if (0xD800..=0xDFFF).contains(&unicode) {
        0
    } else {
        unicode
    };
    if let Some(ch) = char::from_u32(u) {
        let mut utf16 = [0u16; 2];
        let enc = ch.encode_utf16(&mut utf16);
        buf.push('<');
        for unit in enc.iter() {
            let _ = std::fmt::Write::write_fmt(buf, format_args!("{unit:04X}"));
        }
        buf.push('>');
    } else {
        buf.push_str("<0000>");
    }
}

#[cfg(test)]
mod tests {
    use super::{FontEncoding, ProgramKind, sniff_kind};
    use crate::doc::EditDoc;
    use crate::names;
    use pdfrum_object::{Name, ObjRef, Resolve};
    use pdfrum_parser::{Document, LoadOptions, load};
    use std::sync::Arc;

    const TINY: &[u8] = include_bytes!("../../tests/files/tiny.ttf");
    const HELLO: &[u8] = include_bytes!("../../tests/files/hello.pdf");

    fn loaded() -> Document {
        load(Arc::from(HELLO), &LoadOptions::default()).expect("opens")
    }

    fn fetch_dict(edit: &EditDoc<'_>, r: ObjRef) -> pdfrum_object::Dict {
        edit.fetch(r)
            .expect("fetches")
            .as_dict()
            .expect("dict")
            .clone()
    }

    #[test]
    fn sniff_recognises_truetype_and_otto() {
        assert_eq!(sniff_kind(TINY), Some(ProgramKind::TrueType));
        assert_eq!(sniff_kind(b"OTTO\x00\x01"), Some(ProgramKind::OpenTypeCff));
        assert_eq!(sniff_kind(b"%!PS-AdobeFont-1.0"), Some(ProgramKind::Type1));
        assert_eq!(sniff_kind(b"\x80\x01"), Some(ProgramKind::Type1));
        assert_eq!(sniff_kind(b"not a font"), None);
    }

    #[test]
    fn simple_writes_firstchar_lastchar_widths_and_length1() {
        let doc = loaded();
        let mut edit = EditDoc::new(&doc);
        let font = edit.embed_font(TINY, FontEncoding::Simple).expect("embeds");
        let dict = fetch_dict(&edit, font.object());
        assert_eq!(
            dict.name(names::SUBTYPE).map(Name::as_bytes),
            Some(&b"TrueType"[..])
        );
        let first = dict.direct_int(names::FIRST_CHAR).expect("FirstChar");
        let last = dict.direct_int(names::LAST_CHAR).expect("LastChar");
        assert!(last >= first);
        let widths_ref = dict.reference(names::WIDTHS).expect("Widths");
        let widths = edit
            .fetch(widths_ref)
            .expect("widths")
            .as_array()
            .expect("array")
            .clone();
        assert_eq!(
            widths.len(),
            usize::try_from(last - first + 1).expect("fits")
        );

        let desc_ref = dict.reference(names::FONT_DESCRIPTOR).expect("desc");
        let desc = fetch_dict(&edit, desc_ref);
        let flags = desc.direct_int(names::FLAGS).expect("Flags");
        assert_eq!(flags & (1 << 5), 1 << 5, "NonSymbolic");
        let file = desc.reference(names::FONT_FILE2).expect("FontFile2");
        let stream = edit
            .fetch(file)
            .expect("file")
            .as_stream()
            .expect("stream")
            .clone();
        let length1 = stream.dict.direct_int(names::LENGTH1).expect("Length1");
        assert_eq!(length1, i64::try_from(TINY.len()).expect("fits"));
        assert!(desc.reference(names::FONT_FILE3).is_none());
    }

    #[test]
    fn composite_writes_w_and_tounicode() {
        let doc = loaded();
        let mut edit = EditDoc::new(&doc);
        let font = edit
            .embed_font(TINY, FontEncoding::Composite)
            .expect("embeds");
        let dict = fetch_dict(&edit, font.object());
        assert_eq!(
            dict.name(names::SUBTYPE).map(Name::as_bytes),
            Some(&b"Type0"[..])
        );
        assert_eq!(
            dict.name(names::ENCODING).map(Name::as_bytes),
            Some(&b"Identity-H"[..])
        );
        assert!(dict.reference(names::TO_UNICODE).is_some());
        let descendants = dict
            .array(names::DESCENDANT_FONTS, &edit)
            .expect("DescendantFonts");
        let cid_ref = descendants.reference_at(0).expect("cid");
        let cid = fetch_dict(&edit, cid_ref);
        assert_eq!(
            cid.name(names::SUBTYPE).map(Name::as_bytes),
            Some(&b"CIDFontType2"[..])
        );
        let w_ref = cid.reference(names::W).expect("W");
        let w = edit
            .fetch(w_ref)
            .expect("W")
            .as_array()
            .expect("array")
            .clone();
        assert!(!w.is_empty());
        let tu = dict.reference(names::TO_UNICODE).expect("ToUnicode");
        let stream = edit
            .fetch(tu)
            .expect("tu")
            .as_stream()
            .expect("stream")
            .clone();
        let bytes: &[u8] = &stream.data;
        assert!(bytes.windows(9).any(|w| w == b"begincmap"));
        assert!(bytes.windows(7).any(|w| w == b"endcmap"));
    }

    #[test]
    fn otto_tag_selects_fontfile3() {
        let mut otto = TINY.to_vec();
        if let Some(head) = otto.get_mut(..4) {
            head.copy_from_slice(b"OTTO");
        }
        let doc = loaded();
        let mut edit = EditDoc::new(&doc);
        match edit.embed_font(&otto, FontEncoding::Composite) {
            Ok(font) => {
                let dict = fetch_dict(&edit, font.object());
                let descendants = dict
                    .array(names::DESCENDANT_FONTS, &edit)
                    .expect("DescendantFonts");
                let cid = fetch_dict(&edit, descendants.reference_at(0).expect("cid"));
                assert_eq!(
                    cid.name(names::SUBTYPE).map(Name::as_bytes),
                    Some(&b"CIDFontType0"[..])
                );
                let desc = fetch_dict(&edit, cid.reference(names::FONT_DESCRIPTOR).expect("desc"));
                let file = desc.reference(names::FONT_FILE3).expect("FontFile3");
                assert!(desc.reference(names::FONT_FILE2).is_none());
                let stream = edit
                    .fetch(file)
                    .expect("file")
                    .as_stream()
                    .expect("stream")
                    .clone();
                assert_eq!(
                    stream.dict.name(names::SUBTYPE).map(Name::as_bytes),
                    Some(&b"OpenType"[..])
                );
            }
            Err(_) => {
                // A glyf font with an OTTO tag may fail to parse; the sniff
                // still has to call it OpenType/CFF.
                assert_eq!(sniff_kind(&otto), Some(ProgramKind::OpenTypeCff));
            }
        }
    }

    #[test]
    fn encode_unmapped_is_notdef() {
        let doc = loaded();
        let mut edit = EditDoc::new(&doc);
        let font = edit
            .embed_font(TINY, FontEncoding::Composite)
            .expect("embeds");
        let codes = font.encode("Hello");
        assert_eq!(codes.len(), 10, "two bytes per character");
        let standard = edit
            .standard_font(pdfrum_font::StandardFont::Helvetica)
            .expect("standard");
        assert_eq!(standard.encode("Hi"), b"Hi");
        assert_eq!(standard.encode("\u{4e00}"), vec![0]);
    }

    // A composite font's `.notdef` is **two** bytes, not one. A one-byte
    // fallback would shift every code after it by a byte and turn the rest of
    // the string into noise, which is exactly what a shared `.notdef`
    // constant did when `encode_checked` was factored out of `encode`.
    #[test]
    fn a_composite_notdef_keeps_the_two_byte_width() {
        let doc = loaded();
        let mut edit = EditDoc::new(&doc);
        let font = edit
            .embed_font(TINY, FontEncoding::Composite)
            .expect("embeds");
        // One unmappable character between two mappable ones: still three
        // two-byte codes, with the middle one zero.
        let codes = font.encode("H\u{4e00}H");
        assert_eq!(codes.len(), 6, "two bytes per character");
        assert_eq!(codes.get(2..4), Some(&[0, 0][..]), "a two-byte .notdef");
        // And the checked encoder refuses rather than writing a `.notdef` at
        // all. `tiny.ttf` has no glyph for either character, so the refusal
        // names the first one — which is the rule: the first miss stops the
        // whole string.
        let missing = font.encode_checked("H\u{4e00}H").expect_err("refused");
        assert_eq!((missing.character, missing.offset), ('H', 0));
    }

    #[test]
    fn standard_font_has_no_widths_and_no_program() {
        let doc = loaded();
        let mut edit = EditDoc::new(&doc);
        let font = edit
            .standard_font(pdfrum_font::StandardFont::Helvetica)
            .expect("standard");
        let dict = fetch_dict(&edit, font.object());
        assert_eq!(
            dict.name(names::SUBTYPE).map(Name::as_bytes),
            Some(&b"Type1"[..])
        );
        assert_eq!(
            dict.name(names::BASE_FONT).map(Name::as_bytes),
            Some(&b"Helvetica"[..])
        );
        assert_eq!(
            dict.name(names::ENCODING).map(Name::as_bytes),
            Some(&b"WinAnsiEncoding"[..])
        );
        assert!(dict.raw(names::WIDTHS).is_none());
        assert!(dict.raw(names::FONT_DESCRIPTOR).is_none());
    }

    #[test]
    fn junk_is_refused() {
        let doc = loaded();
        let mut edit = EditDoc::new(&doc);
        assert!(
            edit.embed_font(b"not a font", FontEncoding::Simple)
                .is_err()
        );
        assert!(edit.embed_font(&[], FontEncoding::Composite).is_err());
    }
}
