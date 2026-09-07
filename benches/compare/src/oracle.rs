//! `pdfium_test` on a scratch copy, once per file, with the conformance
//! harness's determinism recipe (frozen clock, hermetic fonts, Croscore
//! names). The outputs are cached by content hash so a rerun over the same
//! corpus pays for the oracle once.
//!
//! `--png` and `--txt` never share an invocation (`conformance/src/oracle.rs`
//! records why), and the oracle writes beside its input — which is why the
//! input is a copy and never the checkout's file.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};

/// Where the oracle lives and how it is invoked.
#[derive(Debug, Clone)]
pub struct Oracle {
    pub binary: PathBuf,
    pub font_dir: PathBuf,
    pub cache_root: PathBuf,
    pub dpi: f64,
}

/// What the oracle produced for one file's first page.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OracleOutput {
    /// The 8-bit PNG of page 1 at the run's DPI, or why there is none.
    pub png: Result<PathBuf, String>,
    /// The UTF-32LE text of page 1, or why there is none.
    pub txt: Result<PathBuf, String>,
}

/// FNV-1a over the file's bytes — a cache key, not a security property.
pub fn content_hash(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

impl Oracle {
    /// The determinism arguments every invocation carries.
    fn determinism_args(&self) -> [String; 3] {
        [
            "--time=1399672130".to_owned(),
            "--croscore-font-names".to_owned(),
            format!("--font-dir={}", self.font_dir.display()),
        ]
    }

    /// Runs the oracle on `file`, or returns the cached outputs.
    pub fn outputs(&self, file: &Path, password: Option<&str>) -> Result<OracleOutput> {
        let bytes = std::fs::read(file).with_context(|| format!("reading {}", file.display()))?;
        let key = format!(
            "{:016x}-{}-{:08x}",
            content_hash(&bytes),
            self.dpi.round() as u32,
            content_hash(password.unwrap_or_default().as_bytes()) as u32
        );
        let dir = self.cache_root.join(key);
        let manifest = dir.join("oracle.json");
        if let Ok(text) = std::fs::read_to_string(&manifest)
            && let Ok(cached) = serde_json::from_str::<OracleOutput>(&text)
        {
            return Ok(cached);
        }
        std::fs::create_dir_all(&dir)?;
        let input = dir.join("input.pdf");
        std::fs::write(&input, &bytes)?;

        let scale = self.dpi / 72.0;
        let png = self
            .invoke(
                &input,
                &[
                    "--png",
                    "--md5",
                    &format!("--scale={scale:.10}"),
                    "--pages=0",
                ],
                password,
            )
            .and_then(|()| {
                let path = dir.join("input.pdf.0.png");
                path.is_file()
                    .then_some(path)
                    .ok_or_else(|| anyhow!("oracle wrote no PNG (page 1 failed to render)"))
            })
            .map_err(|err| format!("{err:#}"));
        let txt = self
            .invoke(&input, &["--txt", "--pages=0"], password)
            .and_then(|()| {
                let path = dir.join("input.pdf.0.txt");
                path.is_file()
                    .then_some(path)
                    .ok_or_else(|| anyhow!("oracle wrote no text (page 1 failed to load)"))
            })
            .map_err(|err| format!("{err:#}"));
        let output = OracleOutput { png, txt };
        std::fs::write(&manifest, serde_json::to_string_pretty(&output)?)?;
        Ok(output)
    }

    fn invoke(&self, input: &Path, flags: &[&str], password: Option<&str>) -> Result<()> {
        let output = Command::new(&self.binary)
            .args(self.determinism_args())
            .args(flags)
            .args(password.map(|p| format!("--password={p}")))
            .arg(input)
            .output()
            .with_context(|| format!("running {}", self.binary.display()))?;
        if output.status.success() {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            Err(anyhow!(
                "pdfium_test exited {}: {}",
                output.status,
                stderr
                    .trim()
                    .lines()
                    .chain(stdout.trim().lines())
                    .last()
                    .unwrap_or("")
            ))
        }
    }

    /// The oracle checkout's commit, read from `.git` without running git.
    pub fn checkout_commit(checkout: &Path) -> Option<String> {
        let head = std::fs::read_to_string(checkout.join(".git/HEAD")).ok()?;
        let head = head.trim();
        if let Some(reference) = head.strip_prefix("ref: ") {
            let direct = std::fs::read_to_string(checkout.join(".git").join(reference)).ok();
            if let Some(hash) = direct {
                return Some(hash.trim().chars().take(12).collect());
            }
            let packed = std::fs::read_to_string(checkout.join(".git/packed-refs")).ok()?;
            let line = packed.lines().find(|line| line.ends_with(reference))?;
            return Some(line.split_whitespace().next()?.chars().take(12).collect());
        }
        Some(head.chars().take(12).collect())
    }
}
