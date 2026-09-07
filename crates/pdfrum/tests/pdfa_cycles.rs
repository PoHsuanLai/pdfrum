//! Cyclic and deep object graphs given to `check_pdfa`.
//!
//! The checker walks the object graph rather than the content streams, and
//! two of its walks recurse: the page tree through `/Kids`, and a page's
//! resources through a form `XObject`'s own `/Resources` (ISO 32000-1
//! §7.7.3.2 and §8.10.1). Neither shape is bounded by the parser, because
//! each level is a fresh fetch of an already-parsed object. A Rust stack
//! overflow aborts the process rather than unwinding, so a malformed file
//! reaching one of these walks would take down every other request sharing
//! it; these tests pin that the walks terminate with a report instead.

// `clippy.toml`'s `allow-expect-in-tests` does not reach the helper below,
// and a fixture that will not open is the failure signal there.
#![allow(clippy::expect_used)]

use pdfrum::{Document, PdfaLevel};

fn check(fixture: &str) -> pdfrum::PdfaReport {
    Document::open(fixture)
        .expect("fixture opens")
        .check_pdfa(PdfaLevel::A2b)
}

/// A form `XObject` whose `/Resources` names the same form: the resource
/// walk admits each form once, so the cycle is one extra level rather than
/// an unbounded descent.
#[test]
fn self_referential_form_xobject_terminates() {
    let report = check("tests/fixtures/pdfa_self_referential_form.pdf");

    // The file carries no XMP packet, so it cannot claim conformance — the
    // point is that a report exists at all.
    assert!(!report.conforms());
}

/// A page tree deeper than `Limits::max_page_tree_depth`, built from direct
/// `/Kids` dictionaries that carry no object number for the visited set to
/// remember. The depth cap is what stops it.
#[test]
fn deep_page_tree_terminates() {
    let report = check("tests/fixtures/pdfa_deep_page_tree.pdf");

    assert!(!report.conforms());
}
