//! The golden store: oracle output keyed by PDF content.
//!
//! Layout, under `conformance/goldens/`:
//!
//! ```text
//! <sha256[..16] of the pdf bytes>/
//!   manifest.json          source path, page count, artifact list, oracle md5 lines
//!   input.pdf.0.png        page renders, named exactly as the oracle wrote them
//!   input.pdf.0.txt        page text, transcoded UTF-32LE -> UTF-8
//!   input.pdf.0.annot.txt  per-page annotation dump
//!   metadata.txt           --show-metadata stdout (may legitimately be empty)
//!   pageinfo.txt           --show-pageinfo stdout
//!   structure.txt          --show-structure stdout (empty when untagged)
//! ```
//!
//! Keying by content hash rather than path means the 228 `.in` templates whose
//! expansion equals a checked-in `.pdf` share one golden directory, and a
//! corpus file that moves keeps its goldens. The manifest records every source
//! path that hashes here, so `triage` can still name a file the way a human
//! does.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::json::Json;
use crate::oracle::Md5Line;

/// The content key for a PDF: the first 16 hex digits of its SHA-256.
///
/// Sixteen digits is 64 bits — ample against accidental collision across a
/// few thousand files, and short enough to type in a shell.
pub fn key_for(pdf_bytes: &[u8]) -> String {
    let digest = Sha256::digest(pdf_bytes);
    digest.iter().take(8).fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

/// What one golden directory records about its PDF.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    /// The content key, repeated here so a stray directory is self-describing.
    pub key: String,
    /// Corpus-relative source ids that produced these bytes (usually one; a
    /// `.in` and its checked-in `.pdf` sibling make two).
    pub sources: Vec<String>,
    /// Pages the oracle reported, or `None` when it failed to load the file.
    pub page_count: Option<u32>,
    /// Artifact file names present in the directory, sorted.
    pub artifacts: Vec<String>,
    /// `MD5:` lines from the render pass, keyed by artifact name.
    pub md5: Vec<Md5Line>,
    /// Passes the oracle exited nonzero on — recorded, not fatal: a corpus
    /// file the oracle itself rejects is still a golden ("both must fail").
    pub oracle_failures: Vec<String>,
}

/// Reading a manifest back off disk went wrong.
#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    #[error("manifest is not valid JSON: {0}")]
    Json(#[from] crate::json::JsonError),
    #[error("manifest is missing field {field}")]
    Missing { field: &'static str },
}

impl Manifest {
    /// Renders the manifest as stable JSON text.
    pub fn to_json(&self) -> Json {
        Json::Obj(vec![
            ("key".to_owned(), Json::str(&self.key)),
            (
                "sources".to_owned(),
                Json::Arr(self.sources.iter().map(Json::str).collect()),
            ),
            (
                "page_count".to_owned(),
                self.page_count
                    .map_or(Json::Null, |n| Json::int(u64::from(n))),
            ),
            (
                "artifacts".to_owned(),
                Json::Arr(self.artifacts.iter().map(Json::str).collect()),
            ),
            (
                "md5".to_owned(),
                Json::Arr(
                    self.md5
                        .iter()
                        .map(|line| {
                            Json::Obj(vec![
                                ("artifact".to_owned(), Json::str(line.file_name())),
                                ("digest".to_owned(), Json::str(&line.digest)),
                            ])
                        })
                        .collect(),
                ),
            ),
            (
                "oracle_failures".to_owned(),
                Json::Arr(self.oracle_failures.iter().map(Json::str).collect()),
            ),
        ])
    }

    /// Parses a manifest from JSON text.
    pub fn from_text(text: &str) -> Result<Manifest, ManifestError> {
        let value = Json::parse(text)?;
        let strings = |field: &'static str| -> Vec<String> {
            value
                .get(field)
                .and_then(Json::as_arr)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default()
        };
        let key = value
            .get("key")
            .and_then(Json::as_str)
            .ok_or(ManifestError::Missing { field: "key" })?
            .to_owned();
        let md5 = value
            .get("md5")
            .and_then(Json::as_arr)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| {
                        Some(Md5Line {
                            path: item.get("artifact")?.as_str()?.to_owned(),
                            digest: item.get("digest")?.as_str()?.to_owned(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(Manifest {
            key,
            sources: strings("sources"),
            page_count: value
                .get("page_count")
                .and_then(Json::as_f64)
                .and_then(crate::json::as_u32),
            artifacts: strings("artifacts"),
            md5,
            oracle_failures: strings("oracle_failures"),
        })
    }

    /// The digest recorded for one artifact, if the render pass produced one.
    pub fn digest_of(&self, artifact: &str) -> Option<&str> {
        self.md5
            .iter()
            .find(|line| line.file_name() == artifact)
            .map(|line| line.digest.as_str())
    }
}

/// The on-disk golden store.
#[derive(Debug, Clone)]
pub struct Store {
    pub root: PathBuf,
}

impl Store {
    /// Opens (without creating) a store rooted at `root`.
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Store { root: root.into() }
    }

    /// The directory holding one PDF's goldens.
    pub fn dir(&self, key: &str) -> PathBuf {
        self.root.join(key)
    }

    /// The manifest path for one key.
    pub fn manifest_path(&self, key: &str) -> PathBuf {
        self.dir(key).join("manifest.json")
    }

    /// Whether a complete golden set already exists — the resumability check.
    pub fn has(&self, key: &str) -> bool {
        self.manifest_path(key).is_file()
    }

    /// Reads one manifest.
    pub fn manifest(&self, key: &str) -> std::io::Result<Manifest> {
        let text = std::fs::read_to_string(self.manifest_path(key))?;
        Manifest::from_text(&text)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err.to_string()))
    }

    /// Reads one artifact's bytes.
    pub fn artifact(&self, key: &str, name: &str) -> std::io::Result<Vec<u8>> {
        std::fs::read(self.dir(key).join(name))
    }

    /// Writes a manifest, creating the directory if needed.
    pub fn write_manifest(&self, manifest: &Manifest) -> std::io::Result<()> {
        std::fs::create_dir_all(self.dir(&manifest.key))?;
        std::fs::write(
            self.manifest_path(&manifest.key),
            manifest.to_json().to_pretty(),
        )
    }

    /// Writes one artifact into a key's directory.
    pub fn write_artifact(&self, key: &str, name: &str, bytes: &[u8]) -> std::io::Result<()> {
        let dir = self.dir(key);
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join(name), bytes)
    }
}

/// Every key currently present in a store, sorted.
pub fn keys(root: &Path) -> std::io::Result<Vec<String>> {
    if !root.is_dir() {
        return Ok(Vec::new());
    }
    let mut found = Vec::new();
    for entry in std::fs::read_dir(root)? {
        let path = entry?.path();
        if path.join("manifest.json").is_file()
            && let Some(name) = path.file_name().and_then(|n| n.to_str())
        {
            found.push(name.to_owned());
        }
    }
    found.sort();
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Manifest {
        Manifest {
            key: "0123456789abcdef".to_owned(),
            sources: vec![
                "resources/annots.in".to_owned(),
                "resources/annots.pdf".to_owned(),
            ],
            page_count: Some(2),
            artifacts: vec![
                "input.pdf.0.png".to_owned(),
                "input.pdf.0.txt".to_owned(),
                "metadata.txt".to_owned(),
            ],
            md5: vec![Md5Line {
                path: "input.pdf.0.png".to_owned(),
                digest: "936f6bb7c6f4030f4b05741f2d5fd19e".to_owned(),
            }],
            oracle_failures: vec![],
        }
    }

    #[test]
    fn the_key_is_the_first_sixteen_hex_digits_of_sha256() {
        // sha256("") = e3b0c44298fc1c14...
        let key = key_for(b"");
        assert_eq!(key.len(), 16);
        assert_eq!(key, "e3b0c44298fc1c14");
    }

    #[test]
    fn equal_bytes_key_alike_and_different_bytes_do_not() {
        assert_eq!(key_for(b"%PDF-1.7"), key_for(b"%PDF-1.7"));
        assert_ne!(key_for(b"%PDF-1.7"), key_for(b"%PDF-1.6"));
    }

    #[test]
    fn manifest_round_trips_through_json() {
        let manifest = sample();
        let text = manifest.to_json().to_pretty();
        assert_eq!(Manifest::from_text(&text).unwrap(), manifest);
    }

    #[test]
    fn a_failed_load_round_trips_as_a_null_page_count() {
        let manifest = Manifest {
            page_count: None,
            oracle_failures: vec!["Render".to_owned(), "Text".to_owned()],
            ..sample()
        };
        let text = manifest.to_json().to_pretty();
        let back = Manifest::from_text(&text).unwrap();
        assert_eq!(back.page_count, None);
        assert_eq!(back.oracle_failures, ["Render", "Text"]);
    }

    #[test]
    fn digest_lookup_finds_an_artifact() {
        let manifest = sample();
        assert_eq!(
            manifest.digest_of("input.pdf.0.png"),
            Some("936f6bb7c6f4030f4b05741f2d5fd19e")
        );
        assert_eq!(manifest.digest_of("input.pdf.9.png"), None);
    }

    #[test]
    fn a_manifest_without_a_key_is_an_error() {
        assert!(matches!(
            Manifest::from_text("{}"),
            Err(ManifestError::Missing { field: "key" })
        ));
        assert!(matches!(
            Manifest::from_text("not json"),
            Err(ManifestError::Json(_))
        ));
    }

    #[test]
    fn store_paths_follow_the_documented_layout() {
        let store = Store::at("/goldens");
        assert_eq!(store.dir("abc"), Path::new("/goldens/abc"));
        assert_eq!(
            store.manifest_path("abc"),
            Path::new("/goldens/abc/manifest.json")
        );
        assert!(!store.has("abc"));
    }

    #[test]
    fn a_written_store_is_readable_and_resumable() {
        let root = std::env::temp_dir().join(format!(
            "pdfrum-goldens-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let store = Store::at(&root);
        let manifest = sample();
        assert!(!store.has(&manifest.key));
        store.write_manifest(&manifest).unwrap();
        store
            .write_artifact(&manifest.key, "metadata.txt", b"")
            .unwrap();

        assert!(store.has(&manifest.key));
        assert_eq!(store.manifest(&manifest.key).unwrap(), manifest);
        // An empty dump is a valid golden, not an error.
        assert_eq!(store.artifact(&manifest.key, "metadata.txt").unwrap(), b"");
        assert_eq!(keys(&root).unwrap(), [manifest.key.as_str()]);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn keys_of_a_missing_store_is_empty() {
        assert!(keys(Path::new("/nonexistent/goldens")).unwrap().is_empty());
    }
}
