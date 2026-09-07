//! Talking to the C++ oracle (`pdfium_test`) and reading what it says.
//!
//! Two facts about the binary shape everything here:
//!
//! 1. It writes page artifacts **next to the input PDF**, ignoring the working
//!    directory. The oracle checkout is read-only, so every PDF is copied into
//!    a scratch directory before the oracle sees it.
//! 2. Dumps (`--show-metadata`, `--show-pageinfo`, `--show-structure`) go to
//!    stdout while progress chatter goes to stderr, and the three `--show-*`
//!    flags are **mutually exclusive** — the binary rejects any two together.
//!    Each therefore needs its own invocation.
//!
//! [`require_clean_checkout`] enforces the first of those, from the other
//! side: it refuses to run when the checkout it is about to read has tracked
//! modifications, because a template regenerated over a `.pdf` that disagrees
//! with it can swap a fixture for a different document without changing
//! anything a board run would notice.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Result, bail};

/// One `MD5:<path>:<hash>` line from `--md5` stdout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Md5Line {
    /// Artifact path exactly as the oracle echoed it.
    pub path: String,
    /// Lowercase 32-hex-digit digest.
    pub digest: String,
}

impl Md5Line {
    /// The artifact's base name, which is how the golden store keys it.
    pub fn file_name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }
}

/// Extracts the `MD5:` lines from oracle stdout, ignoring everything else.
///
/// The path field may itself contain colons (corpus files do), so the digest
/// is taken from the **last** colon-separated field and the path is whatever
/// precedes it. A line whose tail is not 32 hex digits is not an MD5 line.
pub fn parse_md5_lines(stdout: &str) -> Vec<Md5Line> {
    stdout
        .lines()
        .filter_map(|line| {
            let rest = line.strip_prefix("MD5:")?;
            let (path, digest) = rest.rsplit_once(':')?;
            let digest = digest.trim_end();
            if !is_md5_digest(digest) || path.is_empty() {
                return None;
            }
            Some(Md5Line {
                path: path.to_owned(),
                digest: digest.to_ascii_lowercase(),
            })
        })
        .collect()
}

fn is_md5_digest(text: &str) -> bool {
    text.len() == 32 && text.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The four oracle invocations that make up one file's golden set.
///
/// Split this way because the `--show-*` flags cannot be combined; the render
/// pass additionally carries `--md5` so its digests land on stdout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pass {
    /// `--png --md5` — page PNGs plus their digests.
    Render,
    /// `--txt` — page text as UTF-32LE.
    Text,
    /// `--annot` — per-page annotation dumps.
    Annot,
    /// `--show-metadata` — document info dictionary, on stdout.
    Metadata,
    /// `--show-pageinfo` — per-page box geometry, on stdout.
    PageInfo,
    /// `--show-structure` — tagged-PDF structure tree, on stdout.
    Structure,
}

impl Pass {
    /// Every pass, in the order goldens are generated.
    pub const ALL: [Pass; 6] = [
        Pass::Render,
        Pass::Text,
        Pass::Annot,
        Pass::Metadata,
        Pass::PageInfo,
        Pass::Structure,
    ];

    /// The oracle flags this pass adds to the determinism recipe.
    pub fn flags(self) -> &'static [&'static str] {
        match self {
            Pass::Render => &["--png", "--md5"],
            Pass::Text => &["--txt"],
            Pass::Annot => &["--annot"],
            Pass::Metadata => &["--show-metadata"],
            Pass::PageInfo => &["--show-pageinfo"],
            Pass::Structure => &["--show-structure"],
        }
    }

    /// Whether this pass produced the named artifact.
    ///
    /// The mirror of what the golden generator harvested, and the way a
    /// comparison decides which goldens a failed oracle run has poisoned.
    #[must_use]
    pub fn owns_artifact(self, name: &str) -> bool {
        let ends = |suffix: &str| name.len() > suffix.len() && name.ends_with(suffix);
        match self {
            Pass::Render => ends(".png"),
            // `.annot.txt` also ends in `.txt`, so exclude it explicitly.
            Pass::Text => ends(".txt") && !ends(".annot.txt"),
            Pass::Annot => ends(".annot.txt"),
            Pass::Metadata => name == "metadata.txt",
            Pass::PageInfo => name == "pageinfo.txt",
            Pass::Structure => name == "structure.txt",
        }
    }

    /// The pass name recorded in a manifest's `oracle_failures`.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Pass::Render => "Render",
            Pass::Text => "Text",
            Pass::Annot => "Annot",
            Pass::Metadata => "Metadata",
            Pass::PageInfo => "PageInfo",
            Pass::Structure => "Structure",
        }
    }

    /// For stdout-dump passes, the artifact name the dump is stored under.
    ///
    /// The file-writing passes return `None` — their artifacts arrive on disk
    /// beside the input instead.
    pub fn stdout_artifact(self) -> Option<&'static str> {
        match self {
            Pass::Metadata => Some("metadata.txt"),
            Pass::PageInfo => Some("pageinfo.txt"),
            Pass::Structure => Some("structure.txt"),
            Pass::Render | Pass::Text | Pass::Annot => None,
        }
    }
}

/// The fixed determinism arguments frozen clock, hermetic
/// fonts, Croscore names. Every invocation carries these.
pub fn determinism_args(font_dir: &Path) -> Vec<String> {
    vec![
        "--time=1399672130".to_owned(),
        "--croscore-font-names".to_owned(),
        format!("--font-dir={}", font_dir.display()),
    ]
}

/// Where the oracle binary and its hermetic font directory live.
#[derive(Debug, Clone)]
pub struct OraclePaths {
    pub binary: PathBuf,
    pub font_dir: PathBuf,
}

/// Whether the read-only oracle checkout may be used despite tracked
/// modifications.
///
/// `--allow-dirty-oracle`, or `PDFRUM_ALLOW_DIRTY_ORACLE=1` in the
/// environment, for someone who knows why their tree differs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirtyPolicy {
    /// Refuse to run against a modified checkout (the default).
    Refuse,
    /// Run anyway, having been told to.
    Allow,
}

/// What `git status --porcelain --untracked-files=no` said about the checkout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckoutState {
    /// No tracked file differs from HEAD.
    Clean,
    /// Tracked files differ, in the order git listed them.
    Modified(Vec<String>),
    /// Not a git repository — a tarball export, say. Nothing to compare.
    NotAGitRepo,
}

/// How many drifted paths the refusal names before it stops.
const NAMED_PATHS: usize = 5;

/// Parses `git status --porcelain --untracked-files=no` into a state.
///
/// The porcelain v1 format is a two-character status field, a space, and the
/// path. Only tracked entries can appear at all once untracked files are
/// suppressed, so every non-empty line is drift — but `??` is still matched
/// explicitly, because a caller that forgets the flag would otherwise be told
/// its legitimately untracked `.pdf`s are corruption.
///
/// A renamed entry (`R  old -> new`) is reported under its new name, which is
/// the path on disk that a `git checkout --` would restore.
pub fn parse_porcelain(stdout: &str) -> CheckoutState {
    let mut modified = Vec::new();
    for line in stdout.lines() {
        if line.len() < 4 || line.starts_with("??") {
            continue;
        }
        let path = &line[3..];
        let path = path.rsplit_once(" -> ").map_or(path, |(_, new)| new);
        modified.push(path.trim_matches('"').to_owned());
    }
    if modified.is_empty() {
        CheckoutState::Clean
    } else {
        CheckoutState::Modified(modified)
    }
}

/// Asks git what the checkout looks like.
///
/// Any failure to run git at all, and any non-zero exit (which is what a
/// non-repository directory produces), is [`CheckoutState::NotAGitRepo`]: the
/// harness's job is to run the board, not to diagnose a git installation, and
/// a checkout unpacked from a tarball is a legitimate way to have one.
fn inspect(checkout: &Path) -> CheckoutState {
    let Ok(output) = Command::new("git")
        .args(["-C"])
        .arg(checkout)
        .args(["status", "--porcelain", "--untracked-files=no"])
        .output()
    else {
        return CheckoutState::NotAGitRepo;
    };
    if !output.status.success() {
        return CheckoutState::NotAGitRepo;
    }
    parse_porcelain(&String::from_utf8_lossy(&output.stdout))
}

/// Refuses to touch a modified oracle checkout.
///
/// Every subcommand that reads the checkout calls this first, because of one
/// class of drift: **a tracked `.pdf` regenerated from a template that
/// disagrees with it.** PDFium's `.in` templates and their committed `.pdf`s
/// are not always in sync, so re-running `fixup_pdf_template.py` over the
/// checkout can silently swap a fixture for a different document. The
/// 2026-09-03 case was `testing/resources/viewer_ref.pdf`: the template says
/// `/Count 1` and the committed file has five pages, so the regeneration
/// replaced a five-page fixture with a one-page one, and every row scored
/// against it was scoring a document the corpus does not contain.
///
/// That class is invisible on its own — a swapped fixture still opens and
/// still renders. What makes it findable is the churn beside it: the same
/// regeneration rewrote 333 other `.pdf`s with every stream `/Length` one
/// byte lower than committed (render-neutral, and the golden store's
/// content-hash keying already absorbs them), and six expected-output files
/// (`*.pdf.0.annot.txt`, `*.0.png`) were overwritten by `pdfium_test` runs
/// pointed at the checkout instead of at a scratch copy (read by nothing of
/// ours). The harmless drift is the symptom; refusing on *any* tracked
/// modification is how the one harmful file is caught with it.
///
/// **Untracked `.pdf`s are not drift.** Around 207 of them sit in
/// `testing/resources`, expanded from `.in` templates, and they are board
/// inputs: the check passes `--untracked-files=no` so that nothing here can
/// ever be read as an argument for cleaning them away.
pub fn require_clean_checkout(checkout: &Path, policy: DirtyPolicy) -> Result<()> {
    let state = inspect(checkout);
    match state {
        CheckoutState::Clean => Ok(()),
        CheckoutState::NotAGitRepo => {
            eprintln!(
                "note: {} is not a git checkout - skipping the read-only hygiene check.",
                checkout.display()
            );
            Ok(())
        }
        CheckoutState::Modified(paths) => {
            let message = dirty_message(checkout, &paths);
            if policy == DirtyPolicy::Allow {
                eprintln!("{message}");
                eprintln!(
                    "note: the dirty-oracle override is set; continuing against the modified tree."
                );
                return Ok(());
            }
            bail!(message)
        }
    }
}

/// The refusal, as one block of text so the test can read it.
fn dirty_message(checkout: &Path, paths: &[String]) -> String {
    use std::fmt::Write as _;

    let mut message = format!(
        "the oracle checkout at {} has drifted: {} tracked file{} modified.\n",
        checkout.display(),
        paths.len(),
        if paths.len() == 1 { " is" } else { "s are" }
    );
    for path in paths.iter().take(NAMED_PATHS) {
        let _ = writeln!(message, "    {path}");
    }
    if paths.len() > NAMED_PATHS {
        let _ = writeln!(message, "    ... and {} more", paths.len() - NAMED_PATHS);
    }
    message.push_str("The checkout is read-only; restore it with:\n");
    let _ = writeln!(
        message,
        "    git -C {} checkout -- testing/resources",
        checkout.display()
    );
    message.push_str(
        "Do NOT clean untracked files (.in-expanded .pdf's are inputs). \
         To proceed anyway: --allow-dirty-oracle or PDFRUM_ALLOW_DIRTY_ORACLE=1.",
    );
    message
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clean_checkout_prints_nothing() {
        assert_eq!(parse_porcelain(""), CheckoutState::Clean);
        assert_eq!(parse_porcelain("\n"), CheckoutState::Clean);
    }

    #[test]
    fn tracked_modifications_are_drift() {
        // Verbatim shape from the 2026-09-03 checkout.
        let stdout = concat!(
            " M testing/resources/annotation_highlight_alpha.pdf\n",
            " M testing/resources/bug_1258634.pdf\n",
            "MM testing/resources/annots.pdf.0.annot.txt\n",
        );
        let CheckoutState::Modified(paths) = parse_porcelain(stdout) else {
            panic!("expected drift");
        };
        assert_eq!(
            paths,
            vec![
                "testing/resources/annotation_highlight_alpha.pdf".to_owned(),
                "testing/resources/bug_1258634.pdf".to_owned(),
                "testing/resources/annots.pdf.0.annot.txt".to_owned(),
            ]
        );
    }

    #[test]
    fn untracked_pdfs_expanded_from_templates_are_not_drift() {
        // The ~207 .in-derived .pdf's are board inputs. `--untracked-files=no`
        // should keep them off this listing entirely; if a caller forgets the
        // flag, the parser must still not call them corruption.
        let stdout = concat!(
            "?? testing/resources/bug_1258634.pdf\n",
            "?? testing/resources/annots.pdf\n",
        );
        assert_eq!(parse_porcelain(stdout), CheckoutState::Clean);
    }

    #[test]
    fn a_deletion_or_a_staged_edit_is_still_drift() {
        let stdout = concat!(
            " D testing/resources/gone.pdf\n",
            "M  testing/resources/staged.pdf\n",
            "A  testing/resources/added.pdf\n",
        );
        let CheckoutState::Modified(paths) = parse_porcelain(stdout) else {
            panic!("expected drift");
        };
        assert_eq!(paths.len(), 3);
    }

    #[test]
    fn a_rename_is_reported_under_its_new_name() {
        // `git checkout --` restores the path on disk, which is the new one.
        let stdout = "R  testing/resources/old.pdf -> testing/resources/new.pdf\n";
        let CheckoutState::Modified(paths) = parse_porcelain(stdout) else {
            panic!("expected drift");
        };
        assert_eq!(paths, vec!["testing/resources/new.pdf".to_owned()]);
    }

    #[test]
    fn the_refusal_names_a_few_paths_and_says_how_to_restore() {
        let paths: Vec<String> = (0..340)
            .map(|i| format!("testing/resources/f{i}.pdf"))
            .collect();
        let message = dirty_message(Path::new("/checkout"), &paths);
        assert!(message.contains("340 tracked files are modified"));
        assert!(message.contains("testing/resources/f0.pdf"));
        assert!(message.contains("testing/resources/f4.pdf"));
        // Bounded: it names five and counts the rest.
        assert!(!message.contains("testing/resources/f5.pdf"));
        assert!(message.contains("... and 335 more"));
        assert!(message.contains("git -C /checkout checkout -- testing/resources"));
        assert!(message.contains("Do NOT clean untracked files"));
        assert!(message.contains("--allow-dirty-oracle"));
        // The reason the check exists is named, not just the symptom.
        assert!(message.contains("read-only"));
    }

    #[test]
    fn one_drifted_file_reads_as_one() {
        let message = dirty_message(Path::new("/checkout"), &["a.pdf".to_owned()]);
        assert!(message.contains("1 tracked file is modified"));
        assert!(!message.contains("and 0 more"));
    }

    #[test]
    fn a_directory_that_is_not_a_repository_is_skipped_rather_than_refused() {
        // A tarball export has no `.git`; the board should run, with a note.
        let state = inspect(Path::new("/"));
        assert_eq!(state, CheckoutState::NotAGitRepo);
        assert!(require_clean_checkout(Path::new("/"), DirtyPolicy::Refuse).is_ok());
    }

    #[test]
    fn parses_the_lines_the_oracle_actually_prints() {
        // Verbatim from `pdfium_test --png --md5 annots.pdf`.
        let stdout = "MD5:annots.pdf.0.png:936f6bb7c6f4030f4b05741f2d5fd19e\n\
                      MD5:annots.pdf.1.png:95fb55c976e9339c800983a128a7a148\n";
        let lines = parse_md5_lines(stdout);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].path, "annots.pdf.0.png");
        assert_eq!(lines[0].digest, "936f6bb7c6f4030f4b05741f2d5fd19e");
        assert_eq!(lines[1].file_name(), "annots.pdf.1.png");
    }

    #[test]
    fn ignores_progress_chatter_and_blank_lines() {
        let stdout = "Processing PDF file annots.pdf.\n\
                      \n\
                      MD5:a.pdf.0.png:0123456789abcdef0123456789abcdef\n\
                      Processed 2 pages.\n";
        assert_eq!(parse_md5_lines(stdout).len(), 1);
    }

    #[test]
    fn keeps_colons_inside_the_path() {
        let stdout = "MD5:/tmp/od:d/a.pdf.0.png:0123456789abcdef0123456789abcdef\n";
        let lines = parse_md5_lines(stdout);
        assert_eq!(lines[0].path, "/tmp/od:d/a.pdf.0.png");
        assert_eq!(lines[0].file_name(), "a.pdf.0.png");
    }

    #[test]
    fn strips_a_leading_directory_for_the_artifact_key() {
        let stdout = "MD5:sub/annots.pdf.0.png:936f6bb7c6f4030f4b05741f2d5fd19e\n";
        assert_eq!(parse_md5_lines(stdout)[0].file_name(), "annots.pdf.0.png");
    }

    #[test]
    fn uppercase_digests_are_normalized() {
        let stdout = "MD5:a.png:0123456789ABCDEF0123456789ABCDEF\n";
        assert_eq!(
            parse_md5_lines(stdout)[0].digest,
            "0123456789abcdef0123456789abcdef"
        );
    }

    #[test]
    fn rejects_malformed_digests() {
        // Too short, too long, non-hex, no colon, empty path.
        let stdout = "MD5:a.png:0123\n\
                      MD5:b.png:0123456789abcdef0123456789abcdefff\n\
                      MD5:c.png:zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz\n\
                      MD5:no-colon-digest\n\
                      MD5::0123456789abcdef0123456789abcdef\n";
        assert!(parse_md5_lines(stdout).is_empty());
    }

    #[test]
    fn tolerates_carriage_returns() {
        let stdout = "MD5:a.png:0123456789abcdef0123456789abcdef\r\n";
        assert_eq!(parse_md5_lines(stdout).len(), 1);
    }

    #[test]
    fn empty_stdout_yields_nothing() {
        assert!(parse_md5_lines("").is_empty());
    }

    #[test]
    fn the_show_flags_never_share_an_invocation() {
        // The binary rejects two --show-* flags together, so each pass must
        // contribute exactly one flag.
        for pass in [Pass::Metadata, Pass::PageInfo, Pass::Structure] {
            assert_eq!(pass.flags().len(), 1);
            assert!(pass.stdout_artifact().is_some());
        }
    }

    #[test]
    fn determinism_args_match_the_plan_recipe() {
        let args = determinism_args(Path::new("/fonts"));
        assert_eq!(
            args,
            vec![
                "--time=1399672130".to_owned(),
                "--croscore-font-names".to_owned(),
                "--font-dir=/fonts".to_owned(),
            ]
        );
    }
}
