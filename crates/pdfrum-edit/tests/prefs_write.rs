//! Written viewer preferences read back through `pdfrum_doc::ViewerPrefs`,
//! and an open action lands where a reader looks for one.

#![expect(
    clippy::expect_used,
    reason = "helpers shared by the tests below; a panic here is a failure"
)]

use std::sync::Arc;

use pdfrum_doc::ViewerPrefs;
use pdfrum_edit::{
    AnnotGoToView, Duplex, EditDoc, SaveOptions, ViewerPreferences, clear_open_action, save,
    set_open_action, set_viewer_preferences,
};
use pdfrum_object::{Name, Object};
use pdfrum_parser::{Document, LoadOptions, load};

const HELLO: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/hello_world.pdf"
);

fn blank_doc() -> Document {
    let bytes: Arc<[u8]> = Arc::from(&std::fs::read(HELLO).expect("fixture")[..]);
    load(bytes, &LoadOptions::default()).expect("load")
}

fn roundtrip(edit: &EditDoc<'_>) -> Document {
    let mut out = Vec::new();
    save(edit, &SaveOptions::default(), &mut out).expect("save");
    load(Arc::from(&out[..]), &LoadOptions::default()).expect("reload")
}

fn prefs_of(doc: &Document) -> ViewerPrefs {
    let catalog = doc.catalog().expect("a catalog");
    ViewerPrefs::read(&catalog, doc)
}

#[test]
fn the_typed_preferences_reach_the_typed_reader() {
    let doc = blank_doc();
    let mut edit = EditDoc::new(&doc);
    set_viewer_preferences(
        &mut edit,
        &ViewerPreferences::new()
            .direction_r2l(true)
            .num_copies(3)
            .duplex(Duplex::DuplexFlipLongEdge)
            .print_scaling(false),
    )
    .expect("set");

    let saved = roundtrip(&edit);
    let prefs = prefs_of(&saved);
    assert!(prefs.is_direction_r2l(&saved));
    assert_eq!(prefs.num_copies(&saved), 3);
    assert_eq!(prefs.duplex(&saved), b"DuplexFlipLongEdge");
    // `print_scaling(false)` is the `None` the reader tests for.
    assert!(!prefs.print_scaling(&saved));
}

#[test]
fn the_booleans_are_written_as_booleans() {
    let doc = blank_doc();
    let mut edit = EditDoc::new(&doc);
    set_viewer_preferences(
        &mut edit,
        &ViewerPreferences::new()
            .hide_toolbar(true)
            .display_doc_title(true)
            .fit_window(false),
    )
    .expect("set");

    let saved = roundtrip(&edit);
    let dict = prefs_of(&saved).dict.expect("a preferences dictionary");
    for (key, expected) in [
        ("HideToolbar", true),
        ("DisplayDocTitle", true),
        ("FitWindow", false),
    ] {
        assert_eq!(
            dict.raw(&Name::from(key)).and_then(Object::as_bool),
            Some(expected),
            "{key}",
        );
    }
    // The reader's `generic_name` is name-typed, so a boolean answers nothing
    // there — which is how the two accessors stay distinguishable.
    assert_eq!(
        prefs_of(&saved).generic_name(&Name::from("HideToolbar")),
        None
    );
}

#[test]
fn a_second_call_keeps_the_keys_it_says_nothing_about() {
    let doc = blank_doc();
    let mut edit = EditDoc::new(&doc);
    set_viewer_preferences(&mut edit, &ViewerPreferences::new().num_copies(4)).expect("first");
    set_viewer_preferences(&mut edit, &ViewerPreferences::new().hide_menubar(true))
        .expect("second");

    let saved = roundtrip(&edit);
    let prefs = prefs_of(&saved);
    // Editing one preference without reading the rest is the point of the
    // merge; a rebuild would have dropped the copies.
    assert_eq!(prefs.num_copies(&saved), 4);
    assert_eq!(
        prefs
            .dict
            .as_ref()
            .and_then(|dict| dict.raw(&Name::from("HideMenubar")))
            .and_then(Object::as_bool),
        Some(true),
    );
}

#[test]
fn an_open_action_is_a_destination_array_naming_the_page() {
    let doc = blank_doc();
    let mut edit = EditDoc::new(&doc);
    set_open_action(&mut edit, 0u32, AnnotGoToView::Fit).expect("set");

    let saved = roundtrip(&edit);
    let catalog = saved.catalog().expect("a catalog");
    let action = catalog
        .array(&Name::from("OpenAction"), &saved)
        .expect("a destination array, not an action dictionary");
    assert_eq!(action.len(), 2);
    let page = saved.page(0u32).expect("page 0");
    assert_eq!(action.reference_at(0), page.reference);
    assert_eq!(
        action.name_at(1).map(|name| name.as_bytes().to_vec()),
        Some(b"Fit".to_vec()),
    );
}

#[test]
fn clearing_an_open_action_says_whether_there_was_one() {
    let doc = blank_doc();
    let mut edit = EditDoc::new(&doc);
    assert!(
        !clear_open_action(&mut edit).expect("clear"),
        "none to clear"
    );

    set_open_action(&mut edit, 0u32, AnnotGoToView::Fit).expect("set");
    assert!(
        clear_open_action(&mut edit).expect("clear"),
        "one was there"
    );

    let saved = roundtrip(&edit);
    let catalog = saved.catalog().expect("a catalog");
    assert!(!catalog.contains_key(&Name::from("OpenAction")));
}
