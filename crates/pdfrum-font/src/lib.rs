//! Font handling (ISO 32000-1 §9): font dictionaries (Type1/TrueType/Type0/
//! Type3/CID), encodings and `/Differences`, `/ToUnicode`, code→CID→GID
//! mapping, glyph outlines and metrics via `skrifa`, substitution and fallback
//! selection, and the per-session glyph cache (SPEC.md §6).
//!
//! # The shape of the problem
//!
//! A PDF font dictionary says almost nothing directly. It names a base font,
//! points at an encoding, and — usually — carries a program. Everything that
//! matters is derived: which glyph a byte selects, what character it stands
//! for, and how wide it is. Those three questions have *different* answers
//! arrived at by *different* ladders, and reproducing the ladders is what this
//! crate is for.
//!
//! [`Font::decode`] is the single entry point that answers all three at once,
//! yielding one [`CharItem`] per character code in a string. Everything above
//! this crate — rendering and text extraction alike — reads that stream and
//! nothing else.
//!
//! ```no_run
//! use pdfrum_common::{Diagnostics, Limits};
//! use pdfrum_font::{FontCache, load};
//! # fn demo(dict: &pdfrum_object::Dict, doc: &impl pdfrum_object::Resolve) -> Option<()> {
//! let cache = FontCache::default();
//! let mut diags = Diagnostics::default();
//! let font = load(dict, doc, &cache, &Limits::default(), &mut diags)?;
//!
//! for item in font.decode(b"Hello") {
//!     let text: String = item.unicode.iter().collect();
//!     println!("code {:#x} -> glyph {} -> {text:?} ({}/1000 em)",
//!              item.code.0, item.gid.0, item.width);
//! }
//! # Some(())
//! # }
//! ```
//!
//! # Damage tolerance
//!
//! A simple font *always* constructs, even with no program, no encoding and no
//! glyphs — PDFium's `LoadCommon` has no failing path, and neither does ours.
//! Only a Type0 font can fail to load, in the four ways [`Error`] names, and
//! only because the oracle treats those as "the resource is not there".
//! Everything else is a [`pdfrum_common::Diagnostics`] entry and a
//! best-effort result.

#![forbid(unsafe_code)]
// Every byte reaching this crate came from an untrusted font program or font
// dictionary: index with `get()`, never with `[]`.
#![warn(clippy::indexing_slicing)]
// Font units are integers carried as floats, character codes are `u8`s cut out
// of wider values, and the metric normalizers are the C++'s own integer
// arithmetic. Each conversion below is pinned by a test.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap
)]

mod cid;
mod descriptor;
pub mod encoding;
mod error;
mod glyphs;
mod ids;
mod simple;
pub mod subst;
#[cfg(test)]
mod test_resolve;
#[cfg(test)]
mod testfonts;
pub mod tounicode;
mod type3;
mod widths;

pub use cid::{CidToGid, CidTransform, Type0Font, VerticalMetrics, cid_transform_to_float};
pub use descriptor::FontDescriptor;
pub use error::Error;
pub use glyphs::{GlyphCache, GlyphKey, GlyphSource, em_adjust, normalize_font_metric};
pub use ids::{CharCode, Cid, FontFlags, FontId, Gid, GlyphName};
pub use simple::{SimpleFont, SimpleKind};
pub use subst::{StandardFont, SubstFont, SubstitutionOptions};
pub use tounicode::ToUnicode;
pub use type3::{MAX_TYPE3_DEPTH, Type3Font};
pub use widths::CidWidths;

use pdfrum_common::kurbo::{BezPath, Rect};
use pdfrum_common::{Diagnostics, Limits};
use pdfrum_object::{Dict, Resolve};
use smallvec::SmallVec;
use std::sync::atomic::{AtomicU64, Ordering};

/// A loaded PDF font, ready to decode strings and produce glyphs.
///
/// Three variants, because PDF has three genuinely different kinds of font and
/// they share almost nothing below the surface: a simple font maps one byte to
/// one glyph through a name, a Type0 font maps a multi-byte code through a
/// CMap to a CID and then to a glyph, and a Type3 font has no glyphs at all —
/// its "glyphs" are content streams the page layer executes.
#[derive(Debug)]
pub enum Font {
    /// `Type1`, MMType1, TrueType, or a font whose `/Subtype` was missing or
    /// unrecognised — PDFium's dispatch sends all of those here.
    Simple(Box<SimpleFont>),
    /// A composite font: `/Type0` with a CID-keyed descendant.
    Type0(Box<Type0Font>),
    /// A font whose glyph procedures are content streams.
    Type3(Box<Type3Font>),
}

/// One decoded character: everything the layers above need about one character
/// code, computed once.
///
/// The fields answer the three separate questions a font is asked. `gid` is
/// glyph selection, `unicode` is what the character *means*, and `width` is
/// how far the pen moves — and none of the three is derivable from the others.
#[derive(Debug, Clone, PartialEq)]
pub struct CharItem {
    /// The character code as the font's encoding delimited it: one byte for a
    /// simple font, whatever the CMap's codespace says for a Type0 font.
    pub code: CharCode,
    /// The CID this code maps to, for a Type0 font only.
    pub cid: Option<Cid>,
    /// The glyph to draw. [`Gid`] cannot express "no glyph": that is
    /// [`CharItem::has_glyph`] being false.
    pub gid: Gid,
    /// The characters this code stands for, usually one and occasionally none
    /// — a ligature glyph maps to several, an unmapped code to zero.
    pub unicode: SmallVec<[char; 2]>,
    /// The advance width in 1000/em text space.
    pub width: f32,
    /// Set when the `GSUB` `vert`/`vrt2` feature substituted a vertical form.
    ///
    /// Not derivable downstream, and load-bearing: it suppresses the Japan1
    /// CID transform, which would otherwise rotate an already-rotated glyph.
    pub vertical_glyph: bool,
    /// False when the ladder found no glyph at all — PDFium's `-1`, which is
    /// distinct from glyph 0 (`.notdef`) and means "draw nothing".
    pub has_glyph: bool,
}

impl CharItem {
    /// The glyph to draw, or `None` when the ladder resolved nothing.
    #[must_use]
    pub fn glyph(&self) -> Option<Gid> {
        self.has_glyph.then_some(self.gid)
    }
}

impl Font {
    /// Decode a string into one [`CharItem`] per character code.
    ///
    /// The one text-decoding entry point rendering and extraction share, so
    /// they cannot disagree about where one character ends and the next
    /// begins — which for a Type0 font is a question only the CMap's codespace
    /// can answer.
    pub fn decode<'a>(&'a self, s: &'a [u8]) -> impl Iterator<Item = CharItem> + 'a {
        Decoder {
            font: self,
            bytes: s,
            offset: 0,
        }
    }

    /// A glyph's outline in 1000/em text space.
    ///
    /// `None` for a missing or degenerate outline, and always for a Type3
    /// font, whose glyphs are content streams rather than outlines.
    ///
    /// This is the uncached path. A renderer drawing many glyphs should go
    /// through [`GlyphCache`] instead, which keys on the substitution
    /// parameters that change the outline for a Multiple-Master face.
    #[must_use]
    pub fn glyph_path(&self, gid: Gid) -> Option<BezPath> {
        self.glyphs().outline(gid, &glyphs::GlyphParams::default())
    }

    /// A glyph's outline in 1000/em text space, **grid-fitted at 64 ppem**.
    ///
    /// The same space [`Self::glyph_path`] returns, so a caller can substitute
    /// one for the other without touching its matrices — which is what a
    /// renderer rasterizing a glyph *bitmap* does, since the oracle hints on
    /// that path and not on the outline one.
    ///
    /// `None` whenever the oracle would not hint either: a face with no table
    /// directory (`!IsTtOt()`, which covers every bare CFF and every Type 1
    /// program, so every base-14 substitution), a Type 3 font, and a face whose
    /// own programs the interpreter refuses — the case
    /// `cfx_face.cpp:849-857` handles by reloading the glyph unhinted. In all
    /// of them the caller falls back to [`Self::glyph_path`] rather than
    /// drawing nothing.
    ///
    /// Uncached, and *expensive*: it builds a hinting instance and runs the
    /// face's bytecode. A renderer should call it only on a bitmap-cache miss.
    #[must_use]
    pub fn hinted_glyph_path(&self, gid: Gid) -> Option<BezPath> {
        self.glyphs().hinted_outline(gid)
    }

    /// Is this a vertical-writing font? Only a Type0 font with a `-V` CMap is.
    #[must_use]
    pub fn is_vertical(&self) -> bool {
        match self {
            Self::Type0(f) => f.cmap.is_vertical(),
            Self::Simple(_) | Self::Type3(_) => false,
        }
    }

    /// Does the font carry its own program, rather than being substituted?
    ///
    /// Consulted far more widely than it looks: PDFium's per-glyph fallback,
    /// its glyph-spacing heuristic and its all-caps aliasing all branch on it,
    /// and a program that *failed to parse* counts as not embedded.
    #[must_use]
    pub fn is_embedded(&self) -> bool {
        match self {
            Self::Simple(f) => f.embedded,
            Self::Type0(f) => f.embedded,
            Self::Type3(_) => false,
        }
    }

    /// Whether character codes can be turned into Unicode at all, which text
    /// extraction uses to decide a font is worth reading.
    #[must_use]
    pub fn is_unicode_compatible(&self) -> bool {
        match self {
            Self::Simple(f) => {
                f.to_unicode.is_some() || f.encoding_kind != encoding::FontEncoding::Builtin
            }
            Self::Type0(f) => f.is_unicode_compatible(),
            Self::Type3(f) => f.to_unicode.is_some(),
        }
    }

    /// The font bounding box in 1000/em text space, after the derivation of
    /// §1.3 has filled in whatever the PDF failed to declare.
    #[must_use]
    pub fn font_bbox(&self) -> Rect {
        match self {
            Self::Simple(f) => f.descriptor.font_bbox,
            Self::Type0(f) => f.descriptor.font_bbox,
            Self::Type3(f) => f.font_bbox,
        }
    }

    /// The ascent in 1000/em text space.
    #[must_use]
    pub fn ascent(&self) -> f32 {
        match self {
            Self::Simple(f) => f.descriptor.ascent,
            Self::Type0(f) => f.descriptor.ascent,
            Self::Type3(_) => 0.0,
        }
    }

    /// The descent in 1000/em text space, normally negative.
    #[must_use]
    pub fn descent(&self) -> f32 {
        match self {
            Self::Simple(f) => f.descriptor.descent,
            Self::Type0(f) => f.descriptor.descent,
            Self::Type3(_) => 0.0,
        }
    }

    /// The Type3 font, when this is one. Its glyph procedures are raw
    /// `/CharProcs` streams that `pdfrum-page` executes.
    #[must_use]
    pub fn type3(&self) -> Option<&Type3Font> {
        match self {
            Self::Type3(f) => Some(f),
            Self::Simple(_) | Self::Type0(_) => None,
        }
    }

    /// The base font name, with any subset prefix already stripped.
    #[must_use]
    pub fn base_font_name(&self) -> &[u8] {
        match self {
            Self::Simple(f) => &f.base_font_name,
            Self::Type0(f) => &f.base_font_name,
            Self::Type3(_) => b"",
        }
    }

    /// This font's identity within a [`FontCache`], for glyph-cache keys.
    #[must_use]
    pub fn id(&self) -> FontId {
        match self {
            Self::Simple(f) => f.id,
            Self::Type0(f) => f.id,
            Self::Type3(f) => f.id,
        }
    }

    /// The substitution record, when the font was substituted rather than
    /// embedded. Carries the synthetic skew and embolden levels a renderer
    /// applies.
    #[must_use]
    pub fn subst(&self) -> Option<&SubstFont> {
        match self {
            Self::Simple(f) => f.subst.as_ref(),
            Self::Type0(f) => f.subst.as_ref(),
            Self::Type3(_) => None,
        }
    }

    /// The width of one character code in 1000/em text space.
    #[must_use]
    pub fn char_width(&self, code: CharCode) -> f32 {
        match self {
            Self::Simple(f) => f.char_width(code),
            Self::Type0(f) => f.char_width(code),
            Self::Type3(f) => f.char_width(code),
        }
    }

    /// Whether the PDF itself declared the advance widths this font reports.
    ///
    /// False only for a **simple** font with no `/Widths` array, where every
    /// width already comes from the face and comparing the two would be
    /// comparing a number against itself. A composite font always answers
    /// true: its `/W` array defaults to `/DW` rather than to the face.
    ///
    /// Read by the glyph-spacing correction of §1.15, which only means
    /// anything when the document's widths and the face's disagree.
    #[must_use]
    pub fn has_declared_widths(&self) -> bool {
        match self {
            Self::Simple(f) => f.has_font_widths(),
            Self::Type0(_) | Self::Type3(_) => true,
        }
    }

    /// Whether this font's glyphs take the glyph-spacing correction of §1.15.
    ///
    /// Reads the font's five relevant facts into a
    /// [`GlyphSpacingGate`](subst::GlyphSpacingGate) and
    /// asks [`applies_glyph_spacing`](subst::applies_glyph_spacing), where the
    /// rule and its reasoning live.
    #[must_use]
    pub fn applies_glyph_spacing(&self) -> bool {
        subst::applies_glyph_spacing(&subst::GlyphSpacingGate {
            vertical: self.is_vertical(),
            embedded: self.is_embedded(),
            declared_widths: self.has_declared_widths(),
            base_font_name: self.base_font_name(),
            subst: self.subst(),
        })
    }

    /// One glyph's own advance width, in 1000/em units, as the *face* declares
    /// it — not as the PDF does.
    ///
    /// Zero when there is no face or the glyph has no advance, which callers
    /// treat as "unknown" rather than as a genuine zero-width glyph.
    #[must_use]
    pub fn glyph_advance(&self, gid: Gid) -> i32 {
        self.glyphs().advance(gid, &glyphs::GlyphParams::default())
    }

    /// The bounding box of one character code's glyph, in 1000/em text space
    /// and **y-up**: `Rect::new(left, bottom, right, top)` with
    /// `bottom <= top`.
    ///
    /// `Rect::ZERO` when there is no glyph, and always for a Type 3 font,
    /// whose glyph boxes are a property of the content streams the page layer
    /// executes rather than of the font.
    ///
    /// Text extraction reads this per code — not per decoded string — when it
    /// builds a character's tight box and when its width ladder has run out of
    /// better answers.
    #[must_use]
    pub fn char_bbox(&self, code: CharCode) -> Rect {
        match self {
            Self::Simple(f) => f.char_bbox(code),
            Self::Type0(f) => f.char_bbox(code),
            Self::Type3(_) => Rect::ZERO,
        }
    }

    /// The Adobe-Japan1 per-CID transform a character takes, when one applies.
    ///
    /// Only a **non-embedded** Japan1 CID font has one, and only for the
    /// hundred and fifty-four CIDs the table lists. It moves the glyph within
    /// its em box without touching the advance, so a renderer applies it to
    /// the drawing origin alone — the pen walks on as if it were not there.
    ///
    /// Non-Japan1, embedded and non-CID fonts all answer `None`, which is the
    /// whole of the C++'s gate (`CPDF_CIDFont::GetCIDTransform`,
    /// `cpdf_cidfont.cpp:878-887`).
    #[must_use]
    pub fn japan1_transform(&self, code: CharCode) -> Option<CidTransform> {
        match self {
            Self::Type0(f) => f.japan1_transform(code),
            Self::Simple(_) | Self::Type3(_) => None,
        }
    }

    /// The width of a string of character codes, decoded through this font's
    /// own encoding and summed.
    ///
    /// Not the same as summing [`char_width`](Self::char_width) over the codes
    /// a caller already has: the string is re-decoded, so a code that does not
    /// round-trip through [`append_char`](Self::append_char) — a simple font's
    /// code above 255, say — comes back as a *different* code and contributes
    /// a different width. That difference is the whole point of the rung this
    /// serves in text extraction's width ladder.
    #[must_use]
    pub fn string_width(&self, bytes: &[u8]) -> f32 {
        self.decode(bytes)
            .map(|item| self.char_width(item.code))
            .sum()
    }

    /// The typographic ascent, truncated to an integer as the C++ stores it.
    #[must_use]
    pub fn type_ascent(&self) -> i32 {
        truncate(self.ascent())
    }

    /// The typographic descent, truncated to an integer, normally negative.
    #[must_use]
    pub fn type_descent(&self) -> i32 {
        truncate(self.descent())
    }

    /// The CID a character code maps to, for a composite font only.
    #[must_use]
    pub fn cid_from_charcode(&self, code: CharCode) -> Option<Cid> {
        match self {
            Self::Type0(f) => Some(f.cid_from_charcode(code)),
            Self::Simple(_) | Self::Type3(_) => None,
        }
    }

    /// The vertical origin of a character code, in 1000/em units, for a
    /// composite font only.
    #[must_use]
    pub fn vert_origin(&self, code: CharCode) -> Option<(f32, f32)> {
        match self {
            Self::Type0(f) => Some(f.vert_origin(code)),
            Self::Simple(_) | Self::Type3(_) => None,
        }
    }

    /// The vertical advance of a character code, in 1000/em units, for a
    /// composite font only. Normally negative.
    #[must_use]
    pub fn vert_width(&self, code: CharCode) -> Option<f32> {
        match self {
            Self::Type0(f) => Some(f.vert_width(code)),
            Self::Simple(_) | Self::Type3(_) => None,
        }
    }

    /// The Unicode a character code stands for, `/ToUnicode` first.
    #[must_use]
    pub fn unicode_from_charcode(&self, code: CharCode) -> SmallVec<[char; 2]> {
        match self {
            Self::Simple(f) => f.unicode_from_charcode(code),
            Self::Type0(f) => f.unicode_from_charcode(code),
            Self::Type3(f) => f.unicode_from_charcode(code),
        }
    }

    /// The character code that produces `unicode`, or `None`.
    ///
    /// The inverse of [`unicode_from_charcode`](Self::unicode_from_charcode),
    /// and the direction appearance generation needs: to *write* a string with
    /// a font the document already carries, a caller has to turn characters
    /// back into the codes that font understands.
    ///
    /// `None` means the font cannot express that character at all, which is
    /// the caller's signal to pick a different font rather than to emit a
    /// code that will draw the wrong glyph.
    ///
    /// ```
    /// use pdfrum_common::{Diagnostics, Limits};
    /// use pdfrum_font::{CharCode, Font, FontCache, StandardFont};
    ///
    /// let font = Font::load_standard(StandardFont::Helvetica, &FontCache::new());
    /// assert_eq!(font.char_code_from_unicode('A'), Some(CharCode(u32::from(b'A'))));
    /// // A character no Latin encoding carries.
    /// assert_eq!(font.char_code_from_unicode('\u{4e00}'), None);
    /// # let _ = (Diagnostics::default(), Limits::default());
    /// ```
    #[must_use]
    pub fn char_code_from_unicode(&self, unicode: char) -> Option<CharCode> {
        match self {
            Self::Simple(f) => f.char_code_from_unicode(unicode),
            Self::Type0(f) => {
                let code = f.charcode_from_unicode(unicode);
                (code.0 != 0).then_some(code)
            }
            Self::Type3(f) => f.char_code_from_unicode(unicode),
        }
    }

    /// Append one character code to a string being built, in the font's own
    /// byte encoding.
    ///
    /// A simple font writes one byte; a composite font writes as many as its
    /// CMap's codespace says, which is the whole reason this is a method
    /// rather than a cast at the call site. Pairs with
    /// [`char_code_from_unicode`](Self::char_code_from_unicode) to turn text
    /// into a string a content stream can show.
    ///
    /// ```
    /// use pdfrum_font::{CharCode, Font, FontCache, StandardFont};
    ///
    /// let font = Font::load_standard(StandardFont::Helvetica, &FontCache::new());
    /// let mut out = Vec::new();
    /// for ch in "Hi".chars() {
    ///     if let Some(code) = font.char_code_from_unicode(ch) {
    ///         font.append_char(&mut out, code);
    ///     }
    /// }
    /// assert_eq!(out, b"Hi");
    /// ```
    pub fn append_char(&self, out: &mut Vec<u8>, code: CharCode) {
        match self {
            // A composite font's codespace decides the width, so only its
            // CMap can encode a code correctly.
            Self::Type0(f) => f.cmap.append_char(out, code),
            Self::Simple(_) | Self::Type3(_) => out.push((code.0 & 0xff) as u8),
        }
    }

    /// Build one of the fourteen standard fonts, with no document behind it.
    ///
    /// Every reader must supply these faces, so a caller that needs to draw
    /// text of its own — an annotation's appearance stream, say — can have one
    /// without inventing a font dictionary. The result is exactly what
    /// synthesizing `/Type /Font /Subtype /Type1 /BaseFont <name> /Encoding
    /// /WinAnsiEncoding` would produce, which is how PDFium's own stock-font
    /// path builds them.
    ///
    /// ```
    /// use pdfrum_font::{CharCode, Font, FontCache, StandardFont};
    ///
    /// let cache = FontCache::new();
    /// let helvetica = Font::load_standard(StandardFont::Helvetica, &cache);
    /// assert_eq!(helvetica.base_font_name(), b"Helvetica");
    ///
    /// // The Couriers are fixed-pitch: every glyph is 600 units wide.
    /// let courier = Font::load_standard(StandardFont::Courier, &cache);
    /// assert_eq!(courier.char_width(CharCode(u32::from(b'i'))), 600.0);
    /// assert_eq!(courier.char_width(CharCode(u32::from(b'W'))), 600.0);
    /// ```
    #[must_use]
    pub fn load_standard(which: StandardFont, cache: &FontCache) -> Self {
        let dict = Dict::from_pairs([
            (
                names::TYPE.clone(),
                pdfrum_object::Object::Name(names::FONT.clone()),
            ),
            (
                names::SUBTYPE.clone(),
                pdfrum_object::Object::Name(pdfrum_object::Name::from("Type1")),
            ),
            (
                names::BASE_FONT.clone(),
                pdfrum_object::Object::Name(pdfrum_object::Name::from(subst::canonical_font_name(
                    which,
                ))),
            ),
            (
                names::ENCODING.clone(),
                pdfrum_object::Object::Name(names::WIN_ANSI_ENCODING.clone()),
            ),
        ]);
        Self::Simple(Box::new(simple::load(
            &dict,
            &pdfrum_object::NoResolve,
            cache,
            &SubstitutionOptions::default(),
            &Limits::default(),
            &mut Diagnostics::with_limit(0),
            false,
        )))
    }

    fn glyphs(&self) -> &GlyphSource {
        match self {
            Self::Simple(f) => &f.glyphs,
            Self::Type0(f) => &f.glyphs,
            Self::Type3(_) => &GlyphSource::None,
        }
    }
}

/// The [`Font::decode`] iterator.
struct Decoder<'a> {
    font: &'a Font,
    bytes: &'a [u8],
    offset: usize,
}

impl Iterator for Decoder<'_> {
    type Item = CharItem;

    fn next(&mut self) -> Option<CharItem> {
        if self.offset >= self.bytes.len() {
            return None;
        }
        Some(match self.font {
            Font::Simple(f) => {
                let byte = *self.bytes.get(self.offset)?;
                self.offset += 1;
                f.char_item(CharCode(u32::from(byte)))
            }
            Font::Type3(f) => {
                let byte = *self.bytes.get(self.offset)?;
                self.offset += 1;
                f.char_item(CharCode(u32::from(byte)))
            }
            Font::Type0(f) => {
                // Only the CMap knows how wide this code is, and a truncated
                // code yields code 0 with the offset left unmoved — which
                // would loop forever, so a stalled offset ends iteration.
                let before = self.offset;
                let code = f.cmap.next_char(self.bytes, &mut self.offset);
                if self.offset <= before {
                    return None;
                }
                f.char_item(code)
            }
        })
    }
}

/// A metric truncated toward zero, which is how the C++ stores ascent and
/// descent: it reads them into `int` fields at load time, so every consumer
/// sees the truncation rather than the declared float.
fn truncate(value: f32) -> i32 {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the saturating cast is the point: a metric outside i32 is nonsense"
    )]
    let truncated = value.trunc() as i32;
    truncated
}

/// Per-document caches: parsed faces, resolved substitutions, font identities.
///
/// Replaces the C++'s two process-wide singletons (`CPDF_FontGlobals` and
/// `CFX_FontMgr`) with a value the document owns, per STYLE.md §1. Cheap to
/// create and `Send + Sync`; the only mutable state is the identity counter.
#[derive(Debug, Default)]
pub struct FontCache {
    next_id: AtomicU64,
}

impl FontCache {
    /// A fresh cache.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Hand out the next font identity.
    pub(crate) fn next_id(&self) -> FontId {
        FontId(self.next_id.fetch_add(1, Ordering::Relaxed))
    }
}

/// Build a [`Font`] from a `/Font` resource dictionary.
///
/// Never panics; damage goes to `diags`. Returns `None` only for the four
/// unrecoverable Type0 cases — every other font kind always constructs, even
/// with no program and no glyphs at all.
///
/// The dispatch has one quirk worth knowing about: a `/TrueType` font whose
/// `/BaseFont` begins with one of five GBK-encoded Chinese family names, and
/// which carries no `/FontFile2`, is built as a **CID font** instead. Real
/// files depend on it.
#[must_use]
pub fn load(
    dict: &Dict,
    r: &impl Resolve,
    cache: &FontCache,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> Option<Font> {
    load_with_options(
        dict,
        r,
        cache,
        &SubstitutionOptions::default(),
        limits,
        diags,
    )
}

/// [`load`], with control over how substitution finds system faces.
#[must_use]
pub fn load_with_options(
    dict: &Dict,
    r: &impl Resolve,
    cache: &FontCache,
    opts: &SubstitutionOptions,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> Option<Font> {
    let subtype = dict.name(names::SUBTYPE).map(|n| n.as_bytes().to_vec());
    match subtype.as_deref() {
        Some(b"Type3") => Some(Font::Type3(Box::new(type3::load(
            dict, r, cache, limits, diags,
        )))),
        Some(b"Type0") => cid::load(dict, r, cache, opts, limits, diags)
            .ok()
            .map(|f| Font::Type0(Box::new(f))),
        Some(b"TrueType") if wants_chinese_cid_rescue(dict, r) => {
            // The GBK-name rescue: build it as a CID font, which then takes
            // its own `/Subtype == TrueType` path and loads GBK-EUC-H.
            match cid::load_gb2312(dict, r, cache, opts, limits, diags) {
                Ok(f) => Some(Font::Type0(Box::new(f))),
                // A matched-but-unusable font still falls through to the
                // ordinary TrueType path, as the C++'s `if (!font)` guard does.
                Err(_) => Some(Font::Simple(Box::new(simple::load(
                    dict, r, cache, opts, limits, diags, true,
                )))),
            }
        }
        Some(b"TrueType") => Some(Font::Simple(Box::new(simple::load(
            dict, r, cache, opts, limits, diags, true,
        )))),
        // Everything else — `/Type1`, `/MMType1`, a missing `/Subtype`, and
        // outright garbage — is a Type 1 font.
        _ => Some(Font::Simple(Box::new(simple::load(
            dict, r, cache, opts, limits, diags, false,
        )))),
    }
}

/// The five GBK-encoded family names that reroute a `/TrueType` font to the
/// CID loader, compared against `/BaseFont`'s **first four bytes**.
///
/// 宋体 (SimSun), 楷体 (KaiTi), 黑体 (HeiTi), 仿宋 (FangSong), 新宋 (XinSong).
const CHINESE_FONT_NAMES: [[u8; 4]; 5] = [
    [0xcb, 0xce, 0xcc, 0xe5],
    [0xbf, 0xac, 0xcc, 0xe5],
    [0xba, 0xda, 0xcc, 0xe5],
    [0xb7, 0xc2, 0xcb, 0xce],
    [0xd0, 0xc2, 0xcb, 0xce],
];

fn wants_chinese_cid_rescue(dict: &Dict, r: &impl Resolve) -> bool {
    let Some(base) = dict.name(names::BASE_FONT) else {
        return false;
    };
    let Some(prefix) = base.as_bytes().get(..4) else {
        return false;
    };
    if !CHINESE_FONT_NAMES.iter().any(|n| n == prefix) {
        return false;
    }
    // Only when there is nothing to draw with: a descriptor carrying a real
    // TrueType program keeps the ordinary path.
    match dict.dict(names::FONT_DESCRIPTOR, r) {
        None => true,
        Some(desc) => desc.raw(names::FONT_FILE2).is_none(),
    }
}

/// The `/Font` dictionary keys this crate reads.
///
/// Declared here rather than in `pdfrum-object`'s shared table because they
/// are font-specific and no other crate spells them.
pub(crate) mod names {
    pub(crate) use pdfrum_object::names::{RESOURCES, SUBTYPE, TYPE, W, WIN_ANSI_ENCODING};
    pdfrum_object::names! {
        /// The character encoding, a name or a dictionary (`/Encoding`).
        ENCODING = "Encoding";
        /// The `/Type` value of a font dictionary (`/Font`).
        FONT = "Font";
        /// The font's PostScript name (`/BaseFont`).
        BASE_FONT = "BaseFont";
        /// The font's metrics and program (`/FontDescriptor`).
        FONT_DESCRIPTOR = "FontDescriptor";
        /// A Type 1 font program (`/FontFile`).
        FONT_FILE = "FontFile";
        /// A TrueType font program (`/FontFile2`).
        FONT_FILE2 = "FontFile2";
        /// A font program in some other format (`/FontFile3`).
        FONT_FILE3 = "FontFile3";
        /// Per-code advance widths (`/Widths`).
        WIDTHS = "Widths";
        /// The first code `/Widths` covers (`/FirstChar`).
        FIRST_CHAR = "FirstChar";
        /// The last code `/Widths` covers (`/LastChar`).
        LAST_CHAR = "LastChar";
        /// The width of a code outside `/Widths` (`/MissingWidth`).
        MISSING_WIDTH = "MissingWidth";
        /// The character-code-to-Unicode CMap (`/ToUnicode`).
        TO_UNICODE = "ToUnicode";
        /// Overrides on the base encoding (`/Differences`).
        DIFFERENCES = "Differences";
        /// The encoding a `/Differences` array modifies (`/BaseEncoding`).
        BASE_ENCODING = "BaseEncoding";
        /// The descriptor flag word (`/Flags`).
        FLAGS = "Flags";
        /// The glyph extents (`/FontBBox`).
        FONT_BBOX = "FontBBox";
        /// Degrees clockwise from vertical (`/ItalicAngle`).
        ITALIC_ANGLE = "ItalicAngle";
        /// Vertical stem thickness (`/StemV`).
        STEM_V = "StemV";
        /// The weight, 100 to 900 (`/FontWeight`).
        FONT_WEIGHT = "FontWeight";
        /// Maximum height above the baseline (`/Ascent`).
        ASCENT = "Ascent";
        /// Maximum depth below the baseline (`/Descent`).
        DESCENT = "Descent";
        /// Height of a capital letter (`/CapHeight`).
        CAP_HEIGHT = "CapHeight";
        /// The one CID font a Type0 font wraps (`/DescendantFonts`).
        DESCENDANT_FONTS = "DescendantFonts";
        /// The CID collection this font is keyed to (`/CIDSystemInfo`).
        CID_SYSTEM_INFO = "CIDSystemInfo";
        /// The collection's character-set name (`/Ordering`).
        ORDERING = "Ordering";
        /// CID to glyph index, a name or a stream (`/CIDToGIDMap`).
        CID_TO_GID_MAP = "CIDToGIDMap";
        /// The default horizontal advance (`/DW`).
        DW = "DW";
        /// Per-CID vertical metrics (`/W2`).
        W2 = "W2";
        /// The default vertical metrics (`/DW2`).
        DW2 = "DW2";
        /// The glyph-space to text-space transform (`/FontMatrix`).
        FONT_MATRIX = "FontMatrix";
        /// A Type3 font's glyph procedures (`/CharProcs`).
        CHAR_PROCS = "CharProcs";
        /// The CMap an `/Encoding` stream inherits from (`/UseCMap`).
        USE_CMAP = "UseCMap";
    }
}

#[cfg(test)]
mod tests {
    // Test expectations are exact values by design.
    #![allow(clippy::float_cmp)]

    use super::*;
    use pdfrum_object::{Name, NoResolve, Object};

    fn simple_dict(subtype: &str, base: &str) -> Dict {
        Dict::from_pairs([
            (names::SUBTYPE.clone(), Object::Name(Name::from(subtype))),
            (names::BASE_FONT.clone(), Object::Name(Name::from(base))),
        ])
    }

    #[test]
    fn a_missing_subtype_loads_as_a_type1_font() {
        let dict = Dict::from_pairs([(
            names::BASE_FONT.clone(),
            Object::Name(Name::from("Helvetica")),
        )]);
        let font = load(
            &dict,
            &NoResolve,
            &FontCache::new(),
            &Limits::default(),
            &mut Diagnostics::default(),
        )
        .expect("a simple font always constructs");
        assert!(matches!(font, Font::Simple(_)));
    }

    #[test]
    fn garbage_subtypes_also_load_as_type1() {
        for subtype in ["Type1", "MMType1", "NotAFontType", ""] {
            let font = load(
                &simple_dict(subtype, "Helvetica"),
                &NoResolve,
                &FontCache::new(),
                &Limits::default(),
                &mut Diagnostics::default(),
            );
            assert!(matches!(font, Some(Font::Simple(_))), "{subtype}");
        }
    }

    #[test]
    fn a_type3_subtype_loads_as_type3() {
        let font = load(
            &simple_dict("Type3", ""),
            &NoResolve,
            &FontCache::new(),
            &Limits::default(),
            &mut Diagnostics::default(),
        )
        .expect("Type3 always constructs");
        assert!(font.type3().is_some());
        // A Type3 font has no outlines at all, by construction.
        assert!(font.glyph_path(Gid(0)).is_none());
    }

    #[test]
    fn a_type0_font_without_descendants_fails_to_load() {
        // The one font kind whose load can fail, and the reason the public
        // entry point returns `Option`.
        assert!(
            load(
                &simple_dict("Type0", "Foo"),
                &NoResolve,
                &FontCache::new(),
                &Limits::default(),
                &mut Diagnostics::default(),
            )
            .is_none()
        );
    }

    #[test]
    fn the_chinese_name_rescue_reroutes_a_truetype_font() {
        // 宋体 in GBK, with no descriptor at all.
        let mut name = vec![0xcb, 0xce, 0xcc, 0xe5];
        name.extend_from_slice(b"-Extra");
        let dict = Dict::from_pairs([
            (names::SUBTYPE.clone(), Object::Name(Name::from("TrueType"))),
            (names::BASE_FONT.clone(), Object::Name(Name::new(name))),
        ]);
        assert!(wants_chinese_cid_rescue(&dict, &NoResolve));
    }

    #[test]
    fn the_chinese_rescue_does_not_fire_for_an_embedded_font() {
        let desc = Dict::from_pairs([(
            names::FONT_FILE2.clone(),
            Object::Ref(pdfrum_object::ObjRef::new(7, 0)),
        )]);
        let dict = Dict::from_pairs([
            (names::SUBTYPE.clone(), Object::Name(Name::from("TrueType"))),
            (
                names::BASE_FONT.clone(),
                Object::Name(Name::new(vec![0xcb, 0xce, 0xcc, 0xe5])),
            ),
            (names::FONT_DESCRIPTOR.clone(), Object::Dict(desc)),
        ]);
        assert!(!wants_chinese_cid_rescue(&dict, &NoResolve));
    }

    #[test]
    fn a_name_shorter_than_four_bytes_never_matches() {
        let dict = Dict::from_pairs([
            (names::SUBTYPE.clone(), Object::Name(Name::from("TrueType"))),
            (names::BASE_FONT.clone(), Object::Name(Name::from("ab"))),
        ]);
        assert!(!wants_chinese_cid_rescue(&dict, &NoResolve));
    }

    #[test]
    fn the_standard_fourteen_all_load_and_name_themselves() {
        let cache = FontCache::new();
        for which in subst::ALL_STANDARD_FONTS {
            let font = Font::load_standard(which, &cache);
            assert_eq!(
                font.base_font_name(),
                subst::canonical_font_name(which).as_bytes(),
                "{which:?}"
            );
            assert!(font.glyph_path(Gid(1)).is_some() || font.glyph_path(Gid(2)).is_some());
        }
    }

    #[test]
    fn a_standard_font_round_trips_ascii_both_ways() {
        let font = Font::load_standard(StandardFont::Times, &FontCache::new());
        for ch in "The quick brown fox! 0123".chars() {
            let Some(code) = font.char_code_from_unicode(ch) else {
                panic!("{ch:?} should be encodable in a Latin font");
            };
            assert_eq!(
                font.unicode_from_charcode(code).as_slice(),
                [ch],
                "{ch:?} did not round-trip"
            );
        }
    }

    #[test]
    fn char_code_from_unicode_declines_what_the_font_cannot_express() {
        let font = Font::load_standard(StandardFont::Helvetica, &FontCache::new());
        for ch in ['\u{4e00}', '\u{3042}', '\u{10000}'] {
            assert_eq!(font.char_code_from_unicode(ch), None, "{ch:?}");
        }
    }

    #[test]
    fn append_char_writes_one_byte_for_a_simple_font() {
        let font = Font::load_standard(StandardFont::Helvetica, &FontCache::new());
        let mut out = Vec::new();
        for ch in "Hello, world!".chars() {
            let code = font
                .char_code_from_unicode(ch)
                .unwrap_or_else(|| panic!("{ch:?} is encodable"));
            font.append_char(&mut out, code);
        }
        assert_eq!(out, b"Hello, world!");
    }

    #[test]
    fn append_char_writes_two_bytes_for_an_identity_composite_font() {
        // A composite font's codespace decides the width, which is the reason
        // `append_char` is a method rather than a byte cast at the call site.
        let descendant = Dict::from_pairs([
            (
                names::SUBTYPE.clone(),
                Object::Name(Name::from("CIDFontType0")),
            ),
            (names::BASE_FONT.clone(), Object::Name(Name::from("Test"))),
        ]);
        let dict = Dict::from_pairs([
            (names::SUBTYPE.clone(), Object::Name(Name::from("Type0"))),
            (
                names::ENCODING.clone(),
                Object::Name(Name::from("Identity-H")),
            ),
            (
                names::DESCENDANT_FONTS.clone(),
                Object::Array(pdfrum_object::Array::of([Object::Dict(descendant)])),
            ),
        ]);
        let font = load(
            &dict,
            &NoResolve,
            &FontCache::new(),
            &Limits::default(),
            &mut Diagnostics::default(),
        )
        .expect("a Type0 font with one descendant loads");

        let mut out = Vec::new();
        font.append_char(&mut out, CharCode(0x0041));
        assert_eq!(out, vec![0x00, 0x41]);
    }

    #[test]
    fn the_courier_widths_are_the_fixed_six_hundred() {
        let font = Font::load_standard(StandardFont::CourierBold, &FontCache::new());
        for ch in "iWm ".chars() {
            let code = font.char_code_from_unicode(ch).expect("encodable");
            assert_eq!(font.char_width(code), 600.0, "{ch:?}");
        }
    }

    #[test]
    fn font_ids_are_distinct() {
        let cache = FontCache::new();
        let a = cache.next_id();
        let b = cache.next_id();
        assert_ne!(a, b);
    }
}

#[cfg(test)]
mod send_sync {
    //! STYLE.md §4: every public type is `Send + Sync`, so rendering pages in
    //! parallel with `rayon` needs no wrapper. A compile failure here is the
    //! whole test.

    use super::*;

    const fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn every_public_type_is_send_and_sync() {
        assert_send_sync::<Font>();
        assert_send_sync::<CharItem>();
        assert_send_sync::<SimpleFont>();
        assert_send_sync::<Type0Font>();
        assert_send_sync::<Type3Font>();
        assert_send_sync::<FontCache>();
        assert_send_sync::<GlyphCache>();
        assert_send_sync::<GlyphKey>();
        assert_send_sync::<GlyphSource>();
        assert_send_sync::<ToUnicode>();
        assert_send_sync::<FontDescriptor>();
        assert_send_sync::<SubstFont>();
        assert_send_sync::<SubstitutionOptions>();
        assert_send_sync::<CidWidths>();
        assert_send_sync::<VerticalMetrics>();
        assert_send_sync::<CidToGid>();
        assert_send_sync::<Error>();
        assert_send_sync::<GlyphName>();
        assert_send_sync::<subst::FontRequest>();
        assert_send_sync::<subst::Substitution>();
        assert_send_sync::<subst::TestFontDb>();
        assert_send_sync::<subst::SystemFontDb>();
    }
}
