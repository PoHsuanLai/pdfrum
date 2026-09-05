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
    assert_eq!(
        plain.split_whitespace().collect::<Vec<_>>(),
        laid.split_whitespace().collect::<Vec<_>>(),
        "the same words, only placed"
    );
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
