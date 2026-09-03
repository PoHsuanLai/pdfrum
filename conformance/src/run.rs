//! `conformance run` — drives `pdfrum-tool` over the corpus and scores it
//! against the golden store (PLAN.md §5).
//!
//! Tiering, per file:
//!
//! - **Tier A, byte-exact:** the text dump (already transcoded to UTF-8 in the
//!   store), the annot/metadata/pageinfo/structure dumps, and the page count.
//! - **Tier B, perceptual:** each page PNG against its golden — exact-match
//!   flag, max channel difference, and grayscale SSIM against the floor from
//!   `thresholds.toml`. The worst page decides the file.
//!
//! At M0 `pdfrum-tool` is a stub, so every file lands on `unsupported-tool`
//! and the scoreboard is a complete, all-failing baseline. That is the point:
//! the fitness function exists before anything can score on it.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};

use crate::corpus::Entry;
use crate::generate::{parse_page_count, scratch_for};
use crate::goldens::{Manifest, Store, key_for};
use crate::pixels;
use crate::scoreboard::{FileResult, Status, TierA, TierB, tag};
use crate::ssim::{self, PixelError};
use crate::thresholds::Thresholds;

/// How the candidate tool is invoked.
#[derive(Debug, Clone)]
pub struct ToolPaths {
    pub binary: PathBuf,
    pub font_dir: PathBuf,
}

/// Whether the tool can be asked to do anything at all.
///
/// Separated from the per-file loop so a missing binary is diagnosed once and
/// every file gets the same honest tag, rather than 1400 spawn errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolState {
    /// The binary exists and answered a probe run.
    Ready,
    /// The binary is missing, unrunnable, or has no usable subcommand.
    Unsupported(String),
}

/// A one-page PDF the probe feeds the tool, with a `/MediaBox` on the page
/// itself so `--show-pageinfo` has something to say about it.
const PROBE_PDF: &[u8] = b"%PDF-1.7\n\
1 0 obj<</Type/Catalog/Pages 2 0 R>>endobj\n\
2 0 obj<</Type/Pages/Kids[3 0 R]/Count 1>>endobj\n\
3 0 obj<</Type/Page/Parent 2 0 R/MediaBox[0 0 200 300]>>endobj\n\
trailer<</Root 1 0 R/Size 4>>\n";

/// What the probe expects that PDF's `--show-pageinfo` to open with.
const PROBE_EXPECTED: &str = "Page 0: MediaBox: 0.00 0.00 200.00 300.00";

/// Probes the candidate tool once, before the corpus loop.
///
/// The probe asks the tool to do something and checks that it did, rather
/// than reading its chatter for stub markers. A negative test was right while
/// the tool printed one fixed line and nothing else, but it cannot survive a
/// real tool: a partial implementation legitimately says "not implemented"
/// about the flags it has not grown yet, and matching on that would tag a
/// working tool `unsupported-tool` and hide every tier it *can* score.
///
/// So: hand it a two-hundred-by-three-hundred page and require the pageinfo
/// line back. A tool that produces it can be asked about the corpus; anything
/// else — a stub, a crash, a binary that is not ours — cannot.
pub fn probe_tool(tool: &ToolPaths) -> ToolState {
    if !tool.binary.is_file() {
        return ToolState::Unsupported(format!(
            "pdfrum-tool not found at {} (build it, or set --tool / PDFRUM_TOOL)",
            tool.binary.display()
        ));
    }
    let probe_dir = std::env::temp_dir().join(format!("pdfrum-probe-{}", std::process::id()));
    let outcome = probe_in(tool, &probe_dir);
    std::fs::remove_dir_all(&probe_dir).ok();
    outcome
}

fn probe_in(tool: &ToolPaths, dir: &Path) -> ToolState {
    let input = dir.join("probe.pdf");
    if std::fs::create_dir_all(dir).is_err() || std::fs::write(&input, PROBE_PDF).is_err() {
        return ToolState::Unsupported(format!("cannot write a probe PDF under {}", dir.display()));
    }
    match Command::new(&tool.binary)
        .arg("--show-pageinfo")
        .arg(&input)
        .output()
    {
        Err(err) => ToolState::Unsupported(format!("cannot run {}: {err}", tool.binary.display())),
        Ok(output) if String::from_utf8_lossy(&output.stdout).contains(PROBE_EXPECTED) => {
            ToolState::Ready
        }
        Ok(output) => ToolState::Unsupported(format!(
            "{} did not answer the pageinfo probe (exit {:?}); it is a stub or not a pdfrum-tool",
            tool.binary.display(),
            output.status.code()
        )),
    }
}

/// Scores one corpus entry against its goldens.
pub fn score_one(
    entry: &Entry,
    tool: &ToolPaths,
    state: &ToolState,
    store: &Store,
    thresholds: &Thresholds,
    scratch: &Path,
    fixup: &Path,
) -> FileResult {
    let result = match state {
        ToolState::Unsupported(reason) => {
            FileResult::unsupported_tool(entry.id.clone(), reason.clone())
        }
        ToolState::Ready => match score_inner(entry, tool, store, thresholds, scratch, fixup) {
            Ok(result) => result,
            Err(err) => FileResult {
                path: entry.id.clone(),
                status: Status::Fail,
                tags: vec![tag::TOOL_ERROR.to_owned()],
                tier_a: TierA::default(),
                tier_b: None,
                notes: format!("{err:#}"),
            },
        },
    };
    std::fs::remove_dir_all(scratch).ok();
    result
}

fn score_inner(
    entry: &Entry,
    tool: &ToolPaths,
    store: &Store,
    thresholds: &Thresholds,
    scratch: &Path,
    fixup: &Path,
) -> Result<FileResult> {
    std::fs::create_dir_all(scratch)?;
    let pdf_bytes = crate::generate::materialize_for_run(entry, scratch, fixup)?;
    let key = key_for(&pdf_bytes);
    let Ok(manifest) = store.manifest(&key) else {
        return Ok(FileResult {
            path: entry.id.clone(),
            status: Status::Fail,
            tags: vec![tag::MISSING_GOLDEN.to_owned()],
            tier_a: TierA::default(),
            tier_b: None,
            notes: format!("no golden for {key}; run generate-goldens first"),
        });
    };

    let input = scratch.join("input.pdf");
    std::fs::write(&input, &pdf_bytes)?;
    let produced = invoke_tool(tool, &input).context("invoking pdfrum-tool")?;

    let mut tags = Vec::new();
    let mut notes = Vec::new();
    if !produced.crashed.is_empty() {
        tags.push(tag::CRASH.to_owned());
        notes.push(format!("died by signal on {}", produced.crashed.join(", ")));
    }
    let tier_a = compare_tier_a(store, &manifest, &produced, &mut tags, &mut notes);
    let tier_b = compare_tier_b(
        store,
        &manifest,
        &produced,
        thresholds.ssim_for(&entry.id),
        &mut tags,
        &mut notes,
    );

    // Tier A cleanliness is the gate the tag list must agree with; assert the
    // two views cannot drift apart.
    debug_assert_eq!(
        tier_a.is_clean(),
        !tags
            .iter()
            .any(|t| t == tag::TIER_A_MISMATCH || t == tag::PAGE_COUNT)
    );
    let status = if tags.is_empty() {
        Status::Pass
    } else {
        Status::Fail
    };
    Ok(FileResult {
        path: entry.id.clone(),
        status,
        tags,
        tier_a,
        tier_b,
        notes: notes.join("; "),
    })
}

/// Everything one tool invocation produced, keyed by artifact name.
#[derive(Debug, Default)]
pub struct Produced {
    pub artifacts: Vec<(String, Vec<u8>)>,
    pub page_count: Option<u32>,
    /// Passes the tool died on by signal — a panic or an abort, which is a
    /// harder failure than a nonzero exit and gets its own tag.
    pub crashed: Vec<String>,
}

impl Produced {
    /// The bytes of one artifact, if the tool produced it.
    pub fn get(&self, name: &str) -> Option<&[u8]> {
        self.artifacts
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, bytes)| bytes.as_slice())
    }
}

/// Runs the candidate tool the same way the oracle was run, and collects what
/// it wrote beside the input plus what it printed.
fn invoke_tool(tool: &ToolPaths, input: &Path) -> Result<Produced> {
    let mut produced = Produced::default();
    for pass in crate::oracle::Pass::ALL {
        let output = Command::new(&tool.binary)
            .args(crate::oracle::determinism_args(&tool.font_dir))
            .args(pass.flags())
            .arg(input)
            .output()?;
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        if produced.page_count.is_none() {
            produced.page_count = parse_page_count(&stderr);
        }
        // `code()` is None exactly when a signal ended the process (PLAN.md
        // §5: "panic = automatic failure, caught per-file").
        if output.status.code().is_none() {
            produced.crashed.push(format!("{pass:?}"));
        }
        if let Some(name) = pass.stdout_artifact() {
            produced
                .artifacts
                .push((name.to_owned(), stdout.into_bytes()));
        } else {
            produced
                .artifacts
                .extend(crate::generate::harvest_for_run(input, pass)?);
        }
    }
    produced.artifacts.sort();
    Ok(produced)
}

/// The non-PNG golden artifacts, which are compared byte-for-byte.
fn compare_tier_a(
    store: &Store,
    manifest: &Manifest,
    produced: &Produced,
    tags: &mut Vec<String>,
    notes: &mut Vec<String>,
) -> TierA {
    let mut tier_a = TierA {
        golden_pages: manifest.page_count,
        actual_pages: produced.page_count,
        ..TierA::default()
    };
    for name in &manifest.artifacts {
        if crate::generate::has_suffix(name, ".png") {
            continue;
        }
        // A golden whose own oracle run failed is a crash artifact, not an
        // answer: the oracle aborted partway and whatever landed on disk is
        // arbitrary. `redact_annot` is the standing example — the dump code
        // has no arm for a redaction subtype and reaching one aborts it, so
        // its `.annot.txt` golden is an empty file that no correct tool can
        // reproduce. Comparing against one would pin the crash as the
        // contract.
        if oracle_failed_for(manifest, name) {
            continue;
        }
        tier_a.compared.push(name.clone());
        let golden = store.artifact(&manifest.key, name).unwrap_or_default();
        let matched = produced.get(name).unwrap_or_default() == golden.as_slice();
        if !matched {
            tier_a.mismatched.push(name.clone());
        }
        if is_text_dump(name) {
            tier_a.text.pages += 1;
            tier_a.text.matched += u32::from(matched);
            // The store holds text transcoded to UTF-8 with the oracle's
            // byte-order mark stripped, so "more than a BOM" is simply "not
            // empty" here. Counting it any other way would have to re-derive
            // what the transcode already decided.
            if !golden.is_empty() {
                tier_a.text.substantive += 1;
                tier_a.text.substantive_matched += u32::from(matched);
            }
        }
    }
    if !tier_a.mismatched.is_empty() {
        tags.push(tag::TIER_A_MISMATCH.to_owned());
        notes.push(format!("dumps differ: {}", tier_a.mismatched.join(", ")));
    }
    if tier_a.golden_pages != tier_a.actual_pages {
        tags.push(tag::PAGE_COUNT.to_owned());
        notes.push(format!(
            "pages {:?} vs oracle {:?}",
            tier_a.actual_pages, tier_a.golden_pages
        ));
    }
    tier_a
}

/// Whether the oracle pass that would have produced this artifact failed.
fn oracle_failed_for(manifest: &Manifest, name: &str) -> bool {
    manifest.oracle_failures.iter().any(|failed| {
        crate::oracle::Pass::ALL
            .iter()
            .any(|pass| pass.label() == failed && pass.owns_artifact(name))
    })
}

/// Whether an artifact is a page's `--txt` dump.
///
/// `.annot.txt` also ends in `.txt` and is a different tier, so it is excluded
/// explicitly — the same distinction the golden generator draws when it
/// harvests the two passes.
fn is_text_dump(name: &str) -> bool {
    crate::generate::has_suffix(name, ".txt")
        && !crate::generate::has_suffix(name, ".annot.txt")
        && name != "metadata.txt"
        && name != "pageinfo.txt"
        && name != "structure.txt"
}

/// Every golden PNG against the tool's render of the same page. The worst
/// page decides the file's score.
fn compare_tier_b(
    store: &Store,
    manifest: &Manifest,
    produced: &Produced,
    floor: f64,
    tags: &mut Vec<String>,
    notes: &mut Vec<String>,
) -> Option<TierB> {
    compare_pngs(
        store,
        manifest,
        produced,
        floor,
        tags,
        notes,
        &PngSet {
            select: |name| {
                crate::generate::has_suffix(name, ".png") && !crate::generate::is_events_png(name)
            },
            fail_tag: tag::PIXEL_FAIL,
        },
    )
}

/// `--send-events` PNGs against the oracle's event-driven goldens.
fn compare_events_pngs(
    store: &Store,
    manifest: &Manifest,
    produced: &Produced,
    floor: f64,
    digest: &str,
    tags: &mut Vec<String>,
    notes: &mut Vec<String>,
) -> Option<TierB> {
    compare_pngs(
        store,
        manifest,
        produced,
        floor,
        tags,
        notes,
        &PngSet {
            select: |name: &str| crate::generate::is_events_png_for(name, digest),
            fail_tag: tag::FORM_EVENTS,
        },
    )
}

/// Which PNGs a pixel walk compares, and the tag a miss scores as.
///
/// `select` is a closure rather than a bare `fn` because the event walk must
/// match one *script's* artifacts, not every event-driven artifact in a
/// golden directory shared by several fixtures.
struct PngSet<F: Fn(&str) -> bool> {
    select: F,
    fail_tag: &'static str,
}

/// Shared pixel walk. `fail_tag` is `pixel-fail` for the plain render and
/// `form-events` for the event-driven one, so the two clusters stay apart.
fn compare_pngs<F: Fn(&str) -> bool>(
    store: &Store,
    manifest: &Manifest,
    produced: &Produced,
    floor: f64,
    tags: &mut Vec<String>,
    notes: &mut Vec<String>,
    set: &PngSet<F>,
) -> Option<TierB> {
    let pngs: Vec<&String> = manifest
        .artifacts
        .iter()
        .filter(|name| (set.select)(name))
        .collect();
    if pngs.is_empty() {
        return None;
    }

    let mut worst = TierB {
        ssim: 1.0,
        exact: true,
        max_channel_diff: 0,
        pages: 0,
    };
    for name in pngs {
        worst.pages += 1;
        let Ok(golden_bytes) = store.artifact(&manifest.key, name) else {
            continue;
        };
        let Some(candidate_bytes) = produced.get(name) else {
            worst.ssim = 0.0;
            worst.exact = false;
            worst.max_channel_diff = u8::MAX;
            tags.push(set.fail_tag.to_owned());
            notes.push(format!("{name} not produced"));
            continue;
        };
        // The oracle's own MD5 line is a free identity check: if the golden's
        // recorded digest covers bytes we already hold and the candidate is
        // byte-identical, the pages are the same and no decode is needed.
        if manifest.digest_of(name).is_some() && golden_bytes == candidate_bytes {
            continue;
        }
        let (Ok(golden), Ok(candidate)) = (
            pixels::decode(&golden_bytes),
            pixels::decode(candidate_bytes),
        ) else {
            worst.ssim = 0.0;
            worst.exact = false;
            tags.push(tag::BAD_PNG.to_owned());
            notes.push(format!("{name}: PNG would not decode"));
            continue;
        };
        match ssim::compare(&golden, &candidate) {
            Ok(diff) => {
                worst.ssim = worst.ssim.min(diff.ssim);
                worst.exact &= diff.exact;
                worst.max_channel_diff = worst.max_channel_diff.max(diff.max_channel_diff);
            }
            Err(PixelError::SizeMismatch { golden, candidate }) => {
                worst.ssim = 0.0;
                worst.exact = false;
                tags.push(tag::SIZE_MISMATCH.to_owned());
                notes.push(format!("{name}: {candidate:?} vs golden {golden:?}"));
            }
            Err(err) => {
                worst.ssim = 0.0;
                worst.exact = false;
                tags.push(tag::BAD_PNG.to_owned());
                notes.push(format!("{name}: {err}"));
            }
        }
    }

    if worst.ssim < floor && !tags.iter().any(|t| t == set.fail_tag) {
        tags.push(set.fail_tag.to_owned());
        notes.push(format!("ssim {:.6} below floor {floor:.6}", worst.ssim));
    }
    Some(worst)
}

/// Builds the per-entry scratch path for a run.
pub fn scratch(base: &Path, index: usize) -> PathBuf {
    scratch_for(base, index)
}

/// Scratch directory for a `--send-events` scoring pass, distinct from the
/// plain-render scratch so the two cannot harvest each other's PNGs.
pub fn scratch_events(base: &Path, index: usize) -> PathBuf {
    base.join(format!("job-evt-{index:06}"))
}

/// Scratch directory for a `--js-transcript` scoring pass, distinct from the
/// other two so no pass can harvest another's materialized PDF.
pub fn scratch_js(base: &Path, index: usize) -> PathBuf {
    base.join(format!("job-js-{index:06}"))
}

/// Scoreboard path for a `--send-events` comparison.
///
/// Distinct from the plain-render row so existing entries do not move.
#[must_use]
pub fn form_events_path(id: &str) -> String {
    format!("{id}#form-events")
}

/// Scores one corpus entry's sibling `.evt` against the event-driven golden.
///
/// Returns `None` when there is no sibling script, or when the tool cannot
/// be asked (those files already have an `unsupported-tool` row). Dispatch
/// is stubbed in `pdfrum-tool`, so a mismatch is the expected baseline.
pub fn score_form_events(
    entry: &Entry,
    tool: &ToolPaths,
    state: &ToolState,
    store: &Store,
    thresholds: &Thresholds,
    scratch: &Path,
    fixup: &Path,
) -> Option<FileResult> {
    let _script = entry.sibling_evt()?;
    if !matches!(state, ToolState::Ready) {
        return None;
    }
    let result = match score_form_events_inner(entry, tool, store, thresholds, scratch, fixup) {
        Ok(result) => result,
        Err(err) => FileResult {
            path: form_events_path(&entry.id),
            status: Status::Fail,
            tags: vec![tag::FORM_EVENTS.to_owned()],
            tier_a: TierA::default(),
            tier_b: None,
            notes: format!("{err:#}"),
        },
    };
    std::fs::remove_dir_all(scratch).ok();
    Some(result)
}

fn score_form_events_inner(
    entry: &Entry,
    tool: &ToolPaths,
    store: &Store,
    thresholds: &Thresholds,
    scratch: &Path,
    fixup: &Path,
) -> Result<FileResult> {
    std::fs::create_dir_all(scratch)?;
    let pdf_bytes = crate::generate::materialize_for_run(entry, scratch, fixup)?;
    let key = key_for(&pdf_bytes);
    let path = form_events_path(&entry.id);
    let script = entry
        .sibling_evt()
        .context("scoring form events without a sibling .evt")?;
    let digest = crate::generate::evt_digest(&std::fs::read(&script)?);

    let Ok(manifest) = store.manifest(&key) else {
        return Ok(FileResult {
            path,
            status: Status::Fail,
            tags: vec![tag::FORM_EVENTS.to_owned()],
            tier_a: TierA::default(),
            tier_b: None,
            notes: format!("no golden for {key}; run generate-goldens first"),
        });
    };
    if !manifest
        .artifacts
        .iter()
        .any(|name| crate::generate::is_events_png_for(name, &digest))
    {
        return Ok(FileResult {
            path,
            status: Status::Fail,
            tags: vec![tag::FORM_EVENTS.to_owned()],
            tier_a: TierA::default(),
            tier_b: None,
            notes: format!("no --send-events golden for script {digest}; run generate-goldens"),
        });
    }

    let input = scratch.join("input.pdf");
    std::fs::write(&input, &pdf_bytes)?;
    std::fs::copy(&script, scratch.join("input.evt"))?;

    let produced = invoke_tool_send_events(tool, &input, &digest)
        .context("invoking pdfrum-tool --send-events")?;
    let mut tags = Vec::new();
    let mut notes = Vec::new();
    if !produced.crashed.is_empty() {
        tags.push(tag::FORM_EVENTS.to_owned());
        notes.push(format!("died by signal on {}", produced.crashed.join(", ")));
    }
    let tier_b = compare_events_pngs(
        store,
        &manifest,
        &produced,
        thresholds.ssim_for(&entry.id),
        &digest,
        &mut tags,
        &mut notes,
    );
    if tags.is_empty() && tier_b.is_none() {
        tags.push(tag::FORM_EVENTS.to_owned());
        notes.push("no --send-events PNGs compared".to_owned());
    }
    let status = if tags.is_empty() {
        Status::Pass
    } else {
        Status::Fail
    };
    Ok(FileResult {
        path,
        status,
        tags,
        tier_a: TierA::default(),
        tier_b,
        notes: notes.join("; "),
    })
}

/// Scoreboard path for a `--js-transcript` comparison.
///
/// Singular, matching `form-events`: the cluster is `js-transcripts`, the row
/// family is `#js-transcript` (docs/design/pdfrum-script.md §6.2).
#[must_use]
pub fn js_transcript_path(id: &str) -> String {
    format!("{id}#js-transcript")
}

/// Whether an entry is one of the 47 `testing/resources/javascript/*.in`
/// fixtures the `js-transcripts` cluster covers.
///
/// Exactly those, and nothing else in the corpus: the cluster's contract is
/// `<stem>_expected.txt` sitting beside the template, which only that
/// directory has.
#[must_use]
fn is_js_fixture(id: &str) -> bool {
    // `Path::extension` rather than `ends_with`, so a file merely *named*
    // `...in` without a dot is not swept in. The match is case-sensitive on
    // purpose: the id is the checkout's own path, and the directory holds
    // exactly `.in`.
    id.starts_with("resources/javascript/")
        && Path::new(id).extension().is_some_and(|ext| ext == "in")
}

/// Scores one javascript fixture's `--js-transcript` stdout against the
/// oracle's `<stem>_expected.txt`.
///
/// Returns `None` for every entry outside `testing/resources/javascript/`,
/// and when the tool cannot be asked — those files already carry an
/// `unsupported-tool` row from the plain pass, and a second one would double
/// the same fact.
pub fn score_js_transcript(
    entry: &Entry,
    tool: &ToolPaths,
    state: &ToolState,
    scratch: &Path,
    fixup: &Path,
) -> Option<FileResult> {
    if !is_js_fixture(&entry.id) {
        return None;
    }
    if !matches!(state, ToolState::Ready) {
        return None;
    }
    let result = match score_js_transcript_inner(entry, tool, scratch, fixup) {
        Ok(result) => result,
        Err(err) => FileResult {
            path: js_transcript_path(&entry.id),
            status: Status::Fail,
            tags: vec![tag::JS_TRANSCRIPT.to_owned()],
            tier_a: TierA::default(),
            tier_b: None,
            notes: format!("{err:#}"),
        },
    };
    std::fs::remove_dir_all(scratch).ok();
    Some(result)
}

fn score_js_transcript_inner(
    entry: &Entry,
    tool: &ToolPaths,
    scratch: &Path,
    fixup: &Path,
) -> Result<FileResult> {
    std::fs::create_dir_all(scratch)?;
    // The python expander, not a Rust one. docs/design/pdfrum-script.md §6.2
    // proposes reimplementing `fixup_pdf_template.py` in Rust; that is a
    // choice rather than a necessity, and every other cluster in this harness
    // already goes through the subprocess. Sharing one expander means the
    // javascript fixtures cannot disagree with the rest of the corpus about
    // what their own bytes are.
    let pdf_bytes = crate::generate::materialize_for_run(entry, scratch, fixup)?;
    let path = js_transcript_path(&entry.id);
    let input = scratch.join("input.pdf");
    std::fs::write(&input, &pdf_bytes)?;
    // **The sibling `.evt` goes beside the PDF**, because the oracle's own
    // text harness puts it there: `TestOneFileImpl.Generate` copies
    // `<test>.evt` next to the generated PDF and `TestText` then runs
    // `pdfium_test --send-events` unconditionally
    // (`testing/tools/test_runner.py:658-680`). Four of the 47 fixtures carry
    // one, and their expected text is a *record of those events* — without
    // the copy their `/AA` scripts never fire and the transcript is empty.
    if let Some(script) = entry.sibling_evt() {
        std::fs::copy(&script, scratch.join("input.evt"))?;
    }

    let output = Command::new(&tool.binary)
        .args(crate::oracle::determinism_args(&tool.font_dir))
        .arg("--js-transcript")
        .arg(&input)
        .output()
        .context("invoking pdfrum-tool --js-transcript")?;
    if output.status.code().is_none() {
        return Ok(FileResult {
            path,
            status: Status::Fail,
            tags: vec![tag::CRASH.to_owned()],
            tier_a: TierA::default(),
            tier_b: None,
            notes: "died by signal on --js-transcript".to_owned(),
        });
    }
    // stdout only. `testing/tools/test_runner.py` captures and compares
    // stdout; stderr carries `pdfium_test`'s own chatter and is never diffed.
    let produced = output.stdout;

    // The three branches of `test_runner.py`, in its order. The first keys on
    // the file being **absent**, not on its content being empty: an
    // `_expected.txt` that exists and is zero bytes takes the *diff* branch
    // (`bug_959274_1`), and an absent one takes `_VerifyEmptyText`
    // (`bug_1445426`). Keying either on the other gets one of the two wrong.
    let expected_path = expected_text_path(entry);
    let Ok(expected) = std::fs::read(&expected_path) else {
        return Ok(if produced.is_empty() {
            pass(path)
        } else {
            FileResult {
                path,
                status: Status::Fail,
                tags: vec![tag::JS_TRANSCRIPT.to_owned()],
                tier_a: TierA::default(),
                tier_b: None,
                notes: format!(
                    "no {} , so stdout must be empty; got {} bytes: {}",
                    expected_path.display(),
                    produced.len(),
                    truncate(&String::from_utf8_lossy(&produced))
                ),
            }
        });
    };

    match first_divergence(&expected, &produced) {
        None => Ok(pass(path)),
        Some((line, want, got)) => Ok(FileResult {
            path,
            status: Status::Fail,
            tags: vec![tag::JS_TRANSCRIPT.to_owned()],
            tier_a: TierA::default(),
            tier_b: None,
            notes: format!(
                "line {line}: expected {}, got {}",
                describe(want.as_deref()),
                describe(got.as_deref())
            ),
        }),
    }
}

/// A `js-transcript` row that matched. No tags, and no threshold: this is a
/// text tier and there is nothing to tune.
fn pass(path: String) -> FileResult {
    FileResult {
        path,
        status: Status::Pass,
        tags: Vec::new(),
        tier_a: TierA::default(),
        tier_b: None,
        notes: String::new(),
    }
}

/// The oracle's expected transcript, beside the template it came from.
fn expected_text_path(entry: &Entry) -> PathBuf {
    let stem = entry
        .source
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    entry.source.with_file_name(format!("{stem}_expected.txt"))
}

/// The first line the two sides disagree on, 1-based, with both sides.
///
/// `testing/tools/text_diff.py` opens both files in **text mode** and diffs
/// `readlines()`. Python's universal newlines turn `\r\n` *and a lone `\r`*
/// into `\n` on both sides before anything is compared; nothing else is
/// normalised, so leading and trailing whitespace, a blank line, and a
/// missing final newline are all significant and all diff.
fn first_divergence(
    expected: &[u8],
    produced: &[u8],
) -> Option<(usize, Option<String>, Option<String>)> {
    let want = text_lines(expected);
    let got = text_lines(produced);
    for index in 0..want.len().max(got.len()) {
        let a = want.get(index);
        let b = got.get(index);
        if a != b {
            return Some((index + 1, a.cloned(), b.cloned()));
        }
    }
    None
}

/// `readlines()` over a universal-newlines text read.
///
/// Each element keeps its terminator, exactly as `readlines()` does, so a
/// final line without one is not equal to the same line with one — which is
/// the difference `difflib` reports as `\ No newline at end of file`.
fn text_lines(bytes: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(bytes);
    // Universal newlines: CRLF first so the CR of a CRLF is not turned into
    // its own line break.
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut lines = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        current.push(ch);
        if ch == '\n' {
            lines.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

/// One side of a divergence, readable in a scoreboard note.
fn describe(line: Option<&str>) -> String {
    match line {
        None => "end of output".to_owned(),
        Some(text) => format!("{:?}", truncate(text)),
    }
}

/// Enough of a line to identify it, and no more: a note goes in a JSON board
/// a person reads.
fn truncate(text: &str) -> String {
    const LIMIT: usize = 120;
    let trimmed = text.trim_end_matches(['\r', '\n']);
    if trimmed.chars().count() <= LIMIT {
        return trimmed.to_owned();
    }
    let head: String = trimmed.chars().take(LIMIT).collect();
    format!("{head}…")
}

/// `--png --md5 --send-events`, harvesting PNGs under the events name.
fn invoke_tool_send_events(tool: &ToolPaths, input: &Path, digest: &str) -> Result<Produced> {
    let output = Command::new(&tool.binary)
        .args(crate::oracle::determinism_args(&tool.font_dir))
        .args(["--png", "--md5", "--send-events"])
        .arg(input)
        .output()?;
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let mut produced = Produced {
        page_count: parse_page_count(&stderr),
        crashed: if output.status.code().is_none() {
            vec!["send-events".to_owned()]
        } else {
            Vec::new()
        },
        artifacts: Vec::new(),
    };
    let harvested = crate::generate::harvest_for_run(input, crate::oracle::Pass::Render)?;
    produced.artifacts = harvested
        .into_iter()
        .filter_map(|(name, bytes)| {
            crate::generate::events_png_name(&name, digest).map(|renamed| (renamed, bytes))
        })
        .collect();
    produced.artifacts.sort();
    Ok(produced)
}

#[cfg(test)]
// Exact float equality is deliberate here: these assertions pin values that
// are exact by construction (SSIM of identical input is 1.0; a threshold
// parsed from text round-trips bit-for-bit). An epsilon would weaken them.
#[allow(clippy::float_cmp, reason = "asserting exactly-representable values")]
mod tests {
    use super::*;
    use crate::goldens::Manifest;
    use crate::oracle::Md5Line;

    fn temp_store(tag: &str) -> (PathBuf, Store) {
        let root = std::env::temp_dir().join(format!(
            "pdfrum-run-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let store = Store::at(root.join("goldens"));
        (root, store)
    }

    fn png(width: u32, height: u32, value: u8) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut out, width, height);
            encoder.set_color(png::ColorType::Rgb);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer
                .write_image_data(&vec![value; (width * height * 3) as usize])
                .unwrap();
        }
        out
    }

    fn manifest_with(artifacts: &[&str], pages: u32) -> Manifest {
        Manifest {
            key: "deadbeefdeadbeef".to_owned(),
            sources: vec!["resources/x.pdf".to_owned()],
            page_count: Some(pages),
            artifacts: artifacts.iter().map(|a| (*a).to_owned()).collect(),
            md5: vec![Md5Line {
                path: "input.pdf.0.png".to_owned(),
                digest: "0".repeat(32),
            }],
            oracle_failures: vec![],
        }
    }

    #[test]
    fn a_missing_tool_binary_is_unsupported_with_a_helpful_message() {
        let state = probe_tool(&ToolPaths {
            binary: PathBuf::from("/nonexistent/pdfrum-tool"),
            font_dir: PathBuf::from("/fonts"),
        });
        match state {
            ToolState::Unsupported(reason) => {
                assert!(reason.contains("pdfrum-tool not found"));
                assert!(reason.contains("PDFRUM_TOOL"));
            }
            ToolState::Ready => panic!("a missing binary must not probe as ready"),
        }
    }

    #[test]
    fn a_binary_that_answers_nothing_useful_is_unsupported() {
        // `true` runs, exits zero and prints nothing -- the shape of a stub.
        // The probe must reject it on what it did not say, not on what it did:
        // a real tool legitimately reports flags it has not implemented, and
        // reading that as a stub marker would tag a working tool's whole
        // corpus `unsupported-tool`.
        let state = probe_tool(&ToolPaths {
            binary: PathBuf::from("/bin/true"),
            font_dir: PathBuf::from("/fonts"),
        });
        match state {
            ToolState::Unsupported(reason) => {
                assert!(reason.contains("pageinfo probe"), "{reason}");
            }
            ToolState::Ready => panic!("a silent binary must not probe as ready"),
        }
    }

    #[test]
    fn the_probe_document_is_the_one_the_probe_expects_an_answer_about() {
        // If the fixture and the expected line ever drift apart, every run
        // silently reports an all-`unsupported-tool` board again.
        assert!(String::from_utf8_lossy(PROBE_PDF).contains("/MediaBox[0 0 200 300]"));
        assert!(PROBE_EXPECTED.contains("0.00 0.00 200.00 300.00"));
        assert!(PROBE_EXPECTED.starts_with("Page 0: MediaBox:"));
    }

    #[test]
    fn an_unsupported_tool_tags_every_file_the_same_way() {
        let entry = Entry {
            id: "corpus/a.pdf".to_owned(),
            source: PathBuf::from("/x/a.pdf"),
            kind: crate::corpus::EntryKind::Pdf,
        };
        let (root, store) = temp_store("unsupported");
        let result = score_one(
            &entry,
            &ToolPaths {
                binary: PathBuf::from("/nonexistent"),
                font_dir: PathBuf::from("/fonts"),
            },
            &ToolState::Unsupported("stub".to_owned()),
            &store,
            &Thresholds::default(),
            &root.join("scratch"),
            Path::new("/nonexistent/fixup.py"),
        );
        assert_eq!(result.status, Status::Fail);
        assert_eq!(result.tags, [tag::UNSUPPORTED_TOOL]);
        assert_eq!(result.tier_b, None);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn identical_dumps_and_pages_are_a_clean_tier_a() {
        let (root, store) = temp_store("tier-a-clean");
        let manifest = manifest_with(&["metadata.txt", "input.pdf.0.txt"], 1);
        store.write_manifest(&manifest).unwrap();
        store
            .write_artifact(&manifest.key, "metadata.txt", b"")
            .unwrap();
        store
            .write_artifact(&manifest.key, "input.pdf.0.txt", b"Hello")
            .unwrap();

        let produced = Produced {
            artifacts: vec![
                ("metadata.txt".to_owned(), b"".to_vec()),
                ("input.pdf.0.txt".to_owned(), b"Hello".to_vec()),
            ],
            page_count: Some(1),
            crashed: vec![],
        };
        let (mut tags, mut notes) = (Vec::new(), Vec::new());
        let tier_a = compare_tier_a(&store, &manifest, &produced, &mut tags, &mut notes);
        assert!(tier_a.is_clean());
        assert!(tags.is_empty());
        assert_eq!(tier_a.compared.len(), 2);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_golden_from_a_crashed_oracle_pass_is_not_compared() {
        // `redact_annot`'s shape: the oracle's annotation dump has no arm for
        // a redaction subtype and aborts on one, leaving a zero-byte file
        // behind. Comparing against it would make the crash the contract, so
        // that pass's artifacts drop out of the comparison entirely — while
        // the passes that *did* succeed still count.
        let (root, store) = temp_store("crashed-pass");
        let mut manifest = manifest_with(&["input.pdf.0.annot.txt", "metadata.txt"], 1);
        manifest.oracle_failures = vec!["Annot".to_owned()];
        store.write_manifest(&manifest).unwrap();
        store
            .write_artifact(&manifest.key, "input.pdf.0.annot.txt", b"")
            .unwrap();
        store
            .write_artifact(&manifest.key, "metadata.txt", b"")
            .unwrap();

        let produced = Produced {
            artifacts: vec![
                (
                    "input.pdf.0.annot.txt".to_owned(),
                    b"Number of annotations: 1\n\n".to_vec(),
                ),
                ("metadata.txt".to_owned(), b"".to_vec()),
            ],
            page_count: Some(1),
            crashed: vec![],
        };
        let (mut tags, mut notes) = (Vec::new(), Vec::new());
        let tier_a = compare_tier_a(&store, &manifest, &produced, &mut tags, &mut notes);
        assert!(tier_a.is_clean());
        assert_eq!(tier_a.compared, ["metadata.txt"]);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn an_empty_golden_dump_must_be_matched_by_empty_output() {
        // --show-metadata legitimately prints nothing; producing text where
        // the oracle printed none is a real Tier A failure.
        let (root, store) = temp_store("empty-dump");
        let manifest = manifest_with(&["metadata.txt"], 1);
        store.write_manifest(&manifest).unwrap();
        store
            .write_artifact(&manifest.key, "metadata.txt", b"")
            .unwrap();

        let produced = Produced {
            artifacts: vec![("metadata.txt".to_owned(), b"Title = x".to_vec())],
            page_count: Some(1),
            crashed: vec![],
        };
        let (mut tags, mut notes) = (Vec::new(), Vec::new());
        let tier_a = compare_tier_a(&store, &manifest, &produced, &mut tags, &mut notes);
        assert_eq!(tier_a.mismatched, ["metadata.txt"]);
        assert_eq!(tags, [tag::TIER_A_MISMATCH]);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn text_dumps_are_counted_apart_from_the_other_artifacts() {
        let (root, store) = temp_store("text-score");
        let manifest = manifest_with(
            &[
                "input.pdf.0.txt",
                "input.pdf.1.txt",
                "input.pdf.0.annot.txt",
                "metadata.txt",
            ],
            2,
        );
        store.write_manifest(&manifest).unwrap();
        // Page 0 holds text, page 1's dump was a bare byte-order mark and so
        // is stored empty. Both the annot dump and the metadata dump end in
        // `.txt` and neither is a text dump.
        store
            .write_artifact(&manifest.key, "input.pdf.0.txt", b"Hello")
            .unwrap();
        store
            .write_artifact(&manifest.key, "input.pdf.1.txt", b"")
            .unwrap();
        store
            .write_artifact(&manifest.key, "input.pdf.0.annot.txt", b"")
            .unwrap();
        store
            .write_artifact(&manifest.key, "metadata.txt", b"")
            .unwrap();

        // Our tool got the empty page right and the substantive one wrong --
        // exactly the shape the nonempty aggregate exists to expose.
        let produced = Produced {
            artifacts: vec![
                ("input.pdf.0.txt".to_owned(), b"Goodbye".to_vec()),
                ("input.pdf.1.txt".to_owned(), b"".to_vec()),
                ("input.pdf.0.annot.txt".to_owned(), b"".to_vec()),
                ("metadata.txt".to_owned(), b"".to_vec()),
            ],
            page_count: Some(2),
            crashed: vec![],
        };
        let (mut tags, mut notes) = (Vec::new(), Vec::new());
        let tier_a = compare_tier_a(&store, &manifest, &produced, &mut tags, &mut notes);
        assert_eq!(tier_a.text.pages, 2);
        assert_eq!(tier_a.text.matched, 1);
        assert_eq!(tier_a.text.substantive, 1);
        assert_eq!(tier_a.text.substantive_matched, 0);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_dump_artifacts_are_not_mistaken_for_page_text() {
        assert!(is_text_dump("input.pdf.0.txt"));
        assert!(is_text_dump("input.pdf.12.txt"));
        assert!(!is_text_dump("input.pdf.0.annot.txt"));
        assert!(!is_text_dump("metadata.txt"));
        assert!(!is_text_dump("pageinfo.txt"));
        assert!(!is_text_dump("structure.txt"));
        assert!(!is_text_dump("input.pdf.0.png"));
    }

    #[test]
    fn a_page_count_disagreement_is_tagged() {
        let (root, store) = temp_store("pages");
        let manifest = manifest_with(&[], 3);
        store.write_manifest(&manifest).unwrap();
        let produced = Produced {
            artifacts: vec![],
            page_count: Some(2),
            crashed: vec![],
        };
        let (mut tags, mut notes) = (Vec::new(), Vec::new());
        let tier_a = compare_tier_a(&store, &manifest, &produced, &mut tags, &mut notes);
        assert!(!tier_a.is_clean());
        assert_eq!(tags, [tag::PAGE_COUNT]);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn an_identical_render_passes_tier_b() {
        let (root, store) = temp_store("tier-b-pass");
        let manifest = manifest_with(&["input.pdf.0.png"], 1);
        store.write_manifest(&manifest).unwrap();
        let bytes = png(16, 16, 200);
        store
            .write_artifact(&manifest.key, "input.pdf.0.png", &bytes)
            .unwrap();

        let produced = Produced {
            artifacts: vec![("input.pdf.0.png".to_owned(), bytes)],
            page_count: Some(1),
            crashed: vec![],
        };
        let (mut tags, mut notes) = (Vec::new(), Vec::new());
        let tier_b = compare_tier_b(&store, &manifest, &produced, 0.99, &mut tags, &mut notes)
            .expect("a png artifact yields a tier B score");
        assert_eq!(tier_b.ssim, 1.0);
        assert!(tier_b.exact);
        assert_eq!(tier_b.pages, 1);
        assert!(tags.is_empty());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_render_below_the_floor_is_a_pixel_fail() {
        let (root, store) = temp_store("tier-b-fail");
        let manifest = manifest_with(&["input.pdf.0.png"], 1);
        store.write_manifest(&manifest).unwrap();
        store
            .write_artifact(&manifest.key, "input.pdf.0.png", &png(16, 16, 0))
            .unwrap();

        let produced = Produced {
            artifacts: vec![("input.pdf.0.png".to_owned(), png(16, 16, 255))],
            page_count: Some(1),
            crashed: vec![],
        };
        let (mut tags, mut notes) = (Vec::new(), Vec::new());
        let tier_b =
            compare_tier_b(&store, &manifest, &produced, 0.99, &mut tags, &mut notes).unwrap();
        assert!(tier_b.ssim < 0.99);
        assert_eq!(tier_b.max_channel_diff, 255);
        assert_eq!(tags, [tag::PIXEL_FAIL]);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_wrong_sized_render_is_tagged_separately_from_a_pixel_fail() {
        let (root, store) = temp_store("size");
        let manifest = manifest_with(&["input.pdf.0.png"], 1);
        store.write_manifest(&manifest).unwrap();
        store
            .write_artifact(&manifest.key, "input.pdf.0.png", &png(16, 16, 100))
            .unwrap();

        let produced = Produced {
            artifacts: vec![("input.pdf.0.png".to_owned(), png(8, 8, 100))],
            page_count: Some(1),
            crashed: vec![],
        };
        let (mut tags, mut notes) = (Vec::new(), Vec::new());
        compare_tier_b(&store, &manifest, &produced, 0.99, &mut tags, &mut notes).unwrap();
        assert!(tags.contains(&tag::SIZE_MISMATCH.to_owned()));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_missing_render_is_a_pixel_fail_not_a_crash() {
        let (root, store) = temp_store("missing-png");
        let manifest = manifest_with(&["input.pdf.0.png"], 1);
        store.write_manifest(&manifest).unwrap();
        store
            .write_artifact(&manifest.key, "input.pdf.0.png", &png(4, 4, 1))
            .unwrap();

        let (mut tags, mut notes) = (Vec::new(), Vec::new());
        let tier_b = compare_tier_b(
            &store,
            &manifest,
            &Produced::default(),
            0.99,
            &mut tags,
            &mut notes,
        )
        .unwrap();
        assert_eq!(tier_b.ssim, 0.0);
        assert_eq!(tags, [tag::PIXEL_FAIL]);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_file_with_no_golden_pngs_has_no_tier_b_score() {
        let (root, store) = temp_store("no-png");
        let manifest = manifest_with(&["metadata.txt"], 1);
        store.write_manifest(&manifest).unwrap();
        let (mut tags, mut notes) = (Vec::new(), Vec::new());
        assert_eq!(
            compare_tier_b(
                &store,
                &manifest,
                &Produced::default(),
                0.99,
                &mut tags,
                &mut notes
            ),
            None
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_worst_page_decides_the_file_score() {
        let (root, store) = temp_store("worst-page");
        let manifest = manifest_with(&["input.pdf.0.png", "input.pdf.1.png"], 2);
        store.write_manifest(&manifest).unwrap();
        let good = png(16, 16, 128);
        store
            .write_artifact(&manifest.key, "input.pdf.0.png", &good)
            .unwrap();
        store
            .write_artifact(&manifest.key, "input.pdf.1.png", &png(16, 16, 0))
            .unwrap();

        let produced = Produced {
            artifacts: vec![
                ("input.pdf.0.png".to_owned(), good),
                ("input.pdf.1.png".to_owned(), png(16, 16, 255)),
            ],
            page_count: Some(2),
            crashed: vec![],
        };
        let (mut tags, mut notes) = (Vec::new(), Vec::new());
        let tier_b =
            compare_tier_b(&store, &manifest, &produced, 0.99, &mut tags, &mut notes).unwrap();
        // Page 0 is perfect, page 1 is black-vs-white: the file scores as page 1.
        assert!(tier_b.ssim < 0.01);
        assert!(!tier_b.exact);
        assert_eq!(tier_b.pages, 2);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn send_events_pngs_are_ignored_by_the_plain_tier_b_walk() {
        // A form-events golden lives in the same directory; comparing it
        // here would fail every existing file that gained one.
        let (root, store) = temp_store("events-ignored");
        let manifest = manifest_with(&["input.pdf.0.png", "input.pdf.0.events-1f4a09c3.png"], 1);
        store.write_manifest(&manifest).unwrap();
        let plain = png(8, 8, 40);
        store
            .write_artifact(&manifest.key, "input.pdf.0.png", &plain)
            .unwrap();
        store
            .write_artifact(
                &manifest.key,
                "input.pdf.0.events-1f4a09c3.png",
                &png(8, 8, 200),
            )
            .unwrap();

        let produced = Produced {
            artifacts: vec![("input.pdf.0.png".to_owned(), plain)],
            page_count: Some(1),
            crashed: vec![],
        };
        let (mut tags, mut notes) = (Vec::new(), Vec::new());
        let tier_b =
            compare_tier_b(&store, &manifest, &produced, 0.99, &mut tags, &mut notes).unwrap();
        assert_eq!(tier_b.pages, 1);
        assert_eq!(tier_b.ssim, 1.0);
        assert!(tags.is_empty(), "{tags:?}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn two_scripts_sharing_a_golden_key_compare_against_their_own_png() {
        // bug_736695_2/3/4 expand to byte-identical PDFs, so they share one
        // golden directory. Before the script digest entered the artifact
        // name, whichever generated first defined the golden for all three
        // and the other two passed against the wrong image.
        let (root, store) = temp_store("events-two-scripts");
        let manifest = manifest_with(
            &[
                "input.pdf.0.png",
                "input.pdf.0.events-aaaaaaaa.png",
                "input.pdf.0.events-bbbbbbbb.png",
            ],
            1,
        );
        store.write_manifest(&manifest).unwrap();
        let (mine, theirs) = (png(8, 8, 10), png(8, 8, 250));
        store
            .write_artifact(&manifest.key, "input.pdf.0.png", &png(8, 8, 40))
            .unwrap();
        store
            .write_artifact(&manifest.key, "input.pdf.0.events-aaaaaaaa.png", &mine)
            .unwrap();
        store
            .write_artifact(&manifest.key, "input.pdf.0.events-bbbbbbbb.png", &theirs)
            .unwrap();

        // A tool that reproduces script "aaaaaaaa" passes on that row...
        let produced = Produced {
            artifacts: vec![("input.pdf.0.events-aaaaaaaa.png".to_owned(), mine)],
            page_count: Some(1),
            crashed: vec![],
        };
        let (mut tags, mut notes) = (Vec::new(), Vec::new());
        let tier_b = compare_events_pngs(
            &store, &manifest, &produced, 0.99, "aaaaaaaa", &mut tags, &mut notes,
        )
        .unwrap();
        assert_eq!(tier_b.pages, 1, "only this script's golden is compared");
        assert!(tags.is_empty(), "{tags:?}");

        // ...and does not thereby pass the sibling script's row.
        let (mut tags, mut notes) = (Vec::new(), Vec::new());
        compare_events_pngs(
            &store, &manifest, &produced, 0.99, "bbbbbbbb", &mut tags, &mut notes,
        )
        .unwrap();
        assert_eq!(tags, [tag::FORM_EVENTS]);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_javascript_deferral_is_retired() {
        // M14 deferred four JavaScript `.evt` fixtures to M15 by name, so the
        // row would be retired rather than forgotten. M15 landed the
        // `js-transcript` cluster; the deferral is gone, and these four score
        // a `#form-events` row like every other `.evt` fixture. This test is
        // what stops it coming back by accident.
        for stem in [
            "resources/javascript/bug_1445426",
            "resources/javascript/bug_1447268",
            "resources/javascript/mouse_events",
            "resources/javascript/public_methods",
        ] {
            assert_eq!(
                form_events_path(&format!("{stem}.in")),
                format!("{stem}.in#form-events")
            );
        }
    }

    #[test]
    fn the_js_transcript_cluster_is_exactly_the_javascript_templates() {
        assert!(is_js_fixture("resources/javascript/consts.in"));
        // The checked-in `.pdf` beside a template is not a second row: the
        // cluster is the 47 `.in` fixtures, and scoring both would double
        // every one of them.
        assert!(!is_js_fixture("resources/javascript/consts.pdf"));
        assert!(!is_js_fixture("resources/pixel/checkbox_radiobutton.in"));
        assert!(!is_js_fixture(
            "corpus/pdfium/annots/annotation_highlight_no_content.pdf"
        ));
    }

    #[test]
    fn js_transcript_path_does_not_collide_with_the_other_two_families() {
        let id = "resources/javascript/consts.in";
        assert_eq!(
            js_transcript_path(id),
            "resources/javascript/consts.in#js-transcript"
        );
        assert_ne!(js_transcript_path(id), form_events_path(id));
        assert_ne!(js_transcript_path(id), id);
    }

    #[test]
    fn the_transcript_diff_normalises_line_endings_and_nothing_else() {
        // Universal newlines: both spellings of a break compare equal, on
        // either side, which is what opening in text mode buys.
        assert_eq!(first_divergence(b"a\r\nb\n", b"a\nb\n"), None);
        assert_eq!(first_divergence(b"a\rb\n", b"a\nb\n"), None);
        // Everything else is significant.
        let (line, want, got) = first_divergence(b"Alert: x\n", b"Alert: x \n").unwrap();
        assert_eq!(line, 1);
        assert_eq!(want.unwrap(), "Alert: x\n");
        assert_eq!(got.unwrap(), "Alert: x \n");
        // A missing final newline is a divergence, as `difflib` reports it.
        assert!(first_divergence(b"a\n", b"a").is_some());
        // A blank line is a line.
        assert_eq!(first_divergence(b"a\n\nb\n", b"a\nb\n").unwrap().0, 2);
        // A short side diverges at the first line the other still has.
        let (line, want, got) = first_divergence(b"a\nb\n", b"a\n").unwrap();
        assert_eq!((line, got), (2, None));
        assert_eq!(want.unwrap(), "b\n");
        // Two empty sides agree, which is the `_VerifyEmptyText` case's
        // neighbour: a zero-byte `_expected.txt` against empty stdout.
        assert_eq!(first_divergence(b"", b""), None);
    }

    #[test]
    fn form_events_path_does_not_collide_with_the_plain_id() {
        assert_eq!(
            form_events_path("resources/pixel/checkbox_radiobutton.pdf"),
            "resources/pixel/checkbox_radiobutton.pdf#form-events"
        );
        assert_ne!(form_events_path("resources/x.pdf"), "resources/x.pdf");
    }

    #[test]
    fn an_events_mismatch_is_tagged_form_events_not_pixel_fail() {
        let (root, store) = temp_store("events-mismatch");
        let manifest = manifest_with(&["input.pdf.0.png", "input.pdf.0.events-1f4a09c3.png"], 1);
        store.write_manifest(&manifest).unwrap();
        store
            .write_artifact(&manifest.key, "input.pdf.0.png", &png(8, 8, 40))
            .unwrap();
        store
            .write_artifact(
                &manifest.key,
                "input.pdf.0.events-1f4a09c3.png",
                &png(8, 8, 0),
            )
            .unwrap();

        let produced = Produced {
            artifacts: vec![("input.pdf.0.events-1f4a09c3.png".to_owned(), png(8, 8, 255))],
            page_count: Some(1),
            crashed: vec![],
        };
        let (mut tags, mut notes) = (Vec::new(), Vec::new());
        let tier_b = compare_events_pngs(
            &store, &manifest, &produced, 0.99, "1f4a09c3", &mut tags, &mut notes,
        )
        .unwrap();
        assert!(tier_b.ssim < 0.99);
        assert_eq!(tags, [tag::FORM_EVENTS]);
        assert!(!tags.iter().any(|t| t == tag::PIXEL_FAIL));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_missing_golden_is_reported_rather_than_scored() {
        let (root, store) = temp_store("no-golden");
        let pdf = root.join("a.pdf");
        std::fs::write(&pdf, b"%PDF-1.7\n").unwrap();
        let entry = Entry {
            id: "resources/a.pdf".to_owned(),
            source: pdf,
            kind: crate::corpus::EntryKind::Pdf,
        };
        let result = score_inner(
            &entry,
            &ToolPaths {
                binary: PathBuf::from("/nonexistent"),
                font_dir: PathBuf::from("/fonts"),
            },
            &store,
            &Thresholds::default(),
            &root.join("scratch"),
            Path::new("/nonexistent/fixup.py"),
        )
        .unwrap();
        assert_eq!(result.tags, [tag::MISSING_GOLDEN]);
        assert_eq!(result.status, Status::Fail);
        std::fs::remove_dir_all(&root).ok();
    }
}

/// What comparing one file under both rasterizers found (Tier C).
#[derive(Debug, Clone, PartialEq)]
pub struct TierCOutcome {
    /// The corpus-relative id.
    pub path: String,
    /// Pages compared under both backends.
    pub pages: u32,
    /// Whether any page failed the contract outright: a size mismatch, one
    /// backend painting nothing, or an interior pixel outside the rounding
    /// budget. Any of the three is an engine bug, not a rasterizer one.
    pub hard_fail: bool,
    /// Whether every page also stayed inside the soft edge budget.
    pub within_budget: bool,
    /// The worst page's share of differing edge pixels.
    pub edge_rate: f64,
    /// The same worst-page edge rate for the analytic backend against the
    /// gating pair's baseline, or `None` when it produced no pages.
    ///
    /// **Reported, never gated.** Tier C's value is that two *independent*
    /// implementations disagree out loud; `pdfrum-raster-agg` shares this
    /// project's engine-facing arithmetic with the engine itself — one
    /// `blend::composite_premultiplied` serves all three backends — so a
    /// disagreement between it and either wrapped backend tests less than a
    /// disagreement between the two wrapped ones, not more. Its column is
    /// here so a divergence is visible, not so it can fail a run.
    pub agg_edge_rate: Option<f64>,
    /// What went wrong, when something did.
    pub note: String,
}

impl TierCOutcome {
    fn skipped(path: String, note: String) -> Self {
        Self {
            path,
            pages: 0,
            hard_fail: false,
            within_budget: true,
            edge_rate: 0.0,
            agg_edge_rate: None,
            note,
        }
    }
}

/// Render one file with each rasterizer and diff the two.
///
/// Both runs are the same binary with the same flags; only `PDFRUM_BACKEND`
/// differs, so anything that differs in the output was decided below the
/// `RenderDevice` seam. A file the tool cannot render at all is *skipped*
/// rather than failed: Tier B is where "we produced nothing" is scored, and
/// counting it twice would let a parse regression masquerade as a backend
/// divergence.
pub fn compare_backends(
    entry: &crate::corpus::Entry,
    tool: &ToolPaths,
    scratch: &Path,
    fixup: &Path,
) -> TierCOutcome {
    let outcome = compare_backends_inner(entry, tool, scratch, fixup);
    std::fs::remove_dir_all(scratch).ok();
    outcome
}

fn compare_backends_inner(
    entry: &crate::corpus::Entry,
    tool: &ToolPaths,
    scratch: &Path,
    fixup: &Path,
) -> TierCOutcome {
    let id = entry.id.clone();
    if std::fs::create_dir_all(scratch).is_err() {
        return TierCOutcome::skipped(id, "no scratch directory".to_owned());
    }
    // `materialize_for_run` *returns* the bytes; writing them under the fixed
    // name is the caller's job, and it is what makes the harvested artifact
    // names line up with the golden store's.
    let Ok(bytes) = crate::generate::materialize_for_run(entry, scratch, fixup) else {
        return TierCOutcome::skipped(id, "could not materialize".to_owned());
    };
    let input = scratch.join("input.pdf");
    if std::fs::write(&input, &bytes).is_err() {
        return TierCOutcome::skipped(id, "could not write the scratch input".to_owned());
    }

    let Some(tiny) = render_with(tool, &input, "tiny-skia") else {
        return TierCOutcome::skipped(id, "tiny-skia produced nothing".to_owned());
    };
    let Some(vello) = render_with(tool, &input, "vello-cpu") else {
        return TierCOutcome::skipped(id, "vello_cpu produced nothing".to_owned());
    };
    // The third backend is rendered but not gated on: see
    // `TierCOutcome::agg_edge_rate` for why it is a reported column. A file
    // it cannot render is not a Tier C failure either, so this is an `Option`
    // rather than an early return.
    let agg = render_with(tool, &input, "agg");

    let mut out = TierCOutcome {
        path: id,
        pages: 0,
        hard_fail: false,
        within_budget: true,
        edge_rate: 0.0,
        agg_edge_rate: None,
        note: String::new(),
    };
    for (name, tiny_png) in &tiny {
        let Some((_, vello_png)) = vello.iter().find(|(n, _)| n == name) else {
            out.hard_fail = true;
            out.note = format!("{name}: only tiny-skia produced a page");
            continue;
        };
        // Byte-identical output needs no decode, and is the common case for
        // a page whose every decision the engine made.
        if tiny_png == vello_png {
            out.pages += 1;
            continue;
        }
        let (Ok(a), Ok(b)) = (
            crate::pixels::decode(tiny_png),
            crate::pixels::decode(vello_png),
        ) else {
            out.hard_fail = true;
            out.note = format!("{name}: a backend's PNG would not decode");
            continue;
        };
        let Some(diff) = crate::tierc::compare(&a, &b) else {
            out.hard_fail = true;
            out.note = format!(
                "{name}: {}x{} vs {}x{} - a size difference is an engine decision",
                a.width, a.height, b.width, b.height
            );
            continue;
        };
        out.pages += 1;
        out.edge_rate = out.edge_rate.max(diff.edge_rate());
        if diff.hard_fail() {
            out.hard_fail = true;
            if out.note.is_empty() {
                out.note = format!(
                    "{name}: {} interior pixels differ (max {} counts){}",
                    diff.interior_differing,
                    diff.max_channel_diff,
                    if diff.one_painted_nothing {
                        "; one backend painted nothing"
                    } else {
                        ""
                    }
                );
            }
        }
        if !diff.passes() {
            out.within_budget = false;
        }
    }
    // The analytic backend's own column, measured against the same baseline
    // the gating pair's first half uses so the three numbers are comparable.
    if let Some(agg) = &agg {
        for (name, tiny_png) in &tiny {
            let Some((_, agg_png)) = agg.iter().find(|(n, _)| n == name) else {
                continue;
            };
            if tiny_png == agg_png {
                out.agg_edge_rate = Some(out.agg_edge_rate.unwrap_or(0.0));
                continue;
            }
            let (Ok(a), Ok(b)) = (
                crate::pixels::decode(tiny_png),
                crate::pixels::decode(agg_png),
            ) else {
                continue;
            };
            if let Some(diff) = crate::tierc::compare(&a, &b) {
                let rate = diff.edge_rate();
                out.agg_edge_rate = Some(out.agg_edge_rate.unwrap_or(0.0).max(rate));
            }
        }
    }
    out
}

/// Run the tool once under one backend and collect the PNGs it wrote.
fn render_with(tool: &ToolPaths, input: &Path, backend: &str) -> Option<Vec<(String, Vec<u8>)>> {
    let status = Command::new(&tool.binary)
        .env("PDFRUM_BACKEND", backend)
        .args(crate::oracle::determinism_args(&tool.font_dir))
        .args(crate::oracle::Pass::Render.flags())
        .arg(input)
        .output()
        .ok()?;
    // A signal-terminated run has no pixels to compare, and Tier B already
    // scores it as a crash.
    status.status.code()?;
    let mut pngs = crate::generate::harvest_for_run(input, crate::oracle::Pass::Render).ok()?;
    pngs.sort();
    (!pngs.is_empty()).then_some(pngs)
}
