//! The M20 phase-5 verbs — `metadata set`, `pages delete`, `pages rotate`,
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
