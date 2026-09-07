//! The slice of XMP a PDF/A check needs, read without an XML parser.
//!
//! # Why there is no dependency here
//!
//! says to write the thirty lines rather than take a crate for
//! them, and this is that case. A general XMP reader would need namespaces,
//! entities, RDF's three container forms and its two syntaxes for a property
//! — a real XML stack. What a PDF/A check actually reads is five scalar
//! properties out of a packet that is, by the specification that governs it,
//! `xpacket`-wrapped UTF-8 RDF: `pdfaid:part`, `pdfaid:conformance`,
//! `dc:title`, `dc:creator` and `xmp:CreateDate`. Each appears either as an
//! element (`<pdfaid:part>1</pdfaid:part>`) or as an attribute on an
//! `rdf:Description` (`pdfaid:part="1"`), and both spellings are two dozen
//! lines to read.
//!
//! # What that costs, stated plainly
//!
//! This reader is *lenient where a validator is strict*. It does not check
//! that the RDF is well-formed, that namespace prefixes are bound to the URIs
//! they should be, or that a property appears exactly once. A packet this
//! module reads a `part` out of may still be rejected by a real XMP parser,
//! which is a known and deliberate source of divergence from veraPDF.
//! The
//! direction of the error matters: leniency *here* makes us report fewer
//! violations than veraPDF rather than inventing ones it does not see.

/// The PDF/A identification schema, when the packet carries one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Identification {
    /// `pdfaid:part` — 1, 2, 3 or 4.
    pub(crate) part: u8,
    /// `pdfaid:conformance` — `A`, `B` or `U`, uppercased.
    pub(crate) conformance: Option<char>,
}

/// The `dc:` and `xmp:` properties that must agree with the document
/// information dictionary.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct InfoProperties {
    /// `dc:title`, which mirrors `/Title`.
    pub(crate) title: Option<String>,
    /// The first `dc:creator`, which mirrors `/Author`.
    pub(crate) author: Option<String>,
    /// `xmp:CreatorTool`, which mirrors `/Creator`.
    pub(crate) creator_tool: Option<String>,
    /// `pdf:Producer`, which mirrors `/Producer`.
    pub(crate) producer: Option<String>,
    /// `pdf:Keywords`, which mirrors `/Keywords`.
    pub(crate) keywords: Option<String>,
}

/// A packet that at least looks like XMP.
///
/// The one structural check made: an RDF root. A `/Metadata` stream holding
/// something else entirely — a stray image, a truncated packet — is the
/// [`Clause::XmpMalformed`](super::Clause::XmpMalformed) case, and it has to
/// be distinguished from a well-formed packet that merely lacks the
/// identification schema, because a converter repairs the two differently.
pub(crate) fn is_xmp(packet: &[u8]) -> bool {
    let text = String::from_utf8_lossy(packet);
    text.contains("<rdf:RDF") || text.contains(":RDF")
}

/// Read `pdfaid:part` and `pdfaid:conformance`.
pub(crate) fn identification(packet: &[u8]) -> Option<Identification> {
    let text = String::from_utf8_lossy(packet);
    let part = property(&text, "pdfaid:part")?.trim().parse::<u8>().ok()?;
    let conformance = property(&text, "pdfaid:conformance")
        .and_then(|value| value.trim().chars().next())
        .map(|c| c.to_ascii_uppercase());
    Some(Identification { part, conformance })
}

/// Read the properties that mirror the document information dictionary.
pub(crate) fn info_properties(packet: &[u8]) -> InfoProperties {
    let text = String::from_utf8_lossy(packet);
    InfoProperties {
        // `dc:title` and `dc:creator` are RDF containers rather than scalars
        // — an `rdf:Alt` of language alternatives and an `rdf:Seq` of names.
        // `first_li` takes the first `rdf:li`, which is the `x-default` title
        // and the primary author in every packet a writer produces, and falls
        // back to the scalar spelling some producers emit anyway.
        title: container(&text, "dc:title").or_else(|| property(&text, "dc:title")),
        author: container(&text, "dc:creator").or_else(|| property(&text, "dc:creator")),
        creator_tool: property(&text, "xmp:CreatorTool"),
        producer: property(&text, "pdf:Producer"),
        keywords: property(&text, "pdf:Keywords"),
    }
}

/// One scalar property, in either of the two spellings RDF allows it.
fn property(text: &str, name: &str) -> Option<String> {
    element(text, name).or_else(|| attribute(text, name))
}

/// `<ns:name ...>value</ns:name>`.
///
/// The open tag is matched up to its `>` rather than by exact text, because a
/// property element legitimately carries attributes (`rdf:parseType`,
/// `xml:lang`).
fn element(text: &str, name: &str) -> Option<String> {
    let open = format!("<{name}");
    let start = text.find(&open)?;
    let rest = text.get(start + open.len()..)?;
    // Reject a longer name with this one as a prefix: `<pdfaid:partial>` is
    // not `<pdfaid:part>`.
    if !rest.starts_with(['>', ' ', '\t', '\r', '\n', '/']) {
        return None;
    }
    let body = rest.get(rest.find('>')? + 1..)?;
    let end = body.find(&format!("</{name}>"))?;
    Some(unescape(body.get(..end)?))
}

/// `ns:name="value"` on an `rdf:Description`, the compact RDF form.
fn attribute(text: &str, name: &str) -> Option<String> {
    for quote in ['"', '\''] {
        let needle = format!("{name}={quote}");
        let Some(start) = text.find(&needle) else {
            continue;
        };
        // The character before must not be a name character, or `pdfaid:part`
        // would match inside a hypothetical `x-pdfaid:part`.
        if text
            .get(..start)
            .and_then(|before| before.chars().next_back())
            .is_some_and(|c| c.is_alphanumeric() || c == ':' || c == '-' || c == '_')
        {
            continue;
        }
        let body = text.get(start + needle.len()..)?;
        let end = body.find(quote)?;
        return Some(unescape(body.get(..end)?));
    }
    None
}

/// The first `rdf:li` inside a container property such as `dc:title`.
fn container(text: &str, name: &str) -> Option<String> {
    let inner = element(text, name)?;
    // `element` already unescaped, so the markup inside the container is
    // intact only if it carried none of the five entities. Read the raw
    // region instead for the nested case.
    let raw = raw_element(text, name)?;
    let value = element(raw, "rdf:li")?;
    Some(if value.is_empty() { inner } else { value })
}

/// The unescaped body of an element, for a caller that needs to look inside.
fn raw_element<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let open = format!("<{name}");
    let start = text.find(&open)?;
    let rest = text.get(start + open.len()..)?;
    let body = rest.get(rest.find('>')? + 1..)?;
    let end = body.find(&format!("</{name}>"))?;
    body.get(..end)
}

/// The five predefined XML entities, which are the only ones an XMP packet
/// may use without a DTD.
fn unescape(value: &str) -> String {
    if !value.contains('&') {
        return value.trim().to_owned();
    }
    value
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        // `&amp;` last: doing it first would let `&amp;lt;` become `<`.
        .replace("&amp;", "&")
        .trim()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PACKET: &str = r#"<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about="" xmlns:pdfaid="http://www.aiim.org/pdfa/ns/id/">
   <pdfaid:part>2</pdfaid:part>
   <pdfaid:conformance>B</pdfaid:conformance>
  </rdf:Description>
  <rdf:Description rdf:about="" xmlns:dc="http://purl.org/dc/elements/1.1/">
   <dc:title><rdf:Alt><rdf:li xml:lang="x-default">A &amp; B</rdf:li></rdf:Alt></dc:title>
   <dc:creator><rdf:Seq><rdf:li>Ada</rdf:li></rdf:Seq></dc:creator>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>"#;

    #[test]
    fn reads_the_identification_schema_as_elements() {
        let id = identification(PACKET.as_bytes()).expect("packet carries pdfaid");
        assert_eq!(id.part, 2);
        assert_eq!(id.conformance, Some('B'));
    }

    #[test]
    fn reads_the_identification_schema_as_attributes() {
        let compact =
            r#"<rdf:RDF><rdf:Description pdfaid:part="1" pdfaid:conformance="b"/></rdf:RDF>"#;
        let id = identification(compact.as_bytes()).expect("compact form is read too");
        assert_eq!(id.part, 1);
        // Uppercased, so a caller compares one spelling.
        assert_eq!(id.conformance, Some('B'));
    }

    #[test]
    fn unwraps_containers_and_entities() {
        let props = info_properties(PACKET.as_bytes());
        assert_eq!(props.title.as_deref(), Some("A & B"));
        assert_eq!(props.author.as_deref(), Some("Ada"));
    }

    #[test]
    fn a_longer_name_with_the_same_prefix_does_not_match() {
        let decoy = r"<rdf:RDF><pdfaid:partial>9</pdfaid:partial></rdf:RDF>";
        assert!(identification(decoy.as_bytes()).is_none());
    }

    #[test]
    fn a_packet_that_is_not_rdf_is_not_xmp() {
        assert!(is_xmp(PACKET.as_bytes()));
        assert!(!is_xmp(b"\x89PNG\r\n\x1a\n"));
    }
}
