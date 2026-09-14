//! A choice field's `/I` is rewritten to agree with the `/V` a fill writes.
//!
//! `/I` indexes `/Opt`; a reader that finds the two disagreeing discards `/I`
//! and matches the text instead. A fill that rewrote `/V` alone left exactly
//! that stale pair behind.

#![expect(
    clippy::expect_used,
    reason = "helpers shared by the tests below; a panic here is a failure"
)]

use std::sync::Arc;

use pdfrum::{Document, SaveOptions};

const LISTBOX: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/listbox_form.pdf"
);

/// Fills `field` with `value` and reloads the saved document.
fn filled(field: &str, value: &str) -> Document {
    let doc = Document::open(LISTBOX).expect("open");
    let mut form = doc.form().expect("form");
    form.set(field, value).expect("the field exists");
    let mut out = Vec::new();
    doc.write_form_to(&mut out, &form, &SaveOptions::default())
        .expect("save");
    Document::from_bytes(Arc::from(out)).expect("reopen")
}

/// The `/I` array of the widget named `field`, as written.
///
/// Read off the annotation rather than the facade's `Field`, which exposes the
/// resolved selection and not the raw key this test is about.
fn indices(doc: &Document, field: &str) -> Vec<i64> {
    let name = pdfrum_object::Name::from("I");
    for page in 0..doc.page_count() {
        let Ok(page) = doc.page(page) else { continue };
        for annot in page.annotations() {
            if annot
                .dict()
                .text(&pdfrum_object::Name::from("T"), doc.parser())
                != Some(field.to_owned())
            {
                continue;
            }
            return annot
                .dict()
                .array(&name, doc.parser())
                .map(|array| {
                    (0..array.len())
                        .filter_map(|index| array.int_at(index))
                        .collect()
                })
                .unwrap_or_default();
        }
    }
    Vec::new()
}

#[test]
fn filling_a_choice_field_rewrites_the_index_to_match() {
    // The fixture ships this field with `/V (Banana)`, `/I [1 3]`, and an
    // `/Opt` of country names — a value that is in neither the options nor the
    // indices. A fill that touches `/V` and leaves `/I` alone keeps the
    // contradiction; this asserts it does not.
    let doc = filled("Listbox_MultiSelectMultipleIndices", "Croatia");
    let form = doc.form().expect("form");
    assert_eq!(
        form.field("Listbox_MultiSelectMultipleIndices")
            .expect("field")
            .stored_value(),
        "Croatia"
    );

    let written = indices(&doc, "Listbox_MultiSelectMultipleIndices");
    assert_eq!(
        written.len(),
        1,
        "one value selects one index, not the two the fixture carried"
    );

    // And the index names the option the value does, which is the agreement
    // `selected_indices_for_interaction` tests before trusting `/I`.
    let options = form
        .field("Listbox_MultiSelectMultipleIndices")
        .expect("field")
        .options();
    let named = options.get(usize::try_from(written[0]).expect("a non-negative index"));
    assert_eq!(named.map(String::as_str), Some("Croatia"));
}

#[test]
fn a_value_that_names_no_option_writes_an_empty_index() {
    // Nothing to point at. An empty array says "no selection" without
    // claiming an index that would contradict the text.
    let doc = filled("Listbox_MultiSelectMultipleIndices", "not-an-option");
    assert!(indices(&doc, "Listbox_MultiSelectMultipleIndices").is_empty());
    assert_eq!(
        doc.form()
            .expect("form")
            .field("Listbox_MultiSelectMultipleIndices")
            .expect("field")
            .stored_value(),
        "not-an-option",
        "the value is still stored; only the index declines to guess"
    );
}

#[test]
fn a_text_field_gains_no_index() {
    // `/I` belongs to `/Ch` alone. A text field that grew one would be
    // writing a key its kind has no meaning for.
    let doc = Document::open(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/text_form.pdf"
    ))
    .expect("open");
    let mut form = doc.form().expect("form");
    form.set("Text Box", "typed").expect("field exists");
    let mut out = Vec::new();
    doc.write_form_to(&mut out, &form, &SaveOptions::default())
        .expect("save");
    let reopened = Document::from_bytes(Arc::from(out)).expect("reopen");
    assert!(indices(&reopened, "Text Box").is_empty());
}
