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
    let pngs: Vec<&String> = manifest
        .artifacts
        .iter()
        .filter(|name| crate::generate::has_suffix(name, ".png"))
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
            tags.push(tag::PIXEL_FAIL.to_owned());
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

    if worst.ssim < floor && !tags.iter().any(|t| t == tag::PIXEL_FAIL) {
        tags.push(tag::PIXEL_FAIL.to_owned());
        notes.push(format!("ssim {:.6} below floor {floor:.6}", worst.ssim));
    }
    Some(worst)
}

/// Builds the per-entry scratch path for a run.
pub fn scratch(base: &Path, index: usize) -> PathBuf {
    scratch_for(base, index)
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
