//! Text objects (ISO 32000-1 §9.4).
//!
//! # Everything collapses into `Tm`
//!
//! A text object is written as `BT`, one `Tm`, one `Tf`, one `Tr`, one `TJ`,
//! `ET`. There is no `Td`, no `T*`, no `Tj`, no `'` or `"` — all positioning
//! goes into the text matrix. `Tc`, `Tw`, `Tz`, `TL` and `Ts` are never
//! written either, so character spacing, word spacing, horizontal scaling,
//! leading and rise are **lost** on a regenerated page. That is the C++'s
//! behavior and matching it is the requirement (see the module docs of
//! [`crate::content`]).
//!
//! # The matrix is transposed
//!
//! `TextObject::matrix` stores the glyph matrix with translation excluded,
//! and the operand order `Tm` wants is `a b c d e f` where `b` and `c` are
//! **swapped** relative to how the object holds them
//! (`cpdf_textobject.cpp:198-202`). Getting this backwards mirrors slanted
//! text about its own baseline, which renders as something almost right —
//! the worst kind of wrong.
//!
//! # A font we cannot classify drops the whole object
//!
//! `Tf` needs a subtype: `Type1`, `TrueType` or `Type0`. A Type 3 font has
//! none of those, and the C++ answers by returning after `q ` and `BT ` are
//! already written, leaving both unclosed. We build into a scratch buffer and
//! discard it, so an unclassifiable font contributes nothing (divergence D6).

use pdfrum_common::kurbo::Affine;
use pdfrum_font::Font;
use pdfrum_page::{TextObject, TextSegment};

use crate::content::num::{write_float, write_matrix};
use crate::names;

/// What `Tf` calls a font, or `None` for one no subtype fits.
///
/// Type 3 is the absent case: its glyphs are content streams rather than a
/// program, so it has no `/Subtype` a synthesized font resource could carry.
#[must_use]
pub fn font_subtype(font: &Font) -> Option<&'static pdfrum_object::Name> {
    match font {
        Font::Simple(simple) => {
            if simple.glyphs.is_truetype() {
                Some(names::TRUE_TYPE)
            } else {
                Some(names::TYPE1)
            }
        }
        Font::Type0(_) => Some(names::TYPE0),
        Font::Type3(_) => None,
    }
}

/// Write a text object's body — everything between `q ` and ` Q`.
///
/// `resource` is the name the font was realized under. Returns `false` when
/// the object cannot be expressed, in which case `out` is untouched.
pub fn emit_text_body(out: &mut String, text: &TextObject, resource: &pdfrum_object::Name) -> bool {
    let Some((font, size)) = text.font.as_ref() else {
        return false;
    };
    if font_subtype(font).is_none() {
        return false;
    }

    let mut body = String::new();
    body.push_str("BT ");

    let matrix = text_matrix(text);
    if matrix != Affine::IDENTITY {
        write_matrix(&mut body, matrix);
        body.push_str(" Tm ");
    }

    body.push('/');
    body.push_str(&String::from_utf8_lossy(&pdfrum_object::name_encode(
        resource.as_bytes(),
    )));
    body.push(' ');
    write_float(&mut body, *size);
    body.push_str(" Tf ");
    body.push_str(&render_mode(text).to_string());
    body.push_str(" Tr ");

    emit_show_array(&mut body, &text.segments);
    body.push_str(" TJ ET");

    out.push_str(&body);
    true
}

/// The `Tm` operand list.
///
/// `b` and `c` are transposed relative to the stored matrix, and the
/// translation comes from the object's position rather than the matrix — the
/// matrix carries orientation and scale only.
fn text_matrix(text: &TextObject) -> Affine {
    let c = text.matrix.as_coeffs();
    Affine::new([
        c.first().copied().unwrap_or(1.0),
        c.get(2).copied().unwrap_or(0.0),
        c.get(1).copied().unwrap_or(0.0),
        c.get(3).copied().unwrap_or(1.0),
        text.position.x,
        text.position.y,
    ])
}

/// `Tr`'s integer operand (ISO 32000-1 table 106).
fn render_mode(text: &TextObject) -> u8 {
    use pdfrum_page::TextRenderMode as M;
    match text.render_mode {
        M::Fill => 0,
        M::Stroke => 1,
        M::FillStroke => 2,
        M::Invisible => 3,
        M::FillClip => 4,
        M::StrokeClip => 5,
        M::FillStrokeClip => 6,
        M::Clip => 7,
    }
}

/// The `[…]` operand of `TJ`: hex strings of char codes, with kerning
/// adjustments as bare numbers between them.
///
/// Codes are written **hex** rather than literal because a code may be any
/// byte, and a literal string would need escapes that vary with the value —
/// hex is one spelling for every byte and is what the goldens show.
fn emit_show_array(out: &mut String, segments: &[TextSegment]) {
    out.push('[');
    for segment in segments {
        // A kerning adjustment of zero moves nothing and is not written.
        if segment.kerning != 0.0 {
            write_float(out, segment.kerning);
        }
        if segment.codes.is_empty() {
            continue;
        }
        out.push('<');
        for byte in &segment.codes {
            out.push_str(&format!("{byte:02X}"));
        }
        out.push('>');
    }
    out.push(']');
}

#[cfg(test)]
mod tests {
    use super::{emit_show_array, emit_text_body, font_subtype};
    use pdfrum_common::kurbo::{Affine, Point};
    use pdfrum_font::{Font, FontCache, StandardFont};
    use pdfrum_object::Name;
    use pdfrum_page::{TextObject, TextRenderMode, TextSegment};
    use std::collections::BTreeMap;
    use std::sync::Arc;

    fn segments(pairs: &[(&[u8], f32)]) -> Box<[TextSegment]> {
        pairs
            .iter()
            .map(|(codes, kerning)| TextSegment {
                codes: (*codes).into(),
                kerning: *kerning,
            })
            .collect()
    }

    fn text(font: Option<Arc<Font>>, matrix: Affine, position: Point) -> TextObject {
        TextObject {
            segments: segments(&[(b"Hello World", 0.0)]),
            position,
            matrix,
            font: font.map(|f| (f, 10.0)),
            render_mode: TextRenderMode::Fill,
            type3_metrics: BTreeMap::new(),
        }
    }

    fn helvetica() -> Arc<Font> {
        Arc::new(Font::load_standard(
            StandardFont::Helvetica,
            &FontCache::new(),
        ))
    }

    fn emit(t: &TextObject) -> Option<String> {
        let mut out = String::new();
        emit_text_body(&mut out, t, &Name::from("FXF1")).then_some(out)
    }

    // ProcessStandardText (:261-326): the whole shape, verbatim.
    #[test]
    fn a_text_object_writes_bt_tm_tf_tr_tj_et() {
        let t = text(
            Some(helvetica()),
            Affine::IDENTITY,
            Point::new(100.0, 100.0),
        );
        assert_eq!(
            emit(&t).as_deref(),
            Some("BT 1 0 0 1 100 100 Tm /FXF1 10 Tf 0 Tr [<48656C6C6F20576F726C64>] TJ ET")
        );
    }

    // The render mode is written as its integer, whatever it is.
    #[test]
    fn the_render_mode_is_written_as_an_integer() {
        for (mode, spelled) in [
            (TextRenderMode::Fill, 0),
            (TextRenderMode::Stroke, 1),
            (TextRenderMode::Invisible, 3),
            (TextRenderMode::FillClip, 4),
            (TextRenderMode::Clip, 7),
        ] {
            let mut t = text(Some(helvetica()), Affine::IDENTITY, Point::ZERO);
            t.render_mode = mode;
            let out = emit(&t).expect("emits");
            assert!(out.contains(&format!("{spelled} Tr ")), "{mode:?}: {out}");
        }
    }

    // The transposition: b and c swap on the way out.
    #[test]
    fn the_text_matrix_transposes_b_and_c() {
        let t = text(
            Some(helvetica()),
            // stored a, b, c, d
            Affine::new([2.0, 3.0, 4.0, 5.0, 0.0, 0.0]),
            Point::new(7.0, 8.0),
        );
        let out = emit(&t).expect("emits");
        // written a, c, b, d, then the position
        assert!(out.contains("2 4 3 5 7 8 Tm"), "got {out}");
    }

    // An identity matrix at the origin needs no `Tm` at all.
    #[test]
    fn an_identity_matrix_at_the_origin_writes_no_tm() {
        let t = text(Some(helvetica()), Affine::IDENTITY, Point::ZERO);
        let out = emit(&t).expect("emits");
        assert!(!out.contains("Tm"), "got {out}");
        assert!(out.starts_with("BT /FXF1"));
    }

    // A font with no subtype drops the whole object rather than leaving `BT`
    // unclosed the way the C++ does (D6).
    #[test]
    fn a_font_we_cannot_classify_emits_nothing() {
        let mut out = String::from("existing");
        let t = text(None, Affine::IDENTITY, Point::ZERO);
        assert!(!emit_text_body(&mut out, &t, &Name::from("FXF1")));
        assert_eq!(out, "existing", "the buffer is left untouched");
    }

    #[test]
    fn a_standard_font_classifies_as_type1() {
        assert_eq!(
            font_subtype(&helvetica()).and_then(|n| n.as_str()),
            Some("Type1")
        );
    }

    // The show array: hex codes, kerning as bare numbers between them.
    #[test]
    fn the_show_array_interleaves_hex_runs_and_kerning() {
        let mut out = String::new();
        emit_show_array(&mut out, &segments(&[(b"AB", 0.0), (b"CD", -25.0)]));
        assert_eq!(out, "[<4142>-25<4344>]");
    }

    #[test]
    fn a_zero_kerning_is_not_written() {
        let mut out = String::new();
        emit_show_array(&mut out, &segments(&[(b"A", 0.0), (b"B", 0.0)]));
        assert_eq!(out, "[<41><42>]");
    }

    #[test]
    fn a_kerning_only_segment_writes_just_the_number() {
        let mut out = String::new();
        emit_show_array(&mut out, &segments(&[(b"A", 0.0), (b"", -100.0)]));
        assert_eq!(out, "[<41>-100]");
    }

    // Every byte gets one two-digit spelling, high bytes included.
    #[test]
    fn high_bytes_spell_as_two_hex_digits() {
        let mut out = String::new();
        emit_show_array(&mut out, &segments(&[(&[0x00, 0xFF, 0x0A], 0.0)]));
        assert_eq!(out, "[<00FF0A>]");
    }

    #[test]
    fn an_empty_show_array_is_still_written() {
        let mut out = String::new();
        emit_show_array(&mut out, &[]);
        assert_eq!(out, "[]");
    }
}
