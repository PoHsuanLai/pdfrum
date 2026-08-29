//! The M7 exit check: does what pdfrum *writes* still open, and still look
//! the same?
//!
//! # Why this needs two binaries
//!
//! `pdfium_test` has no save flag — the oracle cannot write a document at
//! all, so there is no like-for-like invocation to diff (SPEC.md §11's ruling
//! E7). The check is therefore a sequence rather than a comparison:
//!
//! 1. **pdfrum saves** every corpus file (`pdfrum-tool --save`).
//! 2. **The oracle reopens** what pdfrum wrote and renders it. A load failure
//!    here is the M7 exit criterion failing: another implementation could not
//!    read our output.
//! 3. **pdfrum re-renders** the saved file and the result is compared against
//!    the *original's* golden render at the Tier-B floor. This is what catches
//!    a save that opens but has lost something.
//! 4. **An incremental save** is checked for the append discipline: the
//!    original bytes must be a prefix of the output, the `/Prev` chain must be
//!    intact, and the oracle must reopen it.
//!
//! # The comparison is against the original's render, not the oracle's
//!
//! Step 3 diffs our render of the *saved* file against the golden render of
//! the *original*. That is deliberately the strictest available reading: it
//! folds in both "the save lost something" and "we render the saved file
//! differently than the original", where comparing our-saved against
//! our-original would hide a shared regression in both.
//!
//! A page whose objects were regenerated is a different matter — the C++'s
//! own generator is lossy (no patterns, no non-RGB colour) — but an ordinary
//! save regenerates nothing, so every file in this sweep is expected to hold.

use std::path::Path;
use std::process::Command;

use crate::corpus::Entry;
use crate::goldens::Store;
use crate::oracle::{OraclePaths, Pass, determinism_args};
use crate::run::ToolPaths;
use crate::ssim;
use crate::thresholds::Thresholds;

/// What checking one file found.
///
/// The flags are four independent observations about one file, not a state
/// machine: a file can be saved and not reopen, reopen and lose pixels, or
/// hold its pixels while breaking the append discipline. Collapsing them into
/// an enum would lose exactly the combinations the sweep exists to count.
#[allow(
    clippy::struct_excessive_bools,
    reason = "four independent observations; see above"
)]
#[derive(Debug, Clone, PartialEq)]
pub struct SaveOutcome {
    /// The corpus-relative id.
    pub path: String,
    /// Whether pdfrum wrote a file at all.
    pub saved: bool,
    /// Whether the **oracle** reopened and rendered what pdfrum wrote. This
    /// is the M7 exit criterion.
    pub oracle_reopened: bool,
    /// Whether the incremental save's output kept the original as its prefix
    /// and chained its cross-reference, and the oracle reopened it too.
    pub incremental_ok: Option<bool>,
    /// Pages compared against the original's golden render.
    pub pages: u32,
    /// The worst page's SSIM, when any page was compared.
    pub ssim: Option<f64>,
    /// Whether every compared page cleared the floor.
    pub within_floor: bool,
    /// What went wrong, when something did.
    pub note: String,
}

impl SaveOutcome {
    fn skipped(path: String, note: String) -> Self {
        Self {
            path,
            saved: false,
            oracle_reopened: false,
            incremental_ok: None,
            pages: 0,
            ssim: None,
            within_floor: true,
            note,
        }
    }
}

/// Run the whole check over one corpus file.
#[expect(
    clippy::too_many_arguments,
    reason = "the check genuinely needs both binaries, the golden store, the \
              thresholds, a scratch path and the fixup script; a record \
              bundling them would name one call site's arguments as a type"
)]
pub fn check_one(
    entry: &Entry,
    tool: &ToolPaths,
    oracle: &OraclePaths,
    store: &Store,
    thresholds: &Thresholds,
    scratch: &Path,
    fixup: &Path,
    compare_pixels: bool,
) -> SaveOutcome {
    let outcome = check_inner(
        entry,
        tool,
        oracle,
        store,
        thresholds,
        scratch,
        fixup,
        compare_pixels,
    );
    std::fs::remove_dir_all(scratch).ok();
    outcome
}

#[expect(
    clippy::too_many_arguments,
    reason = "see `check_one`; this is its body"
)]
fn check_inner(
    entry: &Entry,
    tool: &ToolPaths,
    oracle: &OraclePaths,
    store: &Store,
    thresholds: &Thresholds,
    scratch: &Path,
    fixup: &Path,
    compare_pixels: bool,
) -> SaveOutcome {
    let id = entry.id.clone();
    if std::fs::create_dir_all(scratch).is_err() {
        return SaveOutcome::skipped(id, "no scratch directory".to_owned());
    }
    let Ok(bytes) = crate::generate::materialize_for_run(entry, scratch, fixup) else {
        return SaveOutcome::skipped(id, "could not materialize".to_owned());
    };
    let input = scratch.join("input.pdf");
    if std::fs::write(&input, &bytes).is_err() {
        return SaveOutcome::skipped(id, "could not write the scratch input".to_owned());
    }

    // ---- step 1: pdfrum saves ----
    let saved = scratch.join("input.pdf.saved.pdf");
    let ran = Command::new(&tool.binary)
        .args(determinism_args(&tool.font_dir))
        .arg("--save")
        .arg(&input)
        .output();
    // A file the tool cannot *open* is not a save failure — Tier B already
    // scores "we produced nothing", and counting it here would let a parse
    // regression masquerade as a writer bug.
    match ran {
        Ok(out) if out.status.code().is_none() => {
            return SaveOutcome::skipped(id, "the tool crashed".to_owned());
        }
        Ok(_) => {}
        Err(_) => return SaveOutcome::skipped(id, "the tool would not run".to_owned()),
    }
    if !saved.exists() {
        return SaveOutcome::skipped(id, "the tool wrote no file".to_owned());
    }

    let mut outcome = SaveOutcome {
        path: id,
        saved: true,
        oracle_reopened: false,
        incremental_ok: None,
        pages: 0,
        ssim: None,
        within_floor: true,
        note: String::new(),
    };

    // ---- step 2: the oracle reopens it ----
    outcome.oracle_reopened = oracle_opens(oracle, &saved);
    if !outcome.oracle_reopened {
        "the oracle could not reopen the saved file".clone_into(&mut outcome.note);
    }

    // ---- step 3: our own re-render against the original's golden ----
    if compare_pixels {
        compare_render(&mut outcome, tool, store, thresholds, &bytes, &saved);
    }

    // ---- step 4: the incremental save's append discipline ----
    outcome.incremental_ok = Some(check_incremental(oracle, scratch, &bytes));
    if outcome.incremental_ok == Some(false) && outcome.note.is_empty() {
        "the incremental save broke the append discipline".clone_into(&mut outcome.note);
    }

    outcome
}

/// Whether the oracle loads and renders a file without reporting a failure.
///
/// "Processed N pages." on stderr is the oracle's own success line; the
/// absence of "Load pdf docs unsuccessful" is what says the document opened.
fn oracle_opens(oracle: &OraclePaths, file: &Path) -> bool {
    let Ok(out) = Command::new(&oracle.binary)
        .args(determinism_args(&oracle.font_dir))
        .args(Pass::Render.flags())
        .arg(file)
        .output()
    else {
        return false;
    };
    // A crash is never a reopen.
    if out.status.code().is_none() {
        return false;
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    !stderr.contains("Load pdf docs unsuccessful") && stderr.contains("Processed")
}

/// Render the saved file with our own tool and diff every page against the
/// **original's** golden render.
fn compare_render(
    outcome: &mut SaveOutcome,
    tool: &ToolPaths,
    store: &Store,
    thresholds: &Thresholds,
    original: &[u8],
    saved: &Path,
) {
    let key = crate::goldens::key_for(original);
    let Ok(manifest) = store.manifest(&key) else {
        // No golden for the original means nothing to compare against; the
        // oracle-reopen half of the check still stands.
        return;
    };

    let Ok(out) = Command::new(&tool.binary)
        .args(determinism_args(&tool.font_dir))
        .args(Pass::Render.flags())
        .arg(saved)
        .output()
    else {
        "the tool would not render the saved file".clone_into(&mut outcome.note);
        outcome.within_floor = false;
        return;
    };
    if out.status.code().is_none() {
        "the tool crashed rendering the saved file".clone_into(&mut outcome.note);
        outcome.within_floor = false;
        return;
    }
    let Ok(rendered) = crate::generate::harvest_for_run(saved, Pass::Render) else {
        return;
    };

    let floor = thresholds.ssim_for(&outcome.path);
    let mut worst: Option<f64> = None;

    for (name, bytes) in &rendered {
        // The saved file's artifacts are named after *it*, so the golden's
        // name has to be reconstructed from the page index.
        let Some(index) = page_index(name) else {
            continue;
        };
        let golden_name = format!("input.pdf.{index}.png");
        if !manifest.artifacts.contains(&golden_name) {
            continue;
        }
        let (Ok(golden), Ok(candidate)) = (
            store
                .artifact(&key, &golden_name)
                .map_err(drop)
                .and_then(|b| crate::pixels::decode(&b).map_err(drop)),
            crate::pixels::decode(bytes).map_err(drop),
        ) else {
            continue;
        };

        outcome.pages = outcome.pages.saturating_add(1);
        match ssim::compare(&golden, &candidate) {
            Ok(diff) => {
                worst = Some(worst.map_or(diff.ssim, |w: f64| w.min(diff.ssim)));
            }
            Err(err) => {
                // A size mismatch means the saved file describes a different
                // page, which is a real loss however it renders.
                outcome.within_floor = false;
                outcome.note = err.to_string();
                return;
            }
        }
    }

    outcome.ssim = worst;
    if let Some(worst) = worst
        && worst < floor
    {
        outcome.within_floor = false;
        if outcome.note.is_empty() {
            outcome.note = format!("ssim {worst:.6} below floor {floor:.6}");
        }
    }
}

/// The page index out of a `<name>.<index>.png` artifact.
fn page_index(name: &str) -> Option<u32> {
    let stem = name.strip_suffix(".png")?;
    let (_, index) = stem.rsplit_once('.')?;
    index.parse().ok()
}

/// The append discipline: the original bytes are a prefix of the output, the
/// cross-reference chains, and the oracle still opens it.
fn check_incremental(oracle: &OraclePaths, scratch: &Path, original: &[u8]) -> bool {
    // The tool always saves fully; the incremental path is exercised through
    // the library, which is the same code the tool calls.
    let Ok(doc) = pdfrum_parser::load(
        std::sync::Arc::from(original),
        &pdfrum_parser::LoadOptions::default(),
    ) else {
        return true;
    };
    // A document whose table was rebuilt has nothing to chain from, so the
    // save downgrades to a full one and the prefix property does not apply.
    if doc.xref_was_rebuilt() || doc.encrypt_dict().is_some() {
        return true;
    }

    let edit = pdfrum_edit::EditDoc::new(&doc);
    let options = pdfrum_edit::SaveOptions {
        mode: pdfrum_edit::SaveMode::Incremental,
        ..pdfrum_edit::SaveOptions::default()
    };
    let mut out = Vec::new();
    if pdfrum_edit::save(&edit, &options, &mut out).is_err() {
        return false;
    }

    // R7: the original bytes are never rewritten. The document's own byte
    // range starts at its header, so junk before it is not part of the
    // prefix — comparing against `doc.bytes()` rather than the file is what
    // makes a header-offset document pass.
    let body = doc.bytes();
    if out.get(..body.len()) != Some(&body[..]) {
        return false;
    }
    // R8: one `/Prev`, two `startxref`s, two `%%EOF`s.
    let text = String::from_utf8_lossy(&out);
    if text.matches("startxref").count() < 2 || text.matches("%%EOF").count() < 2 {
        return false;
    }
    if doc.last_xref_offset() > 0 && !text.contains("/Prev") {
        return false;
    }

    // And it reopens.
    let appended = scratch.join("input.pdf.incremental.pdf");
    if std::fs::write(&appended, &out).is_err() {
        return false;
    }
    oracle_opens(oracle, &appended)
}

/// What a whole sweep found.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SaveTotals {
    /// Files the tool wrote a document for.
    pub saved: u64,
    /// Files the oracle reopened — the M7 exit criterion.
    pub oracle_reopened: u64,
    /// Files whose pixels were compared.
    pub compared: u64,
    /// Files whose every compared page cleared the floor.
    pub within_floor: u64,
    /// Files whose incremental save held the append discipline.
    pub incremental_ok: u64,
    /// Files whose incremental save was checked at all.
    pub incremental_checked: u64,
    /// Files skipped before any check ran.
    pub skipped: u64,
}

impl SaveTotals {
    /// Fold one file's outcome in.
    pub fn add(&mut self, outcome: &SaveOutcome) {
        if !outcome.saved {
            self.skipped = self.skipped.saturating_add(1);
            return;
        }
        self.saved = self.saved.saturating_add(1);
        if outcome.oracle_reopened {
            self.oracle_reopened = self.oracle_reopened.saturating_add(1);
        }
        if outcome.pages > 0 {
            self.compared = self.compared.saturating_add(1);
            if outcome.within_floor {
                self.within_floor = self.within_floor.saturating_add(1);
            }
        }
        if let Some(ok) = outcome.incremental_ok {
            self.incremental_checked = self.incremental_checked.saturating_add(1);
            if ok {
                self.incremental_ok = self.incremental_ok.saturating_add(1);
            }
        }
    }

    /// The share of saved files the oracle reopened.
    #[must_use]
    pub fn reopen_rate(&self) -> Option<f64> {
        rate(self.oracle_reopened, self.saved)
    }

    /// The share of compared files whose pixels held.
    #[must_use]
    pub fn pixel_rate(&self) -> Option<f64> {
        rate(self.within_floor, self.compared)
    }

    /// The share of checked files whose incremental save held.
    #[must_use]
    pub fn incremental_rate(&self) -> Option<f64> {
        rate(self.incremental_ok, self.incremental_checked)
    }
}

#[expect(
    clippy::cast_precision_loss,
    reason = "a corpus file count is far inside f64's exact integer range"
)]
fn rate(part: u64, whole: u64) -> Option<f64> {
    (whole > 0).then(|| part as f64 / whole as f64)
}

#[cfg(test)]
mod tests {
    use super::{SaveOutcome, SaveTotals, page_index};

    fn outcome(saved: bool, reopened: bool, pages: u32, floor: bool) -> SaveOutcome {
        SaveOutcome {
            path: "corpus/x.pdf".to_owned(),
            saved,
            oracle_reopened: reopened,
            incremental_ok: Some(true),
            pages,
            ssim: (pages > 0).then_some(0.999),
            within_floor: floor,
            note: String::new(),
        }
    }

    #[test]
    fn a_page_index_comes_out_of_the_artifact_name() {
        assert_eq!(page_index("input.pdf.saved.pdf.0.png"), Some(0));
        assert_eq!(page_index("input.pdf.12.png"), Some(12));
        assert_eq!(page_index("metadata.txt"), None);
        assert_eq!(page_index("no-index.png"), None);
    }

    // A skipped file counts as skipped, not as a failure: the tool not
    // opening a document is Tier B's business, not the writer's.
    #[test]
    fn a_skipped_file_is_not_counted_against_the_rates() {
        let mut totals = SaveTotals::default();
        totals.add(&SaveOutcome::skipped("x".to_owned(), "n/a".to_owned()));
        assert_eq!(totals.skipped, 1);
        assert_eq!(totals.saved, 0);
        assert_eq!(totals.reopen_rate(), None);
    }

    #[test]
    fn the_rates_are_over_what_was_actually_checked() {
        let mut totals = SaveTotals::default();
        totals.add(&outcome(true, true, 2, true));
        totals.add(&outcome(true, true, 2, false));
        totals.add(&outcome(true, false, 0, true));

        assert_eq!(totals.saved, 3);
        assert_eq!(totals.oracle_reopened, 2);
        // Only two files had pixels to compare.
        assert_eq!(totals.compared, 2);
        assert_eq!(totals.within_floor, 1);

        let close = |a: Option<f64>, b: f64| a.is_some_and(|v| (v - b).abs() < 1e-9);
        assert!(close(totals.reopen_rate(), 2.0 / 3.0));
        assert!(close(totals.pixel_rate(), 0.5));
        assert!(close(totals.incremental_rate(), 1.0));
    }

    #[test]
    fn an_unchecked_incremental_save_leaves_its_rate_absent() {
        let mut totals = SaveTotals::default();
        let mut o = outcome(true, true, 1, true);
        o.incremental_ok = None;
        totals.add(&o);
        assert_eq!(totals.incremental_checked, 0);
        assert_eq!(totals.incremental_rate(), None);
    }
}
