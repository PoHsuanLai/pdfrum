//! The oracle's attachment reader assertions: names, bytes, `/Params` values
//! and their presentation, MIME subtype, and descriptions of every shape.

use pdfrum::Document;

fn open(name: &str) -> pdfrum::Result<Document> {
    Document::open(format!("tests/fixtures/{name}.pdf"))
}

#[test]
fn two_attachments_are_listed_with_their_names_bytes_and_params() {
    let doc = open("embedded_attachments").unwrap();
    let attachments = doc.attachments();
    assert_eq!(attachments.len(), 2);

    let first = &attachments[0];
    assert_eq!(first.file_name(), "1.txt");
    assert_eq!(first.data().unwrap(), b"test");
    assert!(!first.has_param("none"));
    assert_eq!(first.param("none"), None);
    // A number is present but is not text.
    assert!(first.has_param("Size"));
    assert_eq!(first.param("Size").as_deref(), Some(""));
    assert_eq!(
        first.param("CreationDate").as_deref(),
        Some("D:20170712214438-07'00'")
    );

    let second = &attachments[1];
    assert_eq!(second.data().unwrap().len(), 5869);
    assert_eq!(
        second.param("CheckSum").as_deref(),
        Some("<72AFCDDEDF554DDA63C0C88E06F1CE18>")
    );
}

#[test]
fn a_document_without_attachments_lists_none() {
    assert!(open("hello_world").unwrap().attachments().is_empty());
}

#[test]
fn an_attachment_without_an_embedded_file_has_a_name_and_no_data() {
    let doc = open("embedded_attachments_invalid_data").unwrap();
    let attachments = doc.attachments();
    assert_eq!(attachments.len(), 1);
    assert_eq!(attachments[0].file_name(), "1.txt");
    assert!(attachments[0].data().is_none());
}

#[test]
fn a_param_that_is_a_name_reads_as_its_text_and_a_stream_as_nothing() {
    let doc = open("embedded_attachments_invalid_types").unwrap();
    let attachments = doc.attachments();
    assert_eq!(attachments.len(), 2);
    assert_eq!(attachments[0].param("CheckSum").as_deref(), Some("Bad"));
    assert_eq!(attachments[1].param("CheckSum").as_deref(), Some(""));
}

#[test]
fn the_subtype_is_the_embedded_files_mime_type() {
    let doc = open("embedded_attachments").unwrap();
    assert_eq!(
        doc.attachments()[0].subtype().as_deref(),
        Some("text/plain")
    );
}

#[test]
fn a_description_is_its_text_and_anything_else_is_empty() {
    let doc = open("embedded_attachments_with_desc").unwrap();
    let attachments = doc.attachments();
    assert_eq!(attachments.len(), 4);
    assert_eq!(attachments[0].description(), "Hello, World!");
    assert_eq!(attachments[1].description(), "", "absent");
    assert_eq!(attachments[2].description(), "", "a number");
    assert_eq!(attachments[3].description(), "", "empty");
}

// ---- the writers: the oracle's add, set, describe and delete assertions,
// read back through a save and a reopen.

use pdfrum::{AttachmentOptions, SaveOptions};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

fn saved(edit: &pdfrum::DocEdit<'_>) -> pdfrum::Result<Document> {
    Document::from_bytes(Arc::from(bytes_of(edit)?))
}

fn bytes_of(edit: &pdfrum::DocEdit<'_>) -> pdfrum::Result<Vec<u8>> {
    let mut out = Vec::new();
    edit.write_to(&mut out, &SaveOptions::default())?;
    Ok(out)
}

/// No description, MIME type or date: the oracle's own `FPDFDoc_AddAttachment`.
fn plain() -> AttachmentOptions {
    AttachmentOptions::default()
}

fn described() -> AttachmentOptions {
    AttachmentOptions::builder()
        .description("The notes")
        .mime_type("text/plain")
        .modified("D:20260905120000Z00'00'")
        .build()
}

#[test]
fn an_attachments_options_are_written_and_read_back() {
    let doc = open("hello_world").unwrap();
    let mut edit = doc.edit();
    assert_eq!(
        edit.add_attachment("notes.txt", b"Read me", &described())
            .unwrap(),
        0
    );
    let doc = saved(&edit).unwrap();
    let attachment = &doc.attachments()[0];
    assert_eq!(attachment.file_name(), "notes.txt");
    assert_eq!(attachment.data().unwrap(), b"Read me");
    assert_eq!(attachment.description(), "The notes");
    assert_eq!(attachment.subtype().as_deref(), Some("text/plain"));
    assert_eq!(
        attachment.param("ModDate").as_deref(),
        Some("D:20260905120000Z00'00'")
    );
    assert!(attachment.has_param("Size"));
    let checksum = attachment.param("CheckSum").unwrap();
    assert!(
        checksum.starts_with('<') && checksum.len() == 34,
        "an MD5 as hex: {checksum}"
    );
}

#[test]
fn a_plain_attachment_has_no_description_subtype_or_date() {
    let doc = open("hello_world").unwrap();
    let mut edit = doc.edit();
    edit.add_attachment("a.bin", b"\x00\x01", &plain()).unwrap();
    let doc = saved(&edit).unwrap();
    let attachment = &doc.attachments()[0];
    assert_eq!(attachment.description(), "");
    assert_eq!(
        attachment.subtype().as_deref(),
        Some(""),
        "the stream names none"
    );
    assert!(!attachment.has_param("ModDate"));
}

#[test]
fn the_embedded_file_is_typed_and_flate_encoded() {
    let doc = open("hello_world").unwrap();
    let mut edit = doc.edit();
    let payload = vec![b'a'; 4096];
    edit.add_attachment("big.txt", &payload, &described())
        .unwrap();
    let bytes = bytes_of(&edit).unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("/EmbeddedFile"), "typed");
    assert!(
        text.contains("/Subtype/text#2Fplain"),
        "the MIME type as a name"
    );
    assert!(text.contains("/FlateDecode"), "compressed");
    assert!(
        !bytes.windows(payload.len()).any(|w| w == payload),
        "the payload is not stored raw"
    );
    let doc = Document::from_bytes(Arc::from(bytes)).unwrap();
    assert_eq!(doc.attachments()[0].data().unwrap(), payload);
}

#[test]
fn removing_by_name_takes_the_entry_and_leaves_the_others_sorted() {
    let doc = open("embedded_attachments").unwrap();
    let mut edit = doc.edit();
    edit.add_attachment("0.txt", b"first", &plain()).unwrap();
    edit.add_attachment("z.txt", b"last", &plain()).unwrap();
    assert!(edit.remove_attachment("1.txt").unwrap());
    assert!(!edit.remove_attachment("1.txt").unwrap(), "already gone");
    assert!(!edit.remove_attachment("nope").unwrap());
    let doc = saved(&edit).unwrap();
    let names: Vec<String> = doc
        .attachments()
        .iter()
        .map(pdfrum::Attachment::file_name)
        .collect();
    assert_eq!(names, ["0.txt", "attached.pdf", "z.txt"]);
}

#[test]
fn removing_every_attachment_leaves_an_empty_tree_that_reads_as_none() {
    let doc = open("embedded_attachments").unwrap();
    let mut edit = doc.edit();
    assert!(edit.remove_attachment("1.txt").unwrap());
    assert!(edit.remove_attachment("attached.pdf").unwrap());
    assert!(saved(&edit).unwrap().attachments().is_empty());
}

#[test]
fn a_non_ascii_name_and_description_round_trip() {
    let doc = open("hello_world").unwrap();
    let mut edit = doc.edit();
    edit.add_attachment("\u{7f51}\u{9875}.txt", b"x", &{
        let mut o = plain();
        o.description = Some("\u{fc}ber \u{1F3A8}".into());
        o
    })
    .unwrap();
    let doc = saved(&edit).unwrap();
    let attachment = &doc.attachments()[0];
    assert_eq!(attachment.file_name(), "\u{7f51}\u{9875}.txt");
    assert_eq!(attachment.description(), "\u{fc}ber \u{1F3A8}");
}

// ---- the oracle

/// The oracle's `pdfium_test`, when this machine has one: `$PDFRUM_ORACLE_BIN`,
/// else `$PDFRUM_ORACLE_CHECKOUT/out/Release/pdfium_test`, else the sibling
/// `../pdfium-c++` checkout. The same six lines as `embed_image.rs`, for the
/// same reason.
fn oracle_bin() -> Option<PathBuf> {
    let bin = std::env::var_os("PDFRUM_ORACLE_BIN").map_or_else(
        || {
            let checkout = std::env::var_os("PDFRUM_ORACLE_CHECKOUT").map_or_else(
                || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../pdfium-c++"),
                PathBuf::from,
            );
            checkout.join("out/Release/pdfium_test")
        },
        PathBuf::from,
    );
    bin.is_file().then_some(bin)
}

/// `pdfium_test --save-attachments` writes `<file>.attachment.<name>` beside
/// the file; the oracle's reading of what we wrote, byte for byte.
#[test]
fn the_oracle_saves_the_attachments_we_added() {
    let Some(bin) = oracle_bin() else {
        eprintln!("pdfium_test is absent; skipping the oracle round trip");
        return;
    };
    let dir = std::env::temp_dir().join("pdfrum-attachments-oracle");
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).unwrap();
    let every_byte: Vec<u8> = (0..=255u8).collect::<Vec<u8>>().repeat(16);

    // A tree created from nothing, and one grown from the fixture's own.
    for (fixture, file, expected) in [
        (
            "hello_world",
            "grown.pdf",
            vec![
                ("bytes.bin", every_byte.clone()),
                ("notes.txt", b"Read me".to_vec()),
            ],
        ),
        (
            "embedded_attachments",
            "added.pdf",
            vec![
                ("1.txt", b"test".to_vec()),
                ("bytes.bin", every_byte.clone()),
            ],
        ),
    ] {
        let doc = open(fixture).unwrap();
        let mut edit = doc.edit();
        edit.add_attachment("bytes.bin", &every_byte, &{
            let mut o = described();
            o.mime_type = Some("application/octet-stream".into());
            o
        })
        .unwrap();
        if fixture == "hello_world" {
            edit.add_attachment("notes.txt", b"Read me", &described())
                .unwrap();
        } else {
            assert!(edit.remove_attachment("attached.pdf").unwrap());
        }
        std::fs::write(dir.join(file), bytes_of(&edit).unwrap()).unwrap();

        let out = Command::new(&bin)
            .arg("--save-attachments")
            .arg(file)
            .current_dir(&dir)
            .output()
            .unwrap();
        // The exit status is not the verdict: `pdfium_test` reports the
        // attachment feature itself as unsupported and exits non-zero on the
        // fixture too, having written every file. The files are.
        let log = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        for (name, bytes) in expected {
            assert!(
                log.contains(&format!(
                    "Successfully wrote attachment {file}.attachment.{name}"
                )),
                "{log}"
            );
            let written = std::fs::read(dir.join(format!("{file}.attachment.{name}"))).unwrap();
            assert_eq!(written, bytes, "{file}: {name}");
        }
    }
}

#[test]
fn an_added_attachment_is_sorted_by_name_and_carries_its_bytes() {
    let doc = open("embedded_attachments").unwrap();
    let mut edit = doc.edit();
    assert_eq!(
        edit.add_attachment("0.txt", b"Hello!", &plain()).unwrap(),
        0
    );
    assert_eq!(
        edit.add_attachment("z.txt", b"World!", &plain()).unwrap(),
        3
    );
    let doc = saved(&edit).unwrap();
    let attachments = doc.attachments();
    assert_eq!(attachments.len(), 4);
    assert_eq!(attachments[0].file_name(), "0.txt");
    assert_eq!(attachments[0].data().unwrap(), b"Hello!");
    assert_eq!(attachments[3].file_name(), "z.txt");
    assert_eq!(attachments[3].data().unwrap(), b"World!");
}

#[test]
fn an_added_attachment_takes_a_date_a_checksum_and_an_empty_file() {
    let doc = open("embedded_attachments").unwrap();
    let mut edit = doc.edit();
    let index = edit
        .add_attachment("5.txt", b"Hello World!", &plain())
        .unwrap();
    assert_eq!(index, 1);
    assert!(
        edit.set_attachment_param(index, "CreationDate", "D:20170720161527-04'00'")
            .unwrap()
    );
    assert!(
        edit.set_attachment_param(index, "CheckSum", "<ABCDEF01234567899876543210FEDCBA>")
            .unwrap()
    );
    {
        let doc = saved(&edit).unwrap();
        let attachment = &doc.attachments()[1];
        assert_eq!(attachment.file_name(), "5.txt");
        assert_eq!(attachment.data().unwrap(), b"Hello World!");
        assert_eq!(
            attachment.param("CreationDate").as_deref(),
            Some("D:20170720161527-04'00'")
        );
        assert_eq!(
            attachment.param("CheckSum").as_deref(),
            Some("<ABCDEF01234567899876543210FEDCBA>")
        );
    }
    // An empty file, and the checksum the writer computes for it.
    assert!(edit.set_attachment_file(index, b"").unwrap());
    let doc = saved(&edit).unwrap();
    let attachment = &doc.attachments()[1];
    assert_eq!(attachment.data().unwrap(), b"");
    assert_eq!(
        attachment.param("CheckSum").as_deref(),
        Some("<D41D8CD98F00B204E9800998ECF8427E>")
    );
}

#[test]
fn a_document_without_attachments_grows_a_tree() {
    let doc = open("hello_world").unwrap();
    let mut edit = doc.edit();
    assert_eq!(
        edit.add_attachment("0.txt", b"Hello!", &plain()).unwrap(),
        0
    );
    assert_eq!(
        edit.add_attachment("z.txt", b"World!", &plain()).unwrap(),
        1
    );
    let doc = saved(&edit).unwrap();
    let attachments = doc.attachments();
    assert_eq!(attachments.len(), 2);
    assert_eq!(attachments[0].file_name(), "0.txt");
    assert_eq!(attachments[0].data().unwrap(), b"Hello!");
    assert_eq!(attachments[1].file_name(), "z.txt");
    assert_eq!(attachments[1].data().unwrap(), b"World!");
}

#[test]
fn a_non_ascii_param_round_trips() {
    let doc = open("hello_world").unwrap();
    let mut edit = doc.edit();
    let index = edit
        .add_attachment("attachment.txt", b"Test Contents", &plain())
        .unwrap();
    assert!(
        edit.set_attachment_param(index, "test", "\u{f6}\u{e4}")
            .unwrap()
    );
    let doc = saved(&edit).unwrap();
    assert_eq!(
        doc.attachments()[0].param("test").as_deref(),
        Some("\u{f6}\u{e4}")
    );
}

#[test]
fn deleting_the_first_attachment_promotes_the_second() {
    let doc = open("embedded_attachments").unwrap();
    let mut edit = doc.edit();
    assert!(edit.delete_attachment(0).unwrap());
    assert!(!edit.delete_attachment(5).unwrap());
    let doc = saved(&edit).unwrap();
    let attachments = doc.attachments();
    assert_eq!(attachments.len(), 1);
    assert_eq!(attachments[0].file_name(), "attached.pdf");
}

#[test]
fn an_attachment_without_data_can_still_be_deleted() {
    let doc = open("embedded_attachments_invalid_data").unwrap();
    let mut edit = doc.edit();
    assert!(edit.delete_attachment(0).unwrap());
    assert!(saved(&edit).unwrap().attachments().is_empty());
}

#[test]
fn a_description_is_written_as_text() {
    let doc = open("embedded_attachments_with_desc").unwrap();
    let mut edit = doc.edit();
    assert!(edit.set_attachment_description(0, "\u{fc}ber").unwrap());
    let doc = saved(&edit).unwrap();
    assert_eq!(doc.attachments()[0].description(), "\u{fc}ber");
}
