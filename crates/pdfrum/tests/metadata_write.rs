//! `/Trapped` and custom `/Info` keys survive a round trip, and the builder
//! is how a caller outside the crate makes a `Metadata`.

#![expect(
    clippy::expect_used,
    reason = "helpers shared by the tests below; a panic here is a failure"
)]

use pdfrum::{Document, Metadata, SaveOptions, Trapped};

fn hello() -> Document {
    Document::open(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/hello_world.pdf"
    ))
    .expect("fixture")
}

fn roundtrip(doc: &Document, metadata: &Metadata) -> Document {
    let mut edit = doc.edit();
    edit.set_metadata(metadata);
    let mut bytes = Vec::new();
    edit.write_to(&mut bytes, &SaveOptions::default())
        .expect("save");
    Document::from_bytes(bytes).expect("reload")
}

#[test]
fn trapped_round_trips_as_a_name() {
    for state in [Trapped::Yes, Trapped::No, Trapped::Unknown] {
        let doc = hello();
        let metadata = Metadata::builder().trapped(state).build();
        let saved = roundtrip(&doc, &metadata);
        assert_eq!(saved.metadata().trapped, Some(state), "{state:?}");
    }
}

#[test]
fn an_absent_trapped_is_not_the_same_as_unknown() {
    let doc = hello();
    // The fixture has no `/Trapped`, and a PDF/X validator distinguishes an
    // absent key from an explicit `Unknown`.
    assert_eq!(doc.metadata().trapped, None);

    let saved = roundtrip(&doc, &Metadata::builder().trapped(Trapped::Unknown).build());
    assert_eq!(saved.metadata().trapped, Some(Trapped::Unknown));
}

#[test]
fn custom_info_keys_survive_a_round_trip() {
    let doc = hello();
    let metadata = Metadata::builder()
        .title("Report")
        .custom("Department", "Research")
        .custom("Ticket", "PDF-42")
        .build();

    let saved = roundtrip(&doc, &metadata);
    let read = saved.metadata();
    assert_eq!(read.title.as_deref(), Some("Report"));

    let mut custom = read.custom.clone();
    custom.sort();
    assert!(
        custom.contains(&("Department".to_owned(), "Research".to_owned())),
        "{custom:?}",
    );
    assert!(
        custom.contains(&("Ticket".to_owned(), "PDF-42".to_owned())),
        "{custom:?}",
    );
}

#[test]
fn the_named_eight_are_not_repeated_as_custom_keys() {
    let doc = hello();
    let metadata = Metadata::builder().title("Report").author("Ada").build();
    let saved = roundtrip(&doc, &metadata);
    let read = saved.metadata();

    // `/Producer` is stamped by the save, so the interesting assertion is that
    // none of the nine lifted keys shows up twice.
    for (key, _) in &read.custom {
        assert!(
            ![
                "Title",
                "Author",
                "Subject",
                "Keywords",
                "Creator",
                "Producer",
                "CreationDate",
                "ModDate",
                "Trapped",
            ]
            .contains(&key.as_str()),
            "{key} was lifted into a named field and left in `custom` too",
        );
    }
}

#[test]
fn the_builder_leaves_unset_fields_alone() {
    let metadata = Metadata::builder().title("Only a title").build();
    assert_eq!(metadata.title.as_deref(), Some("Only a title"));
    assert_eq!(metadata.author, None);
    assert_eq!(metadata.trapped, None);
    assert!(metadata.custom.is_empty());
}

#[test]
fn a_round_trip_through_the_reader_preserves_what_it_read() {
    let doc = hello();
    let first = roundtrip(
        &doc,
        &Metadata::builder()
            .title("Report")
            .trapped(Trapped::No)
            .custom("Department", "Research")
            .build(),
    );

    // Reading and writing back unchanged is the operation a caller performs
    // when editing one field; nothing may be dropped on the way.
    let read = first.metadata();
    let second = roundtrip(&first, &read);
    let again = second.metadata();

    assert_eq!(again.title.as_deref(), Some("Report"));
    assert_eq!(again.trapped, Some(Trapped::No));
    assert!(
        again
            .custom
            .contains(&("Department".to_owned(), "Research".to_owned())),
        "{:?}",
        again.custom,
    );
}
