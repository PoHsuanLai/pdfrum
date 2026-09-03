//! Type 1 font programs — PFA/PFB containers, `eexec`-encrypted private
//! dictionaries, charstrings, Multiple Master interpolation. The one outline
//! format Fontations does not read; CFF and TrueType are `skrifa`'s.
//! [`Type1Font::parse`] sniffs the container, decrypts and reads the program
//! into a plain value (nothing lazy, `Send + Sync`), and
//! [`Type1Font::instantiate`] blends its outlines at *design* coordinates — a
//! weight of 50–1450, a width of 100–900.
//!
//! ```
//! use pdfrum_common::Diagnostics;
//! use pdfrum_type1::Type1Font;
//!
//! # fn demo(pfb: &[u8]) -> Option<()> {
//! let mut diags = Diagnostics::default();
//! let font = Type1Font::parse(pfb, &Default::default(), &mut diags).ok()?;
//!
//! // Character code to glyph, through the font's built-in encoding.
//! let gid = font.code_to_gid(b'A')?;
//! let (outline, advance) = font.outline(gid)?;
//! assert!(!outline.is_empty());
//!
//! // A Multiple Master face draws at whatever weight is asked for.
//! if let Some(axes) = font.mm_axes() {
//!     let bold = font.instantiate(&[axes[0].max])?;
//!     assert!(bold.outline(gid).is_some());
//!     let _ = advance;
//! }
//! # Some(())
//! # }
//! ```
//!
//! A Type 1 font effectively never fails to construct: short of a missing
//! private section or `/CharStrings`, damage yields a best-effort font, a
//! [`pdfrum_common::Diagnostics`] entry, and partial outlines.

// Two things need this crate, and nothing else does:
//
// 1. Embedded Type 1 programs — a `/FontFile` stream, or a `/FontFile3` whose
//    payload turns out to be Type 1 rather than the usual bare CFF.
// 2. The two generic fallback faces, which are PFB Multiple-Master Type 1 and
//    are the terminal rung of the substitution ladder: when a document names a
//    font nothing on the system resembles, they are what draws it. A renderer
//    that cannot instantiate them at an arbitrary weight draws nothing at all
//    for such a font.
//
// The design-coordinate interface is what the renderer needs: PDFium picks a
// width axis coordinate by bisecting on the advance widths two probe instances
// report, and picks the weight axis straight from the substitution font's
// weight.
//
// Damage tolerated on the way in: a truncated PFB segment, hex that stops
// mid-byte, a charstring that runs off its end, a Multiple-Master declaration
// whose parts disagree.

#![forbid(unsafe_code)]
// Every byte here came from an untrusted `/FontFile` stream: index with
// `get()`, never with `[]`.
#![warn(clippy::indexing_slicing)]
// Font units are integers stored as floats and character codes are `u8`s cut
// out of wider values; the conversions below are the format's own arithmetic,
// each one pinned by a test.
#![allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]

mod blend;
mod charstring;
mod container;
mod eexec;
mod encoding;
mod error;
mod postscript;
mod program;

pub use blend::{AxisKind, MmAxis};
pub use charstring::Glyph;
pub use container::{Container, FontFile, font_file};
pub use eexec::{CHARSTRING_SEED, DEFAULT_LEN_IV, EEXEC_SEED, EEXEC_SKIP, decrypt, encrypt};
pub use encoding::{Encoding, standard_encoding_name, unicode_from_glyph_name};
pub use error::Error;

use blend::Blend;
use pdfrum_common::kurbo::{Affine, BezPath, Rect, Shape};
use pdfrum_common::{DiagKind, Diagnostics, Limits, Severity};
use std::collections::HashMap;

/// A glyph index into a [`Type1Font`]'s `/CharStrings`, in declaration order.
///
/// Type 1 has no glyph-index concept of its own — glyphs are named — so this
/// is our numbering, fixed by the order the dictionary declared them, which is
/// the same convention FreeType and `read-fonts` use.
///
/// Not interchangeable with `pdfrum_font::Gid`, which indexes whatever program
/// a face was loaded from. The two are separate index spaces that coincide
/// numerically only for a face that *is* a Type 1 program; `pdfrum-font` owns
/// the conversion between them.
// A shared identifier in `pdfrum-common` would be wrong: merging them would
// make an sfnt glyph index — `skrifa`'s `GlyphId`, a numbering this crate never
// produces and never sees — assignable to a `/CharStrings` slot with no
// conversion. The `From` impls live at the one boundary that owns both spaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Gid(pub u16);

/// A parsed Type 1 font program.
///
/// A record, not an engine: the fields below are everything the program said,
/// and every method is a pure function of them. Outlines are computed on
/// demand and not cached here — the caller's glyph cache owns that, keyed by
/// the instantiation as well as the glyph.
#[derive(Debug, Clone)]
pub struct Type1Font {
    container: Container,
    font_name: Option<Box<str>>,
    full_name: Option<Box<str>>,
    family_name: Option<Box<str>>,
    italic_angle: f32,
    is_fixed_pitch: bool,
    font_matrix: Affine,
    font_bbox: Rect,
    encoding: Encoding,
    subrs: Vec<Vec<u8>>,
    charstrings: Vec<Vec<u8>>,
    glyph_names: Vec<Box<str>>,
    by_name: HashMap<Box<str>, u16>,
    unicode_map: HashMap<char, u16>,
    blend: Option<Blend>,
}

/// The default `/FontMatrix` when a program does not declare one: 1000 units
/// per em, which is what every Type 1 font in practice uses.
const DEFAULT_MATRIX: Affine = Affine::new([0.001, 0.0, 0.0, 0.001, 0.0, 0.0]);

impl Type1Font {
    /// Parse a font program: sniff PFB/PFA/bare, decrypt `eexec`, read the
    /// cleartext and private dictionaries.
    ///
    /// `/Length1`, `/Length2` and `/Length3` from the font descriptor are
    /// deliberately *not* a parameter. PDFium ignores them, because they are
    /// wrong often enough that trusting them loses more fonts than it saves,
    /// and the container is self-describing anyway.
    ///
    /// # Errors
    ///
    /// [`Error::Empty`] for no bytes, [`Error::PfbSegment`] for a PFB whose
    /// first segment header is unusable, [`Error::NoEexec`] when no private
    /// section can be found, and [`Error::NoCharStrings`] when the program
    /// declares no glyphs.
    pub fn parse(bytes: &[u8], limits: &Limits, diags: &mut Diagnostics) -> Result<Self, Error> {
        let split = container::split(bytes, diags)?;
        let plain = eexec::decrypt(&split.cipher, eexec::EEXEC_SEED, eexec::EEXEC_SKIP);
        // A correctly-keyed decryption always yields printable PostScript in
        // its first line; random bytes almost never do. Checking is what turns
        // "fed the wrong offset" into a clear error instead of an empty font.
        if !looks_like_postscript(&plain) {
            return Err(Error::EexecGarbage);
        }

        let header = program::read_header(&split.clear);
        let private = program::read_private(&plain);
        if private.charstrings.is_empty() {
            return Err(Error::NoCharStrings);
        }

        let cap = limits.max_array_len.min(u16::MAX as usize);
        let (glyph_names, charstrings): (Vec<_>, Vec<_>) =
            private.charstrings.into_iter().take(cap).unzip();

        // Later declarations of a name win, matching PostScript's `put`.
        let by_name: HashMap<Box<str>, u16> = glyph_names
            .iter()
            .enumerate()
            .filter_map(|(i, n)| Some((n.clone(), u16::try_from(i).ok()?)))
            .collect();
        // The synthesized Unicode charmap. First name wins, so a font
        // declaring both `A` and `uni0041` maps U+0041 to the earlier one —
        // the same tie-break FreeType applies.
        let mut unicode_map: HashMap<char, u16> = HashMap::new();
        for (i, name) in glyph_names.iter().enumerate() {
            if let (Some(ch), Ok(gid)) = (encoding::unicode_from_glyph_name(name), u16::try_from(i))
            {
                unicode_map.entry(ch).or_insert(gid);
            }
        }

        let blend = program::build_blend(&header, diags);
        let encoding = header.encoding.unwrap_or(Encoding::Standard);
        note_missing_encoding_glyphs(&encoding, &by_name, diags);

        let font_matrix = header.font_matrix.unwrap_or(DEFAULT_MATRIX);
        Ok(Self {
            container: split.container,
            font_name: header.font_name,
            full_name: header.full_name,
            family_name: header.family_name,
            italic_angle: header.italic_angle,
            is_fixed_pitch: header.is_fixed_pitch,
            font_matrix,
            font_bbox: header.font_bbox.unwrap_or(Rect::ZERO),
            encoding,
            subrs: private.subrs,
            charstrings,
            glyph_names,
            by_name,
            unicode_map,
            blend,
        })
    }

    /// Which wrapper the program arrived in.
    #[must_use]
    pub fn container(&self) -> Container {
        self.container
    }

    /// Design units per em, derived from the `/FontMatrix` — a matrix of
    /// `0.001` means 1000 units per em.
    ///
    /// Rounded to the nearest integer and floored at 1, because a rasterizer
    /// dividing by this must never divide by zero and a font declaring a
    /// degenerate matrix is not worth refusing over.
    #[must_use]
    pub fn units_per_em(&self) -> u16 {
        let sx = self.font_matrix.as_coeffs().first().copied().unwrap_or(0.0);
        if sx.abs() < 1e-12 {
            return 1000;
        }
        let upem = (1.0 / sx).abs().round();
        if upem.is_finite() && (1.0..=f64::from(u16::MAX)).contains(&upem) {
            // Range-checked immediately above, so the cast is exact.
            #[allow(clippy::cast_sign_loss)]
            {
                upem as u16
            }
        } else {
            1000
        }
    }

    /// The `/FontMatrix`: font units to text space.
    #[must_use]
    pub fn font_matrix(&self) -> Affine {
        self.font_matrix
    }

    /// The declared `/FontBBox`, in font units. `Rect::ZERO` when the program
    /// declared none — callers derive one from the glyphs in that case.
    #[must_use]
    pub fn bbox(&self) -> Rect {
        self.font_bbox
    }

    /// Number of glyphs in `/CharStrings`.
    #[must_use]
    pub fn num_glyphs(&self) -> u32 {
        self.charstrings.len() as u32
    }

    /// Whether the program declared `/isFixedPitch true`.
    #[must_use]
    pub fn is_fixed_pitch(&self) -> bool {
        self.is_fixed_pitch
    }

    /// `/ItalicAngle`, in degrees counter-clockwise from vertical (so an
    /// oblique face reports a negative value).
    #[must_use]
    pub fn italic_angle(&self) -> f32 {
        self.italic_angle
    }

    /// `/FontName` — the PostScript name.
    #[must_use]
    pub fn postscript_name(&self) -> Option<&str> {
        self.font_name.as_deref()
    }

    /// `/FullName` from `/FontInfo`.
    #[must_use]
    pub fn full_name(&self) -> Option<&str> {
        self.full_name.as_deref()
    }

    /// `/FamilyName` from `/FontInfo`.
    #[must_use]
    pub fn family_name(&self) -> Option<&str> {
        self.family_name.as_deref()
    }

    /// The font's built-in `/Encoding` vector.
    #[must_use]
    pub fn encoding(&self) -> &Encoding {
        &self.encoding
    }

    /// Character code to glyph, through the built-in encoding.
    #[must_use]
    pub fn code_to_gid(&self, code: u8) -> Option<Gid> {
        self.name_to_gid(self.encoding.glyph_name(code)?)
    }

    /// Unicode scalar to glyph, through the Adobe-Glyph-List charmap
    /// synthesized from the glyph names.
    #[must_use]
    pub fn unicode_to_gid(&self, ch: char) -> Option<Gid> {
        self.unicode_map.get(&ch).copied().map(Gid)
    }

    /// Every Unicode scalar the synthesized charmap maps, in arbitrary order.
    pub fn unicode_pairs(&self) -> impl Iterator<Item = (char, Gid)> + '_ {
        self.unicode_map.iter().map(|(&ch, &gid)| (ch, Gid(gid)))
    }

    /// Glyph name to glyph.
    #[must_use]
    pub fn name_to_gid(&self, name: &str) -> Option<Gid> {
        self.by_name.get(name).copied().map(Gid)
    }

    /// The name a glyph was declared under.
    #[must_use]
    pub fn glyph_name(&self, gid: Gid) -> Option<&str> {
        self.glyph_names.get(gid.0 as usize).map(AsRef::as_ref)
    }

    /// Always true: Type 1 identifies glyphs by name, so every glyph has one.
    #[must_use]
    pub fn has_glyph_names(&self) -> bool {
        true
    }

    /// Every `(glyph name, glyph)` pair, in glyph order.
    pub fn glyph_names(&self) -> impl Iterator<Item = (Gid, &str)> {
        self.glyph_names
            .iter()
            .enumerate()
            .filter_map(|(i, n)| Some((Gid(u16::try_from(i).ok()?), n.as_ref())))
    }

    /// The unscaled outline in font units, and the advance width `hsbw`/`sbw`
    /// declared.
    ///
    /// The outline is *not* transformed by the `/FontMatrix`; a caller that
    /// wants text-space coordinates applies [`font_matrix`](Self::font_matrix)
    /// itself, which is what `pdfrum-font` does so it can compose the matrix
    /// with the text matrix in one step.
    ///
    /// For a Multiple-Master font this uses the weight vector the program
    /// shipped with — [`instantiate`](Self::instantiate) is how a caller asks
    /// for a different one.
    #[must_use]
    pub fn outline(&self, gid: Gid) -> Option<(BezPath, f32)> {
        let weights = self.default_weights();
        let g = self.interpret(gid, &weights, &mut Diagnostics::with_limit(0))?;
        Some((g.path, g.advance))
    }

    /// [`outline`](Self::outline), recording an interpretation failure.
    ///
    /// The outline comes back either way; the diagnostic says whether it is
    /// the whole glyph or as much of it as the charstring allowed.
    #[must_use]
    pub fn outline_with_diagnostics(
        &self,
        gid: Gid,
        diags: &mut Diagnostics,
    ) -> Option<(BezPath, f32)> {
        let weights = self.default_weights();
        let g = self.interpret(gid, &weights, diags)?;
        Some((g.path, g.advance))
    }

    /// The glyph's bounding box in font units, or `None` for an empty glyph
    /// (a space) as well as for a glyph that does not exist.
    #[must_use]
    pub fn glyph_bounds(&self, gid: Gid) -> Option<Rect> {
        let (path, _) = self.outline(gid)?;
        (!path.is_empty()).then(|| path.bounding_box())
    }

    /// The Multiple-Master design axes, or `None` for an ordinary font.
    ///
    /// ```
    /// # use pdfrum_common::Diagnostics;
    /// # use pdfrum_type1::Type1Font;
    /// # fn demo(pfb: &[u8]) -> Option<()> {
    /// let font = Type1Font::parse(pfb, &Default::default(), &mut Diagnostics::default()).ok()?;
    /// for axis in font.mm_axes()? {
    ///     assert!(axis.min <= axis.default && axis.default <= axis.max);
    /// }
    /// # Some(())
    /// # }
    /// ```
    #[must_use]
    pub fn mm_axes(&self) -> Option<&[MmAxis]> {
        self.blend.as_ref().map(|b| b.axes.as_slice())
    }

    /// Instantiate at design coordinates — one per axis, in
    /// [`mm_axes`](Self::mm_axes) order.
    ///
    /// Coordinates outside an axis's range clamp to it, and a short list
    /// leaves the remaining axes at their defaults. Returns `None` for a font
    /// with no Multiple-Master declaration, which is how a caller tells "this
    /// face cannot vary" from "it varied to the value you asked for".
    #[must_use]
    pub fn instantiate(&self, coords: &[f32]) -> Option<Type1Instance<'_>> {
        let blend = self.blend.as_ref()?;
        Some(Type1Instance {
            font: self,
            weights: blend.weights_for(coords),
        })
    }

    /// The weight vector the program shipped with, or an empty slice for a
    /// non-Multiple-Master font.
    #[must_use]
    pub fn default_weight_vector(&self) -> &[f32] {
        self.blend.as_ref().map_or(&[], |b| &b.default_weights)
    }

    fn default_weights(&self) -> Vec<f32> {
        self.blend
            .as_ref()
            .map(|b| b.default_weights.clone())
            .unwrap_or_default()
    }

    fn interpret(&self, gid: Gid, weights: &[f32], diags: &mut Diagnostics) -> Option<Glyph> {
        let code = self.charstrings.get(gid.0 as usize)?;
        let lookup = |name: &str| self.by_name.get(name).map(|g| *g as usize);
        let (glyph, abort) = charstring::interpret(
            code,
            charstring::Env {
                subrs: &self.subrs,
                charstrings: &self.charstrings,
                name_lookup: &lookup,
                weights,
                blend: self.blend.as_ref(),
            },
        );
        if abort.is_some() {
            diags.record(
                Severity::Suspicious,
                DiagKind::Type1CharstringAborted,
                Some(u64::from(gid.0)),
            );
        }
        Some(glyph)
    }
}

/// A Multiple-Master font blended at one point in design space.
///
/// Borrows its font, so it is free to create — the interpolation happens per
/// glyph, at [`outline`](Self::outline) time, exactly as it does in the
/// charstring machine.
#[derive(Debug, Clone)]
pub struct Type1Instance<'a> {
    font: &'a Type1Font,
    weights: Vec<f32>,
}

impl Type1Instance<'_> {
    /// The blended outline in font units, and its advance width.
    ///
    /// The advance is what makes this the interface PDFium's
    /// `AdjustVariationParams` needs: it probes the width axis at both ends,
    /// reads the advance each returns, and interpolates to hit a target width.
    #[must_use]
    pub fn outline(&self, gid: Gid) -> Option<(BezPath, f32)> {
        let g = self
            .font
            .interpret(gid, &self.weights, &mut Diagnostics::with_limit(0))?;
        Some((g.path, g.advance))
    }

    /// The advance width alone, without keeping the outline.
    #[must_use]
    pub fn advance(&self, gid: Gid) -> Option<f32> {
        self.outline(gid).map(|(_, a)| a)
    }

    /// The weight vector this instance blends with — the per-master
    /// coefficients the charstring machine applies.
    #[must_use]
    pub fn weight_vector(&self) -> &[f32] {
        &self.weights
    }

    /// The font this instance came from.
    #[must_use]
    pub fn font(&self) -> &Type1Font {
        self.font
    }
}

/// A decrypted private dictionary starts with PostScript, and a wrongly-keyed
/// one starts with noise. Testing the first non-blank run for printability
/// separates them reliably without demanding an exact prefix, which real fonts
/// vary (`dup /Private`, `/Private`, `2 index /Private`).
fn looks_like_postscript(plain: &[u8]) -> bool {
    let head = plain.get(..64).unwrap_or(plain);
    if head.is_empty() {
        return false;
    }
    let printable = head
        .iter()
        .filter(|b| b.is_ascii_graphic() || b.is_ascii_whitespace())
        .count();
    printable * 4 >= head.len() * 3
}

/// Record encoding entries naming glyphs the font does not define — a
/// subsetted font's calling card, and something a caller may want to know
/// before it decides the font is unusable.
fn note_missing_encoding_glyphs(
    enc: &Encoding,
    by_name: &HashMap<Box<str>, u16>,
    diags: &mut Diagnostics,
) {
    if let Encoding::Custom(table) = enc {
        for (code, slot) in table.iter().enumerate() {
            if let Some(name) = slot
                && !by_name.contains_key(name.as_ref())
            {
                diags.record(
                    Severity::Suspicious,
                    DiagKind::Type1EncodingGlyphMissing,
                    Some(code as u64),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests;
