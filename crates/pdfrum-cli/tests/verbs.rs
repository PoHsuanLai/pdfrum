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
