//! Loading a `/Font` resource into a [`Font`].

use pdfrum_common::kurbo::{BezPath, Rect};
use pdfrum_common::{Diagnostics, Limits};
use pdfrum_object::{Dict, ObjRef, Resolve};
use smallvec::SmallVec;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use crate::cid::{self, CidTransform, Type0Font};
use crate::encoding;
use crate::glyphs::{self, GlyphSource, SynthGlyph};
use crate::ids::{CharCode, Cid, FontId, Gid};
use crate::names;
use crate::simple::{self, SimpleFont};
use crate::subst::{self, StandardFont, SubstFont, SubstitutionOptions};
use crate::type3::{self, Type3Font};

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
    /// The glyph to draw, or `None` when the ladder found no glyph at all.
    ///
    /// `None` is PDFium's `-1`, which is distinct from glyph 0 (`.notdef`):
    /// `.notdef` draws a box, `None` draws nothing. The two were a `Gid` and
    /// a `bool` beside it until the pair could express a state that has no
    /// meaning -- "no glyph" carrying an index.
    pub gid: Option<Gid>,
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
    /// through [`crate::GlyphCache`] instead, which keys on the substitution
    /// parameters that change the outline for a Multiple-Master face.
    #[must_use]
    pub fn glyph_path(&self, gid: Gid) -> Option<BezPath> {
        self.glyphs().outline(gid, glyphs::GlyphParams::default())
    }

    /// A glyph's outline in 1000/em text space, **grid-fitted at 64 ppem**.
    ///
    /// The same space [`Self::glyph_path`] returns, so a caller can substitute
    /// one for the other without touching its matrices — which is what a
    /// renderer rasterizing a glyph *bitmap* does, since hinting applies to
    /// that path and not to the outline one.
    ///
    /// `None` for every font that is not hinted: a face with no table
    /// directory (every bare CFF and every Type 1 program, so every base-14
    /// substitution), a Type 3 font, and a face whose own programs the
    /// interpreter refuses. In all of them the caller falls back to
    /// [`Self::glyph_path`] rather than drawing nothing.
    ///
    /// Uncached, and *expensive*: it builds a hinting instance and runs the
    /// face's bytecode. A renderer should call it only on a bitmap-cache miss.
    // The refused-programs arm is `cfx_face.cpp:849-857`, which reloads the
    // glyph unhinted rather than failing; falling back to `glyph_path` lands
    // in the same place.
    #[must_use]
    pub fn hinted_glyph_path(&self, gid: Gid) -> Option<BezPath> {
        self.glyphs().hinted_outline(gid)
    }

    /// The synthetic italic and embolden the glyph-*bitmap* side applies,
    /// resolved against the device matrix's two horizontal components.
    ///
    /// The bitmap side is a second call site with its *own* two levels
    /// (`CFX_Face::RenderGlyph`, `cfx_face.cpp:769-778` and `:806-816`), not
    /// the path side's, which is why it cannot simply reuse the outline the
    /// glyph cache already adjusted: the render-path embolden level depends on
    /// the *device* matrix, which only the renderer knows, and the render-path
    /// skew is the effective one rather than the plain one.
    ///
    /// `xx` and `xy` are the oracle's own 16.16 quantities —
    /// `matrix.a / 64 * 65536` and `matrix.c / 64 * 65536`
    /// (`cfx_face.cpp:766-767`) — because the embolden table's `/ 36655` is
    /// calibrated to that scale and nothing else. The [`SynthGlyph`] that
    /// comes back is therefore in **device pixels**, and belongs on an outline
    /// already mapped into that space.
    ///
    /// `None` where the C++ returns a negative level and `RenderGlyph` bails
    /// out with a null bitmap (`cfx_face.cpp:809-811`): a substitution weight
    /// of 1400 or more, which is past the table. A caller draws nothing.
    #[must_use]
    pub fn render_synth(&self, xx: i32, xy: i32) -> Option<SynthGlyph> {
        let Some(subst) = self.subst() else {
            return Some(SynthGlyph::NONE);
        };
        let is_cid = matches!(self, Self::Type0(_));
        let level = subst.embolden_level_for_render(is_cid, xx, xy)?;
        Some(SynthGlyph {
            skew: subst.effective_skew(is_cid),
            vertical: self.is_vertical(),
            // The level is a strength in the 26.6 units the transformed
            // outline is loaded in, so it becomes device pixels by /64 —
            // which is the space the caller has already mapped the outline
            // into by the time it asks.
            embolden: f64::from(level) / 64.0,
        })
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
    /// the former working note has filled in whatever the PDF failed to declare.
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
    /// Read by the glyph-spacing correction (`applies_glyph_spacing`), which
    /// only means anything when the document's widths and the face's disagree.
    #[must_use]
    pub(crate) fn has_declared_widths(&self) -> bool {
        match self {
            Self::Simple(f) => f.has_font_widths(),
            Self::Type0(_) | Self::Type3(_) => true,
        }
    }

    /// Whether this font's glyphs take the glyph-spacing correction.
    ///
    /// Reads the font's five relevant facts and asks the glyph-spacing
    /// rule, where the reasoning lives.
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
        self.glyphs().advance(gid, glyphs::GlyphParams::default())
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
    /// Non-Japan1, embedded and non-CID fonts all answer `None` — those three
    /// tests are the whole gate, and there is no fourth.
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

    pub(crate) fn glyphs(&self) -> &GlyphSource {
        match self {
            Self::Simple(f) => &f.glyphs,
            Self::Type0(f) => &f.glyphs,
            Self::Type3(_) => &GlyphSource::None,
        }
    }

    /// Whether this character's glyph is drawn from this font (`ShouldUseFont`).
    ///
    /// A Type 3 font has no glyph indices and never takes `GetCharPosList`, so
    /// it always answers true: there is no Arial stand-in for a content stream.
    #[must_use]
    pub fn should_use_own_glyph(&self, gid: Option<Gid>) -> bool {
        match self {
            Self::Type3(_) => true,
            Self::Simple(f) => crate::fallback::should_use_own_glyph(
                f.embedded,
                f.is_truetype,
                f.to_unicode.is_some(),
                gid,
            ),
            Self::Type0(f) => crate::fallback::should_use_own_glyph(
                f.embedded,
                false,
                f.to_unicode.is_some(),
                gid,
            ),
        }
    }

    /// The Arial stand-in `GetCharPosList` draws when [`should_use_own_glyph`]
    /// fails. Created on first miss; `None` if even Arial failed to load.
    #[must_use]
    pub fn glyph_fallback(&self) -> Option<&crate::GlyphFallback> {
        match self {
            Self::Type3(_) => None,
            Self::Simple(f) => crate::fallback::ensure(
                &f.fallback,
                f.id,
                f.is_truetype,
                f.descriptor.flags,
                f.descriptor.stem_v,
                f.descriptor.italic_angle,
                false,
            ),
            Self::Type0(f) => crate::fallback::ensure(
                &f.fallback,
                f.id,
                false,
                f.descriptor.flags,
                f.descriptor.stem_v,
                f.descriptor.italic_angle,
                f.cmap.is_vertical(),
            ),
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

/// Per-document caches: loaded fonts, and the font-identity counter.
///
/// A value the document owns rather than process-wide state, so two documents
/// loaded on two threads never share a face or a font identity. `Send + Sync`
/// and shared by `Arc`, so every session over one document — every worker of
/// a parallel render, and a text run and a render run alike — loads each font
/// once between them rather than once each.
///
/// # What is cached, and what is not
///
/// The key is the [`ObjRef`] that named the `/Font` resource. A font
/// dictionary written **inline**, with no reference of its own, is not cached
/// and is loaded afresh at every use: two inline copies genuinely are two
/// fonts, and there is no document-scoped identity to key them on.
///
/// The value is an `Arc<Font>`, so a hit shares the whole loaded font — its
/// parsed `/ToUnicode`, its CID tables and its glyph cache — rather than
/// rebuilding them. Text extraction's duplicate suppression compares fonts by
/// that pointer, so sharing is load-bearing for correctness as well as speed.
///
/// A dictionary that would not load caches its `None` too: that is as stable
/// an answer as a font, and re-deriving it per page is the same wasted work.
///
/// # Why the substitution options are not part of the key
///
/// Every load under one document must make the same substitution choice — a
/// substitution that varied between two `Tf` operators naming the same
/// resource would give one line of text different metrics from the next — so
/// a cache is created for one set of options and used with those. The caller
/// that owns the options owns the cache: `pdfrum_page::BuildContext` carries
/// both, in one value, and hands this out by `Arc`.
#[derive(Debug, Default)]
pub struct FontCache {
    next_id: AtomicU64,
    /// Loaded fonts, keyed on the reference that named them.
    loaded: RwLock<HashMap<ObjRef, Option<Arc<Font>>>>,
}

impl FontCache {
    /// A fresh cache.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The font `reference` names, loading it on the first ask and sharing it
    /// on every later one.
    ///
    /// `load` runs at most once per reference per cache in the uncontended
    /// case, and never under the lock — two threads asking for two different
    /// fonts do not serialize on each other. Two threads racing on the *same*
    /// reference may both load; whichever inserts first is the shared
    /// instance and both callers get that one `Arc`, so the loser's copy is
    /// dropped rather than replacing an instance another page already holds.
    /// That costs one duplicate parse and keeps the loader off the lock.
    pub fn get_or_load<F>(&self, reference: ObjRef, load: F) -> Option<Arc<Font>>
    where
        F: FnOnce() -> Option<Font>,
    {
        if let Ok(map) = self.loaded.read()
            && let Some(hit) = map.get(&reference)
        {
            return hit.clone();
        }
        let font = load().map(Arc::new);
        match self.loaded.write() {
            Ok(mut map) => map.entry(reference).or_insert(font).clone(),
            // A poisoned lock means another thread panicked mid-load. The
            // font itself is fine; hand it back uncached rather than panic.
            Err(_) => font,
        }
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

pub(crate) fn wants_chinese_cid_rescue(dict: &Dict, r: &impl Resolve) -> bool {
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
