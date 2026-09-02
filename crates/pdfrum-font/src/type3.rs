//! Type 3 fonts: glyphs that are content streams.
//!
//! A Type 3 font has no face, no outlines and no glyph indices — its glyphs
//! are `/CharProcs` entries that the page layer executes as content streams in
//! the font's own coordinate system. This crate's job is therefore only to
//! resolve a character code to the *name* of a procedure, and to carry the
//! matrix and widths that place it (`docs/design/pdfrum-font.md` §1.11).

use crate::encoding::{FontEncoding, adobe_char_name};
use crate::{
    CharCode, CharItem, FontCache, FontId, Gid, GlyphName, ToUnicode, names, simple, tounicode,
};
use pdfrum_common::kurbo::{Affine, Rect};
use pdfrum_common::{Diagnostics, Limits};
use pdfrum_object::{Dict, Resolve, Stream};
use smallvec::SmallVec;

/// The maximum nesting of Type 3 glyph procedures, which may themselves show
/// text in a Type 3 font (`kMaxType3FormLevel`). Ported verbatim.
pub const MAX_TYPE3_DEPTH: u32 = 4;

/// A font whose glyphs are content streams.
#[derive(Debug)]
pub struct Type3Font {
    /// This font's identity.
    pub(crate) id: FontId,
    /// `/FontMatrix`, mapping glyph space to text space.
    pub font_matrix: Affine,
    /// `/CharProcs`: glyph name to content stream.
    pub(crate) char_procs: Dict,
    /// `/Resources` for the glyph procedures, which fall back to the page's.
    pub resources: Option<Dict>,
    /// The `/Differences` overlay. **Not cleared after loading**, unlike a
    /// simple font's — a Type 3 font consults it for the life of the font,
    /// because it is the only way a code names a procedure.
    pub(crate) encoding: [Option<GlyphName>; 256],
    /// Which predefined set the encoding resolved to, usually `Builtin`.
    pub(crate) encoding_kind: FontEncoding,
    /// Widths in **glyph space × 1000**, `0` meaning "ask the procedure".
    ///
    /// An `i32` table defaulting to zero, unlike a simple font's `u16` table
    /// defaulting to the unset sentinel.
    pub(crate) widths: [i32; 256],
    /// `/FontBBox`, scaled into glyph units.
    pub(crate) font_bbox: Rect,
    /// The `/ToUnicode` CMap.
    pub(crate) to_unicode: Option<ToUnicode>,
}

impl Type3Font {
    /// The glyph procedure for a character code.
    ///
    /// `None` when the code names nothing — which for a font with **no
    /// `/Encoding` at all** is every code, because the encoding stays
    /// `Builtin` and only `/Differences` can name a procedure.
    #[must_use]
    pub fn char_proc(&self, code: CharCode, r: &impl Resolve) -> Option<Stream> {
        let name = self.char_proc_name(code)?;
        let key = pdfrum_object::Name::new(name.to_vec());
        self.char_procs.stream(&key, r)
    }

    /// The name of the glyph procedure for a code.
    #[must_use]
    pub(crate) fn char_proc_name(&self, code: CharCode) -> Option<&[u8]> {
        adobe_char_name(self.encoding_kind, &self.encoding, code.0)
    }

    /// The advance width for a code, in glyph units.
    ///
    /// A code at or above 256 reads code **0**, matching the simple-font
    /// clamp. Zero means the PDF declared nothing and the procedure's own
    /// `d0`/`d1` operator supplies the width — which only the page layer can
    /// see, so this returns 0 and the caller asks it.
    #[must_use]
    pub(crate) fn char_width(&self, code: CharCode) -> f32 {
        let code = if code.0 >= 256 { 0 } else { code.0 as usize };
        self.widths.get(code).copied().unwrap_or(0) as f32
    }

    /// The Unicode a code stands for, from `/ToUnicode` only — a Type 3 font
    /// has no encoding table to fall back on.
    #[must_use]
    pub(crate) fn unicode_from_charcode(&self, code: CharCode) -> SmallVec<[char; 2]> {
        self.to_unicode
            .as_ref()
            .map(|tu| tu.lookup(code))
            .unwrap_or_default()
    }

    /// The character code that produces `unicode`, or `None`.
    ///
    /// A Type 3 font has no encoding table to scan, so this is `/ToUnicode`'s
    /// reverse map or nothing.
    #[must_use]
    pub(crate) fn char_code_from_unicode(&self, unicode: char) -> Option<CharCode> {
        let code = self.to_unicode.as_ref()?.reverse(unicode);
        (code.0 != 0).then_some(code)
    }

    pub(crate) fn char_item(&self, code: CharCode) -> CharItem {
        CharItem {
            code,
            cid: None,
            // A Type 3 font has no glyph indices by construction; the C++'s
            // `GlyphFromCharCode` returns -1 unconditionally.
            gid: Gid(0),
            unicode: self.unicode_from_charcode(code),
            width: self.char_width(code),
            vertical_glyph: false,
            has_glyph: false,
        }
    }
}

/// Load a Type 3 font.
///
/// Cannot fail. Note `/Encoding` is read **only when present**: a Type 3 font
/// without one keeps a `Builtin` encoding, which names nothing, so no glyph
/// resolves at all — a real and reachable state.
pub(crate) fn load(
    dict: &Dict,
    r: &impl Resolve,
    cache: &FontCache,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> Type3Font {
    let (font_matrix, xscale, yscale) = match dict.raw(names::FONT_MATRIX) {
        Some(_) => {
            let m = dict.matrix(names::FONT_MATRIX, r);
            let c = m.as_coeffs();
            (m, c[0], c[3])
        }
        None => (Affine::IDENTITY, 1.0, 1.0),
    };

    let font_bbox = match dict.array(names::FONT_BBOX, r) {
        Some(b) => {
            // Scaled by the matrix's diagonal, then into glyph units.
            Rect::new(
                f64::from(b.number_at_or_zero(0)) * xscale * 1000.0,
                f64::from(b.number_at_or_zero(1)) * yscale * 1000.0,
                f64::from(b.number_at_or_zero(2)) * xscale * 1000.0,
                f64::from(b.number_at_or_zero(3)) * yscale * 1000.0,
            )
        }
        None => Rect::ZERO,
    };

    let mut widths = [0i32; 256];
    let start = dict.int(names::FIRST_CHAR, r).unwrap_or(0);
    if let (Ok(start), Some(w)) = (usize::try_from(start), dict.array(names::WIDTHS, r))
        && start < 256
    {
        let count = w.len().min(256).min(256 - start);
        for i in 0..count {
            let value = f64::from(w.number_at_or_zero(i)) * xscale * 1000.0;
            if let Some(slot) = widths.get_mut(start + i) {
                // The C++ rounds through a float, not a truncation.
                *slot = round_f32(value);
            }
        }
    }

    let mut encoding: [Option<GlyphName>; 256] = [const { None }; 256];
    let mut encoding_kind = FontEncoding::Builtin;
    if dict.raw(names::ENCODING).is_some() {
        simple::load_pdf_encoding(
            dict,
            r,
            b"",
            crate::FontFlags::DEFAULT,
            false,
            false,
            &mut encoding_kind,
            &mut encoding,
        );
    }

    let to_unicode = dict
        .stream(names::TO_UNICODE, r)
        .map(|s| {
            let bytes = pdfrum_filters::decode_chain(&s, 0, r, limits, diags).data;
            tounicode::parse(&bytes, limits, diags)
        })
        .filter(|m| !m.is_empty());

    Type3Font {
        id: cache.next_id(),
        font_matrix,
        char_procs: dict.dict(names::CHAR_PROCS, r).unwrap_or_default(),
        resources: dict.dict(names::RESOURCES, r),
        encoding,
        encoding_kind,
        widths,
        font_bbox,
        to_unicode,
    }
}

fn round_f32(v: f64) -> i32 {
    let r = v.round();
    if r.is_nan() {
        0
    } else if r >= f64::from(i32::MAX) {
        i32::MAX
    } else if r <= f64::from(i32::MIN) {
        i32::MIN
    } else {
        r as i32
    }
}

#[cfg(test)]
mod tests {
    // Test expectations are exact values by design.
    #![allow(clippy::float_cmp)]
    use super::*;
    use pdfrum_object::{Array, Name, NoResolve, Object};

    fn font_dict(pairs: Vec<(&Name, Object)>) -> Dict {
        Dict::from_pairs(pairs.into_iter().map(|(k, v)| (k.clone(), v)))
    }

    fn load_it(dict: &Dict) -> Type3Font {
        load(
            dict,
            &NoResolve,
            &FontCache::new(),
            &Limits::default(),
            &mut Diagnostics::default(),
        )
    }

    #[test]
    fn without_an_encoding_no_code_names_anything() {
        // The reachable dead end: a `Builtin` encoding with no `/Differences`
        // means `char_proc_name` is `None` for every one of the 256 codes.
        let f = load_it(&Dict::new());
        assert_eq!(f.encoding_kind, FontEncoding::Builtin);
        for code in 0..256u32 {
            assert!(f.char_proc_name(CharCode(code)).is_none(), "code {code}");
        }
    }

    #[test]
    fn differences_name_the_procedures() {
        let enc = font_dict(vec![(
            names::DIFFERENCES,
            Object::Array(Array::of([
                Object::Int(97),
                Object::Name(Name::from("square")),
                Object::Name(Name::from("triangle")),
            ])),
        )]);
        let f = load_it(&font_dict(vec![(names::ENCODING, Object::Dict(enc))]));
        assert_eq!(f.char_proc_name(CharCode(97)), Some(&b"square"[..]));
        assert_eq!(f.char_proc_name(CharCode(98)), Some(&b"triangle"[..]));
        // An `/Encoding` **dictionary** promotes the encoding from `Builtin`
        // to `Standard` (§1.7), so an uncovered code falls through to the
        // predefined set rather than naming nothing — which is a different
        // outcome from a font with no `/Encoding` at all.
        assert_eq!(f.encoding_kind, FontEncoding::Standard);
        assert_eq!(f.char_proc_name(CharCode(99)), Some(&b"c"[..]));
    }

    #[test]
    fn the_font_matrix_defaults_to_identity() {
        let f = load_it(&Dict::new());
        assert_eq!(f.font_matrix, Affine::IDENTITY);
    }

    #[test]
    fn widths_are_scaled_by_the_matrix_and_by_a_thousand() {
        let f = load_it(&font_dict(vec![
            (
                names::FONT_MATRIX,
                Object::Array(Array::of([
                    Object::Real(0.01),
                    Object::Int(0),
                    Object::Int(0),
                    Object::Real(0.01),
                    Object::Int(0),
                    Object::Int(0),
                ])),
            ),
            (names::FIRST_CHAR, Object::Int(97)),
            (
                names::WIDTHS,
                Object::Array(Array::of([Object::Int(50), Object::Int(75)])),
            ),
        ]));
        // 50 * 0.01 * 1000 == 500.
        assert_eq!(f.char_width(CharCode(97)), 500.0);
        assert_eq!(f.char_width(CharCode(98)), 750.0);
        // Undeclared codes are zero, meaning "ask the procedure".
        assert_eq!(f.char_width(CharCode(99)), 0.0);
    }

    #[test]
    fn a_code_at_or_above_256_reads_code_zero() {
        let f = load_it(&font_dict(vec![
            (names::FIRST_CHAR, Object::Int(0)),
            (names::WIDTHS, Object::Array(Array::of([Object::Int(1)]))),
        ]));
        // Width at code 0 is 1 * 1 * 1000.
        assert_eq!(f.char_width(CharCode(0)), 1000.0);
        assert_eq!(f.char_width(CharCode(256)), 1000.0);
        assert_eq!(f.char_width(CharCode(u32::MAX)), 1000.0);
    }

    #[test]
    fn widths_beyond_the_table_are_dropped_without_wrapping() {
        let f = load_it(&font_dict(vec![
            (names::FIRST_CHAR, Object::Int(254)),
            (
                names::WIDTHS,
                Object::Array(Array::of([
                    Object::Int(1),
                    Object::Int(2),
                    Object::Int(3),
                    Object::Int(4),
                ])),
            ),
        ]));
        assert_eq!(f.char_width(CharCode(254)), 1000.0);
        assert_eq!(f.char_width(CharCode(255)), 2000.0);
        // The remaining two had nowhere to go and did not wrap to code 0.
        assert_eq!(f.char_width(CharCode(0)), 0.0);
    }

    #[test]
    fn a_first_char_past_the_table_drops_every_width() {
        let f = load_it(&font_dict(vec![
            (names::FIRST_CHAR, Object::Int(300)),
            (names::WIDTHS, Object::Array(Array::of([Object::Int(9)]))),
        ]));
        assert!(f.widths.iter().all(|&w| w == 0));
    }

    #[test]
    fn the_bbox_is_scaled_into_glyph_units() {
        let f = load_it(&font_dict(vec![
            (
                names::FONT_MATRIX,
                Object::Array(Array::of([
                    Object::Real(0.001),
                    Object::Int(0),
                    Object::Int(0),
                    Object::Real(0.001),
                    Object::Int(0),
                    Object::Int(0),
                ])),
            ),
            (
                names::FONT_BBOX,
                Object::Array(Array::of([
                    Object::Int(0),
                    Object::Int(0),
                    Object::Int(1000),
                    Object::Int(1000),
                ])),
            ),
        ]));
        // 1000 * 0.001 * 1000 == 1000, up to the float representation of
        // 0.001 — which is why this compares within a unit rather than exactly.
        assert!((f.font_bbox.x1 - 1000.0).abs() < 1.0, "{:?}", f.font_bbox);
        assert!((f.font_bbox.y1 - 1000.0).abs() < 1.0, "{:?}", f.font_bbox);
        assert_eq!((f.font_bbox.x0, f.font_bbox.y0), (0.0, 0.0));
    }

    #[test]
    fn a_type3_font_has_no_glyphs_at_all() {
        let f = load_it(&Dict::new());
        let item = f.char_item(CharCode(65));
        assert!(!item.has_glyph);
        assert_eq!(item.glyph(), None);
    }

    #[test]
    fn the_depth_cap_is_four() {
        assert_eq!(MAX_TYPE3_DEPTH, 4);
    }
}
