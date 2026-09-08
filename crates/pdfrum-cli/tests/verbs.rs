//! The verbs — `metadata set`, `pages delete`, `pages rotate`,
//! `attach add|remove`, `stamp text|image` — each run on a fixture into a
//! scratch directory and read back with our own commands, and each checked
//! once through the oracle's `pdfium_test` when the checkout is on this
//! machine (`PDFRUM_ORACLE_CHECKOUT`; skipped with a message otherwise).

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

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

/// The stderr of a run that must fail with exit 1.
fn refused(args: &[&str]) -> Result<String, String> {
    let out = run(args).map_err(|e| e.to_string())?;
    if out.status.code() != Some(1) {
        return Err(format!("{args:?}: expected exit 1: {out:?}"));
    }
    Ok(String::from_utf8_lossy(&out.stderr).into_owned())
}

fn scratch(name: &str) -> std::io::Result<PathBuf> {
    let dir = std::env::temp_dir().join(format!("pdfrum-verbs-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// `pdfrum info --json` of a written file.
fn info(path: &Path) -> Result<serde_json::Value, String> {
    json(&["info", path.to_str().ok_or("non-utf8 path")?, "--json"])
}

/// The oracle's `pdfium_test`, when `PDFRUM_ORACLE_CHECKOUT` names a built
/// checkout; `None`, with a line on stderr, otherwise.
fn oracle_bin() -> Option<PathBuf> {
    let Some(checkout) = std::env::var_os("PDFRUM_ORACLE_CHECKOUT") else {
        eprintln!("PDFRUM_ORACLE_CHECKOUT unset: oracle check skipped");
        return None;
    };
    let bin = Path::new(&checkout).join("out/Release/pdfium_test");
    if !bin.is_file() {
        eprintln!("{}: not built; oracle check skipped", bin.display());
        return None;
    }
    Some(bin)
}

/// `pdfium_test <args> <name>` run in `dir` (it writes beside the file);
/// stdout and stderr together. The exit status is the caller's to judge:
/// `--save-attachments` exits non-zero after writing every file.
fn oracle(bin: &Path, dir: &Path, args: &[&str], name: &str) -> std::io::Result<(bool, String)> {
    let out = Command::new(bin)
        .args(args)
        .arg(name)
        .current_dir(dir)
        .output()?;
    Ok((
        out.status.success(),
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
    ))
}

const HELLO: &str = "fixtures/hello_world_2_pages.pdf";

// ---- metadata set -----------------------------------------------------------

#[test]
fn metadata_set_changes_the_keys_named_and_keeps_the_rest() {
    let dir = scratch("metadata").unwrap();
    let first = dir.join("first.pdf");
    let line = stdout(&[
        "metadata",
        "set",
        HELLO,
        "--title",
        "A Title",
        "--author",
        "An Author",
        "--keywords",
        "one, two",
        "-o",
        first.to_str().unwrap(),
    ])
    .unwrap();
    assert_eq!(line, format!("{}: metadata set, 3 keys\n", first.display()));
    let m = &info(&first).unwrap()["metadata"];
    assert_eq!(m["title"], "A Title");
    assert_eq!(m["author"], "An Author");
    assert_eq!(m["keywords"], "one, two");
    assert!(m["subject"].is_null());
    assert!(
        m["modification_date"]
            .as_str()
            .is_some_and(|d| d.starts_with("D:20")),
        "the save stamps the date: {m}"
    );

    // A second pass sets one key and clears one; the others survive.
    let second = dir.join("second.pdf");
    let line = stdout(&[
        "metadata",
        "set",
        first.to_str().unwrap(),
        "--subject",
        "A Subject",
        "--clear",
        "keywords,modified",
        "-o",
        second.to_str().unwrap(),
    ])
    .unwrap();
    assert_eq!(
        line,
        format!("{}: metadata set, 1 key, 2 cleared\n", second.display())
    );
    let m = &info(&second).unwrap()["metadata"];
    assert_eq!(m["title"], "A Title");
    assert_eq!(m["author"], "An Author");
    assert_eq!(m["subject"], "A Subject");
    assert!(m["keywords"].is_null(), "cleared: {m}");

    // Reproducible: the bytes repeat, and the save stamps no date of its
    // own, so a cleared `modified` stays cleared.
    let third = dir.join("third.pdf");
    let args = [
        "metadata",
        "set",
        second.to_str().unwrap(),
        "--creator",
        "pdfrum tests",
        "--clear",
        "modified",
        "--deterministic",
        "-o",
        third.to_str().unwrap(),
    ];
    stdout(&args).unwrap();
    let once = std::fs::read(&third).unwrap();
    stdout(&args).unwrap();
    assert_eq!(once, std::fs::read(&third).unwrap());
    let m = &info(&third).unwrap()["metadata"];
    assert_eq!(m["creator"], "pdfrum tests");
    assert!(m["modification_date"].is_null(), "not stamped: {m}");

    let err = refused(&[
        "metadata",
        "set",
        HELLO,
        "--clear",
        "date",
        "-o",
        dir.join("no.pdf").to_str().unwrap(),
    ])
    .unwrap();
    assert!(err.contains("not a metadata key"), "{err}");
    let err = refused(&[
        "metadata",
        "set",
        HELLO,
        "-o",
        dir.join("no.pdf").to_str().unwrap(),
    ])
    .unwrap();
    assert!(err.contains("nothing to change"), "{err}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_oracle_shows_the_metadata_we_set() {
    let Some(bin) = oracle_bin() else {
        return;
    };
    let dir = scratch("metadata-oracle").unwrap();
    stdout(&[
        "metadata",
        "set",
        HELLO,
        "--title",
        "A Title",
        "--author",
        "An Author",
        "--subject",
        "A Subject",
        "--keywords",
        "one, two",
        "--creator",
        "A Creator",
        "-o",
        dir.join("info.pdf").to_str().unwrap(),
    ])
    .unwrap();
    let (ok, log) = oracle(&bin, &dir, &["--show-metadata"], "info.pdf").unwrap();
    assert!(ok, "{log}");
    for line in [
        "Title        = A Title (",
        "Author       = An Author (",
        "Subject      = A Subject (",
        "Keywords     = one, two (",
        "Creator      = A Creator (",
    ] {
        assert!(log.contains(line), "missing {line:?} in:\n{log}");
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

// ---- pages delete, pages rotate --------------------------------------------

/// The rotation of every page of a written file, in order.
fn rotations(path: &Path) -> Result<Vec<u64>, String> {
    Ok(info(path)?["page_boxes"]
        .as_array()
        .ok_or("no page_boxes")?
        .iter()
        .map(|p| p["rotation"].as_u64().unwrap_or(u64::MAX))
        .collect())
}

#[test]
fn rotate_turns_from_where_a_page_stands_and_delete_keeps_the_rest_in_order() {
    let dir = scratch("pages").unwrap();
    let all = dir.join("all.pdf");
    let line = stdout(&[
        "pages",
        "rotate",
        HELLO,
        "--by",
        "90",
        "-o",
        all.to_str().unwrap(),
    ])
    .unwrap();
    assert_eq!(line, format!("{}: 2 pages rotated by 90\n", all.display()));
    assert_eq!(rotations(&all).unwrap(), [90, 90]);

    // Relative: page 1 back by a quarter, then on by three quarters.
    let back = dir.join("back.pdf");
    let line = stdout(&[
        "pages",
        "rotate",
        all.to_str().unwrap(),
        "--pages",
        "1",
        "--by",
        "-90",
        "-o",
        back.to_str().unwrap(),
    ])
    .unwrap();
    assert_eq!(line, format!("{}: 1 page rotated by -90\n", back.display()));
    assert_eq!(rotations(&back).unwrap(), [0, 90]);
    let on = dir.join("on.pdf");
    stdout(&[
        "pages",
        "rotate",
        back.to_str().unwrap(),
        "--by",
        "270",
        "-o",
        on.to_str().unwrap(),
    ])
    .unwrap();
    assert_eq!(rotations(&on).unwrap(), [270, 0]);

    // Delete the first page; the page left is the one that stood at 90.
    let fewer = dir.join("fewer.pdf");
    let line = stdout(&[
        "pages",
        "delete",
        back.to_str().unwrap(),
        "--pages",
        "1",
        "-o",
        fewer.to_str().unwrap(),
    ])
    .unwrap();
    assert_eq!(
        line,
        format!("{}: 1 page deleted, 1 left\n", fewer.display())
    );
    assert_eq!(rotations(&fewer).unwrap(), [90]);
    let three = dir.join("three.pdf");
    let line = stdout(&[
        "pages",
        "delete",
        "fixtures/annotiter.pdf",
        "--pages",
        "1,3",
        "-o",
        three.to_str().unwrap(),
    ])
    .unwrap();
    assert_eq!(
        line,
        format!("{}: 2 pages deleted, 1 left\n", three.display())
    );

    let no = dir.join("no.pdf");
    let err = refused(&[
        "pages",
        "delete",
        HELLO,
        "--pages",
        "1-end",
        "-o",
        no.to_str().unwrap(),
    ])
    .unwrap();
    assert!(err.contains("every page"), "{err}");
    let err = refused(&[
        "pages",
        "rotate",
        HELLO,
        "--by",
        "45",
        "-o",
        no.to_str().unwrap(),
    ])
    .unwrap();
    assert!(err.contains("--by takes 90, 180, 270 or -90"), "{err}");
    assert!(!no.exists(), "nothing written on a refusal");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A PNG's width and height, from its IHDR chunk.
fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    let field = |at: usize| {
        bytes
            .get(at..at + 4)
            .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    };
    Some((field(16)?, field(20)?))
}

#[test]
fn the_oracle_counts_the_pages_left_and_draws_the_turned_page_on_its_side() {
    let Some(bin) = oracle_bin() else {
        return;
    };
    let dir = scratch("pages-oracle").unwrap();
    stdout(&[
        "pages",
        "delete",
        "fixtures/annotiter.pdf",
        "--pages",
        "2",
        "-o",
        dir.join("fewer.pdf").to_str().unwrap(),
    ])
    .unwrap();
    let (ok, log) = oracle(&bin, &dir, &["--show-pageinfo"], "fewer.pdf").unwrap();
    assert!(ok && log.contains("Processed 2 pages."), "{log}");

    // bookmarks.pdf is 612 by 792; turned a quarter, it renders 792 by 612.
    stdout(&[
        "pages",
        "rotate",
        "fixtures/bookmarks.pdf",
        "--pages",
        "1",
        "--by",
        "90",
        "-o",
        dir.join("turned.pdf").to_str().unwrap(),
    ])
    .unwrap();
    let (ok, log) = oracle(&bin, &dir, &["--png", "--pages=0"], "turned.pdf").unwrap();
    assert!(ok, "{log}");
    let png = std::fs::read(dir.join("turned.pdf.0.png")).unwrap();
    assert_eq!(png_size(&png), Some((792, 612)));
    std::fs::remove_dir_all(&dir).unwrap();
}

// ---- attach add, attach remove ---------------------------------------------

const WITH_FOUR: &str = "fixtures/embedded_attachments_with_desc.pdf";

/// `extract attachments --json` of a written file, written into `into`.
fn attachments(path: &Path, into: &Path) -> Result<serde_json::Value, String> {
    json(&[
        "extract",
        "attachments",
        path.to_str().ok_or("non-utf8 path")?,
        "-o",
        into.to_str().ok_or("non-utf8 path")?,
        "--json",
    ])
}

#[test]
fn attach_add_names_and_types_the_files_and_refuses_a_name_twice() {
    let dir = scratch("attach").unwrap();
    let notes = dir.join("notes.txt");
    std::fs::write(&notes, "Read me").unwrap();
    let added = dir.join("added.pdf");
    let line = stdout(&[
        "attach",
        "add",
        HELLO,
        "fixtures/mona_lisa.jpg",
        notes.to_str().unwrap(),
        "--description",
        "for the record",
        "-o",
        added.to_str().unwrap(),
    ])
    .unwrap();
    assert_eq!(line, format!("{}: 2 attachments added\n", added.display()));
    let back = dir.join("back");
    let rows = attachments(&added, &back).unwrap();
    let rows = rows.as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["name"], "mona_lisa.jpg", "sorted by name");
    assert_eq!(rows[0]["subtype"], "image/jpeg");
    assert_eq!(rows[0]["description"], "for the record");
    assert_eq!(rows[1]["name"], "notes.txt");
    assert_eq!(rows[1]["subtype"], "text/plain");
    assert_eq!(rows[1]["size"], 7);
    assert_eq!(
        std::fs::read(back.join("mona_lisa.jpg")).unwrap(),
        std::fs::read(fixture("mona_lisa.jpg")).unwrap()
    );
    assert_eq!(std::fs::read(back.join("notes.txt")).unwrap(), b"Read me");

    // --mime for one file overrides the guess; two files with it is refused.
    let typed = dir.join("typed.pdf");
    stdout(&[
        "attach",
        "add",
        added.to_str().unwrap(),
        "--mime",
        "text/markdown",
        "fixtures/bug_740166_expected.txt",
        "--deterministic",
        "-o",
        typed.to_str().unwrap(),
    ])
    .unwrap();
    let rows = attachments(&typed, &dir.join("back2")).unwrap();
    assert_eq!(rows.as_array().map(Vec::len), Some(3));
    assert_eq!(rows[0]["name"], "bug_740166_expected.txt");
    assert_eq!(rows[0]["subtype"], "text/markdown");
    let no = dir.join("no.pdf");
    let err = refused(&[
        "attach",
        "add",
        HELLO,
        "--mime",
        "text/plain",
        notes.to_str().unwrap(),
        "fixtures/mona_lisa.jpg",
        "-o",
        no.to_str().unwrap(),
    ])
    .unwrap();
    assert!(err.contains("--mime names one type"), "{err}");
    let err = refused(&[
        "attach",
        "add",
        added.to_str().unwrap(),
        notes.to_str().unwrap(),
        "-o",
        no.to_str().unwrap(),
    ])
    .unwrap();
    assert!(err.contains("already attached"), "{err}");

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn attach_remove_takes_the_names_out_and_refuses_one_that_is_not_there() {
    let dir = scratch("attach-remove").unwrap();
    let no = dir.join("no.pdf");
    let fewer = dir.join("fewer.pdf");
    let line = stdout(&[
        "attach",
        "remove",
        WITH_FOUR,
        "2.txt",
        "4.txt",
        "-o",
        fewer.to_str().unwrap(),
    ])
    .unwrap();
    assert_eq!(
        line,
        format!("{}: 2 attachments removed\n", fewer.display())
    );
    let rows = attachments(&fewer, &dir.join("back3")).unwrap();
    let names: Vec<&str> = rows
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["1.txt", "3.txt"]);
    let err = refused(&[
        "attach",
        "remove",
        fewer.to_str().unwrap(),
        "2.txt",
        "-o",
        no.to_str().unwrap(),
    ])
    .unwrap();
    assert!(err.contains("no attachment named \"2.txt\""), "{err}");
    assert!(!no.exists());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_oracle_saves_what_we_attached_and_not_what_we_removed() {
    let Some(bin) = oracle_bin() else {
        return;
    };
    let dir = scratch("attach-oracle").unwrap();
    let every_byte: Vec<u8> = (0..=255u8).collect::<Vec<u8>>().repeat(16);
    std::fs::write(dir.join("bytes.bin"), &every_byte).unwrap();
    stdout(&[
        "attach",
        "add",
        WITH_FOUR,
        dir.join("bytes.bin").to_str().unwrap(),
        "-o",
        dir.join("added.pdf").to_str().unwrap(),
    ])
    .unwrap();
    stdout(&[
        "attach",
        "remove",
        dir.join("added.pdf").to_str().unwrap(),
        "3.txt",
        "-o",
        dir.join("removed.pdf").to_str().unwrap(),
    ])
    .unwrap();
    // The exit status is not the verdict: pdfium_test reports the attachment
    // feature itself as unsupported and exits non-zero, having written
    // every file. The log and the files are.
    let (_, log) = oracle(&bin, &dir, &["--save-attachments"], "removed.pdf").unwrap();
    for name in ["1.txt", "2.txt", "4.txt", "bytes.bin"] {
        assert!(
            log.contains(&format!(
                "Successfully wrote attachment removed.pdf.attachment.{name}"
            )),
            "{log}"
        );
    }
    assert!(!log.contains("3.txt"), "removed: {log}");
    assert_eq!(
        std::fs::read(dir.join("removed.pdf.attachment.bytes.bin")).unwrap(),
        every_byte
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

// ---- stamp text, stamp image -------------------------------------------------

#[test]
fn stamp_text_marks_every_page_in_the_face_and_size_asked_for() {
    let dir = scratch("stamp-text").unwrap();
    let stamped = dir.join("stamped.pdf");
    let line = stdout(&[
        "stamp",
        "text",
        HELLO,
        "DRAFT",
        "--position",
        "bottom-right",
        "--opacity",
        "0.5",
        "--angle",
        "30",
        "--size",
        "24",
        "--rgb",
        "#ff0000",
        "--font",
        "Helvetica-Bold",
        "-o",
        stamped.to_str().unwrap(),
    ])
    .unwrap();
    assert_eq!(line, format!("{}: stamped 2 pages\n", stamped.display()));
    let pages = json(&["extract", "text", stamped.to_str().unwrap(), "--json"]).unwrap();
    let pages = pages.as_array().unwrap();
    assert_eq!(pages.len(), 2);
    for page in pages {
        let text = page["text"].as_str().unwrap();
        assert!(text.contains("DRAFT"), "{text}");
        assert!(
            text.contains("Hello, world!"),
            "the page's own text: {text}"
        );
    }
    let words = json(&["extract", "words", stamped.to_str().unwrap(), "--json"]).unwrap();
    let draft = words
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["text"] == "DRAFT")
        .unwrap();
    assert_eq!(draft["font"], "Helvetica-Bold");
    assert!(
        (draft["size"].as_f64().unwrap() - 24.0).abs() < 0.01,
        "{draft}"
    );
    // Turned 30 degrees, the word's box is wider than the word; its centre
    // is in the bottom-right quarter of the 200 by 200 page.
    let mid =
        |a: &str, b: &str| f64::midpoint(draft[a].as_f64().unwrap(), draft[b].as_f64().unwrap());
    assert!(
        mid("x0", "x1") > 100.0 && mid("y0", "y1") < 100.0,
        "bottom right: {draft}"
    );

    let no = dir.join("no.pdf");
    for (args, reason) in [
        (["--opacity", "2"], "--opacity is 0"),
        (["--font", "Arial"], "standard 14"),
        (["--rgb", "red"], "not a colour"),
        (["--size", "0"], "--size must be"),
    ] {
        let err = refused(&[
            "stamp",
            "text",
            HELLO,
            "DRAFT",
            args[0],
            args[1],
            "-o",
            no.to_str().unwrap(),
        ])
        .unwrap();
        assert!(err.contains(reason), "{args:?}: {err}");
    }
    let usage = run(&["stamp", "text", HELLO, "DRAFT", "--position", "middle"]).unwrap();
    assert_eq!(usage.status.code(), Some(2), "clap's refusal");
    assert!(!no.exists());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn stamp_image_draws_the_picture_on_every_page_at_the_width_asked_for() {
    let dir = scratch("stamp-image").unwrap();
    let stamped = dir.join("stamped.pdf");
    let line = stdout(&[
        "stamp",
        "image",
        HELLO,
        "fixtures/mona_lisa.jpg",
        "--width",
        "50",
        "--position",
        "top-left",
        "--opacity",
        "0.8",
        "--deterministic",
        "-o",
        stamped.to_str().unwrap(),
    ])
    .unwrap();
    assert_eq!(line, format!("{}: stamped 2 pages\n", stamped.display()));
    let once = std::fs::read(&stamped).unwrap();
    // One object drawn on both pages: one row with two uses, and with
    // --all one row per page.
    let images = json(&["extract", "images", stamped.to_str().unwrap(), "--json"]).unwrap();
    let images = images.as_array().unwrap();
    assert_eq!(images.len(), 1, "{images:?}");
    assert_eq!(images[0]["uses"], 2);
    assert_eq!(images[0]["format"], "jpg", "the JPEG is kept as it is");
    assert_eq!(
        (&images[0]["width"], &images[0]["height"]),
        (&serde_json::json!(120), &serde_json::json!(120))
    );
    let draws = json(&[
        "extract",
        "images",
        stamped.to_str().unwrap(),
        "--all",
        "--json",
    ])
    .unwrap();
    let draws = draws.as_array().unwrap();
    assert_eq!(draws.len(), 2, "{draws:?}");
    assert_eq!(draws[0]["page"], 1);
    assert_eq!(draws[1]["page"], 2);
    assert_eq!(
        draws[0]["object"], draws[1]["object"],
        "one object serves every page"
    );
    stdout(&[
        "stamp",
        "image",
        HELLO,
        "fixtures/mona_lisa.jpg",
        "--width",
        "50",
        "--position",
        "top-left",
        "--opacity",
        "0.8",
        "--deterministic",
        "-o",
        stamped.to_str().unwrap(),
    ])
    .unwrap();
    assert_eq!(once, std::fs::read(&stamped).unwrap(), "reproducible");

    let no = dir.join("no.pdf");
    let err = refused(&[
        "stamp",
        "image",
        HELLO,
        "fixtures/bug_740166_expected.txt",
        "-o",
        no.to_str().unwrap(),
    ])
    .unwrap();
    assert!(err.contains("not a JPEG or PNG file"), "{err}");
    let err = refused(&[
        "stamp",
        "image",
        HELLO,
        "fixtures/mona_lisa.jpg",
        "--width",
        "0",
        "-o",
        no.to_str().unwrap(),
    ])
    .unwrap();
    assert!(err.contains("--width must be"), "{err}");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// The page-0 text `pdfium_test --txt` wrote beside `name`: UTF-32LE.
fn oracle_text(dir: &Path, name: &str) -> std::io::Result<String> {
    let raw = std::fs::read(dir.join(format!("{name}.0.txt")))?;
    Ok(raw
        .as_chunks::<4>()
        .0
        .iter()
        .map(|&unit| u32::from_le_bytes(unit))
        .map(|cp| char::from_u32(cp).unwrap_or('\u{FFFD}'))
        .collect())
}

/// The `MD5:` line of a `pdfium_test --md5` run.
fn md5_of(log: &str) -> Option<String> {
    log.lines()
        .find(|line| line.contains("MD5:"))
        .and_then(|line| line.rsplit(':').next())
        .map(|hash| hash.trim().to_owned())
}

#[test]
fn the_oracle_extracts_the_text_stamp_and_renders_the_image_stamp() {
    let Some(bin) = oracle_bin() else {
        return;
    };
    let dir = scratch("stamp-oracle").unwrap();
    std::fs::copy(fixture("hello_world_2_pages.pdf"), dir.join("plain.pdf")).unwrap();
    stdout(&[
        "stamp",
        "text",
        HELLO,
        "CONFIDENTIAL",
        "--angle",
        "30",
        "--opacity",
        "0.4",
        "--size",
        "20",
        "-o",
        dir.join("text.pdf").to_str().unwrap(),
    ])
    .unwrap();
    stdout(&[
        "stamp",
        "image",
        HELLO,
        "fixtures/mona_lisa.jpg",
        "--width",
        "100",
        "-o",
        dir.join("image.pdf").to_str().unwrap(),
    ])
    .unwrap();
    let (ok, log) = oracle(&bin, &dir, &["--txt", "--pages=0"], "text.pdf").unwrap();
    assert!(ok, "{log}");
    let text = oracle_text(&dir, "text.pdf").unwrap();
    assert!(text.contains("CONFIDENTIAL"), "{text}");
    assert!(text.contains("Hello, world!"), "{text}");
    let (ok, plain) = oracle(&bin, &dir, &["--md5", "--png", "--pages=0"], "plain.pdf").unwrap();
    assert!(ok, "{plain}");
    let (ok, image) = oracle(&bin, &dir, &["--md5", "--png", "--pages=0"], "image.pdf").unwrap();
    assert!(ok, "{image}");
    assert!(md5_of(&plain).is_some());
    assert_ne!(
        md5_of(&plain),
        md5_of(&image),
        "the picture changed the page"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

// ---- every verb takes -o - ----------------------------------------------------

#[test]
fn every_new_verb_writes_to_stdout_on_a_dash() {
    let dir = scratch("dash").unwrap();
    let notes = dir.join("notes.txt");
    std::fs::write(&notes, "Read me").unwrap();
    let commands: Vec<Vec<&str>> = vec![
        vec!["metadata", "set", HELLO, "--title", "T"],
        vec!["pages", "delete", HELLO, "--pages", "1"],
        vec!["pages", "rotate", HELLO, "--by", "90"],
        vec!["attach", "add", HELLO, notes.to_str().unwrap()],
        vec!["attach", "remove", WITH_FOUR, "1.txt"],
        vec!["stamp", "text", HELLO, "DRAFT"],
        vec!["stamp", "image", HELLO, "fixtures/mona_lisa.jpg"],
    ];
    for mut args in commands {
        args.extend(["-o", "-"]);
        let out = run(&args).unwrap();
        assert!(out.status.success(), "{args:?}: {out:?}");
        assert!(out.stdout.starts_with(b"%PDF-"), "{args:?}");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("pdfrum: -: "), "{args:?}: {err}");
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

/// `extract markdown -o DIR` writes a document whose image links resolve
/// from beside it, and go on resolving after the directory is moved.
///
/// The regression this pins: the link used to be the same path the image was
/// written to, which is relative to the working directory rather than to the
/// Markdown. The natural invocation -- Markdown and images in one directory
/// -- then produced `![](out/x.png)` inside `out/doc.md`, and every link was
/// dead. Asserting the files exist is not enough to catch that; the test has
/// to resolve each link from the Markdown's own directory.
#[test]
fn markdown_image_links_resolve_from_beside_the_document() -> Result<(), String> {
    let dir = scratch("markdown-images").map_err(|e| e.to_string())?;
    let pdf = fixture("rotated_image.pdf");
    stdout(&[
        "extract",
        "markdown",
        &pdf.display().to_string(),
        "-o",
        &dir.display().to_string(),
    ])?;

    let doc = dir.join("rotated_image.md");
    let text = std::fs::read_to_string(&doc).map_err(|e| format!("{}: {e}", doc.display()))?;

    let links: Vec<&str> = text
        .match_indices("](")
        .filter_map(|(i, _)| {
            let rest = &text[i + 2..];
            rest.find(')').map(|end| &rest[..end])
        })
        .filter(|l| *l != "image")
        .collect();
    assert!(!links.is_empty(), "no image links written:\n{text}");

    for link in &links {
        assert!(
            !link.contains('/'),
            "a link carrying a directory cannot survive a move: {link}"
        );
        assert!(
            dir.join(link).is_file(),
            "link does not resolve from beside the document: {link}"
        );
    }

    // The whole point of a bare name: the directory moves as one piece.
    let moved = dir.with_file_name("markdown-images-moved");
    let _ = std::fs::remove_dir_all(&moved);
    std::fs::rename(&dir, &moved).map_err(|e| e.to_string())?;
    for link in &links {
        assert!(
            moved.join(link).is_file(),
            "link stopped resolving after the directory moved: {link}"
        );
    }
    Ok(())
}
