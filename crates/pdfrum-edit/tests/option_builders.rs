//! The options types are `#[non_exhaustive]`, so a builder is the only way to
//! fill one in from outside the crate — and each builder starts at the
//! defaults rather than at zero.

use kurbo::Size;
use pdfrum_crypt::Permissions;
use pdfrum_edit::{Encryption, IdSource, ImportOptions, NUpOptions, SaveMode, SaveOptions};

#[test]
fn a_save_options_builder_starts_at_the_defaults() {
    let built = SaveOptions::builder().build();
    assert_eq!(built, SaveOptions::default());
}

#[test]
fn every_save_setting_is_reachable() {
    let options = SaveOptions::builder()
        .mode(SaveMode::Incremental)
        .keep_original(false)
        .remove_security(true)
        .subset_new_fonts(true)
        .id_source(IdSource::Fixed([9; 16]))
        .build();

    assert_eq!(options.mode, SaveMode::Incremental);
    assert!(!options.keep_original);
    assert!(options.remove_security);
    assert!(options.subset_new_fonts);
    assert_eq!(options.id_source, IdSource::Fixed([9; 16]));
    // Untouched settings keep their defaults.
    assert_eq!(options.version, SaveOptions::default().version);
    assert_eq!(options.encrypt, None);
}

#[test]
fn an_encryption_builder_grants_everything_until_told_otherwise() {
    let built = Encryption::builder().build();
    assert!(built.user_password.is_empty());
    assert!(built.owner_password.is_empty());
    assert_eq!(built.permissions, Permissions::ALL);
    assert!(built.encrypt_metadata);
}

#[test]
fn every_encryption_setting_is_reachable() {
    let encryption = Encryption::builder()
        .user_password(b"reader".to_vec())
        .owner_password(b"owner".to_vec())
        .permissions(Permissions::NONE)
        .encrypt_metadata(false)
        .build();

    assert_eq!(encryption.user_password, b"reader");
    assert_eq!(encryption.owner_password, b"owner");
    assert_eq!(encryption.permissions, Permissions::NONE);
    assert!(!encryption.encrypt_metadata);
}

#[test]
fn an_encryption_reaches_a_save_options() {
    let options = SaveOptions::builder()
        .encrypt(Encryption::builder().user_password(b"x".to_vec()).build())
        .build();
    assert_eq!(
        options.encrypt.map(|encryption| encryption.user_password),
        Some(b"x".to_vec()),
    );
}

#[test]
fn an_import_options_builder_starts_at_the_defaults() {
    assert_eq!(ImportOptions::builder().build(), ImportOptions::default());

    let options = ImportOptions::builder()
        .at(4u32)
        .viewer_preferences(true)
        .build();
    assert_eq!(u32::from(options.at), 4);
    assert!(options.viewer_preferences);
}

#[test]
fn the_nup_builder_names_which_number_is_which() {
    assert_eq!(NUpOptions::builder().build(), NUpOptions::default());

    // A4 landscape, four up. The bare tuple would not say which number is the
    // width or which is the column count; these two setters do.
    let options = NUpOptions::builder()
        .sheet(Size::new(842.0, 595.0))
        .grid(2, 2)
        .build();
    assert_eq!(options.sheet, (842.0, 595.0));
    assert_eq!(options.grid, (2, 2));
}
