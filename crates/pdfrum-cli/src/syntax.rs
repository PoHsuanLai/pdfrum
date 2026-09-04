//! Objects in PDF syntax, for a person: `inspect object` prints them and
//! `hash` digests them.
//!
//! The spelling is canonical rather than the file's: names `#`-escaped
//! where they must be, strings as `(…)` with the four escapes or `<…>`
//! when the file used hex, reals with the shortest round-tripping digits,
//! dictionaries in the file's own key order. Two documents that mean the
//! same thing print the same, which is what a semantic hash needs.

use std::fmt::Write;

use pdfrum::{Array, Dict, Name, Object, PdfString, Stream};

/// `object` as PDF syntax, nested structures indented by `depth`.
pub fn object(out: &mut String, object: &Object, depth: usize) {
    match object {
        Object::Null => out.push_str("null"),
        Object::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Object::Int(i) => {
            let _ = write!(out, "{i}");
        }
        Object::Real(r) => real(out, *r),
        Object::Str(s) => string(out, s),
        Object::Name(n) => name(out, n),
        Object::Array(a) => array(out, a, depth),
        Object::Dict(d) => dict(out, d, depth),
        Object::Stream(s) => stream(out, s, depth),
        Object::Ref(r) => {
            let _ = write!(out, "{} {} R", r.num, r.generation);
        }
    }
}

/// `dict` with each key once, the last value the file gave it: what a
/// reader sees. The parser keeps duplicate keys as the file wrote them —
/// and the merged trailer of a revision chain is one such dictionary — so
/// a printer and a digest ask for the effective view.
pub fn effective(dict: &Dict) -> Dict {
    let mut out = Dict::from_pairs([]);
    for (key, value) in dict.iter() {
        out.insert(key.clone(), value.clone());
    }
    out
}

fn real(out: &mut String, r: f32) {
    if r.is_finite() && r.fract() == 0.0 && r.abs() < 1e9 {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "whole and below 1e9: every such f32 is an exact i64"
        )]
        let whole = r as i64;
        let _ = write!(out, "{whole}");
    } else {
        let _ = write!(out, "{r}");
    }
}

fn name(out: &mut String, n: &Name) {
    out.push('/');
    for &b in n.as_bytes() {
        if b.is_ascii_graphic()
            && !matches!(
                b,
                b'#' | b'/' | b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'%'
            )
        {
            out.push(char::from(b));
        } else {
            let _ = write!(out, "#{b:02X}");
        }
    }
}

fn string(out: &mut String, s: &PdfString) {
    if s.hex {
        out.push('<');
        for b in &s.bytes {
            let _ = write!(out, "{b:02X}");
        }
        out.push('>');
        return;
    }
    out.push('(');
    for &b in &s.bytes {
        match b {
            b'(' | b')' | b'\\' => {
                out.push('\\');
                out.push(char::from(b));
            }
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            b'\t' => out.push_str("\\t"),
            0x20..=0x7e => out.push(char::from(b)),
            other => {
                let _ = write!(out, "\\{other:03o}");
            }
        }
    }
    out.push(')');
}

fn array(out: &mut String, a: &Array, depth: usize) {
    out.push('[');
    for (i, item) in a.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        object(out, item, depth + 1);
    }
    out.push(']');
}

fn dict(out: &mut String, d: &Dict, depth: usize) {
    if d.is_empty() {
        out.push_str("<< >>");
        return;
    }
    let pad = "  ".repeat(depth + 1);
    out.push_str("<<\n");
    for (key, value) in d.iter() {
        out.push_str(&pad);
        name(out, key);
        out.push(' ');
        object(out, value, depth + 1);
        out.push('\n');
    }
    out.push_str(&"  ".repeat(depth));
    out.push_str(">>");
}

fn stream(out: &mut String, s: &Stream, depth: usize) {
    dict(out, &s.dict, depth);
    let _ = write!(out, "\nstream … {} bytes … endstream", s.data.len());
}

#[cfg(test)]
mod tests {
    use super::object;
    use pdfrum::{Array, Dict, Name, ObjRef, Object, PdfString};

    #[test]
    fn every_kind_prints_in_pdf_syntax() {
        let mut out = String::new();
        let dict = Dict::from_pairs([
            (Name::from("Type"), Object::Name(Name::from("Page"))),
            (Name::from("Count"), Object::Int(3)),
            (Name::from("Scale"), Object::Real(0.5)),
            (Name::from("Whole"), Object::Real(2.0)),
            (
                Name::from("Title"),
                Object::Str(PdfString::literal(b"a(b)\\c\n")),
            ),
            (Name::from("ID"), Object::Str(PdfString::hex(b"\x01\xff"))),
            (Name::from("Odd name"), Object::Null),
            (
                Name::from("Kids"),
                Object::Array(Array::from_iter([
                    Object::Ref(ObjRef::new(4, 0)),
                    Object::Bool(true),
                ])),
            ),
        ]);
        object(&mut out, &Object::Dict(dict), 0);
        assert_eq!(
            out,
            "<<\n  /Type /Page\n  /Count 3\n  /Scale 0.5\n  /Whole 2\n  /Title (a\\(b\\)\\\\c\\n)\n  /ID <01FF>\n  /Odd#20name null\n  /Kids [4 0 R true]\n>>"
        );
    }
}
