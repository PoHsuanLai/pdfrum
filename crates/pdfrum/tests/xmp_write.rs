//! XMP metadata written and read back through `Document::xmp_metadata`.

#![expect(
    clippy::expect_used,
    reason = "helpers shared by the tests below; a panic here is a failure"
)]

use std::sync::Arc;

use pdfrum::{Document, SaveOptions};

const HELLO: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/hello_world.pdf"
);

const PACKET: &[u8] = br#"<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF
 xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description
 rdf:about="" xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title><rdf:Alt>
 <rdf:li xml:lang="x-default">Written by pdfrum</rdf:li></rdf:Alt></dc:title>
 </rdf:Description></rdf:RDF></x:xmpmeta>
<?xpacket end="w"?>"#;

/// Saves `edit` and reloads the result.
fn roundtrip(edit: &mut pdfrum::DocEdit<'_>) -> Document {
    let mut out = Vec::new();
    edit.write_to(&mut out, &SaveOptions::default())
        .expect("save");
    Document::from_bytes(Arc::from(out)).expect("reopen")
}

#[test]
fn a_packet_round_trips_byte_for_byte() {
    let doc = Document::open(HELLO).expect("open");
    assert!(doc.xmp_metadata().is_none(), "the fixture carries no XMP");

    let mut edit = doc.edit();
    edit.set_xmp_metadata(Some(PACKET)).expect("set");
    let saved = roundtrip(&mut edit);

    assert_eq!(
        saved.xmp_metadata().as_deref(),
        Some(PACKET),
        "the packet is written verbatim, not re-serialized"
    );
}

#[test]
fn the_stream_is_never_compressed() {
    // A reader that scans a file for the packet without parsing the PDF has
    // to be able to find it, which is why the writer exempts `/Metadata`.
    let doc = Document::open(HELLO).expect("open");
    let mut edit = doc.edit();
    edit.set_xmp_metadata(Some(PACKET)).expect("set");
    let mut out = Vec::new();
    edit.write_to(&mut out, &SaveOptions::default())
        .expect("save");

    let needle = b"Written by pdfrum";
    assert!(
        out.windows(needle.len()).any(|w| w == needle),
        "the packet is readable in the raw bytes"
    );
}

#[test]
fn none_removes_the_stream() {
    let doc = Document::open(HELLO).expect("open");
    let mut edit = doc.edit();
    edit.set_xmp_metadata(Some(PACKET)).expect("set");
    let with = roundtrip(&mut edit);
    assert!(with.xmp_metadata().is_some());

    let mut edit = with.edit();
    edit.set_xmp_metadata(None).expect("clear");
    let without = roundtrip(&mut edit);
    assert!(without.xmp_metadata().is_none());
}

#[test]
fn replacing_a_packet_keeps_one_stream() {
    let doc = Document::open(HELLO).expect("open");
    let mut edit = doc.edit();
    edit.set_xmp_metadata(Some(b"<x:xmpmeta xmlns:x='adobe:ns:meta/'/>"))
        .expect("set");
    let first = roundtrip(&mut edit);

    let mut edit = first.edit();
    edit.set_xmp_metadata(Some(PACKET)).expect("replace");
    let second = roundtrip(&mut edit);

    assert_eq!(second.xmp_metadata().as_deref(), Some(PACKET));
}
