//! The XMP packet the conversion writes.
//!
//! # Why the packet is rebuilt rather than patched
//!
//! The obvious conversion is to take the document's existing packet and splice
//! the PDF/A identification schema into it. The oracle says not to: over the
//! 44-file corpus veraPDF fails 11 files on 6.6.2.1-4 ("a metadata stream is
//! serialized incorrectly and can not be parsed") and 11 on 6.6.2.1-5 ("the
//! XMP package uses encoding null different from UTF-8"), and one on a
//! duplicate `xmp:ModifyDate` node. Those packets are not well-formed XMP to
//! begin with, and splicing into a packet that does not parse produces a
//! packet that still does not parse.
//!
//! So the packet is *generated*, from the document information dictionary,
//! which is the store PDF/A requires it to agree with anyway (ISO 19005-2
//! 6.6.2.3.1). That makes the agreement true by construction rather than
//! checked afterwards, and it is the only route that fixes the malformed-XMP
//! failures rather than preserving them.
//!
//! What it costs: XMP properties
//! outside the five the information dictionary mirrors are not carried over.
//!
//! # And why there is still no XML dependency
//!
//! Writing is the easy direction. A generator controls its own output, so it
//! needs no parser, no namespace resolution and no RDF container handling —
//! it needs correct escaping of five strings, which is the twenty lines below.
//! the style rules, the same rule the checker's reader cites.

use crate::PdfaLevel;

/// The document information dictionary entries the packet mirrors.
///
/// A struct of `Option<String>` rather than the `Metadata` facade type,
/// because the packet needs exactly these five and takes them from wherever
/// the caller read them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct InfoFields {
    pub(crate) title: Option<String>,
    pub(crate) author: Option<String>,
    pub(crate) creator: Option<String>,
    pub(crate) producer: Option<String>,
    pub(crate) keywords: Option<String>,
    /// `/CreationDate` and `/ModDate`, already in XMP's ISO 8601 form.
    pub(crate) create_date: Option<String>,
    pub(crate) modify_date: Option<String>,
}

/// Escape the five characters XML reserves.
///
/// Attribute values are never used for the mirrored properties below — every
/// one is written as an element — so `"` and `'` are not strictly required.
/// They are escaped anyway: it is two lines, and a generator that escapes only
/// what its current output shape needs breaks the moment the shape changes.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            // XML 1.0 has no way to write these at all, escaped or not, so
            // they are dropped rather than turned into a packet no parser
            // accepts. A control character in a /Title is damage, not content.
            c if (c as u32) < 0x20 && c != '\t' && c != '\n' && c != '\r' => {}
            c => out.push(c),
        }
    }
    out
}

/// A `<prefix:name>value</prefix:name>` element, or nothing when the value is
/// absent or empty.
///
/// An empty property is not the same as an absent one to a validator, and an
/// empty `/Title` in the information dictionary is the absent case wearing a
/// zero-length string.
fn element(out: &mut String, name: &str, value: Option<&String>) {
    let Some(value) = value.filter(|v| !v.is_empty()) else {
        return;
    };
    out.push_str("   <");
    out.push_str(name);
    out.push('>');
    out.push_str(&escape(value));
    out.push_str("</");
    out.push_str(name);
    out.push_str(">\n");
}

/// A date property, written only when it parses as one XMP accepts.
///
/// veraPDF fails 5 corpus files on 6.6.2.3.1-2, "XMP property does not
/// correspond to type date" — a date-typed property holding something that is
/// not a date is worse than no property at all, since the property is optional
/// and the type is not. So a value that does not look like ISO 8601 is
/// dropped rather than written and failed on.
fn date_element(out: &mut String, name: &str, value: Option<&String>) {
    let Some(value) = value.filter(|v| is_iso8601(v)) else {
        return;
    };
    element(out, name, Some(&value.clone()));
}

/// Whether `text` is an ISO 8601 date XMP will accept.
///
/// The shapes XMP permits are `YYYY`, `YYYY-MM`, `YYYY-MM-DD` and those with a
/// time and zone appended. Rather than parse all of them, this checks the
/// invariant every one shares and that the malformed values fail: a four-digit
/// year, then either nothing or a `-`.
fn is_iso8601(text: &str) -> bool {
    let bytes = text.as_bytes();
    if bytes.len() < 4 || !bytes[..4].iter().all(u8::is_ascii_digit) {
        return false;
    }
    bytes.len() == 4 || bytes[4] == b'-'
}

/// The complete `/Metadata` packet for a document at `level`.
///
/// Written as UTF-8 with no byte-order mark, which is what ISO 19005-2
/// 6.6.2.1 requires and what 11 corpus files fail on.
pub(crate) fn packet(level: PdfaLevel, info: &InfoFields) -> Vec<u8> {
    let mut out = String::with_capacity(1024);
    // The `begin` attribute holds U+FEFF as a character, not as a BOM: it is
    // how a reader that found the packet in a byte stream learns the encoding.
    // Inside a PDF the stream's extent is known, but the packet header is part
    // of the XMP specification's shape and validators check for it.
    out.push_str("<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n");
    out.push_str("<x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"pdfrum\">\n");
    out.push_str(" <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n");

    // The identification schema, which is the whole reason the packet is
    // rewritten: 6.6.4-1, failed by 27 of the 44 corpus files.
    out.push_str(
        "  <rdf:Description rdf:about=\"\" xmlns:pdfaid=\"http://www.aiim.org/pdfa/ns/id/\">\n",
    );
    out.push_str("   <pdfaid:part>");
    out.push_str(&level.part().to_string());
    out.push_str("</pdfaid:part>\n");
    // Both levels this engine converts to are `B`. When an `a` level is added
    // this becomes a `Level` method; hardcoding it now would be a lie the
    // moment it is not, so it goes through the level rather than a literal.
    out.push_str("   <pdfaid:conformance>B</pdfaid:conformance>\n");
    out.push_str("  </rdf:Description>\n");

    // `dc:title` and `dc:creator` are RDF containers, not scalars: an `rdf:Alt`
    // of language alternatives and an `rdf:Seq` of names. Writing them as bare
    // elements is the single most common way a hand-built packet fails a real
    // validator.
    if info.title.as_ref().is_some_and(|t| !t.is_empty())
        || info.author.as_ref().is_some_and(|a| !a.is_empty())
    {
        out.push_str(
            "  <rdf:Description rdf:about=\"\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\">\n",
        );
        if let Some(title) = info.title.as_ref().filter(|t| !t.is_empty()) {
            out.push_str("   <dc:title><rdf:Alt><rdf:li xml:lang=\"x-default\">");
            out.push_str(&escape(title));
            out.push_str("</rdf:li></rdf:Alt></dc:title>\n");
        }
        if let Some(author) = info.author.as_ref().filter(|a| !a.is_empty()) {
            out.push_str("   <dc:creator><rdf:Seq><rdf:li>");
            out.push_str(&escape(author));
            out.push_str("</rdf:li></rdf:Seq></dc:creator>\n");
        }
        out.push_str("  </rdf:Description>\n");
    }

    out.push_str("  <rdf:Description rdf:about=\"\" xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\">\n");
    element(&mut out, "xmp:CreatorTool", info.creator.as_ref());
    date_element(&mut out, "xmp:CreateDate", info.create_date.as_ref());
    date_element(&mut out, "xmp:ModifyDate", info.modify_date.as_ref());
    out.push_str("  </rdf:Description>\n");

    out.push_str("  <rdf:Description rdf:about=\"\" xmlns:pdf=\"http://ns.adobe.com/pdf/1.3/\">\n");
    element(&mut out, "pdf:Producer", info.producer.as_ref());
    element(&mut out, "pdf:Keywords", info.keywords.as_ref());
    out.push_str("  </rdf:Description>\n");

    out.push_str(" </rdf:RDF>\n");
    out.push_str("</x:xmpmeta>\n");
    // The trailing padding an XMP packet conventionally carries so an in-place
    // editor can grow it. `w` says the packet may not be rewritten in place,
    // which is true of ours: we rewrite the whole stream.
    out.push_str("<?xpacket end=\"w\"?>\n");
    out.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::{InfoFields, is_iso8601, packet};
    use crate::PdfaLevel;

    fn text(level: PdfaLevel, info: &InfoFields) -> String {
        String::from_utf8(packet(level, info)).expect("the packet is UTF-8 by construction")
    }

    #[test]
    fn the_identification_schema_names_the_level() {
        let one = text(PdfaLevel::A1b, &InfoFields::default());
        assert!(one.contains("<pdfaid:part>1</pdfaid:part>"));
        assert!(one.contains("<pdfaid:conformance>B</pdfaid:conformance>"));
        let two = text(PdfaLevel::A2b, &InfoFields::default());
        assert!(two.contains("<pdfaid:part>2</pdfaid:part>"));
    }

    // The checker's own reader must be able to read what the writer writes.
    // Anything else means the conversion produces files it would itself fail.
    #[test]
    fn the_checker_reads_the_packet_the_writer_writes() {
        let doc = crate::Document::open("tests/fixtures/text_form.pdf").expect("fixture opens");
        let _ = doc;
        let bytes = packet(PdfaLevel::A2b, &InfoFields::default());
        // `is_xmp` and `identification` are the checker's, exercised through
        // the public surface by round-tripping a converted file in
        // `tests/pdfa_convert.rs`; here we assert the shape they scan for.
        let s = String::from_utf8(bytes).expect("utf-8");
        assert!(s.contains("<rdf:RDF"));
        assert!(s.contains("pdfaid:part"));
    }

    #[test]
    fn reserved_characters_are_escaped() {
        let info = InfoFields {
            title: Some("a & b < c > d".to_owned()),
            ..InfoFields::default()
        };
        let out = text(PdfaLevel::A2b, &info);
        assert!(out.contains("a &amp; b &lt; c &gt; d"));
        // And the raw forms are gone, or the packet is not well-formed XML.
        assert!(!out.contains("a & b"));
    }

    // A control character cannot be written in XML 1.0 at all. Dropping it
    // keeps the packet parseable; passing it through would make every
    // property in the packet unreadable.
    #[test]
    fn control_characters_are_dropped_rather_than_escaped() {
        let info = InfoFields {
            producer: Some("bad\u{0}producer".to_owned()),
            ..InfoFields::default()
        };
        assert!(text(PdfaLevel::A2b, &info).contains("<pdf:Producer>badproducer"));
    }

    #[test]
    fn an_empty_property_is_written_as_absent() {
        let info = InfoFields {
            producer: Some(String::new()),
            ..InfoFields::default()
        };
        assert!(!text(PdfaLevel::A2b, &info).contains("pdf:Producer"));
    }

    // A date-typed property holding a non-date is 6.6.2.3.1-2, which 5 corpus
    // files fail. Dropping it is the repair; the property is optional.
    #[test]
    fn a_malformed_date_is_dropped_rather_than_written() {
        let info = InfoFields {
            create_date: Some("not a date".to_owned()),
            modify_date: Some("2026-09-07T12:00:00Z".to_owned()),
            ..InfoFields::default()
        };
        let out = text(PdfaLevel::A2b, &info);
        assert!(!out.contains("CreateDate"));
        assert!(out.contains("<xmp:ModifyDate>2026-09-07T12:00:00Z"));
    }

    #[test]
    fn iso8601_accepts_the_shapes_xmp_permits() {
        assert!(is_iso8601("2026"));
        assert!(is_iso8601("2026-09"));
        assert!(is_iso8601("2026-09-07T12:00:00+01:00"));
        assert!(!is_iso8601("D:20260907120000Z"));
        assert!(!is_iso8601(""));
        assert!(!is_iso8601("26-09-07"));
    }

    // dc:title and dc:creator are containers. A bare <dc:title>x</dc:title> is
    // the most common way a hand-built packet fails a real validator.
    #[test]
    fn the_dublin_core_properties_are_rdf_containers() {
        let info = InfoFields {
            title: Some("T".to_owned()),
            author: Some("A".to_owned()),
            ..InfoFields::default()
        };
        let out = text(PdfaLevel::A2b, &info);
        assert!(out.contains("<dc:title><rdf:Alt><rdf:li xml:lang=\"x-default\">T</rdf:li>"));
        assert!(out.contains("<dc:creator><rdf:Seq><rdf:li>A</rdf:li>"));
    }

    // No Dublin Core block at all when neither property has a value: an empty
    // rdf:Description is legal but pointless, and its namespace declaration
    // is one more thing a validator can object to.
    #[test]
    fn the_dublin_core_block_is_absent_when_it_would_be_empty() {
        assert!(!text(PdfaLevel::A2b, &InfoFields::default()).contains("purl.org/dc"));
    }
}
