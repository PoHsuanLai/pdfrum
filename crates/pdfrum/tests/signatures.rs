//! The oracle's signature assertions: counts, and every value-dictionary
//! entry the reader exposes, on the four fixtures that pin them.

use pdfrum::Document;

fn open(name: &str) -> pdfrum::Result<Document> {
    Document::open(format!("tests/fixtures/{name}.pdf"))
}

#[test]
fn a_document_with_two_signature_fields_lists_both_in_order() {
    let doc = open("two_signatures").unwrap();
    let signatures = doc.signatures();
    assert_eq!(signatures.len(), 2);
    assert_ne!(signatures[0].contents(), signatures[1].contents());
}

#[test]
fn a_document_without_signatures_lists_none() {
    assert!(open("hello_world").unwrap().signatures().is_empty());
}

#[test]
fn the_contents_are_the_signature_bytes_as_written() {
    let doc = open("two_signatures").unwrap();
    let expected: [u8; 20] = [
        0x30, 0x80, 0x06, 0x09, 0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x07, 0x02, 0xA0, 0x80,
        0x30, 0x80, 0x02, 0x01, 0x01,
    ];
    assert_eq!(doc.signatures()[0].contents(), expected);
}

#[test]
fn the_byte_range_is_the_arrays_integers() {
    let doc = open("two_signatures").unwrap();
    assert_eq!(doc.signatures()[0].byte_range(), [0, 10, 30, 10]);
}

#[test]
fn the_sub_filter_is_the_name_and_absent_when_unwritten() {
    let doc = open("two_signatures").unwrap();
    assert_eq!(
        doc.signatures()[0].sub_filter().as_deref(),
        Some("ETSI.CAdES.detached")
    );
    let doc = open("signature_no_sub_filter").unwrap();
    assert_eq!(doc.signatures()[0].sub_filter(), None);
}

#[test]
fn the_reason_is_the_strings_text() {
    let doc = open("signature_reason").unwrap();
    assert_eq!(doc.signatures()[0].reason().as_deref(), Some("test reason"));
    let doc = open("two_signatures").unwrap();
    assert_eq!(doc.signatures()[0].reason(), None);
}

#[test]
fn the_time_is_the_date_string_as_written() {
    let doc = open("two_signatures").unwrap();
    assert_eq!(
        doc.signatures()[0].time().as_deref(),
        Some("D:20200624093114+02'00'")
    );
}

#[test]
fn the_doc_mdp_permission_is_the_transform_params_p() {
    let doc = open("docmdp").unwrap();
    assert_eq!(doc.signatures()[0].doc_mdp_permission(), 1);
    let doc = open("two_signatures").unwrap();
    assert_eq!(doc.signatures()[0].doc_mdp_permission(), 0);
}
