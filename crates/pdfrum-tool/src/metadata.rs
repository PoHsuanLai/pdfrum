//! `--show-metadata`: the document information dictionary (ISO 32000-1 §14.3.3).
//!
//! The oracle prints one line per tag it can read:
//!
//! ```text
//! Title        = Untitled (18 bytes)
//! Producer     = Skia/PDF m69 (26 bytes)
//! ```
//!
//! Three things about that line are not obvious and all three are load-bearing:
//!
//! - **The byte count is the C API's, not the text's.** It is the size of a
//!   UTF-16LE encoding *including* its two-byte terminator, so an empty value
//!   is `(2 bytes)` and a three-character CJK title is `(8 bytes)`. A
//!   character outside the BMP counts as two units.
//! - **An absent key still prints.** Only a document with no readable `/Info`
//!   dictionary at all suppresses a tag; a dictionary that simply lacks
//!   `/Title` prints `Title        =  (2 bytes)`. That distinction is exactly
//!   the difference between an empty golden and an eight-line one, so it is
//!   the first thing to get right.
//! - **The value is not necessarily a string.** A `/Name` is decoded like one,
//!   and a stream is decoded from its data; numbers, arrays and dictionaries
//!   read as empty.

use pdfrum_object::{Dict, Name, Object, Resolve, names};

/// The eight tags, in the order the oracle asks for them.
///
/// Order is part of the output: the dump prints them in this sequence, not in
/// the dictionary's own.
pub const TAGS: [&Name; 8] = [
    names::TITLE,
    names::AUTHOR,
    names::SUBJECT,
    names::KEYWORDS,
    names::CREATOR,
    names::PRODUCER,
    names::CREATION_DATE,
    names::MOD_DATE,
];

/// One tag's reading: the text to print and the byte count to report.
///
/// The two can disagree. A value carrying an embedded U+0000 — which
/// `PDFDocEncoding` produces for bytes `0x7F`, `0x92` and `0xA0` — is printed
/// only up to that point, while the count covers the whole string, because
/// the oracle measures the encoded buffer and then prints it as a
/// NUL-terminated C string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetaText {
    /// The UTF-16 code units the value encodes to, terminator excluded.
    pub units: Vec<u16>,
}

impl MetaText {
    /// The `(N bytes)` figure: code units plus the terminator, two bytes each.
    pub fn byte_len(&self) -> usize {
        (self.units.len() + 1) * 2
    }

    /// The units up to the first NUL — what `%ls` of a C string prints.
    pub fn displayed(&self) -> &[u16] {
        let end = self
            .units
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(self.units.len());
        self.units.get(..end).unwrap_or_default()
    }
}

/// The `/Info` dictionary, if the document has one the oracle would use.
///
/// Deliberately strict in the same two ways the C++ is: the trailer's `/Info`
/// must be an **indirect reference** (a dictionary written inline in the
/// trailer is not found), and its target must be a dictionary. Either failure
/// means the whole dump is empty rather than eight empty lines.
pub fn info_dict(trailer: &Dict, r: &impl Resolve) -> Option<Dict> {
    let reference = trailer.reference(names::INFO)?;
    r.fetch(reference).ok()?.as_dict().cloned()
}

/// Reads one tag out of an information dictionary.
///
/// Follows one level of indirection, then decodes by type: strings and names
/// through the text-string rules, everything else as empty. A reference whose
/// target is itself a reference reads as empty, which is the C++'s
/// single-level resolution showing through.
pub fn read_tag(info: &Dict, tag: &Name, r: &impl Resolve) -> MetaText {
    // `Object::to_text` is exactly the C++'s `GetUnicodeText` for every type
    // that matters here: strings and names decode through the text-string
    // rules, everything else answers empty. The one divergence is a stream,
    // whose decoded data the C++ would read; running the filter chain for a
    // value no corpus file uses is not worth the dependency, and the status
    // doc records it rather than hiding it.
    let text = match info.raw(tag) {
        None => String::new(),
        // One level only: an indirect object whose body is another reference
        // reads as empty, which is what the C++'s `GetDirectInternal` does.
        Some(Object::Ref(reference)) => {
            r.fetch(*reference).map(|o| o.to_text()).unwrap_or_default()
        }
        Some(object) => object.to_text(),
    };
    MetaText {
        units: text.encode_utf16().collect(),
    }
}

/// Renders the whole dump for a document, or nothing when `/Info` is unusable.
pub fn render(trailer: &Dict, r: &impl Resolve) -> String {
    let Some(info) = info_dict(trailer, r) else {
        return String::new();
    };
    let mut out = String::new();
    for tag in TAGS {
        let value = read_tag(&info, tag, r);
        let Some(rendered) = render_line(tag, &value) else {
            // A surrogate in the buffer stops the C library's `%ls`
            // conversion mid-line and the rest of the dump never runs.
            break;
        };
        out.push_str(&rendered);
    }
    out
}

/// One `Tag          = value (N bytes)` line.
///
/// Returns `None` when the value holds an unpaired surrogate. The oracle
/// hands the buffer to `printf("%ls")`, whose UTF-8 conversion refuses a
/// surrogate code point: output stops after the `= ` it has already written,
/// no newline is produced, and the remaining tags are never reached. Nothing
/// in the corpus triggers this, but a dump that quietly printed a replacement
/// character instead would be wrong in a way Tier A could not see.
fn render_line(tag: &Name, value: &MetaText) -> Option<String> {
    let name = tag.as_str()?;
    let text = String::from_utf16(value.displayed()).ok()?;
    Some(format!(
        "{name:<12} = {text} ({} bytes)\n",
        value.byte_len()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdfrum_object::{Array, NoResolve, ObjRef, PdfString};

    fn info_with(pairs: impl IntoIterator<Item = (&'static str, Object)>) -> Dict {
        Dict::from_pairs(
            pairs
                .into_iter()
                .map(|(key, value)| (Name::from(key), value)),
        )
    }

    fn text_of(info: &Dict, key: &str) -> MetaText {
        read_tag(info, &Name::from(key), &NoResolve)
    }

    #[test]
    fn the_byte_count_includes_the_utf16_terminator() {
        // "Untitled" is 8 code units: (8 + 1) * 2 = 18, exactly what the
        // oracle prints for testing/resources/bug_451265.pdf.
        let info = info_with([("Title", Object::Str(PdfString::literal(b"Untitled")))]);
        assert_eq!(text_of(&info, "Title").byte_len(), 18);
    }

    #[test]
    fn an_absent_key_still_reports_two_bytes() {
        // The empty-but-present line is what distinguishes "no Info dict"
        // (nothing printed) from "Info dict without this key".
        let info = info_with([]);
        let value = text_of(&info, "Title");
        assert!(value.units.is_empty());
        assert_eq!(value.byte_len(), 2);
    }

    #[test]
    fn a_supplementary_character_counts_as_two_units() {
        let info = info_with([(
            "Title",
            Object::Str(PdfString::hex(b"\xFE\xFF\xD8\x3D\xDE\x00")),
        )]);
        // U+1F600 is one scalar but two UTF-16 units: (2 + 1) * 2 = 6.
        assert_eq!(text_of(&info, "Title").byte_len(), 6);
    }

    #[test]
    fn a_name_is_decoded_like_a_string() {
        // `/Title /Hello` prints `Hello`, not nothing -- CPDF_Name overrides
        // GetUnicodeText the same way CPDF_String does.
        let info = info_with([("Title", Object::Name(Name::from("Hello")))]);
        let value = text_of(&info, "Title");
        assert_eq!(String::from_utf16(&value.units).unwrap(), "Hello");
        assert_eq!(value.byte_len(), 12);
    }

    #[test]
    fn numbers_arrays_and_dictionaries_read_as_empty() {
        let info = info_with([
            ("Title", Object::Int(42)),
            ("Author", Object::Array(Array::of([Object::Int(1)]))),
            ("Subject", Object::Dict(Dict::new())),
            ("Keywords", Object::Bool(true)),
        ]);
        for key in ["Title", "Author", "Subject", "Keywords"] {
            assert_eq!(text_of(&info, key).byte_len(), 2, "{key}");
        }
    }

    #[test]
    fn a_utf16be_bom_is_decoded_and_the_bom_is_not_counted() {
        // FE FF then U+672A U+547D U+540D: three units, (3 + 1) * 2 = 8,
        // matching the corpus golden for bug_182.pdf's neighbours.
        let info = info_with([(
            "Title",
            Object::Str(PdfString::hex(b"\xFE\xFF\x67\x2A\x54\x7D\x54\x0D")),
        )]);
        let value = text_of(&info, "Title");
        assert_eq!(value.byte_len(), 8);
        assert_eq!(String::from_utf16(&value.units).unwrap(), "未命名");
    }

    #[test]
    fn the_display_string_stops_at_an_embedded_nul() {
        // PDFDocEncoding maps 0x7F to U+0000, so the count and the printed
        // text legitimately disagree.
        let value = MetaText {
            units: vec![0x41, 0x0000, 0x42],
        };
        assert_eq!(value.displayed(), [0x41]);
        assert_eq!(value.byte_len(), 8);
    }

    #[test]
    fn a_document_with_no_info_reference_dumps_nothing() {
        let trailer = Dict::from_pairs([(names::ROOT.clone(), Object::Ref(ObjRef::new(1, 0)))]);
        assert_eq!(render(&trailer, &NoResolve), "");
    }

    #[test]
    fn an_inline_info_dictionary_is_not_found() {
        // The C++ reads the trailer entry through `ToReference`, so a
        // dictionary written directly into the trailer yields no Info at all.
        let trailer = Dict::from_pairs([(
            names::INFO.clone(),
            Object::Dict(info_with([(
                "Title",
                Object::Str(PdfString::literal(b"x")),
            )])),
        )]);
        assert_eq!(render(&trailer, &NoResolve), "");
    }

    #[test]
    fn every_tag_is_rendered_in_the_oracles_order() {
        let names: Vec<&str> = TAGS.iter().filter_map(|t| t.as_str()).collect();
        assert_eq!(
            names,
            [
                "Title",
                "Author",
                "Subject",
                "Keywords",
                "Creator",
                "Producer",
                "CreationDate",
                "ModDate"
            ]
        );
    }

    #[test]
    fn a_line_pads_the_tag_to_twelve_columns() {
        let value = MetaText {
            units: "Skia/PDF m69".encode_utf16().collect(),
        };
        assert_eq!(
            render_line(names::PRODUCER, &value).unwrap(),
            "Producer     = Skia/PDF m69 (26 bytes)\n"
        );
        // `CreationDate` is exactly twelve characters, so no padding is added
        // and the separator still lands in the same column.
        let stamp = MetaText {
            units: "D:20180912174625+00'00'".encode_utf16().collect(),
        };
        assert_eq!(
            render_line(names::CREATION_DATE, &stamp).unwrap(),
            "CreationDate = D:20180912174625+00'00' (48 bytes)\n"
        );
    }

    #[test]
    fn an_unpaired_surrogate_stops_the_dump() {
        let value = MetaText {
            units: vec![0xD800],
        };
        assert_eq!(render_line(names::TITLE, &value), None);
    }
}
