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

use crate::term::{Style, Term};

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

/// What an object is, in a few words, for a hint beside a reference or a
/// column in the cross-reference table: a dictionary's `/Type` (and
/// `/Subtype`), a stream's filter and length, an array's length, or the
/// scalar itself.
pub fn describe(value: &Object) -> String {
    match value {
        Object::Dict(d) => typed(d).unwrap_or_else(|| format!("dict, {} keys", d.len())),
        Object::Stream(s) => {
            let filter = s.dict.raw(&Name::from("Filter")).map_or_else(
                || "raw".to_owned(),
                |f| {
                    let mut text = String::new();
                    match f {
                        Object::Array(a) => {
                            for (i, item) in a.iter().enumerate() {
                                if i > 0 {
                                    text.push(' ');
                                }
                                object_text(&mut text, item);
                            }
                        }
                        other => object_text(&mut text, other),
                    }
                    text
                },
            );
            let kind = typed(&s.dict).map_or(String::new(), |t| format!("{t}, "));
            format!(
                "{kind}stream {filter} {}",
                crate::out::bytes(s.data.len() as u64)
            )
        }
        Object::Array(a) => format!("array of {}", a.len()),
        Object::Ref(r) => format!("{} {} R", r.num, r.generation),
        scalar => {
            let mut text = String::new();
            object(&mut text, scalar, 0);
            text
        }
    }
}

/// `/Type` and `/Subtype` of a dictionary, as `Page` or `Font Type1`.
fn typed(d: &Dict) -> Option<String> {
    let name_of = |key: &str| match d.raw(&Name::from(key)) {
        Some(Object::Name(n)) => Some(String::from_utf8_lossy(n.as_bytes()).into_owned()),
        _ => None,
    };
    match (name_of("Type"), name_of("Subtype")) {
        (Some(t), Some(st)) => Some(format!("{t} {st}")),
        (Some(t), None) => Some(t),
        (None, Some(st)) => Some(st),
        (None, None) => None,
    }
}

fn object_text(out: &mut String, o: &Object) {
    object(out, o, 0);
}

/// The canonical dump, painted: dictionary keys in `Key`, names and
/// object references in `Ident`, the framing keywords in `Heading`, a
/// `% …` hint in `Muted`. The text itself is unchanged, so a pipe sees
/// exactly what [`object`] wrote.
pub fn highlight(text: &str, term: Term) -> String {
    if !term.color {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len() * 2);
    for (i, line) in text.lines().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let (code, hint) = match line.find("  % ") {
            Some(at) => (&line[..at], Some(&line[at..])),
            None => (line, None),
        };
        paint_line(&mut out, code, term);
        if let Some(h) = hint {
            out.push_str(&term.paint(Style::Muted, h));
        }
    }
    out
}

fn paint_line(out: &mut String, line: &str, term: Term) {
    let trimmed = line.trim_start();
    let indent = &line[..line.len() - trimmed.len()];
    out.push_str(indent);
    if matches!(trimmed, "endobj" | "trailer")
        || trimmed.ends_with(" obj")
        || trimmed.starts_with("stream ")
    {
        out.push_str(&term.paint(Style::Heading, trimmed));
        return;
    }
    // A dictionary entry: the key, then the value.
    let (key, rest) = if trimmed.starts_with('/') {
        match trimmed.find(' ') {
            Some(at) => (Some(&trimmed[..at]), &trimmed[at..]),
            None => (Some(trimmed), ""),
        }
    } else {
        (None, trimmed)
    };
    if let Some(k) = key {
        out.push_str(&term.paint(Style::Key, k));
    }
    paint_tokens(out, rest, term);
}

/// Names and `N G R` references in `Ident`; everything else as it is.
fn paint_tokens(out: &mut String, text: &str, term: Term) {
    let tokens: Vec<&str> = text.split(' ').collect();
    let mut i = 0;
    while i < tokens.len() {
        if i > 0 {
            out.push(' ');
        }
        let t = tokens[i];
        let is_int = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
        let opened = &t[..t.len() - t.trim_start_matches('[').len()];
        let num = &t[opened.len()..];
        if i + 2 < tokens.len()
            && is_int(num)
            && is_int(tokens[i + 1])
            && (tokens[i + 2] == "R" || tokens[i + 2].starts_with("R]"))
        {
            let r = format!("{num} {} R", tokens[i + 1]);
            out.push_str(opened);
            out.push_str(&term.paint(Style::Ident, &r));
            out.push_str(&tokens[i + 2][1..]);
            i += 3;
            continue;
        }
        let bare = t.trim_start_matches('[').trim_end_matches(']');
        if bare.starts_with('/') {
            let lead = &t[..t.len() - t.trim_start_matches('[').len()];
            let trail = &t[t.trim_end_matches(']').len()..];
            out.push_str(lead);
            out.push_str(&term.paint(Style::Ident, bare));
            out.push_str(trail);
        } else {
            out.push_str(t);
        }
        i += 1;
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
    use super::{describe, highlight, object};
    use crate::term::{Graphics, Term};
    use pdfrum::{Array, Dict, Name, ObjRef, Object, PdfString};

    #[test]
    fn highlighting_paints_keys_names_and_references_and_leaves_the_text_alone() {
        let term = Term {
            color: true,
            hyperlinks: false,
            graphics: Graphics::Off,
            interactive: false,
        };
        let text = "1 0 obj\n<<\n  /Type /Catalog\n  /Pages 2 0 R  % Pages\n  /Kids [4 0 R 5 0 R]\n>>\nendobj";
        let painted = highlight(text, term);
        assert!(painted.contains("\x1b[1m1 0 obj\x1b[0m"), "{painted}");
        assert!(
            painted.contains("\x1b[2m/Type\x1b[0m \x1b[36m/Catalog\x1b[0m"),
            "{painted}"
        );
        assert!(
            painted.contains("\x1b[36m2 0 R\x1b[0m\x1b[2m  % Pages\x1b[0m"),
            "{painted}"
        );
        assert!(
            painted.contains("[\x1b[36m4 0 R\x1b[0m \x1b[36m5 0 R\x1b[0m]"),
            "{painted}"
        );
        let plain = Term {
            color: false,
            ..term
        };
        assert_eq!(highlight(text, plain), text);
    }

    #[test]
    fn describe_names_what_a_reference_points_at() {
        let page = Dict::from_pairs([(Name::from("Type"), Object::Name(Name::from("Page")))]);
        assert_eq!(describe(&Object::Dict(page)), "Page");
        let font = Dict::from_pairs([
            (Name::from("Type"), Object::Name(Name::from("Font"))),
            (Name::from("Subtype"), Object::Name(Name::from("Type1"))),
        ]);
        assert_eq!(describe(&Object::Dict(font)), "Font Type1");
        assert_eq!(
            describe(&Object::Array(Array::from_iter([Object::Int(1)]))),
            "array of 1"
        );
        assert_eq!(describe(&Object::Int(7)), "7");
    }

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
