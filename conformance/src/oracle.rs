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

use std::path::{Path, PathBuf};

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

/// The fixed determinism arguments from PLAN.md §4: frozen clock, hermetic
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

#[cfg(test)]
mod tests {
    use super::*;

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
