//! Text objects (ISO 32000-1 §9.4).
//!
//! # Everything collapses into `Tm`
//!
//! A text object is written as `BT`, one `Tm`, one `Tf`, one `Tr`, one `TJ`,
//! `ET`. There is no `Td`, no `T*`, no `Tj`, no `'` or `"` — all positioning
//! goes into the text matrix. `Tc`, `Tw`, `Tz`, `TL` and `Ts` are never
//! written either, so character spacing, word spacing, horizontal scaling,
//! leading and rise are **lost** on a regenerated page. That is a limit of
//! this emitter rather than a requirement (see the module docs of
//! [`crate::content`]).
//!
//! # The matrix is written as the object holds it
//!
//! `TextObject::matrix` stores the glyph matrix with translation excluded, in
//! kurbo's `[a b c d]` layout — the same layout `Tm`'s operands take, and the
//! one the parser reads them into. It is written straight through, with the
//! object's position as the translation. Swapping `b` and `c` here — which
//! the C++ does only to undo its own transposed *storage* — mirrors slanted
//! text about its baseline and turns a rotation the other way, which renders
//! as something almost right: the worst kind of wrong.
//!
//! # A font we cannot classify drops the whole object
//!
//! `Tf` needs a subtype: `Type1`, `TrueType` or `Type0`. A Type 3 font has
//! none of those, and the C++ answers by returning after `q ` and `BT ` are
//! already written, leaving both unclosed. We build into a scratch buffer and
//! discard it, so an unclassifiable font contributes nothing (divergence D6).
//!
//! A constructed object (`font: None`, `font_source: Some`) is the exception.
//! The caller named the dict, so the Type 3 refusal does not apply; size
//! lives in the glyph matrix (`Affine::scale(size)`). We write it as `Tf`
//! and divide it out of `Tm`, matching the stream a parsed object of that
//! size writes. The previous `font: None` refusal was the latent bug that
//! made `TextBuilder` non-functional for new text.

use pdfrum_common::kurbo::Affine;
use pdfrum_font::Font;
use pdfrum_page::{TextObject, TextSegment};
use std::fmt::Write as _;

use crate::content::num::{write_float, write_matrix};
use crate::names;

/// What `Tf` calls a font, or `None` for one no subtype fits.
///
/// Type 3 is the absent case: its glyphs are content streams rather than a
/// program, so it has no `/Subtype` a synthesized font resource could carry.
#[must_use]
pub(crate) fn font_subtype(font: &Font) -> Option<&'static pdfrum_object::Name> {
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
pub(crate) fn emit_text_body(
    out: &mut String,
    text: &TextObject,
    resource: &pdfrum_object::Name,
) -> bool {
    // A parsed object has `font: Some`; a constructed one has `font: None`
    // and `font_source: Some`, with size only in the matrix. Refusing the
    // latter was the latent bug that made `TextBuilder` emit nothing for
    // new text — loading a Helvetica stand-in just to pass the subtype
    // check was the wrong kind of fix. Skip the Type 3 refusal here: the
    // caller named the dict.
    let (size, constructed) = match text.font.as_ref() {
        Some((font, size)) => {
            if font_subtype(font).is_none() {
                return false;
            }
            (*size, false)
        }
        None if text.font_source.is_some() => (matrix_font_size(text.matrix), true),
        None => return false,
    };

    let mut body = String::new();
    body.push_str("BT ");

    // Constructed objects store size in the matrix. Writing that scale as
    // `Tf` and dividing it out of `Tm` matches the parsed-object spelling
    // (`10 Tf` + identity `Tm`) and is cleaner than `Tf 1` with the scale
    // left in the matrix.
    let matrix = text_matrix(text, constructed.then_some(size));
    if matrix != Affine::IDENTITY {
        write_matrix(&mut body, matrix);
        body.push_str(" Tm ");
    }

    body.push('/');
    body.push_str(&String::from_utf8_lossy(&pdfrum_object::name_encode(
        resource.as_bytes(),
    )));
    body.push(' ');
    write_float(&mut body, size);
    body.push_str(" Tf ");
    body.push_str(&render_mode(text).to_string());
    body.push_str(" Tr ");

    emit_show_array(&mut body, &text.segments);
    body.push_str(" TJ ET");

    out.push_str(&body);
    true
}

/// Font size a constructed text object stores in its glyph matrix.
///
/// A constructed object writes `Affine::scale(size)`; the x-axis length of
/// that linear part is the size. Zero if the matrix is degenerate.
fn matrix_font_size(matrix: Affine) -> f32 {
    let c = matrix.as_coeffs();
    let sx = c.first().copied().unwrap_or(0.0);
    let sy = c.get(1).copied().unwrap_or(0.0);
    #[expect(
        clippy::cast_possible_truncation,
        reason = "PDF numbers are f32 (SPEC §2); the geometry vocabulary is f64"
    )]
    {
        sx.hypot(sy) as f32
    }
}

/// The `Tm` operand list.
///
/// The linear part is the stored matrix's, in the order it is stored, and
/// the translation comes from the object's position rather than the matrix —
/// the matrix carries orientation and scale only. `divide_out` is the font
/// size to take out of the linear part for a constructed object, whose
/// matrix *is* that size.
fn text_matrix(text: &TextObject, divide_out: Option<f32>) -> Affine {
    let [a, b, c, d, _, _] = text.matrix.as_coeffs();
    let scale = divide_out
        .map(f64::from)
        .filter(|factor| factor.abs() > f64::EPSILON);
    let divide = |value: f64| scale.map_or(value, |factor| value / factor);
    Affine::new([
        divide(a),
        divide(b),
        divide(c),
        divide(d),
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
            let _ = write!(out, "{byte:02X}");
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
    use pdfrum_object::{Name, ObjRef};
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
            font_source: None,
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

    // The stored layout is `Tm`'s: no swap on the way out, and the position
    // is the translation. A quarter turn stays a quarter turn the same way.
    #[test]
    fn the_text_matrix_is_written_as_stored() {
        let t = text(
            Some(helvetica()),
            // stored a, b, c, d
            Affine::new([2.0, 3.0, 4.0, 5.0, 0.0, 0.0]),
            Point::new(7.0, 8.0),
        );
        let out = emit(&t).expect("emits");
        assert!(out.contains("2 3 4 5 7 8 Tm"), "got {out}");

        // A quarter turn counter-clockwise, as `Affine::rotate` would build
        // it but with exact zeros.
        let turned = text(
            Some(helvetica()),
            Affine::new([0.0, 1.0, -1.0, 0.0, 0.0, 0.0]),
            Point::ZERO,
        );
        let out = emit(&turned).expect("emits");
        assert!(out.contains("0 1 -1 0 0 0 Tm"), "got {out}");
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

    // A constructed object stores size only in the matrix. Writing `Tf` with
    // that size and `Tm` with the scale divided out renders at 24 units;
    // leaving the scale in both, or emitting nothing, would not.
    #[test]
    fn a_constructed_object_emits_tf_tm_at_the_matrix_size() {
        let t = TextObject {
            segments: segments(&[(b"Hi", 0.0)]),
            position: Point::new(20.0, 40.0),
            matrix: Affine::scale(24.0),
            font: None,
            font_source: Some(ObjRef::new(1, 0)),
            render_mode: TextRenderMode::Fill,
            type3_metrics: BTreeMap::new(),
        };
        assert_eq!(
            emit(&t).as_deref(),
            Some("BT 1 0 0 1 20 40 Tm /FXF1 24 Tf 0 Tr [<4869>] TJ ET")
        );
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
