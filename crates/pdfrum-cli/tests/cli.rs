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

/// [`run`] with bytes on stdin and extra environment variables.
fn run_with(args: &[&str], stdin: &[u8], env: &[(&str, &str)]) -> std::io::Result<Output> {
    use std::io::Write;
    let mut child = Command::new(env!("CARGO_BIN_EXE_pdfrum"))
        .args(args)
        .envs(env.iter().copied())
        .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    if let Some(mut pipe) = child.stdin.take() {
        pipe.write_all(stdin)?;
    }
    child.wait_with_output()
}

/// A fixture's bytes, for feeding stdin.
fn fixture(name: &str) -> std::io::Result<Vec<u8>> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name),
    )
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
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("nonesuch.pdf"), "{err}");
    assert_eq!(
        err.matches("os error 2").count(),
        1,
        "each cause is said once: {err}"
    );
}

// ---- phase 2: pages, forms, whole-file commands ---------------------------

fn scratch(name: &str) -> std::io::Result<std::path::PathBuf> {
    let dir = std::env::temp_dir().join(format!("pdfrum-cli-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn pages_of(path: &Path) -> Result<serde_json::Value, String> {
    let path = path.to_str().ok_or("non-utf8 path")?;
    Ok(json(&["info", path, "--json"])?["page_boxes"].clone())
}

#[test]
fn merge_joins_files_in_order_and_split_takes_them_apart_again() {
    let dir = scratch("merge").unwrap();
    let merged = dir.join("merged.pdf");
    let out = run(&[
        "pages",
        "merge",
        "fixtures/hello_world_2_pages.pdf",
        "fixtures/bookmarks.pdf",
        "-o",
        merged.to_str().unwrap(),
    ])
    .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let pages = pages_of(&merged).unwrap();
    assert_eq!(pages.as_array().map(Vec::len), Some(4));
    assert_eq!(pages[0]["width"], 200.0);
    assert_eq!(pages[2]["width"], 612.0);

    let split = dir.join("split");
    let out = run(&[
        "pages",
        "split",
        merged.to_str().unwrap(),
        "-o",
        split.to_str().unwrap(),
    ])
    .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    for n in 1..=4 {
        let one = split.join(format!("merged-{n}.pdf"));
        assert_eq!(
            pages_of(&one).unwrap().as_array().map(Vec::len),
            Some(1),
            "{}",
            one.display()
        );
    }
    let text = stdout(&[
        "extract",
        "text",
        split.join("merged-2.pdf").to_str().unwrap(),
    ])
    .unwrap();
    assert!(
        text.contains("Goodbye"),
        "page 2 of the merge is page 2 of hello_world: {text}"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn slice_keeps_rotates_and_crops_and_reorder_takes_any_order() {
    let dir = scratch("slice").unwrap();
    let sliced = dir.join("sliced.pdf");
    let out = run(&[
        "pages",
        "slice",
        "fixtures/bookmarks.pdf",
        "--pages",
        "2",
        "--rotate",
        "90",
        "--crop",
        "0,0,300,400",
        "-o",
        sliced.to_str().unwrap(),
    ])
    .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let pages = pages_of(&sliced).unwrap();
    assert_eq!(pages.as_array().map(Vec::len), Some(1));
    assert_eq!(pages[0]["rotation"], 90);
    assert_eq!(
        pages[0]["crop_box"],
        serde_json::json!([0.0, 0.0, 300.0, 400.0])
    );
    assert_eq!(
        pages[0]["width"], 400.0,
        "rotated a quarter turn, the crop's height is the width"
    );

    let reordered = dir.join("reordered.pdf");
    let out = run(&[
        "pages",
        "reorder",
        "fixtures/hello_world_2_pages.pdf",
        "--pages",
        "2,1,1",
        "-o",
        reordered.to_str().unwrap(),
    ])
    .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v = json(&["extract", "text", reordered.to_str().unwrap(), "--json"]).unwrap();
    let texts: Vec<&str> = v
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["text"].as_str().unwrap())
        .collect();
    assert_eq!(texts.len(), 3);
    assert!(
        texts[0].contains("Goodbye") && texts[1].contains("Hello") && texts[2].contains("Hello")
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn nup_and_booklet_lay_pages_out_on_sheets() {
    let dir = scratch("nup").unwrap();
    let nup = dir.join("nup.pdf");
    let out = run(&[
        "pages",
        "nup",
        "fixtures/bookmarks.pdf",
        "--grid",
        "2x1",
        "--sheet",
        "1000x500",
        "-o",
        nup.to_str().unwrap(),
    ])
    .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let pages = pages_of(&nup).unwrap();
    assert_eq!(
        pages.as_array().map(Vec::len),
        Some(1),
        "two pages, one sheet"
    );
    assert_eq!(pages[0]["width"], 1000.0);

    let booklet = dir.join("booklet.pdf");
    let out = run(&[
        "pages",
        "booklet",
        "fixtures/hello_world_2_pages.pdf",
        "-o",
        booklet.to_str().unwrap(),
    ])
    .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let pages = pages_of(&booklet).unwrap();
    assert_eq!(
        pages.as_array().map(Vec::len),
        Some(2),
        "padded to four pages, two sides"
    );
    assert_eq!(pages[0]["width"], 400.0, "two 200 pt pages side by side");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn create_makes_a_page_per_image() {
    let dir = scratch("create").unwrap();
    let png_path = dir.join("tiny.png");
    {
        let file = std::fs::File::create(&png_path).unwrap();
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), 2, 3);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer
            .write_image_data(&[
                255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 0, 0, 255, 255, 255, 0, 255,
            ])
            .unwrap();
    }
    let created = dir.join("created.pdf");
    let out = run(&[
        "pages",
        "create",
        "fixtures/mona_lisa.jpg",
        png_path.to_str().unwrap(),
        "--dpi",
        "72",
        "-o",
        created.to_str().unwrap(),
    ])
    .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let pages = pages_of(&created).unwrap();
    assert_eq!(pages.as_array().map(Vec::len), Some(2));
    assert_eq!(pages[0]["width"], 120.0, "mona_lisa is 120 px at 72 dpi");
    assert_eq!(pages[1]["width"], 2.0);
    assert_eq!(pages[1]["height"], 3.0);
    let out = run(&[
        "render",
        "--pages",
        "2",
        "--scale",
        "1",
        "-o",
        "-",
        created.to_str().unwrap(),
    ])
    .unwrap();
    assert!(out.status.success() && out.stdout.starts_with(b"\x89PNG"));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn forms_dump_fill_and_flatten_round_trip() {
    assert_eq!(
        stdout(&["forms", "dump", "fixtures/text_form.pdf"]).unwrap(),
        expected("forms_dump_text_form.txt").unwrap()
    );
    let dir = scratch("forms").unwrap();
    let data = dir.join("fill.json");
    std::fs::write(&data, r#"{"Text Box": "filled by pdfrum"}"#).unwrap();
    let filled = dir.join("filled.pdf");
    let out = run(&[
        "forms",
        "fill",
        "fixtures/text_form.pdf",
        "--data",
        data.to_str().unwrap(),
        "-o",
        filled.to_str().unwrap(),
    ])
    .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v = json(&["forms", "dump", filled.to_str().unwrap(), "--json"]).unwrap();
    assert_eq!(v[0]["value"], "filled by pdfrum");

    let bad = dir.join("bad.json");
    std::fs::write(&bad, r#"{"Nonesuch": "x"}"#).unwrap();
    let out = run(&[
        "forms",
        "fill",
        "fixtures/text_form.pdf",
        "--data",
        bad.to_str().unwrap(),
        "-o",
        filled.to_str().unwrap(),
    ])
    .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("Nonesuch"));

    let flat = dir.join("flat.pdf");
    let out = run(&[
        "forms",
        "flatten",
        filled.to_str().unwrap(),
        "-o",
        flat.to_str().unwrap(),
    ])
    .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let annots = json(&["extract", "annotations", flat.to_str().unwrap(), "--json"]).unwrap();
    assert_eq!(
        annots.as_array().map(Vec::len),
        Some(0),
        "the widget is baked in: {annots}"
    );
    assert!(
        stdout(&["forms", "dump", flat.to_str().unwrap()])
            .unwrap()
            .contains("no form fields")
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn repair_optimize_and_decrypt_rewrite_the_file() {
    let dir = scratch("file").unwrap();
    let repaired = dir.join("repaired.pdf");
    let out = run(&[
        "repair",
        "fixtures/parser_rebuildxref_correct.pdf",
        "-o",
        repaired.to_str().unwrap(),
    ])
    .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let strict = run(&["doctor", "--strict", repaired.to_str().unwrap()]).unwrap();
    assert_eq!(strict.status.code(), Some(0), "the rewrite is clean");

    let a = dir.join("a.pdf");
    let b = dir.join("b.pdf");
    for path in [&a, &b] {
        let out = run(&[
            "optimize",
            "fixtures/bookmarks.pdf",
            "--deterministic",
            "-o",
            path.to_str().unwrap(),
        ])
        .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    assert_eq!(
        std::fs::read(&a).unwrap(),
        std::fs::read(&b).unwrap(),
        "--deterministic is byte-identical"
    );
    assert_eq!(pages_of(&a).unwrap().as_array().map(Vec::len), Some(2));

    let refused = run(&["info", "fixtures/encrypted.pdf"]).unwrap();
    assert_eq!(refused.status.code(), Some(1), "no password, no document");
    let dec = dir.join("dec.pdf");
    let out = run(&[
        "security",
        "decrypt",
        "--password",
        "1234",
        "fixtures/encrypted.pdf",
        "-o",
        dec.to_str().unwrap(),
    ])
    .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v = json(&["info", dec.to_str().unwrap(), "--json"]).unwrap();
    assert_eq!(v["encrypted"], false);
    let again = run(&[
        "security",
        "decrypt",
        dec.to_str().unwrap(),
        "-o",
        dir.join("x.pdf").to_str().unwrap(),
    ])
    .unwrap();
    assert_eq!(
        again.status.code(),
        Some(1),
        "decrypting a clear file is an error, not a no-op"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

// ---- phase 3: the terminal -----------------------------------------------

#[test]
fn search_prints_hits_with_their_page_and_exits_1_on_none() {
    let text = stdout(&["search", "-i", "WORLD", "fixtures/hello_world_2_pages.pdf"]).unwrap();
    assert_eq!(text.lines().count(), 4, "{text}");
    assert!(text.starts_with("page 1:Hello, world!"));
    let none = run(&["search", "nonesuch", "fixtures/hello_world_2_pages.pdf"]).unwrap();
    assert_eq!(none.status.code(), Some(1));
    let v = json(&[
        "search",
        "Goodbye",
        "fixtures/hello_world_2_pages.pdf",
        "--json",
    ])
    .unwrap();
    assert_eq!(v.as_array().map(Vec::len), Some(2));
    assert_eq!(v[1]["page"], 2);
    assert_eq!(v[0]["line"], "Goodbye, world!");
    assert!(v[0]["rects"][0].is_array(), "a box per hit: {v}");
}

#[test]
fn colour_and_hyperlinks_are_off_in_a_pipe_unless_asked_and_no_color_wins() {
    let plain = stdout(&["search", "world", "fixtures/hello_world_2_pages.pdf"]).unwrap();
    assert!(!plain.contains('\u{1b}'), "no escapes in a pipe: {plain:?}");
    let painted = stdout(&[
        "search",
        "world",
        "fixtures/hello_world_2_pages.pdf",
        "--color",
        "always",
    ])
    .unwrap();
    assert!(
        painted.contains("\u{1b}[1;33mworld\u{1b}[0m"),
        "{painted:?}"
    );
    let linked = stdout(&[
        "extract",
        "toc",
        "fixtures/bookmarks.pdf",
        "--hyperlinks",
        "always",
    ])
    .unwrap();
    assert!(
        linked.contains("\u{1b}]8;;file://") && linked.contains("#page=1\u{1b}\\"),
        "{linked:?}"
    );
    let shown = stdout(&[
        "extract",
        "toc",
        "fixtures/bookmarks.pdf",
        "--hyperlinks",
        "always",
        "--color",
        "always",
    ])
    .unwrap();
    assert!(
        shown.contains("#page=1\u{1b}\\\u{1b}[4;36m1\u{1b}[0m\u{1b}]8;;"),
        "a link is underlined when colour is on: {shown:?}"
    );
    let out = Command::new(env!("CARGO_BIN_EXE_pdfrum"))
        .args([
            "search",
            "world",
            "fixtures/hello_world_2_pages.pdf",
            "--color",
            "auto",
        ])
        .env("NO_COLOR", "1")
        .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests"))
        .output()
        .unwrap();
    assert!(!String::from_utf8_lossy(&out.stdout).contains('\u{1b}'));
}

#[test]
fn preview_draws_half_blocks_when_told_to_and_view_refuses_a_pipe() {
    let out = run(&[
        "preview",
        "fixtures/hello_world_2_pages.pdf",
        "--graphics",
        "halfblock",
        "--width",
        "24",
    ])
    .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8(out.stdout).unwrap();
    let rows = text.lines().count();
    assert!(rows >= 8, "{rows} rows of cells");
    assert!(
        text.lines().all(|l| l.matches('\u{2580}').count() == 24),
        "24 cells per row"
    );
    assert!(text.contains("\u{1b}[38;2;"));
    let off = run(&["preview", "fixtures/hello_world_2_pages.pdf"]).unwrap();
    assert_eq!(off.status.code(), Some(1), "no pictures in a pipe");
    assert!(String::from_utf8_lossy(&off.stderr).contains("render"));
    let view = run(&["view", "fixtures/hello_world_2_pages.pdf"]).unwrap();
    assert_eq!(view.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&view.stderr).contains("needs a terminal"));
}

// ---- phase 4: markdown and layout ----------------------------------------

#[test]
fn markdown_reads_the_structure_tree_and_keeps_unclaimed_text() {
    assert_eq!(
        stdout(&["extract", "markdown", "fixtures/tagged_alt_text.pdf"]).unwrap(),
        "![Black Image](image)\n"
    );
    assert_eq!(
        stdout(&["extract", "markdown", "fixtures/tagged_actual_text.pdf"]).unwrap(),
        "Actual Text\n\n![Actual Text](image)\n"
    );
    assert_eq!(
        stdout(&["extract", "markdown", "fixtures/tagged_marked_content.pdf"]).unwrap(),
        expected("markdown_tagged_marked_content.txt").unwrap()
    );
    let v = json(&[
        "extract",
        "markdown",
        "fixtures/tagged_alt_text.pdf",
        "--json",
    ])
    .unwrap();
    assert_eq!(v[0]["page"], 1);
    assert!(v[0]["markdown"].as_str().unwrap().starts_with("!["));
}

#[test]
fn markdown_writes_and_links_the_images_it_shows_when_asked() {
    // The corpus guide, tagged: 85 figures over eleven pages, each the
    // image drawn under its marked-content id, named by page and by the
    // image's place on the page in drawing order.
    let guide = "../../../benches/corpus/text_quick_start.pdf";
    let dir = scratch("markdown-images").unwrap();
    let dir_arg = dir.to_str().unwrap();
    // `-o DIR` writes the document into DIR beside its images, so the
    // Markdown is read back from the file rather than from stdout.
    stdout(&["extract", "markdown", guide, "-o", dir_arg]).unwrap();
    let md = std::fs::read_to_string(dir.join("text_quick_start.md")).unwrap();
    assert!(!md.contains("](image)"), "{md}");
    let links: Vec<&str> = md
        .lines()
        .filter(|l| l.starts_with("!["))
        .map(|l| l.rsplit_once("](").unwrap().1.trim_end_matches(')'))
        .collect();
    assert_eq!(links.len(), 85, "{md}");
    for link in &links {
        // A bare name, resolved from beside the document: a link carrying a
        // directory would not survive the directory being moved.
        assert!(!link.contains('/'), "{link}");
        assert!(dir.join(link).is_file(), "{link} was not written");
    }
    assert_eq!(links[0], "text_quick_start-p1-17.png");
    assert_eq!(links[1], "text_quick_start-p2-13.jpg");
    // The 85 images, plus the document itself.
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 86);
    // Read as a document, the running title and folio are gone.
    assert!(
        !md.lines()
            .any(|l| l == "Quick Guide" || l.contains("www.foxitsoftware.com")),
        "{md}"
    );
    // Without `-o` the placeholder stays and the text is the same.
    let plain = stdout(&["extract", "markdown", guide]).unwrap();
    assert_eq!(plain.matches("](image)").count(), 85);
    let words = |s: &str| {
        s.lines()
            .filter(|l| !l.starts_with("!["))
            .collect::<Vec<_>>()
            .join("\n")
    };
    assert_eq!(words(&plain), words(&md));
    // One page alone is read as a page; its files are still named by it.
    stdout(&["extract", "markdown", guide, "--pages", "2", "-o", dir_arg]).unwrap();
    let one = std::fs::read_to_string(dir.join("text_quick_start.md")).unwrap();
    assert!(one.contains("](text_quick_start-p2-13.jpg)"), "{one}");
    assert!(!one.contains("](image)"), "{one}");
}

#[test]
fn markdown_falls_back_to_typography_and_layout_keeps_columns() {
    assert_eq!(
        stdout(&["extract", "markdown", "fixtures/hello_world_2_pages.pdf"]).unwrap(),
        expected("markdown_hello_world_2_pages.txt").unwrap()
    );
    assert_eq!(
        stdout(&["extract", "text", "--layout", "fixtures/weblinks.pdf"]).unwrap(),
        expected("layout_weblinks.txt").unwrap()
    );
    let plain = stdout(&["extract", "text", "fixtures/weblinks.pdf"]).unwrap();
    let laid = expected("layout_weblinks.txt").unwrap();
    assert!(
        laid.lines().next().unwrap().starts_with("      "),
        "indented by its x: {laid:?}"
    );
    // Plain text is content order; the layout view is page order, top to
    // bottom, and this fixture draws its last line first. Same words, then.
    let mut plain_words: Vec<&str> = plain.split_whitespace().collect();
    let mut laid_words: Vec<&str> = laid.split_whitespace().collect();
    plain_words.sort_unstable();
    laid_words.sort_unstable();
    assert_eq!(plain_words, laid_words, "the same words, only placed");
}

// ---- phase 5: security encrypt -------------------------------------------

fn encrypt_hello(dir: &Path, name: &str) -> Result<std::path::PathBuf, String> {
    let locked = dir.join(name);
    let out = run(&[
        "security",
        "encrypt",
        "fixtures/hello_world_2_pages.pdf",
        "--user-password",
        "reader",
        "--owner-password",
        "owner",
        "--allow",
        "print",
        "--deterministic",
        "-o",
        locked.to_str().ok_or("non-utf8 path")?,
    ])
    .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned());
    }
    Ok(locked)
}

#[test]
fn encrypt_locks_the_file_with_the_permissions_asked_for() {
    let dir = scratch("encrypt").unwrap();
    let locked = encrypt_hello(&dir, "locked.pdf").unwrap();
    let refused = run(&["info", locked.to_str().unwrap()]).unwrap();
    assert_eq!(refused.status.code(), Some(1), "no password, no document");
    let v = json(&[
        "info",
        "--password",
        "reader",
        locked.to_str().unwrap(),
        "--json",
    ])
    .unwrap();
    assert_eq!(v["encrypted"], true);
    assert_eq!(v["permissions"]["print"], true);
    assert_eq!(v["permissions"]["copy"], false);
    let text = stdout(&[
        "extract",
        "text",
        "--password",
        "owner",
        locked.to_str().unwrap(),
    ])
    .unwrap();
    assert!(text.contains("Hello, world!") && text.contains("Goodbye, world!"));
    let bytes = std::fs::read(&locked).unwrap();
    assert!(
        !bytes.windows(5).any(|w| w == b"Hello"),
        "plaintext must not be in the file"
    );
    let again = encrypt_hello(&dir, "again.pdf").unwrap();
    assert_eq!(
        std::fs::read(&again).unwrap(),
        bytes,
        "--deterministic is byte-identical"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn encrypt_round_trips_through_decrypt_and_refuses_what_it_should() {
    let dir = scratch("encrypt2").unwrap();
    let locked = encrypt_hello(&dir, "locked.pdf").unwrap();
    let unlocked = dir.join("unlocked.pdf");
    let out = run(&[
        "security",
        "decrypt",
        "--password",
        "owner",
        locked.to_str().unwrap(),
        "-o",
        unlocked.to_str().unwrap(),
    ])
    .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        stdout(&["extract", "text", unlocked.to_str().unwrap()]).unwrap(),
        stdout(&["extract", "text", "fixtures/hello_world_2_pages.pdf"]).unwrap()
    );
    let twice = run(&[
        "security",
        "encrypt",
        "--password",
        "owner",
        locked.to_str().unwrap(),
        "--owner-password",
        "x",
        "-o",
        dir.join("x.pdf").to_str().unwrap(),
    ])
    .unwrap();
    assert_eq!(
        twice.status.code(),
        Some(1),
        "an encrypted input is refused"
    );
    assert!(String::from_utf8_lossy(&twice.stderr).contains("decrypt"));
    let bad = run(&[
        "security",
        "encrypt",
        "fixtures/hello_world_2_pages.pdf",
        "--owner-password",
        "x",
        "--allow",
        "fly",
        "-o",
        dir.join("y.pdf").to_str().unwrap(),
    ])
    .unwrap();
    assert_eq!(bad.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&bad.stderr).contains("--allow"));
    std::fs::remove_dir_all(&dir).unwrap();
}

/// The oracle opens what this crate encrypted: `pdfium_test --password`
/// extracts the same text from the locked file as from the plain one. Skipped
/// when the checkout is not on this machine.
#[test]
fn the_oracle_opens_what_we_encrypted() {
    let Some(checkout) = std::env::var_os("PDFRUM_ORACLE_CHECKOUT") else {
        eprintln!("PDFRUM_ORACLE_CHECKOUT unset: oracle round trip skipped");
        return;
    };
    let pdfium_test = Path::new(&checkout).join("out/Release/pdfium_test");
    if !pdfium_test.exists() {
        eprintln!(
            "{}: not built; oracle round trip skipped",
            pdfium_test.display()
        );
        return;
    }
    let dir = scratch("encrypt-oracle").unwrap();
    let plain = dir.join("plain.pdf");
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/hello_world_2_pages.pdf"),
        &plain,
    )
    .unwrap();
    let locked = encrypt_hello(&dir, "locked.pdf").unwrap();
    let oracle_text = |file: &Path, password: Option<&str>| -> String {
        let mut cmd = Command::new(&pdfium_test);
        cmd.arg("--txt");
        if let Some(p) = password {
            cmd.arg(format!("--password={p}"));
        }
        let out = cmd.arg(file).output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let mut text = String::new();
        for n in 0..2 {
            let page = file.with_extension(format!("pdf.{n}.txt"));
            let bytes = std::fs::read(&page).unwrap_or_else(|e| panic!("{}: {e}", page.display()));
            // pdfium_test writes UTF-16LE with a BOM.
            let units: Vec<u16> = bytes
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect();
            text.push_str(&String::from_utf16_lossy(&units));
        }
        text
    };
    assert_eq!(
        oracle_text(&locked, Some("reader")),
        oracle_text(&plain, None)
    );
    assert!(!oracle_text(&plain, None).is_empty());
    std::fs::remove_dir_all(&dir).unwrap();
}

// ---- phase 5: forensics and polish -----------------------------------------

#[test]
fn inspect_object_prints_pdf_syntax_and_decodes_a_stream() {
    assert_eq!(
        stdout(&[
            "inspect",
            "object",
            fx("fixtures/hello_world_2_pages.pdf"),
            "1"
        ])
        .unwrap(),
        expected("object_hello_catalog.txt").unwrap()
    );
    // Object 7 is the first page's content stream: 83 bytes of Flate.
    let content = stdout(&[
        "inspect",
        "object",
        fx("fixtures/hello_world_2_pages.pdf"),
        "7",
        "--decode",
    ])
    .unwrap();
    assert!(content.contains("(Hello, world!) Tj"), "{content}");
    let not_a_stream = run(&[
        "inspect",
        "object",
        fx("fixtures/hello_world_2_pages.pdf"),
        "1",
        "--decode",
    ])
    .unwrap();
    assert_eq!(not_a_stream.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&not_a_stream.stderr).contains("not a stream"));
    let missing = run(&[
        "inspect",
        "object",
        fx("fixtures/hello_world_2_pages.pdf"),
        "99",
    ])
    .unwrap();
    assert_eq!(missing.status.code(), Some(1));
}

#[test]
fn inspect_xref_shows_every_entry_and_the_merged_trailer() {
    assert_eq!(
        stdout(&["inspect", "xref", fx("fixtures/bug_1484283.pdf")]).unwrap(),
        expected("xref_bug_1484283.txt").unwrap()
    );
    let v = json(&["inspect", "xref", fx("fixtures/bug_1484283.pdf"), "--json"]).unwrap();
    assert_eq!(v["rebuilt"], false);
    assert_eq!(v["entries"], 6);
    let rows = v["rows"].as_array().unwrap();
    let in_stream = rows.iter().find(|r| r["kind"] == "in_stream").unwrap();
    assert_eq!(in_stream["object"], 2);
    assert_eq!(in_stream["stream"], 5);
    assert_eq!(in_stream["index"], 0);
    assert!(
        rows.iter()
            .all(|r| r["kind"] != "offset" || r["offset"].is_number())
    );
    let rebuilt = json(&[
        "inspect",
        "xref",
        fx("fixtures/parser_rebuildxref_correct.pdf"),
        "--json",
    ])
    .unwrap();
    assert_eq!(rebuilt["rebuilt"], true);
}

#[test]
fn inspect_revisions_lists_the_chain_and_revision_writes_an_earlier_file() {
    assert_eq!(
        stdout(&["inspect", "revisions", fx("fixtures/bug_1484283.pdf")]).unwrap(),
        expected("revisions_bug_1484283.txt").unwrap()
    );
    let v = json(&[
        "inspect",
        "revisions",
        fx("fixtures/bug_1484283.pdf"),
        "--json",
    ])
    .unwrap();
    assert_eq!(v.as_array().unwrap().len(), 2);
    assert_eq!(v[0]["xref_stream"], false);
    assert_eq!(v[1]["xref_stream"], true);
    assert_eq!(v[1]["end"], 1399);

    let dir = scratch("revision").unwrap();
    let first = dir.join("rev1.pdf");
    stdout(&[
        "inspect",
        "revision",
        fx("fixtures/bug_1484283.pdf"),
        "--rev",
        "1",
        "-o",
        first.to_str().unwrap(),
    ])
    .unwrap();
    // The first revision is the file's first 633 bytes, and opens on its own.
    let whole =
        std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/bug_1484283.pdf"))
            .unwrap();
    assert_eq!(std::fs::read(&first).unwrap(), whole[..633]);
    let opened = json(&["info", first.to_str().unwrap(), "--json"]).unwrap();
    assert_eq!(opened["pages"], 1);

    let out_of_range = run(&[
        "inspect",
        "revision",
        fx("fixtures/bug_1484283.pdf"),
        "--rev",
        "3",
        "-o",
        dir.join("none.pdf").to_str().unwrap(),
    ])
    .unwrap();
    assert_eq!(out_of_range.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out_of_range.stderr).contains("1..=2"));

    // A single-revision file has a chain of one; a rebuilt one has none.
    let one = json(&[
        "inspect",
        "revisions",
        fx("fixtures/hello_world_2_pages.pdf"),
        "--json",
    ])
    .unwrap();
    assert_eq!(one.as_array().unwrap().len(), 1);
    let none = stdout(&[
        "inspect",
        "revisions",
        fx("fixtures/parser_rebuildxref_correct.pdf"),
    ])
    .unwrap();
    assert!(none.contains("no revisions"));
}

#[test]
fn inspect_structure_walks_the_tree_with_its_content_ids_and_text() {
    assert_eq!(
        stdout(&["inspect", "structure", fx("fixtures/tagged_alt_text.pdf")]).unwrap(),
        expected("structure_tagged_alt_text.txt").unwrap()
    );
    let v = json(&[
        "inspect",
        "structure",
        fx("fixtures/tagged_actual_text.pdf"),
        "--json",
    ])
    .unwrap();
    let rows = v.as_array().unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["kind"], "Document");
    assert_eq!(rows[0]["depth"], 0);
    assert_eq!(rows[2]["kind"], "Figure");
    assert_eq!(rows[2]["depth"], 2);
    assert_eq!(rows[2]["content_ids"], serde_json::json!([0]));
    assert_eq!(rows[2]["actual_text"], "Actual Text");
    let untagged = stdout(&[
        "inspect",
        "structure",
        fx("fixtures/hello_world_2_pages.pdf"),
    ])
    .unwrap();
    assert!(untagged.contains("no structure tree"));
}

#[test]
fn extract_images_keeps_jpeg_bytes_and_decodes_the_rest_to_png() {
    assert_eq!(
        stdout(&["extract", "images", fx("fixtures/rotated_image.pdf")]).unwrap(),
        expected("images_rotated_image.txt").unwrap()
    );
    let dir = scratch("images").unwrap();
    // A JPEG placed by `pages create` comes back out byte for byte.
    let mona = dir.join("mona.pdf");
    stdout(&[
        "pages",
        "create",
        fx("fixtures/mona_lisa.jpg"),
        "--deterministic",
        "-o",
        mona.to_str().unwrap(),
    ])
    .unwrap();
    let out = dir.join("out");
    let v = json(&[
        "extract",
        "images",
        mona.to_str().unwrap(),
        "-o",
        out.to_str().unwrap(),
        "--json",
    ])
    .unwrap();
    assert_eq!(v[0]["format"], "jpg");
    assert_eq!(v[0]["width"], 120);
    assert_eq!(v[0]["uses"], 1);
    let written = v[0]["written"].as_str().unwrap();
    assert!(written.ends_with("mona-1.jpg"), "{written}");
    assert_eq!(
        std::fs::read(written).unwrap(),
        std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mona_lisa.jpg"))
            .unwrap()
    );
    // A Flate image and an image mask are decoded to PNG.
    let v = json(&[
        "extract",
        "images",
        fx("fixtures/bug_674771.pdf"),
        "-o",
        out.to_str().unwrap(),
        "--json",
    ])
    .unwrap();
    assert_eq!(v[0]["is_mask"], true);
    assert_eq!(v[0]["format"], "png");
    let png = std::fs::read(v[0]["written"].as_str().unwrap()).unwrap();
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    assert!(
        stdout(&["extract", "images", fx("fixtures/hello_world_2_pages.pdf")])
            .unwrap()
            .contains("no images")
    );
}

#[test]
fn extract_images_folds_repeated_draws_and_leaves_spacers_out_unless_asked() {
    // The corpus guide draws one 15x15 bullet sixteen times and no spacers;
    // the FQA file draws two-pixel spacers 301 times and nothing else.
    let guide = "../../../benches/corpus/text_quick_start.pdf";
    let v = json(&["extract", "images", guide, "--json"]).unwrap();
    let bullet = v
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["width"] == 15 && r["height"] == 15)
        .expect("the bullet is listed once");
    assert_eq!(bullet["uses"], 16);
    assert_eq!(bullet["object"], 33);
    let all = json(&["extract", "images", guide, "--all", "--json"]).unwrap();
    assert!(all.as_array().unwrap().len() > v.as_array().unwrap().len());
    assert!(all.as_array().unwrap().iter().all(|r| r["uses"] == 1));

    let fqa = "../../../benches/corpus/image_en_fqa.pdf";
    assert!(
        stdout(&["extract", "images", fqa])
            .unwrap()
            .contains("no images")
    );
    let spacers = json(&["extract", "images", fqa, "--all", "--json"]).unwrap();
    assert_eq!(spacers.as_array().unwrap().len(), 301);
    assert!(spacers.as_array().unwrap().iter().all(|r| r["width"] == 2));
}

#[test]
fn extract_words_lists_every_word_with_its_box_font_and_size() {
    assert_eq!(
        stdout(&["extract", "words", fx("fixtures/hello_world_2_pages.pdf")]).unwrap(),
        expected("words_hello_world_2_pages.txt").unwrap()
    );
    let v = json(&[
        "extract",
        "words",
        fx("fixtures/hello_world_2_pages.pdf"),
        "--pages",
        "2",
        "--json",
    ])
    .unwrap();
    let rows = v.as_array().unwrap();
    assert_eq!(rows.len(), 4, "{v}");
    let texts: Vec<&str> = rows.iter().map(|r| r["text"].as_str().unwrap()).collect();
    assert_eq!(texts, ["Hello,", "world!", "Goodbye,", "world!"]);
    assert!(rows.iter().all(|r| r["page"] == 2));
    assert_eq!(rows[0]["font"], "Times-Roman");
    assert_eq!(rows[0]["size"], 12.0);
    assert_eq!(rows[2]["font"], "Helvetica");
    assert_eq!(rows[2]["size"], 16.0);
    // The range slices the word back out of the page's text.
    assert_eq!(rows[0]["start"], 0);
    assert_eq!(rows[0]["end"], 6);
    assert_eq!(rows[1]["start"], 7);
    for r in rows {
        for key in ["x0", "y0", "x1", "y1"] {
            assert!(r[key].is_number(), "{r}");
        }
        assert!(r["x0"].as_f64() < r["x1"].as_f64(), "{r}");
    }
    // Left to right on a line, and the second line above the first in
    // y-up page space.
    assert!(rows[0]["x1"].as_f64() <= rows[1]["x0"].as_f64());
    assert!(rows[2]["y0"].as_f64() > rows[0]["y1"].as_f64());
    // The corpus guide's first page has 33 words -- the oracle's own count
    // for this page, which our text has matched byte for byte since M28.
    let guide = json(&[
        "extract",
        "words",
        "../../../benches/corpus/text_quick_start.pdf",
        "--pages",
        "1",
        "--json",
    ])
    .unwrap();
    assert_eq!(guide.as_array().unwrap().len(), 33);
    assert!(
        stdout(&["extract", "words", fx("fixtures/bug_674771.pdf")])
            .unwrap()
            .contains("no words")
    );
}

/// A small PDF written by hand with a correct cross-reference table: one
/// indirect object per entry of `objects`, numbered from 1, object 1 the
/// catalog.
fn pdf_of(objects: &[&str]) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for offset in offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    out
}

#[test]
fn inspect_object_json_encodes_every_kind_and_hints_at_references() {
    use serde_json::json as j;
    let v = json(&[
        "inspect",
        "object",
        fx("fixtures/hello_world_2_pages.pdf"),
        "1",
        "--json",
    ])
    .unwrap();
    assert_eq!(v["object"], 1);
    assert_eq!(v["generation"], 0);
    assert_eq!(v["value"]["Type"], j!({"name": "Catalog"}));
    assert_eq!(v["value"]["Pages"], j!({"ref": [2, 0]}));
    assert_eq!(v["hints"], j!({"Pages": "Pages"}), "{v}");
    // A stream is its dictionary and a summary; the data is `--decode`'s.
    let v = json(&[
        "inspect",
        "object",
        fx("fixtures/hello_world_2_pages.pdf"),
        "7",
        "--json",
    ])
    .unwrap();
    assert_eq!(v["value"]["dict"]["Length"], 83);
    assert_eq!(v["value"]["stream"], j!({"length": 83, "filters": []}));
    assert_eq!(v["hints"], j!({}));
    let v = json(&[
        "inspect",
        "object",
        fx("fixtures/rotated_image.pdf"),
        "5",
        "--json",
    ])
    .unwrap();
    assert_eq!(
        v["value"]["stream"]["filters"],
        j!(["ASCIIHexDecode", "FlateDecode"])
    );
    assert_eq!(v["value"]["dict"]["Subtype"], j!({"name": "Image"}));
    // Strings: text when they decode to text, the bytes as hex when not.
    let dir = scratch("object_json").unwrap();
    let file = dir.join("strings.pdf");
    std::fs::write(
        &file,
        pdf_of(&[
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [4 0 R] /Count 1 >>",
            "<< /Text (Hello) /Bin <01FF03> /Uni <FEFF00E9> /Real 1.5 /On true /Nothing null /List [1 (a) /N] /Text (again) >>",
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] >>",
        ]),
    )
    .unwrap();
    let v = json(&["inspect", "object", file.to_str().unwrap(), "3", "--json"]).unwrap();
    assert_eq!(
        v["value"],
        j!({
            "Text": {"string": "again"},
            "Bin": {"hex": "01ff03"},
            "Uni": {"string": "\u{e9}"},
            "Real": 1.5,
            "On": true,
            "Nothing": null,
            "List": [1, {"string": "a"}, {"name": "N"}],
        }),
        "{v}"
    );
    let v = json(&["inspect", "object", file.to_str().unwrap(), "4", "--json"]).unwrap();
    assert_eq!(v["hints"], j!({"Parent": "Pages"}));
    assert_eq!(v["value"]["MediaBox"], j!([0, 0, 10, 10]));
    let both = run(&[
        "inspect",
        "object",
        fx("fixtures/hello_world_2_pages.pdf"),
        "7",
        "--json",
        "--decode",
    ])
    .unwrap();
    assert_eq!(both.status.code(), Some(2), "they conflict");
}

/// The keys of a JSON document's first object: the object itself, or the
/// first element of an array.
fn keys_of(v: &serde_json::Value) -> Result<Vec<String>, String> {
    let object = v.as_array().map_or(v, |a| &a[0]);
    let mut keys: Vec<String> = object
        .as_object()
        .ok_or_else(|| format!("not an object: {object}"))?
        .keys()
        .cloned()
        .collect();
    keys.sort();
    Ok(keys)
}

/// Each `--json` command with a fixture run that prints every key it has a
/// value for; the schema's example must carry all of them.
fn schema_runs() -> Vec<(Vec<&'static str>, Vec<&'static str>)> {
    let hello = "fixtures/hello_world_2_pages.pdf";
    vec![
        (vec!["extract", "words"], vec!["extract", "words", hello]),
        (vec!["info"], vec!["info", "fixtures/two_signatures.pdf"]),
        (
            vec!["doctor"],
            vec!["doctor", "fixtures/parser_rebuildxref_correct.pdf"],
        ),
        (vec!["search"], vec!["search", "world", hello]),
        (vec!["hash"], vec!["hash", hello]),
        (vec!["diff"], vec!["diff", hello, "fixtures/bookmarks.pdf"]),
        (vec!["extract", "text"], vec!["extract", "text", hello]),
        (
            vec!["extract", "links"],
            vec!["extract", "links", "fixtures/annots_action_handling.pdf"],
        ),
        (
            vec!["extract", "toc"],
            vec!["extract", "toc", "fixtures/bookmarks.pdf"],
        ),
        (
            vec!["extract", "attachments"],
            vec![
                "extract",
                "attachments",
                "fixtures/embedded_attachments_with_desc.pdf",
            ],
        ),
        (
            vec!["extract", "annotations"],
            vec!["extract", "annotations", "fixtures/annotiter.pdf"],
        ),
        (
            vec!["extract", "signatures"],
            vec!["extract", "signatures", "fixtures/two_signatures.pdf"],
        ),
        (
            vec!["extract", "images"],
            vec!["extract", "images", "fixtures/rotated_image.pdf"],
        ),
        (
            vec!["extract", "fonts"],
            vec!["extract", "fonts", "fixtures/bigtable_mini.pdf"],
        ),
        (
            vec!["forms", "dump"],
            vec!["forms", "dump", "fixtures/text_form.pdf"],
        ),
        (
            vec!["inspect", "object"],
            vec!["inspect", "object", hello, "1"],
        ),
        (
            vec!["inspect", "xref"],
            vec!["inspect", "xref", "fixtures/bug_1484283.pdf"],
        ),
        (
            vec!["inspect", "revisions"],
            vec!["inspect", "revisions", "fixtures/bug_1484283.pdf"],
        ),
        (
            vec!["inspect", "structure"],
            vec!["inspect", "structure", "fixtures/tagged_alt_text.pdf"],
        ),
    ]
}

#[test]
fn schema_lists_the_json_commands_and_its_examples_carry_every_real_key() {
    let listing = stdout(&["schema"]).unwrap();
    let header = listing.lines().next().unwrap_or_default();
    assert!(
        header.starts_with("COMMAND") && header.ends_with("DESCRIPTION"),
        "{listing}"
    );
    for command in ["info", "extract words", "inspect object", "hash", "diff"] {
        assert!(
            listing
                .lines()
                .any(|l| l.starts_with(&format!("{command}  "))),
            "{command} is not listed:\n{listing}"
        );
    }
    // Each example has every key the command printed on a fixture (an
    // optional key the fixture lacks is still in the example).
    for (schema, real) in schema_runs() {
        let mut args = vec!["schema"];
        args.extend(&schema);
        let example = json(&args).unwrap();
        let mut args = real.clone();
        args.push("--json");
        let out = run(&args).unwrap();
        let printed: serde_json::Value =
            serde_json::from_slice(&out.stdout).unwrap_or_else(|e| panic!("{real:?}: {e}"));
        assert_eq!(example.is_array(), printed.is_array(), "{schema:?}");
        let expected = keys_of(&example).unwrap();
        for key in keys_of(&printed).unwrap() {
            assert!(
                expected.contains(&key),
                "{schema:?}: `{key}` is not in the schema"
            );
        }
    }
    let unknown = run(&["schema", "extract", "nothing"]).unwrap();
    assert_eq!(unknown.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&unknown.stderr).contains("no --json output"));
}

#[test]
fn inspect_object_hints_at_what_a_reference_is() {
    let text = stdout(&[
        "inspect",
        "object",
        fx("fixtures/hello_world_2_pages.pdf"),
        "1",
    ])
    .unwrap();
    assert!(text.contains("/Pages 2 0 R  % Pages"), "{text}");
    let xref = stdout(&["inspect", "xref", fx("fixtures/bug_1484283.pdf")]).unwrap();
    assert!(xref.contains("/Root 1 0 R  % Catalog"), "{xref}");
    assert!(xref.contains("ObjStm, stream raw"), "{xref}");
}

#[test]
fn extract_fonts_lists_and_writes_embedded_programs() {
    assert_eq!(
        stdout(&["extract", "fonts", fx("fixtures/bigtable_mini.pdf")]).unwrap(),
        expected("fonts_bigtable_mini.txt").unwrap()
    );
    let dir = scratch("fonts").unwrap();
    let v = json(&[
        "extract",
        "fonts",
        fx("fixtures/bug_488948351.pdf"),
        "-o",
        dir.to_str().unwrap(),
        "--json",
    ])
    .unwrap();
    assert_eq!(v[0]["kind"], "truetype");
    assert_eq!(v[0]["name"], "AAAAAI+CambriaMath");
    assert_eq!(v[0]["size"], 883);
    let written = v[0]["written"].as_str().unwrap();
    assert!(written.ends_with("AAAAAI+CambriaMath-8.ttf"), "{written}");
    let ttf = std::fs::read(written).unwrap();
    assert_eq!(ttf.len(), 883);
    assert!(
        ttf.starts_with(&[0, 1, 0, 0]) || ttf.starts_with(b"true"),
        "a TrueType program starts with its version tag: {:?}",
        &ttf[..4]
    );
    assert!(
        stdout(&["extract", "fonts", fx("fixtures/hello_world_2_pages.pdf")])
            .unwrap()
            .contains("no fonts")
    );
}

#[test]
fn diff_reports_text_and_pixels_per_page_and_exits_1_on_a_difference() {
    let same = run(&[
        "diff",
        fx("fixtures/hello_world_2_pages.pdf"),
        fx("fixtures/hello_world_2_pages.pdf"),
        "--visual",
    ])
    .unwrap();
    assert!(same.status.success(), "{same:?}");
    assert!(same.stdout.is_empty());

    let dir = scratch("diff").unwrap();
    let one_page = dir.join("one.pdf");
    stdout(&[
        "pages",
        "slice",
        fx("fixtures/hello_world_2_pages.pdf"),
        "--pages",
        "1",
        "-o",
        one_page.to_str().unwrap(),
    ])
    .unwrap();
    let out = run(&[
        "diff",
        fx("fixtures/hello_world_2_pages.pdf"),
        one_page.to_str().unwrap(),
        "--visual",
        "-o",
        dir.join("pictures").to_str().unwrap(),
    ])
    .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.starts_with("pages  2 vs 1\n"), "{text}");
    assert!(
        text.contains("page 2\n  - Hello, world!\n  - Goodbye, world!\n"),
        "{text}"
    );
    assert!(
        !text.contains("page 1\n"),
        "page 1 is the same in both: {text}"
    );
    assert!(dir.join("pictures/diff-2.png").exists());
    assert!(!dir.join("pictures/diff-1.png").exists());

    // Exit 1 carries the JSON too: different is the answer, not a failure.
    let out = run(&[
        "diff",
        fx("fixtures/hello_world_2_pages.pdf"),
        fx("fixtures/weblinks.pdf"),
        "--json",
    ])
    .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["left_pages"], 2);
    assert_eq!(v["right_pages"], 1);
    assert_eq!(
        v["pages"][0]["removed"],
        serde_json::json!(["Hello, world!", "Goodbye, world!"])
    );
    assert_eq!(v["pages"][0]["added"].as_array().unwrap().len(), 7);
    assert!(v["pages"][0]["pixels"].is_null());
}

#[test]
fn hash_prints_three_fingerprints_and_the_semantic_one_ignores_the_save() {
    let v = json(&["hash", fx("fixtures/hello_world_2_pages.pdf"), "--json"]).unwrap();
    assert_eq!(
        v["sha256"],
        "67431cbec27df7cc86a57da5ff11b5151628fa93f661da79b9d272ec85bcdb03"
    );
    assert_eq!(v["objects"], 7);
    assert!(v["id"].is_null(), "the fixture has no /ID");
    let semantic = v["semantic"].as_str().unwrap().to_owned();
    assert_eq!(semantic.len(), 64);

    // Two rewrites with fresh /IDs: different files, the same document.
    let dir = scratch("hash").unwrap();
    let (a, b) = (dir.join("a.pdf"), dir.join("b.pdf"));
    for path in [&a, &b] {
        stdout(&[
            "repair",
            fx("fixtures/hello_world_2_pages.pdf"),
            "-o",
            path.to_str().unwrap(),
        ])
        .unwrap();
    }
    let ha = json(&["hash", a.to_str().unwrap(), "--json"]).unwrap();
    let hb = json(&["hash", b.to_str().unwrap(), "--json"]).unwrap();
    assert_ne!(ha["id"], hb["id"]);
    assert_ne!(ha["sha256"], hb["sha256"]);
    assert_eq!(ha["semantic"], hb["semantic"]);
    assert_ne!(
        ha["semantic"].as_str().unwrap(),
        semantic,
        "a rewrite renumbers"
    );

    let text = stdout(&["hash", a.to_str().unwrap()]).unwrap();
    assert!(text.starts_with("file          "), "{text}");
    assert!(text.contains("\ndocument id   "), "{text}");
    assert!(text.contains("\ncontent hash  "), "{text}");
}

#[test]
fn completions_and_manual_pages_come_from_the_command_tree() {
    let bash = stdout(&["completions", "bash"]).unwrap();
    assert!(bash.contains("_pdfrum()"), "{bash}");
    assert!(bash.contains("inspect"), "{bash}");
    let zsh = stdout(&["completions", "zsh"]).unwrap();
    assert!(zsh.starts_with("#compdef pdfrum"), "{zsh}");
    let fish = stdout(&["completions", "fish"]).unwrap();
    assert!(
        fish.contains("__fish_seen_subcommand_from extract"),
        "{fish}"
    );

    let dir = scratch("man").unwrap();
    let report = stdout(&["manpage", "-o", dir.to_str().unwrap()]).unwrap();
    assert!(report.contains("manual pages"), "{report}");
    let root = std::fs::read_to_string(dir.join("pdfrum.1")).unwrap();
    assert!(root.starts_with(".ie"), "{root}");
    assert!(root.contains(".SH NAME"), "{root}");
    let leaf = std::fs::read_to_string(dir.join("pdfrum-extract-text.1")).unwrap();
    assert!(leaf.contains("pdfrum\\-extract\\-text"), "{leaf}");
    assert!(leaf.contains("layout"), "{leaf}");
    assert!(
        !dir.join("pdfrum-help.1").exists(),
        "clap's help gets no page"
    );
    let count = std::fs::read_dir(&dir).unwrap().count();
    assert!(count > 40, "{count} pages");
}

// ---- M20 phase 1: composition ----------------------------------------------

#[test]
fn a_dash_reads_the_document_from_stdin() {
    let bytes = fixture("two_signatures.pdf").unwrap();
    let piped = run_with(&["info", "-"], &bytes, &[]).unwrap();
    assert!(
        piped.status.success(),
        "{}",
        String::from_utf8_lossy(&piped.stderr)
    );
    let piped = String::from_utf8(piped.stdout).unwrap();
    let from_file = expected("info_two_signatures.txt").unwrap();
    assert!(piped.starts_with("file         -\n"), "{piped}");
    assert_eq!(
        piped.lines().skip(1).collect::<Vec<_>>(),
        from_file.lines().skip(1).collect::<Vec<_>>(),
        "the same report but for the file line"
    );

    let text = run_with(
        &["extract", "text", "-"],
        &fixture("hello_world_2_pages.pdf").unwrap(),
        &[],
    )
    .unwrap();
    assert_eq!(
        String::from_utf8(text.stdout).unwrap(),
        expected("text_hello_world_2_pages.txt").unwrap()
    );

    let locked = run_with(
        &["info", "-", "--password", "1234", "--json"],
        &fixture("encrypted.pdf").unwrap(),
        &[],
    )
    .unwrap();
    assert!(locked.status.success(), "{locked:?}");
    let v: serde_json::Value = serde_json::from_slice(&locked.stdout).unwrap();
    assert_eq!(v["file"], "-");
    assert_eq!(v["encrypted"], true);
    let refused = run_with(&["info", "-"], &fixture("encrypted.pdf").unwrap(), &[]).unwrap();
    assert_eq!(refused.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("cannot open -"),
        "{refused:?}"
    );

    // `hash -` is the file's own hash: the bytes are the bytes.
    let h = run_with(
        &["hash", "-", "--json"],
        &fixture("hello_world_2_pages.pdf").unwrap(),
        &[],
    )
    .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&h.stdout).unwrap();
    assert_eq!(
        v["sha256"],
        "67431cbec27df7cc86a57da5ff11b5151628fa93f661da79b9d272ec85bcdb03"
    );

    // Files derived from stdin are named after it.
    let dir = scratch("stdin-render").unwrap();
    let template = dir.join("{stem}-{n}.png");
    let out = run_with(
        &[
            "render",
            "--dpi",
            "36",
            "-o",
            template.to_str().unwrap(),
            "-",
        ],
        &fixture("hello_world_2_pages.pdf").unwrap(),
        &[],
    )
    .unwrap();
    assert!(out.status.success(), "{out:?}");
    assert!(dir.join("stdin-1.png").exists());
    std::fs::remove_dir_all(&dir).unwrap();

    for verb in ["preview", "view"] {
        let out = run_with(&[verb, "-"], &bytes, &[]).unwrap();
        assert_eq!(out.status.code(), Some(1), "{verb} refuses stdin");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("give a path"),
            "{out:?}"
        );
    }
}

#[test]
fn a_dash_output_writes_the_pdf_to_stdout_and_the_summary_to_stderr() {
    // The chain from PLAN.md: slice to stdout, read it back from stdin.
    let sliced = run(&[
        "pages",
        "slice",
        "fixtures/hello_world_2_pages.pdf",
        "--pages",
        "1",
        "-o",
        "-",
    ])
    .unwrap();
    assert!(sliced.status.success(), "{sliced:?}");
    assert!(
        sliced.stdout.starts_with(b"%PDF-"),
        "the PDF and nothing else"
    );
    assert_eq!(
        String::from_utf8_lossy(&sliced.stderr),
        "pdfrum: -: 1 pages\n",
        "the summary is a notice"
    );
    let text = run_with(&["extract", "text", "-"], &sliced.stdout, &[]).unwrap();
    assert_eq!(
        String::from_utf8_lossy(&text.stdout),
        "Hello, world!\nGoodbye, world!\n",
        "page 1 alone, no form feed"
    );
    let info = run_with(&["info", "-", "--json"], &sliced.stdout, &[]).unwrap();
    let v: serde_json::Value = serde_json::from_slice(&info.stdout).unwrap();
    assert_eq!(v["pages"], 1);
}

#[test]
fn every_writing_command_takes_a_dash_output() {
    let dir = scratch("dash-out").unwrap();
    let data = dir.join("fill.json");
    std::fs::write(&data, r#"{"Text Box": "x"}"#).unwrap();
    let data = data.to_str().unwrap();
    let commands: Vec<Vec<&str>> = vec![
        vec![
            "pages",
            "merge",
            "fixtures/hello_world_2_pages.pdf",
            "fixtures/bookmarks.pdf",
        ],
        vec![
            "pages",
            "reorder",
            "fixtures/hello_world_2_pages.pdf",
            "--pages",
            "2,1",
        ],
        vec!["pages", "create", "fixtures/mona_lisa.jpg"],
        vec!["pages", "nup", "fixtures/bookmarks.pdf"],
        vec!["pages", "booklet", "fixtures/hello_world_2_pages.pdf"],
        vec!["forms", "fill", "fixtures/text_form.pdf", "--data", data],
        vec!["forms", "flatten", "fixtures/text_form.pdf"],
        vec!["repair", "fixtures/parser_rebuildxref_correct.pdf"],
        vec!["optimize", "fixtures/bookmarks.pdf"],
        vec![
            "security",
            "decrypt",
            "--password",
            "1234",
            "fixtures/encrypted.pdf",
        ],
        vec![
            "security",
            "encrypt",
            "fixtures/bookmarks.pdf",
            "--owner-password",
            "x",
        ],
        vec![
            "inspect",
            "revision",
            "fixtures/bug_1484283.pdf",
            "--rev",
            "1",
        ],
    ];
    for mut args in commands {
        args.extend(["-o", "-"]);
        let out = run(&args).unwrap();
        assert!(out.status.success(), "{args:?}: {out:?}");
        assert!(out.stdout.starts_with(b"%PDF-"), "{args:?}");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("pdfrum: -: "), "{args:?}: {err}");
    }
    let png = run(&[
        "render",
        "--pages",
        "1",
        "fixtures/hello_world_2_pages.pdf",
        "-o",
        "-",
    ])
    .unwrap();
    assert!(png.stdout.starts_with(b"\x89PNG"));
    assert!(
        String::from_utf8_lossy(&png.stderr).starts_with("pdfrum: -: page 1, "),
        "{png:?}"
    );
    let split = run(&[
        "pages",
        "split",
        "fixtures/hello_world_2_pages.pdf",
        "-o",
        "-",
    ])
    .unwrap();
    assert_eq!(split.status.code(), Some(1), "split writes many files");
    assert!(String::from_utf8_lossy(&split.stderr).contains("directory"));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn info_hash_and_doctor_take_several_files_and_go_on_past_a_bad_one() {
    assert_eq!(
        stdout(&[
            "info",
            "fixtures/two_signatures.pdf",
            "fixtures/bookmarks.pdf"
        ])
        .unwrap(),
        expected("info_two_files.txt").unwrap(),
        "one record per file, a blank line between"
    );
    let v = json(&[
        "info",
        "fixtures/two_signatures.pdf",
        "fixtures/bookmarks.pdf",
        "--json",
    ])
    .unwrap();
    assert_eq!(v[0]["file"], "fixtures/two_signatures.pdf");
    assert_eq!(v[1]["file"], "fixtures/bookmarks.pdf");
    assert_eq!(v[1]["pages"], 2);
    let one = json(&["info", "fixtures/bookmarks.pdf", "--json"]).unwrap();
    assert!(one.is_object(), "one file stays one document");

    let v = json(&[
        "hash",
        "fixtures/hello_world_2_pages.pdf",
        "fixtures/bookmarks.pdf",
        "--json",
    ])
    .unwrap();
    assert_eq!(v.as_array().map(Vec::len), Some(2));
    assert_eq!(v[0]["objects"], 7);

    // A missing file is reported and the rest are still answered; exit 1.
    let out = run(&[
        "hash",
        "fixtures/nonesuch.pdf",
        "fixtures/hello_world_2_pages.pdf",
    ])
    .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("nonesuch.pdf"));
    assert!(
        String::from_utf8_lossy(&out.stdout).starts_with("file          fixtures/hello_world"),
        "{out:?}"
    );

    let strict = run(&[
        "doctor",
        "--strict",
        "fixtures/hello_world_2_pages.pdf",
        "fixtures/parser_rebuildxref_correct.pdf",
    ])
    .unwrap();
    assert_eq!(strict.status.code(), Some(3), "any file with notices");
    let text = String::from_utf8_lossy(&strict.stdout);
    assert!(
        text.starts_with("file   fixtures/hello_world_2_pages.pdf\nstate  clean\n\nfile"),
        "{text}"
    );
    let v = json(&[
        "doctor",
        "fixtures/hello_world_2_pages.pdf",
        "fixtures/parser_rebuildxref_correct.pdf",
        "--json",
    ])
    .unwrap();
    assert_eq!(v[1]["recovered"], 4);
}

#[test]
fn search_names_the_file_for_several_files_like_grep() {
    let two = run(&[
        "search",
        "-i",
        "world",
        "fixtures/hello_world_2_pages.pdf",
        "fixtures/bookmarks.pdf",
    ])
    .unwrap();
    assert!(two.status.success(), "{two:?}");
    let text = String::from_utf8_lossy(&two.stdout);
    assert!(
        text.starts_with("fixtures/hello_world_2_pages.pdf:page 1:Hello, world!\n"),
        "{text}"
    );
    assert_eq!(text.lines().count(), 4);
    let quiet = stdout(&[
        "search",
        "world",
        "--no-filename",
        "fixtures/hello_world_2_pages.pdf",
        "fixtures/bookmarks.pdf",
    ])
    .unwrap();
    assert!(quiet.starts_with("page 1:"), "{quiet}");
    let named = stdout(&["search", "-H", "world", "fixtures/hello_world_2_pages.pdf"]).unwrap();
    assert!(
        named.starts_with("fixtures/hello_world_2_pages.pdf:page 1:"),
        "{named}"
    );

    let none = run(&[
        "search",
        "nonesuch",
        "fixtures/hello_world_2_pages.pdf",
        "fixtures/bookmarks.pdf",
    ])
    .unwrap();
    assert_eq!(none.status.code(), Some(1), "no file had a hit");
    let bad = run(&[
        "search",
        "world",
        "fixtures/nonesuch.pdf",
        "fixtures/hello_world_2_pages.pdf",
    ])
    .unwrap();
    assert_eq!(bad.status.code(), Some(1), "hits, but a file failed");
    assert!(String::from_utf8_lossy(&bad.stdout).contains(":page 1:"));

    let v = json(&[
        "search",
        "world",
        "fixtures/hello_world_2_pages.pdf",
        "fixtures/bookmarks.pdf",
        "--json",
    ])
    .unwrap();
    assert_eq!(v[0]["file"], "fixtures/hello_world_2_pages.pdf");
    assert_eq!(v[0]["hits"].as_array().map(Vec::len), Some(4));
    assert_eq!(v[1]["hits"].as_array().map(Vec::len), Some(0));
}

#[test]
fn quiet_drops_the_notices_and_verbose_lists_every_diagnostic() {
    let damaged = "fixtures/parser_rebuildxref_correct.pdf";
    let normal = run(&["info", damaged]).unwrap();
    assert!(
        String::from_utf8_lossy(&normal.stderr).contains("needed recovery (4 notices"),
        "{normal:?}"
    );
    let quiet = run(&["info", "-q", damaged]).unwrap();
    assert!(quiet.status.success());
    assert!(quiet.stderr.is_empty(), "{quiet:?}");
    assert_eq!(quiet.stdout, normal.stdout);
    let verbose = run(&["info", "--verbose", damaged]).unwrap();
    let err = String::from_utf8_lossy(&verbose.stderr);
    let lines: Vec<&str> = err.lines().collect();
    assert_eq!(lines.len(), 4, "{err}");
    assert!(
        lines
            .iter()
            .all(|l| l.starts_with("pdfrum: fixtures/parser_rebuildxref_correct.pdf: ")),
        "{err}"
    );
    assert!(
        err.contains("recovered at byte 471: KeywordResync"),
        "{err}"
    );
    assert!(
        !err.contains("needed recovery"),
        "the list, not the pointer"
    );

    let both = run(&["info", "-q", "-v", damaged]).unwrap();
    assert_eq!(both.status.code(), Some(2), "they conflict");
    let error = run(&["info", "--quiet", "fixtures/nonesuch.pdf"]).unwrap();
    assert_eq!(error.status.code(), Some(1));
    assert!(!error.stderr.is_empty(), "errors still print");
    let piped = run(&[
        "pages",
        "slice",
        "-q",
        "fixtures/hello_world_2_pages.pdf",
        "--pages",
        "1",
        "-o",
        "-",
    ])
    .unwrap();
    assert!(piped.stdout.starts_with(b"%PDF-"));
    assert!(piped.stderr.is_empty(), "{piped:?}");
}

#[test]
fn the_password_comes_from_the_environment_when_the_flag_is_absent() {
    let from_env = run_with(
        &["info", "fixtures/encrypted.pdf", "--json"],
        b"",
        &[("PDFRUM_PASSWORD", "1234")],
    )
    .unwrap();
    assert!(from_env.status.success(), "{from_env:?}");
    let v: serde_json::Value = serde_json::from_slice(&from_env.stdout).unwrap();
    assert_eq!(v["encrypted"], true);
    let wrong_env = run_with(
        &["info", "fixtures/encrypted.pdf"],
        b"",
        &[("PDFRUM_PASSWORD", "nope")],
    )
    .unwrap();
    assert_eq!(wrong_env.status.code(), Some(1));
    // The flag wins over the variable.
    let flag_wins = run_with(
        &["info", "--password", "1234", "fixtures/encrypted.pdf"],
        b"",
        &[("PDFRUM_PASSWORD", "nope")],
    )
    .unwrap();
    assert!(flag_wins.status.success(), "{flag_wins:?}");
    // The value never shows in the help.
    let help = run_with(&["info", "--help"], b"", &[("PDFRUM_PASSWORD", "s3cret")]).unwrap();
    let text = String::from_utf8_lossy(&help.stdout);
    assert!(text.contains("PDFRUM_PASSWORD"), "{text}");
    assert!(!text.contains("s3cret"), "{text}");
}

/// Every line parses as one JSON object, and together they are what
/// `--json` gives as an array.
fn jsonl_matches_json(args: &[&str]) -> Result<(), String> {
    let mut lines_args = args.to_vec();
    lines_args.push("--jsonl");
    let text = stdout(&lines_args)?;
    let lines: Vec<serde_json::Value> = text
        .lines()
        .map(|l| serde_json::from_str(l).map_err(|e| format!("{args:?}: {l}: {e}")))
        .collect::<Result<_, _>>()?;
    if lines.iter().any(|l| !l.is_object()) {
        return Err(format!("{args:?}: a line is not an object: {text}"));
    }
    let mut json_args = args.to_vec();
    json_args.push("--json");
    let array = json(&json_args)?;
    if array.as_array().map(Vec::as_slice) != Some(lines.as_slice()) {
        return Err(format!(
            "{args:?}: --jsonl and --json disagree\n{text}\n{array}"
        ));
    }
    Ok(())
}

#[test]
fn jsonl_prints_one_object_per_line_on_every_list_command() {
    for args in [
        vec!["search", "world", "fixtures/hello_world_2_pages.pdf"],
        vec!["extract", "links", "fixtures/annots_action_handling.pdf"],
        vec!["extract", "annotations", "fixtures/annotiter.pdf"],
        vec!["extract", "images", "fixtures/rotated_image.pdf"],
        vec!["extract", "fonts", "fixtures/bigtable_mini.pdf"],
        vec!["extract", "words", "fixtures/hello_world_2_pages.pdf"],
        vec![
            "extract",
            "attachments",
            "fixtures/embedded_attachments_with_desc.pdf",
        ],
        vec!["inspect", "revisions", "fixtures/bug_1484283.pdf"],
        vec!["inspect", "structure", "fixtures/tagged_actual_text.pdf"],
        vec!["forms", "dump", "fixtures/text_form.pdf"],
    ] {
        jsonl_matches_json(&args).unwrap();
    }
    // `inspect xref --json` is the table with its trailer; `--jsonl` is its
    // entries, one per line.
    let lines = stdout(&["inspect", "xref", "fixtures/bug_1484283.pdf", "--jsonl"]).unwrap();
    assert_eq!(lines.lines().count(), 6, "{lines}");
    let first: serde_json::Value = serde_json::from_str(lines.lines().next().unwrap()).unwrap();
    assert!(first["object"].is_number(), "{first}");
    // Nothing found is no lines at all, and exit 0.
    let none = stdout(&["forms", "dump", "fixtures/bookmarks.pdf", "--jsonl"]).unwrap();
    assert!(none.is_empty(), "{none:?}");
    // Several files: each hit names its file.
    let two = stdout(&[
        "search",
        "world",
        "fixtures/hello_world_2_pages.pdf",
        "fixtures/bookmarks.pdf",
        "--jsonl",
    ])
    .unwrap();
    let hit: serde_json::Value = serde_json::from_str(two.lines().next().unwrap()).unwrap();
    assert_eq!(hit["file"], "fixtures/hello_world_2_pages.pdf");
    assert_eq!(hit["page"], 1);
    let both = run(&[
        "extract",
        "links",
        "fixtures/weblinks.pdf",
        "--json",
        "--jsonl",
    ])
    .unwrap();
    assert_eq!(both.status.code(), Some(2), "they conflict");
}

// ---- javascript (a feature, off by default) --------------------------------

/// The transcript PDFium's own harness expects for a fixture, beside it.
#[cfg(feature = "javascript")]
fn transcript_of(name: &str) -> std::io::Result<String> {
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(format!("{name}_expected.txt")),
    )
}

#[cfg(feature = "javascript")]
#[test]
fn scripts_run_prints_the_transcript_the_oracle_expects() {
    for name in ["console_methods", "bug_740166"] {
        let got = stdout(&[
            "scripts",
            "run",
            &format!("fixtures/{name}.pdf"),
            "--time",
            "1700000000",
        ])
        .unwrap();
        assert_eq!(got, transcript_of(name).unwrap(), "{name}");
    }
    let v = json(&[
        "scripts",
        "run",
        fx("fixtures/bug_740166.pdf"),
        "--time",
        "1700000000",
        "--json",
    ])
    .unwrap();
    let lines = v.as_array().unwrap();
    assert_eq!(lines.len(), 4);
    assert_eq!(lines[0]["line"], "Alert: Values = 1 .9999 2");
    let none = stdout(&["scripts", "run", fx("fixtures/hello_world_2_pages.pdf")]).unwrap();
    assert_eq!(none, "no script output\n");
}

#[cfg(feature = "javascript")]
#[test]
fn forms_fill_scripts_writes_back_what_the_open_action_assigned() {
    // `open_action_echo.pdf`'s open action copies `source` into `echo`
    // with a `!` — through a Flate-encoded script stream once saved, which
    // is the case a viewer meets and the one a raw read got wrong.
    let dir = scratch("fill-scripts").unwrap();
    let values = dir.join("values.json");
    std::fs::write(&values, r#"{"source": "hi"}"#).unwrap();
    let filled = dir.join("filled.pdf");
    let line = stdout(&[
        "forms",
        "fill",
        fx("fixtures/open_action_echo.pdf"),
        "--data",
        values.to_str().unwrap(),
        "--scripts",
        "-o",
        filled.to_str().unwrap(),
    ])
    .unwrap();
    assert!(line.ends_with("1 field set, 1 by scripts\n"), "{line}");
    let fields = json(&["forms", "dump", filled.to_str().unwrap(), "--json"]).unwrap();
    let value = |name: &str| {
        fields
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["name"] == name)
            .map(|f| f["value"].clone())
            .unwrap()
    };
    assert_eq!(value("source"), "hi");
    assert_eq!(value("echo"), "hi!");
    // Without --scripts the open action is not run and `echo` stays empty.
    let plain = dir.join("plain.pdf");
    stdout(&[
        "forms",
        "fill",
        fx("fixtures/open_action_echo.pdf"),
        "--data",
        values.to_str().unwrap(),
        "-o",
        plain.to_str().unwrap(),
    ])
    .unwrap();
    let fields = json(&["forms", "dump", plain.to_str().unwrap(), "--json"]).unwrap();
    let echo = fields
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["name"] == "echo")
        .unwrap();
    assert_eq!(echo["value"], "");
}

/// The corpus guide: eleven pages, 1240 x 1753 px at 150 dpi.
const GUIDE: &str = "../../../benches/corpus/text_quick_start.pdf";

#[test]
fn max_pixels_refuses_a_render_above_the_cap_with_exit_4_and_writes_nothing() {
    let dir = scratch("max-pixels").unwrap();
    let template = dir.join("{n}.png");
    let refused = run(&[
        "render",
        "--max-pixels",
        "1000",
        "-o",
        template.to_str().unwrap(),
        fx(GUIDE),
    ])
    .unwrap();
    assert_eq!(refused.status.code(), Some(4));
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert_eq!(
        stderr,
        "pdfrum: render of 1240 x 1753 px (2.1 megapixels) is above the cap of 0 megapixels; \
         render at a smaller scale or raise `Limits::max_render_pixels`\n"
    );
    assert!(refused.stdout.is_empty());
    assert_eq!(
        std::fs::read_dir(&dir).unwrap().count(),
        0,
        "refused before anything was drawn"
    );

    let rendered = run(&[
        "render",
        "--max-pixels",
        "100M",
        "--pages",
        "1",
        "--dpi",
        "36",
        "-o",
        template.to_str().unwrap(),
        fx(GUIDE),
    ])
    .unwrap();
    assert!(
        rendered.status.success(),
        "{}",
        String::from_utf8_lossy(&rendered.stderr)
    );
    let png = std::fs::read(dir.join("1.png")).unwrap();
    assert_eq!(&png[..4], b"\x89PNG");
    std::fs::remove_dir_all(&dir).unwrap();

    // The grammar: `K`, `M`, `G` or digits; anything else is clap's exit 2.
    for junk in ["lots", "1.5M", "100MiB"] {
        let out = run(&["render", "--max-pixels", junk, fx(GUIDE)]).unwrap();
        assert_eq!(out.status.code(), Some(2), "{junk:?}");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("is not a number of pixels"),
            "{junk:?}"
        );
    }
}

#[test]
fn time_limit_stops_the_command_with_exit_4_and_a_generous_one_changes_nothing() {
    // One millisecond is spent by the open or by the first page load after
    // it — the checks are at the boundaries the library has, so which one
    // says so depends on the machine; the budget and the code do not.
    let stopped = run(&["--time-limit", "1ms", "extract", "text", fx(GUIDE)]).unwrap();
    assert_eq!(stopped.status.code(), Some(4));
    let stderr = String::from_utf8_lossy(&stopped.stderr);
    assert!(stderr.starts_with("pdfrum: "), "{stderr}");
    assert!(
        stderr.contains("time limit of 1 ms exceeded while "),
        "{stderr}"
    );
    assert!(
        stderr.ends_with("; allow more time or do less\n"),
        "{stderr}"
    );

    let plain = stdout(&["extract", "text", fx(GUIDE)]).unwrap();
    let budgeted = stdout(&["--time-limit", "10s", "extract", "text", fx(GUIDE)]).unwrap();
    assert_eq!(
        budgeted, plain,
        "a budget that is not spent changes nothing"
    );
    assert_eq!(plain.matches('\u{c}').count(), 10, "eleven pages");

    // Several files under a spent budget: each reported, exit 4 at the end.
    let several = run(&[
        "--time-limit",
        "1ms",
        "info",
        fx(GUIDE),
        fx("fixtures/hello_world_2_pages.pdf"),
    ])
    .unwrap();
    assert_eq!(several.status.code(), Some(4));
    let stderr = String::from_utf8_lossy(&several.stderr);
    assert_eq!(stderr.matches("time limit of 1 ms exceeded").count(), 2);

    for junk in ["5", "1.5s", "5 s", "1d"] {
        let out = run(&["--time-limit", junk, "info", fx(GUIDE)]).unwrap();
        assert_eq!(out.status.code(), Some(2), "{junk:?}");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("is not a duration"),
            "{junk:?}"
        );
    }
}
