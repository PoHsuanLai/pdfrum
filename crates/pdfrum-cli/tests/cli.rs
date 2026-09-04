//! The binary, run on the fixtures, against expected output files.
//!
//! Text output is compared byte for byte with `expected/<name>.txt`; JSON
//! output is parsed and its keys checked, so a reformat does not fail it
//! but a renamed key does. Regenerate an expected file deliberately, by
//! running the same command and reading the diff — the file is the contract.

use std::path::Path;
use std::process::{Command, Output, Stdio};

fn expected(name: &str) -> std::io::Result<String> {
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/expected")
            .join(name),
    )
}

fn run(args: &[&str]) -> std::io::Result<Output> {
    Command::new(env!("CARGO_BIN_EXE_pdfrum"))
        .args(args)
        .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests"))
        .output()
}

/// Stdout of a run that must succeed; the error names the exit code and
/// carries stderr.
fn stdout(args: &[&str]) -> Result<String, String> {
    let out = run(args).map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!(
            "{args:?} exited {:?}: {}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    String::from_utf8(out.stdout).map_err(|e| e.to_string())
}

fn json(args: &[&str]) -> Result<serde_json::Value, String> {
    let text = stdout(args)?;
    serde_json::from_str(&text).map_err(|e| format!("{args:?}: not JSON ({e}):\n{text}"))
}

/// The expected files name fixtures relative to `tests/`, which is where
/// the binary runs, so the paths inside them are stable.
const fn fx(name: &str) -> &str {
    name
}

#[test]
fn info_is_the_summary_and_the_json_has_the_same_facts() {
    assert_eq!(
        stdout(&["info", fx("fixtures/two_signatures.pdf")]).unwrap(),
        expected("info_two_signatures.txt").unwrap()
    );
    let v = json(&["info", fx("fixtures/bookmarks.pdf"), "--json"]).unwrap();
    assert_eq!(v["pages"], 2);
    assert_eq!(v["outline_entries"], 6);
    assert_eq!(v["encrypted"], false);
    assert_eq!(v["permissions"]["print"], true);
    assert_eq!(
        v["page_boxes"][0]["media_box"],
        serde_json::json!([0.0, 0.0, 612.0, 792.0])
    );
    assert!(v["metadata"].is_object());
}

#[test]
fn doctor_lists_what_was_recovered_and_strict_exits_3() {
    assert_eq!(
        stdout(&["doctor", fx("fixtures/parser_rebuildxref_correct.pdf")]).unwrap(),
        expected("doctor_rebuildxref.txt").unwrap()
    );
    let strict = run(&[
        "doctor",
        "--strict",
        fx("fixtures/parser_rebuildxref_correct.pdf"),
    ])
    .unwrap();
    assert_eq!(strict.status.code(), Some(3));
    let clean = run(&["doctor", "--strict", fx("fixtures/hello_world_2_pages.pdf")]).unwrap();
    assert_eq!(clean.status.code(), Some(0));
    let v = json(&[
        "doctor",
        fx("fixtures/parser_rebuildxref_correct.pdf"),
        "--json",
    ])
    .unwrap();
    assert_eq!(v["xref_rebuilt"], true);
    assert_eq!(v["recovered"], 4);
    assert_eq!(v["notices"].as_array().map(Vec::len), Some(4));
}

#[test]
fn text_separates_pages_with_a_form_feed_and_honours_pages() {
    assert_eq!(
        stdout(&["extract", "text", fx("fixtures/hello_world_2_pages.pdf")]).unwrap(),
        expected("text_hello_world_2_pages.txt").unwrap()
    );
    let second = stdout(&[
        "extract",
        "text",
        "--pages",
        "2",
        fx("fixtures/hello_world_2_pages.pdf"),
    ])
    .unwrap();
    assert!(!second.contains('\u{c}'), "one page, no form feed");
    let v = json(&[
        "extract",
        "text",
        fx("fixtures/hello_world_2_pages.pdf"),
        "--json",
    ])
    .unwrap();
    assert_eq!(v[1]["page"], 2);
    assert!(v[1]["text"].as_str().unwrap().contains("Goodbye"));
}

#[test]
fn a_bad_page_selection_is_a_usage_error_with_a_reason() {
    let out = run(&[
        "extract",
        "text",
        "--pages",
        "0",
        fx("fixtures/hello_world_2_pages.pdf"),
    ])
    .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("numbered from 1"));
    let out = run(&[
        "extract",
        "text",
        "--pages",
        "9",
        fx("fixtures/hello_world_2_pages.pdf"),
    ])
    .unwrap();
    assert!(String::from_utf8_lossy(&out.stderr).contains("past the last page"));
}

#[test]
fn links_come_from_annotations_and_from_the_text() {
    assert_eq!(
        stdout(&[
            "extract",
            "links",
            fx("fixtures/annots_action_handling.pdf")
        ])
        .unwrap(),
        expected("links_annots_action_handling.txt").unwrap()
    );
    let v = json(&["extract", "links", fx("fixtures/weblinks.pdf"), "--json"]).unwrap();
    let rows = v.as_array().unwrap();
    assert!(rows.iter().all(|r| r["kind"] == "text"), "{v}");
    assert!(
        rows.iter()
            .any(|r| r["uri"].as_str().unwrap().starts_with("http"))
    );
    let v = json(&[
        "extract",
        "links",
        fx("fixtures/annots_action_handling.pdf"),
        "--json",
    ])
    .unwrap();
    assert_eq!(v[0]["kind"], "uri");
    assert_eq!(v[1]["kind"], "page");
    assert_eq!(v[1]["target_page"], 2);
}

#[test]
fn toc_is_the_outline_indented_by_depth() {
    assert_eq!(
        stdout(&["extract", "toc", fx("fixtures/bookmarks.pdf")]).unwrap(),
        expected("toc_bookmarks.txt").unwrap()
    );
    let v = json(&["extract", "toc", fx("fixtures/bookmarks.pdf"), "--json"]).unwrap();
    assert_eq!(v.as_array().map(Vec::len), Some(6));
    assert_eq!(v[2]["depth"], 1);
}

#[test]
fn attachments_are_listed_and_written() {
    assert_eq!(
        stdout(&[
            "extract",
            "attachments",
            fx("fixtures/embedded_attachments_with_desc.pdf")
        ])
        .unwrap(),
        expected("attachments_with_desc.txt").unwrap()
    );
    let dir = std::env::temp_dir().join(format!("pdfrum-cli-attachments-{}", std::process::id()));
    let v = json(&[
        "extract",
        "attachments",
        fx("fixtures/embedded_attachments_with_desc.pdf"),
        "-o",
        dir.to_str().unwrap(),
        "--json",
    ])
    .unwrap();
    assert_eq!(v.as_array().map(Vec::len), Some(4));
    assert_eq!(v[0]["description"], "Hello, World!");
    assert_eq!(std::fs::read(dir.join("1.txt")).unwrap().len(), 3);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn annotations_and_signatures_print_their_fields() {
    assert_eq!(
        stdout(&[
            "extract",
            "annotations",
            "--pages",
            "1",
            fx("fixtures/annotiter.pdf")
        ])
        .unwrap(),
        expected("annotations_annotiter_page1.txt").unwrap()
    );
    assert_eq!(
        stdout(&["extract", "signatures", fx("fixtures/signature_reason.pdf")]).unwrap(),
        expected("signatures_reason.txt").unwrap()
    );
    let v = json(&[
        "extract",
        "signatures",
        fx("fixtures/two_signatures.pdf"),
        "--json",
    ])
    .unwrap();
    assert_eq!(v.as_array().map(Vec::len), Some(2));
    assert_eq!(v[0]["sub_filter"], "ETSI.CAdES.detached");
}

#[test]
fn render_writes_a_png_per_page_and_one_page_to_stdout() {
    let dir = std::env::temp_dir().join(format!("pdfrum-cli-render-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let template = dir.join("{stem}-{n}.png");
    let out = run(&[
        "render",
        "--dpi",
        "36",
        "-o",
        template.to_str().unwrap(),
        fx("fixtures/hello_world_2_pages.pdf"),
    ])
    .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    for n in 1..=2 {
        let png = std::fs::read(dir.join(format!("hello_world_2_pages-{n}.png"))).unwrap();
        assert_eq!(&png[..4], b"\x89PNG");
    }
    std::fs::remove_dir_all(&dir).unwrap();

    let out = run(&[
        "render",
        "--pages",
        "1",
        "-o",
        "-",
        fx("fixtures/hello_world_2_pages.pdf"),
    ])
    .unwrap();
    assert!(out.status.success());
    assert_eq!(&out.stdout[..4], b"\x89PNG");
    let two = run(&["render", "-o", "-", fx("fixtures/hello_world_2_pages.pdf")]).unwrap();
    assert_eq!(
        two.status.code(),
        Some(1),
        "two pages cannot go to one stdout"
    );
}

#[test]
fn a_closed_pipe_is_not_an_error() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_pdfrum"))
        .args(["extract", "annotations", "fixtures/annotiter.pdf"])
        .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // Close the read end before the child has written, so its writes hit
    // a broken pipe.
    drop(child.stdout.take());
    let status = child.wait().unwrap();
    assert!(status.success(), "{status:?}");
}

#[test]
fn a_missing_file_is_exit_1_with_the_path_named() {
    let out = run(&["info", "fixtures/nonesuch.pdf"]).unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("nonesuch.pdf"));
}
