//! The exit check: does what pdfrum *writes* still open, and still look
//! the same?
//!
//! # Why this needs two binaries
//!
//! `pdfium_test` has no save flag — the oracle cannot write a document at
//! all, so there is no like-for-like invocation to diff. The check is therefore a sequence rather than a comparison:
//!
//! 1. **pdfrum saves** every corpus file (`pdfrum-tool --save`).
//! 2. **The oracle reopens** what pdfrum wrote and renders it. A load failure
//!    here means another implementation could not read our output.
//! 3. **pdfrum re-renders** the saved file and the result is compared against
//!    the *original's* golden render at the Tier-B floor. This is what catches
//!    a save that opens but has lost something.
//! 4. **An incremental save** is checked for the append discipline: the
//!    original bytes must be a prefix of the output, the `/Prev` chain must be
//!    intact, and the oracle must reopen it.
//!
//! # Two pixel numbers, because they answer different questions
//!
//! Step 3 diffs our render of the *saved* file against the golden render of
//! the *original*. That is the absolute number, and on a file our renderer
//! already gets slightly wrong it reports the renderer's gap rather than the
//! writer's — the same SSIM the ordinary Tier-B sweep reports, to the digit.
//!
//! So the sweep reports a second number beside it: whether the saved file's
//! SSIM **matches the original's**. That one isolates what is actually
//! about. A file whose original renders at 0.9856 and whose saved copy also
//! renders at 0.9856 has lost nothing in the save, and saying so is more
//! honest than counting it as a save failure.
//!
//! A page whose objects were regenerated is a different matter — the C++'s
//! own generator is lossy (no patterns, no non-RGB colour) — but an ordinary
//! save regenerates nothing, so every file in this sweep is expected to hold
//! its original's fidelity exactly.
//!
//! # The encrypted files carry a password through every step
//!
//! Since an encrypted document saves *encrypted*, so its whole sweep runs
//! under a password: pdfrum opens it with `--password=`, saves it, and the
//! oracle is handed the same password to reopen it with: a file another
//! implementation can open with the password it was given, and which renders
//! what it always did.
//!
//! It also asks the question the plaintext sweep cannot: the harness first
//! checks that the oracle **fails** to open the saved file with no password.
//! A writer that emitted plaintext under an `/Encrypt` declaration, or that
//! quietly removed the security, would sail through every other check here.
//!
//! The passwords are not discoverable from the files, so they live in
//! [`PASSWORDS`] — recovered from the oracle's own embedder tests and recorded
//! in §4.2.

use std::path::Path;
use std::process::Command;

use crate::corpus::Entry;
use crate::goldens::Store;
use crate::oracle::{OraclePaths, Pass, determinism_args};
use crate::run::ToolPaths;
use crate::ssim;
use crate::thresholds::Thresholds;

/// How far below the original a saved file's SSIM may land before the
/// difference is the writer's rather than the rasterizer's.
const SSIM_EPSILON: f64 = 1e-6;

/// The corpus's encrypted fixtures and a password that opens each.
///
/// Matched on the entry id's **file name**, so the same table serves a file
/// found under `resources/` and one found under `corpus/`. Every password is
/// one the oracle's `cpdf_security_handler_embeddertest.cpp` uses, spelled as
/// the bytes `--password=` will carry: `hôtel` and `âge` are Latin-1 there,
/// and the tools accept either spelling, so the UTF-8 form is passed because
/// that is what survives a command line.
///
/// A file not in this table is swept as a plaintext document; if it turns out
/// to be encrypted, the tool will fail to open it and the sweep records a
/// skip rather than a failure, exactly as it did before .
///
/// The two `_bad_okey` fixtures are deliberately absent. Their `/O` entry is
/// truncated, so neither the oracle nor this reader opens them *at all* —
/// which is what those files exist to pin. Listing a password for them would
/// only turn a correct refusal into a skip with a misleading reason.
pub const PASSWORDS: &[(&str, &str)] = &[
    ("encrypted_hello_world_r2.pdf", "hôtel"),
    ("encrypted_hello_world_r3.pdf", "hôtel"),
    ("encrypted_hello_world_r5.pdf", "hôtel"),
    ("encrypted_hello_world_r6.pdf", "hôtel"),
    ("encrypted.pdf", "1234"),
    ("bug_644.pdf", "a"),
];

/// The password for one corpus entry, if it has one.
///
/// The id is a slashed corpus path, so the lookup is on its last segment.
#[must_use]
pub fn password_for(id: &str) -> Option<&'static str> {
    let name = id.rsplit('/').next().unwrap_or(id);
    PASSWORDS
        .iter()
        .find(|(fixture, _)| *fixture == name)
        .map(|(_, password)| *password)
}

/// The `--password=` argument for one entry, as a zero- or one-element list
/// so it can be spliced into a command line unconditionally.
///
/// An empty `--password=` is not the same as no flag: the oracle reads the
/// former as "no password given", so passing it would be harmless, but
/// omitting it keeps the command lines of unencrypted files byte-identical to
/// what they were before — which is what makes the scoreboard comparable.
fn password_args(password: Option<&str>) -> Vec<String> {
    password
        .map(|p| format!("--password={p}"))
        .into_iter()
        .collect()
}

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
    /// Whether the **oracle** reopened and rendered what pdfrum wrote.
    pub oracle_reopened: bool,
    /// Whether the incremental save's output kept the original as its prefix
    /// and chained its cross-reference, and the oracle reopened it too.
    pub incremental_ok: Option<bool>,
    /// Whether this file was swept under a password, i.e. it is one of the
    /// encrypted fixtures.
    pub encrypted: bool,
    /// For an encrypted file: whether the saved copy is **still** encrypted,
    /// which the harness establishes by the oracle *refusing* to open it
    /// without a password. `None` for a plaintext file.
    pub still_encrypted: Option<bool>,
    /// Pages compared against the original's golden render.
    pub pages: u32,
    /// The worst page's SSIM against the original's golden, when any page
    /// was compared.
    pub ssim: Option<f64>,
    /// The same measurement over the **original** file, so the two can be
    /// compared. A saved file that matches its original has lost nothing,
    /// whatever the absolute number says about the renderer.
    pub original_ssim: Option<f64>,
    /// Whether every compared page cleared the floor.
    pub within_floor: bool,
    /// Whether the saved file renders no worse than the original did. This
    /// is what is about: the writer preserving what the reader saw.
    pub matches_original: bool,
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
            encrypted: false,
            still_encrypted: None,
            pages: 0,
            ssim: None,
            original_ssim: None,
            within_floor: true,
            matches_original: true,
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
    let password = password_for(&id);
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
        .args(password_args(password))
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
        encrypted: password.is_some(),
        still_encrypted: None,
        pages: 0,
        ssim: None,
        original_ssim: None,
        within_floor: true,
        matches_original: true,
        note: String::new(),
    };

    // ---- step 2: the oracle reopens it, under the same password ----
    outcome.oracle_reopened = oracle_opens(oracle, &saved, password);
    if !outcome.oracle_reopened {
        outcome.note = match password {
            Some(_) => "the oracle could not reopen the saved file with its password".to_owned(),
            None => "the oracle could not reopen the saved file".to_owned(),
        };
    }

    // ---- step 2b: and refuses it without one ----
    //
    // Only meaningful for a file that had a password to begin with. A save
    // that dropped the security, or wrote plaintext under an `/Encrypt`
    // declaration, opens here — and that is the one failure every other check
    // in this sweep would miss.
    if password.is_some() {
        let opens_unprotected = oracle_opens(oracle, &saved, None);
        outcome.still_encrypted = Some(!opens_unprotected);
        if opens_unprotected && outcome.note.is_empty() {
            "the saved file opened without a password".clone_into(&mut outcome.note);
        }
    }

    // ---- step 3: our own re-render against the original's golden ----
    if compare_pixels {
        compare_render(
            &mut outcome,
            tool,
            store,
            thresholds,
            Subject {
                original: &bytes,
                saved: &saved,
                input: &input,
                password,
            },
        );
    }

    // ---- step 4: the incremental save's append discipline ----
    outcome.incremental_ok = Some(check_incremental(oracle, scratch, &bytes, password));
    if outcome.incremental_ok == Some(false) && outcome.note.is_empty() {
        "the incremental save broke the append discipline".clone_into(&mut outcome.note);
    }

    outcome
}

/// Whether the oracle loads and renders a file without reporting a failure.
///
/// "Processed N pages." on stderr is the oracle's own success line; the
/// absence of "Load pdf docs unsuccessful" is what says the document opened.
fn oracle_opens(oracle: &OraclePaths, file: &Path, password: Option<&str>) -> bool {
    let Ok(out) = Command::new(&oracle.binary)
        .args(determinism_args(&oracle.font_dir))
        .args(password_args(password))
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

/// The three files one comparison reads, and how to open them.
///
/// A record rather than three more arguments: `original` is the input's bytes
/// (the golden's key), `input` and `saved` are the same document on disk
/// before and after the save, and `password` opens both. Naming them together
/// is what keeps the two `Path`s from being passed the wrong way round.
#[derive(Debug, Clone, Copy)]
struct Subject<'a> {
    /// The input document's bytes, which key its golden.
    original: &'a [u8],
    /// The saved copy, on disk.
    saved: &'a Path,
    /// The input, on disk.
    input: &'a Path,
    /// The password both need, when the document is encrypted.
    password: Option<&'a str>,
}

/// Render both the saved file and the original, and diff each against the
/// original's golden.
///
/// Two numbers come out. The absolute one is the saved file's SSIM against
/// the golden — comparable with the ordinary Tier-B sweep. The relative one
/// is whether the saved file did *as well as the original did*, which is
/// what says the save itself lost nothing.
fn compare_render(
    outcome: &mut SaveOutcome,
    tool: &ToolPaths,
    store: &Store,
    thresholds: &Thresholds,
    subject: Subject<'_>,
) {
    let Subject {
        original,
        saved,
        input,
        password,
    } = subject;
    let key = crate::goldens::key_for(original);
    let Ok(manifest) = store.manifest(&key) else {
        // No golden for the original means nothing to compare against; the
        // oracle-reopen half of the check still stands.
        return;
    };

    let Some(rendered) = render_with_tool(tool, saved, password) else {
        "the tool would not render the saved file".clone_into(&mut outcome.note);
        outcome.within_floor = false;
        outcome.matches_original = false;
        return;
    };
    // The same measurement over the input, so the two are comparable.
    let baseline = render_with_tool(tool, input, password).unwrap_or_default();

    let floor = thresholds.ssim_for(&outcome.path);
    let mut worst: Option<f64> = None;
    let mut worst_original: Option<f64> = None;

    for (name, bytes) in &baseline {
        let Some(index) = page_index(name) else {
            continue;
        };
        let golden_name = format!("input.pdf.{index}.png");
        let (Ok(golden), Ok(candidate)) = (
            store
                .artifact(&key, &golden_name)
                .map_err(drop)
                .and_then(|b| crate::pixels::decode(&b).map_err(drop)),
            crate::pixels::decode(bytes).map_err(drop),
        ) else {
            continue;
        };
        if let Ok(diff) = ssim::compare(&golden, &candidate) {
            worst_original = Some(worst_original.map_or(diff.ssim, |w: f64| w.min(diff.ssim)));
        }
    }
    outcome.original_ssim = worst_original;

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

    // The relative reading: did the save cost anything? Nothing rendered, or
    // no baseline to compare against, is not a loss by any reading.
    outcome.matches_original = match (worst, worst_original) {
        // `SSIM_EPSILON` absorbs the last bits of a rasterizer's own
        // nondeterminism, which are not the writer's doing.
        (Some(saved), Some(original)) => saved + SSIM_EPSILON >= original,
        (None, _) | (Some(_), None) => true,
    };

    if let Some(worst) = worst
        && worst < floor
    {
        outcome.within_floor = false;
        if outcome.note.is_empty() {
            outcome.note = match worst_original {
                Some(original) if outcome.matches_original => format!(
                    "ssim {worst:.6} below floor {floor:.6} \
                     (the original renders at {original:.6}; the save lost nothing)"
                ),
                _ => format!("ssim {worst:.6} below floor {floor:.6}"),
            };
        }
    }
}

/// Render one file with our own tool, returning its PNG artifacts.
fn render_with_tool(
    tool: &ToolPaths,
    file: &Path,
    password: Option<&str>,
) -> Option<Vec<(String, Vec<u8>)>> {
    let out = Command::new(&tool.binary)
        .args(determinism_args(&tool.font_dir))
        .args(password_args(password))
        .args(Pass::Render.flags())
        .arg(file)
        .output()
        .ok()?;
    // A crash produced nothing worth harvesting.
    out.status.code()?;
    crate::generate::harvest_for_run(file, Pass::Render).ok()
}

/// The page index out of a `<name>.<index>.png` artifact.
fn page_index(name: &str) -> Option<u32> {
    let stem = name.strip_suffix(".png")?;
    let (_, index) = stem.rsplit_once('.')?;
    index.parse().ok()
}

/// The append discipline: the original bytes are a prefix of the output, the
/// cross-reference chains, and the oracle still opens it.
fn check_incremental(
    oracle: &OraclePaths,
    scratch: &Path,
    original: &[u8],
    password: Option<&str>,
) -> bool {
    // The tool always saves fully; the incremental path is exercised through
    // the library, which is the same code the tool calls.
    let options = pdfrum_parser::LoadOptions {
        password: password.map(|p| p.as_bytes().to_vec()),
        ..pdfrum_parser::LoadOptions::default()
    };
    let Ok(doc) = pdfrum_parser::load(std::sync::Arc::from(original), &options) else {
        return true;
    };
    // A document whose table was rebuilt has nothing to chain from, so the
    // save downgrades to a full one and the prefix property does not apply.
    //
    // An encrypted document is no longer excused: since its appended
    // objects are enciphered under the key the original bytes already use, so
    // the append discipline applies to it exactly as it does to any other
    // file.
    if doc.xref_was_rebuilt() {
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
    // R8: the appended section adds its own `startxref`, its own `%%EOF` and
    // a `/Prev` naming the original's table.
    //
    // The counts are *relative to what the original held*, not absolute. A
    // real file can end in a malformed `%EOF` — `bug_440028542.pdf` does —
    // and requiring two `%%EOF`s in the output would then fail a save that
    // did everything right, because the prefix contributed none.
    let text = String::from_utf8_lossy(&out);
    let before = String::from_utf8_lossy(&body[..]);
    if text.matches("startxref").count() <= before.matches("startxref").count() {
        return false;
    }
    if text.matches("%%EOF").count() <= before.matches("%%EOF").count() {
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
    oracle_opens(oracle, &appended, password)
}

/// What a whole sweep found.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SaveTotals {
    /// Files the tool wrote a document for.
    pub saved: u64,
    /// Files the oracle reopened.
    pub oracle_reopened: u64,
    /// Files whose pixels were compared.
    pub compared: u64,
    /// Files whose every compared page cleared the floor.
    pub within_floor: u64,
    /// Files whose saved copy rendered no worse than the original did — the
    /// number that isolates what the *save* cost.
    pub matches_original: u64,
    /// Files whose incremental save held the append discipline.
    pub incremental_ok: u64,
    /// Files whose incremental save was checked at all.
    pub incremental_checked: u64,
    /// Encrypted files swept under a password.
    pub encrypted: u64,
    /// Encrypted files whose saved copy the oracle refused to open without a
    /// password.
    pub still_encrypted: u64,
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
            if outcome.matches_original {
                self.matches_original = self.matches_original.saturating_add(1);
            }
        }
        if outcome.encrypted {
            self.encrypted = self.encrypted.saturating_add(1);
            if outcome.still_encrypted == Some(true) {
                self.still_encrypted = self.still_encrypted.saturating_add(1);
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

    /// The share of compared files whose pixels cleared the Tier-B floor
    /// outright — comparable with the ordinary sweep's number.
    #[must_use]
    pub fn pixel_rate(&self) -> Option<f64> {
        rate(self.within_floor, self.compared)
    }

    /// The share of compared files whose saved copy rendered no worse than
    /// the original. This is the number is graded on: it separates what
    /// the writer cost from what the renderer already owed.
    #[must_use]
    pub fn fidelity_rate(&self) -> Option<f64> {
        rate(self.matches_original, self.compared)
    }

    /// The share of checked files whose incremental save held.
    #[must_use]
    pub fn incremental_rate(&self) -> Option<f64> {
        rate(self.incremental_ok, self.incremental_checked)
    }

    /// The share of encrypted files whose saved copy is still encrypted.
    ///
    /// Pair with [`Self::reopen_rate`]: the oracle opens the file *with* the
    /// password and refuses it *without*.
    #[must_use]
    pub fn still_encrypted_rate(&self) -> Option<f64> {
        rate(self.still_encrypted, self.encrypted)
    }
}

/// `part` as a fraction of `whole`, or `None` when there is no whole.
///
/// Shared with the mutation sweep, which counts the same kind of thing.
#[expect(
    clippy::cast_precision_loss,
    reason = "a corpus file count is far inside f64's exact integer range"
)]
pub fn rate(part: u64, whole: u64) -> Option<f64> {
    (whole > 0).then(|| part as f64 / whole as f64)
}

#[cfg(test)]
mod tests {
    use super::{SaveOutcome, SaveTotals, page_index, password_args, password_for};

    fn outcome(saved: bool, reopened: bool, pages: u32, floor: bool) -> SaveOutcome {
        SaveOutcome {
            path: "corpus/x.pdf".to_owned(),
            saved,
            oracle_reopened: reopened,
            incremental_ok: Some(true),
            encrypted: false,
            still_encrypted: None,
            pages,
            ssim: (pages > 0).then_some(0.999),
            original_ssim: (pages > 0).then_some(0.999),
            within_floor: floor,
            matches_original: true,
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

    // The two pixel numbers answer different questions, and this is the case
    // that separates them: a file our renderer already gets slightly wrong
    // renders *identically* after a save. The absolute number counts it as a
    // failure (it is below the floor); the fidelity number does not (the
    // save cost nothing).
    #[test]
    fn a_file_below_the_floor_can_still_have_lost_nothing() {
        let mut totals = SaveTotals::default();
        let mut o = outcome(true, true, 1, false);
        o.ssim = Some(0.985_592);
        o.original_ssim = Some(0.985_592);
        o.matches_original = true;
        totals.add(&o);

        let close = |a: Option<f64>, b: f64| a.is_some_and(|v| (v - b).abs() < 1e-9);
        assert!(close(totals.pixel_rate(), 0.0), "below the floor");
        assert!(close(totals.fidelity_rate(), 1.0), "and lost nothing");
    }

    #[test]
    fn a_save_that_really_lost_something_fails_both() {
        let mut totals = SaveTotals::default();
        let mut o = outcome(true, true, 1, false);
        o.ssim = Some(0.5);
        o.original_ssim = Some(0.99);
        o.matches_original = false;
        totals.add(&o);

        assert_eq!(totals.pixel_rate(), Some(0.0));
        assert_eq!(totals.fidelity_rate(), Some(0.0));
    }

    // ---- : the encrypted half of the sweep ----

    #[test]
    fn a_password_is_found_by_file_name_wherever_the_file_sits() {
        assert_eq!(password_for("resources/encrypted.pdf"), Some("1234"));
        assert_eq!(password_for("corpus/fx/encrypted.pdf"), Some("1234"));
        assert_eq!(password_for("encrypted.pdf"), Some("1234"));
        assert_eq!(
            password_for("resources/encrypted_hello_world_r6.pdf"),
            Some("hôtel")
        );
        assert_eq!(password_for("resources/hello.pdf"), None);
        // A name that merely contains a fixture's is not that fixture.
        assert_eq!(password_for("resources/not_encrypted.pdf"), None);
    }

    // An unencrypted file's command line is unchanged from before , which
    // is what keeps its scoreboard row comparable.
    #[test]
    fn only_a_password_adds_an_argument() {
        assert!(password_args(None).is_empty());
        assert_eq!(password_args(Some("1234")), vec!["--password=1234"]);
    }

    #[test]
    fn the_encrypted_rate_is_over_the_encrypted_files_alone() {
        let mut totals = SaveTotals::default();
        // Two encrypted files, one of which saved decrypted.
        let mut good = outcome(true, true, 1, true);
        good.encrypted = true;
        good.still_encrypted = Some(true);
        let mut bad = outcome(true, true, 1, true);
        bad.encrypted = true;
        bad.still_encrypted = Some(false);
        // And a plaintext one, which contributes to neither.
        totals.add(&good);
        totals.add(&bad);
        totals.add(&outcome(true, true, 1, true));

        assert_eq!(totals.encrypted, 2);
        assert_eq!(totals.still_encrypted, 1);
        assert_eq!(totals.still_encrypted_rate(), Some(0.5));
        assert_eq!(totals.saved, 3);
    }

    // With no encrypted file in the sweep the rate is absent rather than
    // zero, so a limited run does not report a failure it never checked.
    #[test]
    fn a_sweep_with_no_encrypted_files_reports_no_rate() {
        let mut totals = SaveTotals::default();
        totals.add(&outcome(true, true, 1, true));
        assert_eq!(totals.still_encrypted_rate(), None);
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
