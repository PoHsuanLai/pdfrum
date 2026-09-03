//! The CID glyph ladder — the most intricate lookup in this crate.
//!
//! Two halves that share almost nothing. The **embedded** half is short: a
//! `CIDFontType0` uses its CID as its glyph index outright, and a
//! `CIDFontType2` either indexes a `/CIDToGIDMap` table or goes through a
//! charmap. The **substituted** half is long, because a system face knows
//! nothing about the document's CID collection and the ladder has to
//! reconstruct a Unicode to look up instead.

use super::{CidFontKind, CidToGid, Type0Font, cid_charmap};
use crate::encoding::{FaceEncoding, FontEncoding, adobe_char_name, unicode_from_adobe_name};
use crate::glyphs::Charmap;
use crate::{CharCode, Gid, GlyphName};

/// The Adobe CourierStd rescue offset. Its CIDs sit exactly this far below
/// the standard encoding's codes.
const COURIER_STD_OFFSET: u32 = 31;

/// U+2502 BOX DRAWINGS LIGHT VERTICAL, which is **never** rotated into a
/// vertical form even in a vertical font — it is already vertical.
const BOX_DRAWINGS_LIGHT_VERTICAL: u32 = 0x2502;

/// An empty `/Differences` array: the CourierStd rescue names its glyph out of
/// a base encoding alone, with nothing overriding it.
const NO_DIFFS: [Option<GlyphName>; 256] = [const { None }; 256];

/// Resolve a character code to a glyph, and whether a vertical form was
/// substituted.
pub(super) fn resolve(font: &Type0Font, code: CharCode) -> (Option<Gid>, bool) {
    // Half 1 — no embedded program, and either no `/CIDToGIDMap` stream or a
    // known collection to derive a Unicode from.
    let has_stream = matches!(font.cid_to_gid, CidToGid::Stream(_));
    if !font.embedded && (!has_stream || pdfrum_cmap::has_cid2unicode(font.charset)) {
        return substituted(font, code);
    }
    embedded(font, code)
}

/// Half 1: a substituted face, reached through a reconstructed Unicode.
fn substituted(font: &Type0Font, code: CharCode) -> (Option<Gid>, bool) {
    let cid = font.cid_from_charcode(code);

    if font.cid_to_gid == CidToGid::Identity {
        return (Some(Gid(cid.0)), false);
    }

    // Three sources of a Unicode, in order: the collection's table, the
    // scalar derivation, and finally `/ToUnicode`.
    let mut unicode: u16 = 0;
    if cid.0 != 0 && pdfrum_cmap::has_cid2unicode(font.charset) {
        unicode = pdfrum_cmap::unicode_from_cid(font.charset, cid).map_or(0, |c| c as u16);
    }
    if unicode == 0 {
        unicode = font.scalar_unicode(code);
    }
    if unicode == 0 {
        unicode = font
            .unicode_from_charcode(code)
            .first()
            .map_or(0, |c| *c as u16);
    }

    if unicode == 0 {
        return courier_std_rescue(font, code);
    }

    // The Japan1 fixups: a backslash becomes a slash, and a yen sign becomes a
    // backslash — both because the Japanese encodings historically overload
    // the same code point.
    if font.charset == pdfrum_cmap::CidSet::Japan1 {
        if unicode == u16::from(b'\\') {
            unicode = u16::from(b'/');
        } else if unicode == 0xA5 {
            unicode = 0x5C;
        }
    }

    if !font.glyphs.is_some() {
        return (Some(Gid(unicode)), false);
    }
    let charmaps = font.glyphs.charmaps();
    let n = charmaps.len();

    // Charmap negotiation. If no Unicode charmap exists, scan for one whose
    // reverse mapping recognises the **character code** — note it probes with
    // `code`, not with the `unicode` just computed, which reads like an
    // oversight and is preserved because it decides which charmap is used.
    let mut charmap = Charmap::Unicode;
    let mut lookup_code = u32::from(unicode);
    if !charmaps.iter().any(|c| c.is_unicode()) {
        let mut found = None;
        for (i, id) in charmaps.iter().enumerate() {
            let ret = id
                .face_encoding()
                .charcode_from_unicode((code.0 & 0xffff) as u16);
            if ret != 0 {
                found = Some((i, ret));
                break;
            }
        }
        match found {
            Some((i, ret)) => {
                charmap = Charmap::Index(i);
                lookup_code = ret;
            }
            None if n != 0 => {
                charmap = Charmap::Index(0);
                lookup_code = code.0 & 0xffff;
            }
            None => {}
        }
    }

    if n != 0 {
        let (gid, vertical) = glyph_index(font, charmap, lookup_code);
        return if gid == 0 {
            (None, false)
        } else {
            (Some(Gid(gid)), vertical)
        };
    }
    // A face with **zero** charmaps returns the Unicode value itself as a
    // glyph index, which is nonsense for most faces and is what the C++ does.
    (Some(Gid(unicode)), false)
}

/// The Adobe CourierStd rescue, taken when no Unicode could be derived.
///
/// A hard-coded `+= 31` with no general rule behind it: Adobe's CourierStd
/// numbers its CIDs 31 below the standard encoding, so shifting the code by
/// 31 and reading it as a standard-encoding code recovers a glyph name.
/// Every exit that fails returns the (shifted) code itself as a glyph index.
fn courier_std_rescue(font: &Type0Font, code: CharCode) -> (Option<Gid>, bool) {
    if !font.adobe_courier_std {
        return (nonzero_gid(code.0), false);
    }
    let code = CharCode(code.0.wrapping_add(COURIER_STD_OFFSET));

    let charmaps = font.glyphs.charmaps();
    let ms_unicode = charmaps.iter().any(|c| c.is_unicode());
    let mac_roman = !ms_unicode && charmaps.contains(&crate::glyphs::CharmapId::MAC_ROMAN);
    let encoding = if ms_unicode {
        FontEncoding::WinAnsi
    } else if mac_roman {
        FontEncoding::MacRoman
    } else {
        FontEncoding::Standard
    };

    let Some(name) = adobe_char_name(encoding, &NO_DIFFS, code.0) else {
        return (nonzero_gid(code.0), false);
    };
    let name = name.to_vec();
    let nu = unicode_from_adobe_name(&name);
    if nu == 0 {
        return (nonzero_gid(code.0), false);
    }

    // The Standard arm returns early and **unchecked** — a zero result is
    // returned as glyph 0 rather than falling through to the code.
    if encoding == FontEncoding::Standard {
        let g = font.glyphs.char_index(Charmap::Unicode, u32::from(nu));
        return (Some(Gid(g)), false);
    }

    let index = if encoding == FontEncoding::WinAnsi {
        font.glyphs.char_index(Charmap::Unicode, u32::from(nu))
    } else {
        let mac = FaceEncoding::AppleRoman.charcode_from_unicode(nu);
        if mac != 0 {
            font.glyphs.char_index(Charmap::Unicode, mac)
        } else {
            font.glyphs.name_index(&name)
        }
    };
    if index == 0 || index == u16::MAX {
        return (nonzero_gid(code.0), false);
    }
    (Some(Gid(index)), false)
}

/// A code as a glyph index, or nothing when the code is zero.
fn nonzero_gid(code: u32) -> Option<Gid> {
    if code == 0 {
        None
    } else {
        Some(Gid((code & 0xffff) as u16))
    }
}

/// Half 2: an embedded program.
fn embedded(font: &Type0Font, code: CharCode) -> (Option<Gid>, bool) {
    if !font.glyphs.is_some() {
        return (None, false);
    }
    let cid = font.cid_from_charcode(code);

    if let CidToGid::Stream(table) = &font.cid_to_gid {
        // A big-endian `u16` table indexed by CID.
        let Some(pos) = usize::from(cid.0).checked_mul(2) else {
            return (None, false);
        };
        let (Some(&hi), Some(&lo)) = (table.get(pos), table.get(pos + 1)) else {
            return (None, false);
        };
        return (Some(Gid(u16::from(hi) * 256 + u16::from(lo))), false);
    }

    // `CIDFontType0`: the CID *is* the glyph index.
    if font.kind == CidFontKind::Type1 {
        return (Some(Gid(cid.0)), false);
    }
    // A predefined CMap with an embedded program also uses CID as GID —
    // `has_no_direct_table` is true exactly for a predefined CMap.
    if font.embedded && font.cmap.has_no_direct_table() {
        return (Some(Gid(cid.0)), false);
    }
    if font.cmap.coding() == pdfrum_cmap::CidCoding::Unknown {
        return (Some(Gid(cid.0)), false);
    }

    let charmap = cid_charmap(&font.glyphs, font.cmap.coding());
    if charmap == Charmap::None {
        return (Some(Gid(cid.0)), false);
    }
    let is_unicode_charmap = match charmap {
        Charmap::Index(i) => font
            .glyphs
            .charmaps()
            .get(i)
            .is_some_and(|c| c.is_unicode()),
        Charmap::Unicode => true,
        Charmap::None => false,
    };
    let lookup = if is_unicode_charmap {
        match font.unicode_from_charcode(code).first() {
            Some(c) => *c as u32,
            None => return (None, false),
        }
    } else {
        code.0
    };
    let (gid, vertical) = glyph_index(font, charmap, lookup);
    (Some(Gid(gid)), vertical)
}

/// Look a code up and apply vertical substitution (`GetGlyphIndex`).
fn glyph_index(font: &Type0Font, charmap: Charmap, code: u32) -> (u16, bool) {
    let index = font.glyphs.char_index(charmap, code);
    // The one character never rotated: it is already a vertical stroke.
    if code == BOX_DRAWINGS_LIGHT_VERTICAL {
        return (index, false);
    }
    if index == 0 || !font.is_vertical() {
        return (index, false);
    }
    match font.gsub().vertical_glyph(index) {
        Some(v) => (v, true),
        None => (index, false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_zero_code_yields_no_glyph_while_a_nonzero_one_yields_itself() {
        assert_eq!(nonzero_gid(0), None);
        assert_eq!(nonzero_gid(31), Some(Gid(31)));
        assert_eq!(nonzero_gid(287), Some(Gid(287)));
        // The C++ narrows to an `int` and the caller to a `u16`; a code past
        // 65 535 wraps rather than failing.
        assert_eq!(nonzero_gid(0x1_0000), Some(Gid(0)));
    }

    #[test]
    fn the_courier_offset_is_exactly_thirty_one() {
        // Pinned because the constant has no derivation — it is the offset
        // Adobe's CourierStd happens to use.
        assert_eq!(COURIER_STD_OFFSET, 31);
    }

    #[test]
    fn the_never_rotated_character_is_box_drawings_light_vertical() {
        assert_eq!(BOX_DRAWINGS_LIGHT_VERTICAL, 0x2502);
    }
}
