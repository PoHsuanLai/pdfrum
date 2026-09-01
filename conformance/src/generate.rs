//! `conformance generate-goldens` — drives the C++ oracle over the corpus and
//! fills the golden store.
//!
//! The shape of the work is dictated by two properties of `pdfium_test`:
//!
//! - It writes page artifacts **next to its input**, ignoring the working
//!   directory, and the oracle checkout is read-only. Every PDF is therefore
//!   copied into a per-file scratch directory named `input.pdf`, so artifact
//!   names are `input.pdf.<page>.png` regardless of where the file came from.
//! - The three `--show-*` flags are mutually exclusive, so a full golden set
//!   costs six invocations (render, text, annot, and one per dump).
//!
//! Empty dumps are valid goldens: a PDF with no `/Info` dictionary produces
//! no `--show-metadata` output at all, and an untagged PDF produces no
//! structure tree. Recording the empty file is what lets Tier A assert "our
//! tool must also print nothing".

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::corpus::{Entry, EntryKind};
use crate::goldens::{Manifest, Store, key_for};
use crate::oracle::{Md5Line, OraclePaths, Pass, determinism_args, parse_md5_lines};
use crate::transcode::utf32le_to_utf8;

/// The name every PDF is copied to inside its scratch directory, which fixes
/// the artifact naming across the whole store.
const INPUT_NAME: &str = "input.pdf";

/// The sibling `.evt` is copied next to [`INPUT_NAME`] so the oracle's
/// `.pdf` → `.evt` replacement (`pdfium_test.cc:2154-2156`) finds it.
const INPUT_EVT: &str = "input.evt";

/// Suffix that keeps a `--send-events` PNG from colliding with the plain
/// render of the same page (`input.pdf.0.png` vs `input.pdf.0.events.png`).
pub const EVENTS_PNG_SUFFIX: &str = ".events.png";

/// What happened to one corpus entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Goldens were generated and written.
    Generated { key: String, artifacts: usize },
    /// A golden directory already existed (and `--force` was not given).
    Skipped { key: String },
    /// The entry could not be turned into a PDF, or the oracle could not be
    /// run at all. Oracle *failures on a valid PDF* are not errors — they are
    /// recorded in the manifest.
    Failed { reason: String },
}

/// One entry's result, paired with its id for reporting.
#[derive(Debug, Clone)]
pub struct EntryOutcome {
    pub id: String,
    pub outcome: Outcome,
}

/// Verifies the oracle is present and usable, with a message that says what to
/// do about it when it is not.
pub fn check_oracle(paths: &OraclePaths) -> Result<()> {
    if !paths.binary.is_file() {
        bail!(
            "oracle binary not found at {}\n\
             \n\
             Build it in the read-only checkout, or point the harness elsewhere:\n\
             \n  cd /mnt/data2/pdfium/pdfium-c++\n\
               gn gen out/Release --args='is_debug=false pdf_enable_v8=false \
             pdf_enable_xfa=false pdf_use_skia=false pdf_is_standalone=true'\n\
               ninja -C out/Release pdfium_test pdfium_diff\n\
             \n\
             Then re-run, or set --oracle <path> / PDFRUM_ORACLE=<path>.",
            paths.binary.display()
        );
    }
    if !paths.font_dir.is_dir() {
        bail!(
            "hermetic font directory not found at {}\n\
             The determinism recipe (PLAN.md §4) needs third_party/test_fonts \
             from the oracle checkout; pass --font-dir to override.",
            paths.font_dir.display()
        );
    }
    Ok(())
}

/// Generates (or skips) the goldens for one corpus entry.
///
/// `scratch` is a directory this call owns exclusively; it is created and
/// removed here, so the read-only oracle checkout is never written to.
pub fn generate_one(
    entry: &Entry,
    oracle: &OraclePaths,
    store: &Store,
    scratch: &Path,
    fixup: &Path,
    force: bool,
) -> EntryOutcome {
    let outcome = match generate_inner(entry, oracle, store, scratch, fixup, force) {
        Ok(outcome) => outcome,
        Err(err) => Outcome::Failed {
            reason: format!("{err:#}"),
        },
    };
    std::fs::remove_dir_all(scratch).ok();
    EntryOutcome {
        id: entry.id.clone(),
        outcome,
    }
}

fn generate_inner(
    entry: &Entry,
    oracle: &OraclePaths,
    store: &Store,
    scratch: &Path,
    fixup: &Path,
    force: bool,
) -> Result<Outcome> {
    std::fs::create_dir_all(scratch)
        .with_context(|| format!("creating scratch dir {}", scratch.display()))?;

    let pdf_bytes = materialize(entry, scratch, fixup)?;
    let key = key_for(&pdf_bytes);

    if store.has(&key) && !force {
        // Still record this source against the existing manifest: a `.in` and
        // its checked-in `.pdf` sibling expand to the same bytes, and both
        // names should be findable.
        if let Ok(mut manifest) = store.manifest(&key)
            && !manifest.sources.contains(&entry.id)
        {
            manifest.sources.push(entry.id.clone());
            manifest.sources.sort();
            store.write_manifest(&manifest).ok();
        }
        if entry.sibling_evt().is_none() {
            return Ok(Outcome::Skipped { key });
        }
        // Plain goldens already exist; still fill in the `--send-events`
        // PNGs when they are missing.
        let input = scratch.join(INPUT_NAME);
        std::fs::write(&input, &pdf_bytes)
            .with_context(|| format!("writing {}", input.display()))?;
        copy_sibling_evt(entry, scratch)?;
        let added = ensure_event_goldens(entry, oracle, store, scratch, &input, &key)?;
        return Ok(if added > 0 {
            Outcome::Generated {
                key,
                artifacts: added,
            }
        } else {
            Outcome::Skipped { key }
        });
    }

    // The oracle works on this copy, never on the checkout.
    let input = scratch.join(INPUT_NAME);
    std::fs::write(&input, &pdf_bytes).with_context(|| format!("writing {}", input.display()))?;
    copy_sibling_evt(entry, scratch)?;

    let mut artifacts: Vec<(String, Vec<u8>)> = Vec::new();
    let mut md5: Vec<Md5Line> = Vec::new();
    let mut failures: Vec<String> = Vec::new();
    let mut page_count = None;

    for pass in Pass::ALL {
        let run = run_pass(oracle, pass, &input)?;
        if !run.ok {
            failures.push(pass.label().to_owned());
        }
        if page_count.is_none() {
            page_count = parse_page_count(&run.stderr);
        }
        // A dump pass: stdout *is* the artifact, empty output included.
        // A file-writing pass leaves its artifacts beside the input instead.
        if let Some(name) = pass.stdout_artifact() {
            artifacts.push((name.to_owned(), run.stdout.into_bytes()));
        } else {
            if pass == Pass::Render {
                md5 = parse_md5_lines(&run.stdout);
            }
            artifacts.extend(harvest(scratch, pass)?);
        }
    }

    let (event_artifacts, event_md5) = harvest_send_events(entry, oracle, scratch, &input)?;
    artifacts.extend(event_artifacts);
    md5.extend(event_md5);

    let mut names: Vec<String> = artifacts.iter().map(|(name, _)| name.clone()).collect();
    names.sort();
    names.dedup();

    let manifest = Manifest {
        key: key.clone(),
        sources: vec![entry.id.clone()],
        page_count,
        artifacts: names,
        md5,
        oracle_failures: failures,
    };

    for (name, bytes) in &artifacts {
        store
            .write_artifact(&key, name, bytes)
            .with_context(|| format!("writing golden artifact {name}"))?;
    }
    store
        .write_manifest(&manifest)
        .with_context(|| format!("writing manifest for {key}"))?;

    Ok(Outcome::Generated {
        key,
        artifacts: artifacts.len(),
    })
}

/// Produces the PDF bytes for an entry: read directly, or expand the template.
///
/// Shared with `run`, which must materialize each entry exactly the way
/// `generate-goldens` did or the content keys would not line up.
pub fn materialize_for_run(entry: &Entry, scratch: &Path, fixup: &Path) -> Result<Vec<u8>> {
    materialize(entry, scratch, fixup)
}

/// Collects the artifacts a pass wrote beside `input`, for the candidate tool.
pub fn harvest_for_run(input: &Path, pass: Pass) -> Result<Vec<(String, Vec<u8>)>> {
    let dir = input.parent().unwrap_or(Path::new("."));
    harvest(dir, pass)
}

/// Whether a stored artifact is a `--send-events` PNG rather than a plain
/// render of the same page.
#[must_use]
pub fn is_events_png(name: &str) -> bool {
    has_suffix(name, EVENTS_PNG_SUFFIX)
}

/// The distinct name a plain render PNG is stored under after `--send-events`.
///
/// `input.pdf.0.png` → `input.pdf.0.events.png`. Already-renamed names pass
/// through so a second rename cannot collide with itself.
#[must_use]
pub fn events_png_name(render_name: &str) -> Option<String> {
    let stem = render_name.strip_suffix(".png")?;
    if stem.ends_with(".events") {
        return Some(render_name.to_owned());
    }
    Some(format!("{stem}.events.png"))
}

/// Copies the source sibling `.evt` to `input.evt` next to the scratch PDF.
fn copy_sibling_evt(entry: &Entry, scratch: &Path) -> Result<()> {
    let Some(src) = entry.sibling_evt() else {
        return Ok(());
    };
    let dest = scratch.join(INPUT_EVT);
    std::fs::copy(&src, &dest)
        .with_context(|| format!("copying {} to {}", src.display(), dest.display()))?;
    Ok(())
}

/// Runs the oracle with `--send-events` and stores the renamed PNGs when
/// they are not already in the manifest.
fn ensure_event_goldens(
    entry: &Entry,
    oracle: &OraclePaths,
    store: &Store,
    scratch: &Path,
    input: &Path,
    key: &str,
) -> Result<usize> {
    if entry.sibling_evt().is_none() {
        return Ok(0);
    }
    let Ok(mut manifest) = store.manifest(key) else {
        return Ok(0);
    };
    if manifest.artifacts.iter().any(|name| is_events_png(name)) {
        return Ok(0);
    }
    let (event_artifacts, event_md5) = harvest_send_events(entry, oracle, scratch, input)?;
    if event_artifacts.is_empty() {
        return Ok(0);
    }
    let added = event_artifacts.len();
    for (name, bytes) in &event_artifacts {
        store
            .write_artifact(key, name, bytes)
            .with_context(|| format!("writing golden artifact {name}"))?;
        manifest.artifacts.push(name.clone());
    }
    manifest.md5.extend(event_md5);
    manifest.artifacts.sort();
    manifest.artifacts.dedup();
    store
        .write_manifest(&manifest)
        .with_context(|| format!("writing manifest for {key}"))?;
    Ok(added)
}

/// One named artifact harvested from a scratch directory.
type Harvested = (String, Vec<u8>);

/// One `--send-events --png --md5` oracle run, with PNGs renamed so they
/// cannot collide with the plain render.
fn harvest_send_events(
    entry: &Entry,
    oracle: &OraclePaths,
    scratch: &Path,
    input: &Path,
) -> Result<(Vec<Harvested>, Vec<Md5Line>)> {
    if entry.sibling_evt().is_none() {
        return Ok((Vec::new(), Vec::new()));
    }
    let run = run_send_events(oracle, input)?;
    let harvested = harvest(scratch, Pass::Render)?;
    let artifacts: Vec<(String, Vec<u8>)> = harvested
        .into_iter()
        .filter_map(|(name, bytes)| events_png_name(&name).map(|renamed| (renamed, bytes)))
        .collect();
    let md5 = parse_md5_lines(&run.stdout)
        .into_iter()
        .filter_map(|line| {
            let renamed = events_png_name(line.file_name())?;
            Some(Md5Line {
                path: renamed,
                digest: line.digest,
            })
        })
        .collect();
    Ok((artifacts, md5))
}

fn run_send_events(oracle: &OraclePaths, input: &Path) -> Result<Run> {
    let output = Command::new(&oracle.binary)
        .args(determinism_args(&oracle.font_dir))
        .args(["--png", "--md5", "--send-events"])
        .arg(input)
        .output()
        .context("running the oracle (--send-events)")?;
    Ok(Run {
        ok: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

fn materialize(entry: &Entry, scratch: &Path, fixup: &Path) -> Result<Vec<u8>> {
    match entry.kind {
        EntryKind::Pdf => std::fs::read(&entry.source)
            .with_context(|| format!("reading {}", entry.source.display())),
        EntryKind::Template => expand_template(&entry.source, scratch, fixup),
    }
}

/// Runs `fixup_pdf_template.py` into the scratch directory.
///
/// `--output-dir` is mandatory: without it the script writes the expanded
/// `.pdf` beside the `.in` inside the read-only checkout.
fn expand_template(source: &Path, scratch: &Path, fixup: &Path) -> Result<Vec<u8>> {
    let expanded = scratch.join("expanded");
    std::fs::create_dir_all(&expanded)?;
    let output = Command::new("python3")
        .arg(fixup)
        .arg(format!("--output-dir={}", expanded.display()))
        .arg(source)
        .output()
        .with_context(|| format!("running {} on {}", fixup.display(), source.display()))?;
    if !output.status.success() {
        bail!(
            "fixup_pdf_template.py failed on {}: {}",
            source.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let stem = source
        .file_stem()
        .and_then(|s| s.to_str())
        .context("template has no file stem")?;
    let produced = expanded.join(format!("{stem}.pdf"));
    std::fs::read(&produced)
        .with_context(|| format!("template expanded to no PDF at {}", produced.display()))
}

/// One oracle invocation's captured output.
struct Run {
    ok: bool,
    stdout: String,
    stderr: String,
}

fn run_pass(oracle: &OraclePaths, pass: Pass, input: &Path) -> Result<Run> {
    let output = Command::new(&oracle.binary)
        .args(determinism_args(&oracle.font_dir))
        .args(pass.flags())
        .arg(input)
        .output()
        .with_context(|| format!("running the oracle ({pass:?})"))?;
    Ok(Run {
        ok: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

/// Collects the files a pass wrote beside the input, transcoding text dumps.
fn harvest(scratch: &Path, pass: Pass) -> Result<Vec<(String, Vec<u8>)>> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(scratch)? {
        let path = entry?.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if name == INPUT_NAME || path.is_dir() {
            continue;
        }
        // The stdout passes write no files; their artifacts arrive on the
        // stream instead.
        if !pass.owns_artifact(name) || pass.stdout_artifact().is_some() {
            continue;
        }
        let bytes = std::fs::read(&path)?;
        let bytes = if pass == Pass::Text {
            // Store text as UTF-8 so Tier A is a plain byte comparison.
            // Undecodable output is kept verbatim rather than dropped: it is
            // still the oracle's answer, and Tier A can still diff it.
            utf32le_to_utf8(&bytes).map_or(bytes, String::into_bytes)
        } else {
            bytes
        };
        found.push((name.to_owned(), bytes));
        // Consume it so the next pass's harvest sees a clean directory.
        std::fs::remove_file(&path).ok();
    }
    found.sort();
    Ok(found)
}

/// Whether an oracle-written artifact name ends in `suffix`.
///
/// A plain suffix test rather than an extension lookup: the names are
/// multi-part (`input.pdf.0.annot.txt`), so `Path::extension` sees only
/// `txt` and cannot tell an annot dump from a text dump.
pub fn has_suffix(name: &str, suffix: &str) -> bool {
    name.len() > suffix.len() && name.ends_with(suffix)
}

/// Reads `Processed N pages.` out of oracle stderr.
pub fn parse_page_count(stderr: &str) -> Option<u32> {
    stderr.lines().find_map(|line| {
        line.trim()
            .strip_prefix("Processed ")?
            .strip_suffix(" pages.")?
            .parse()
            .ok()
    })
}

/// A per-entry scratch directory under `base`, unique and collision-free.
pub fn scratch_for(base: &Path, index: usize) -> PathBuf {
    base.join(format!("job-{index:06}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_page_count_the_oracle_prints() {
        assert_eq!(
            parse_page_count("Processing PDF file x.pdf.\nProcessed 2 pages.\n"),
            Some(2)
        );
        assert_eq!(parse_page_count("Processed 0 pages.\n"), Some(0));
        assert_eq!(parse_page_count("Processed 137 pages.\n"), Some(137));
    }

    #[test]
    fn a_missing_or_malformed_page_line_is_none() {
        assert_eq!(parse_page_count(""), None);
        assert_eq!(parse_page_count("Processing PDF file x.pdf.\n"), None);
        assert_eq!(parse_page_count("Processed many pages.\n"), None);
        assert_eq!(parse_page_count("Processed 2 pages"), None);
    }

    #[test]
    fn a_missing_oracle_binary_gives_actionable_advice() {
        let err = check_oracle(&OraclePaths {
            binary: PathBuf::from("/nonexistent/pdfium_test"),
            font_dir: PathBuf::from("/nonexistent/fonts"),
        })
        .unwrap_err();
        let message = format!("{err:#}");
        assert!(message.contains("oracle binary not found"));
        assert!(message.contains("ninja -C out/Release pdfium_test"));
        assert!(message.contains("PDFRUM_ORACLE"));
    }

    #[test]
    fn a_missing_font_dir_is_reported_separately() {
        // Point the binary at something that exists so only the fonts fail.
        let binary = std::env::current_exe().unwrap();
        let err = check_oracle(&OraclePaths {
            binary,
            font_dir: PathBuf::from("/nonexistent/fonts"),
        })
        .unwrap_err();
        assert!(format!("{err:#}").contains("hermetic font directory not found"));
    }

    #[test]
    fn scratch_dirs_do_not_collide() {
        let base = Path::new("/tmp/base");
        assert_ne!(scratch_for(base, 0), scratch_for(base, 1));
        assert_eq!(scratch_for(base, 42), Path::new("/tmp/base/job-000042"));
    }

    #[test]
    fn events_png_names_cannot_collide_with_the_plain_render() {
        assert_eq!(
            events_png_name("input.pdf.0.png").as_deref(),
            Some("input.pdf.0.events.png")
        );
        assert_eq!(
            events_png_name("input.pdf.0.events.png").as_deref(),
            Some("input.pdf.0.events.png")
        );
        assert!(events_png_name("input.pdf.0.txt").is_none());
        assert!(is_events_png("input.pdf.0.events.png"));
        assert!(!is_events_png("input.pdf.0.png"));
    }
}
