//! Enumerating the test corpus in the oracle checkout.
//!
//! Three sources feed the harness, and all three are read-only:
//!
//! - `testing/corpus/**/*.pdf` — the 836-file regression corpus.
//! - `testing/resources/**/*.pdf` — checked-in fixtures.
//! - `testing/resources/**/*.in` — hand-written templates, valid PDFs only
//!   after `testing/tools/fixup_pdf_template.py` fills in byte offsets.
//!
//! Skipped: anything under an `xfa_specific` or `xfa` directory (we build
//! without XFA, so the oracle cannot render them), and any file named in
//! `testing/SUPPRESSIONS` for our `linux/nov8/noxfa/agg` build.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// One corpus entry: where it comes from and how it is named.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Entry {
    /// Stable identity in the scoreboard and thresholds file, e.g.
    /// `corpus/fx/text/hello.pdf` or `resources/annots.in`. Always
    /// forward-slashed so it is identical on every platform.
    pub id: String,
    /// Absolute path inside the read-only oracle checkout.
    pub source: PathBuf,
    /// How the PDF bytes are obtained.
    pub kind: EntryKind,
}

impl Entry {
    /// The sibling `.evt` script, if one sits next to this PDF or template.
    ///
    /// `pdfium_test --send-events` looks for the same stem with `.evt` in
    /// place of `.pdf`; templates use the same stem as their `.in`.
    #[must_use]
    pub fn sibling_evt(&self) -> Option<PathBuf> {
        let evt = self.source.with_extension("evt");
        evt.is_file().then_some(evt)
    }
}

/// Whether an entry is already a PDF or must be expanded first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EntryKind {
    /// A `.pdf` file, used as-is.
    Pdf,
    /// A `.in` template, expanded by `fixup_pdf_template.py`.
    Template,
}

/// Why an entry was left out of the run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SkipReason {
    /// Lives under an `xfa_specific`/`xfa` directory.
    Xfa,
    /// Named in `testing/SUPPRESSIONS` for our build configuration.
    Suppressed,
}

/// The corpus roots inside the oracle checkout.
#[derive(Debug, Clone)]
pub struct Roots {
    pub corpus: PathBuf,
    pub resources: PathBuf,
}

impl Roots {
    /// Derives the roots from a `pdfium-c++` checkout directory.
    pub fn under(checkout: &Path) -> Self {
        Roots {
            corpus: checkout.join("testing/corpus"),
            resources: checkout.join("testing/resources"),
        }
    }
}

/// The outcome of walking the corpus.
#[derive(Debug, Clone, Default)]
pub struct Listing {
    /// Entries to run, sorted by `id` so every run is ordered identically.
    pub entries: Vec<Entry>,
    /// Entries deliberately left out, with the reason.
    pub skipped: Vec<(String, SkipReason)>,
}

/// Walks both roots and applies the XFA and suppression filters.
///
/// `suppressed` holds bare file names, as SUPPRESSIONS lists them; a name
/// matches an entry when it equals the entry's file name, so a suppression
/// covers the `.pdf` and its `.in` template alike.
pub fn list(roots: &Roots, suppressed: &BTreeSet<String>) -> std::io::Result<Listing> {
    let mut listing = Listing::default();
    collect(&roots.corpus, "corpus", suppressed, &mut listing)?;
    collect(&roots.resources, "resources", suppressed, &mut listing)?;
    listing.entries.sort();
    listing.skipped.sort();
    listing.skipped.dedup();
    Ok(listing)
}

fn collect(
    root: &Path,
    prefix: &str,
    suppressed: &BTreeSet<String>,
    listing: &mut Listing,
) -> std::io::Result<()> {
    if !root.is_dir() {
        return Ok(());
    }
    for path in walk(root)? {
        let Some(kind) = classify(&path) else {
            continue;
        };
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        let id = format!("{prefix}/{}", slashed(relative));
        if let Some(reason) = skip_reason(&path, suppressed) {
            listing.skipped.push((id, reason));
            continue;
        }
        listing.entries.push(Entry {
            id,
            source: path,
            kind,
        });
    }
    Ok(())
}

/// Depth-first directory walk, skipping `.git` and other dot directories.
fn walk(root: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let path = entry?.path();
            let is_hidden = path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with('.'));
            if is_hidden {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else {
                found.push(path);
            }
        }
    }
    Ok(found)
}

fn classify(path: &Path) -> Option<EntryKind> {
    match path.extension().and_then(|e| e.to_str()) {
        Some("pdf") => Some(EntryKind::Pdf),
        Some("in") => Some(EntryKind::Template),
        _ => None,
    }
}

fn skip_reason(path: &Path, suppressed: &BTreeSet<String>) -> Option<SkipReason> {
    if is_xfa(path) {
        return Some(SkipReason::Xfa);
    }
    let name = path.file_name()?.to_str()?;
    // SUPPRESSIONS names the `.pdf`; a template expands to that same name.
    let as_pdf = name.strip_suffix(".in").map(|stem| format!("{stem}.pdf"));
    if suppressed.contains(name) || as_pdf.is_some_and(|p| suppressed.contains(&p)) {
        return Some(SkipReason::Suppressed);
    }
    None
}

fn is_xfa(path: &Path) -> bool {
    path.components()
        .any(|component| matches!(component.as_os_str().to_str(), Some("xfa_specific" | "xfa")))
}

fn slashed(path: &Path) -> String {
    path.components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "pdfrum-corpus-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn touch(root: &Path, relative: &str) {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"%PDF-1.7\n").unwrap();
    }

    #[test]
    fn finds_pdfs_and_templates_and_ignores_everything_else() {
        let root = temp_dir("kinds");
        let (corpus, resources) = (root.join("corpus"), root.join("resources"));
        touch(&corpus, "fx/text/hello.pdf");
        touch(&resources, "annots.pdf");
        touch(&resources, "annots.in");
        touch(&resources, "README.md");
        touch(&resources, "frag.fragment");

        let roots = Roots {
            corpus: corpus.clone(),
            resources: resources.clone(),
        };
        let listing = list(&roots, &BTreeSet::new()).unwrap();
        let ids: Vec<&str> = listing.entries.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "corpus/fx/text/hello.pdf",
                "resources/annots.in",
                "resources/annots.pdf"
            ]
        );
        assert_eq!(listing.entries[1].kind, EntryKind::Template);
        assert_eq!(listing.entries[2].kind, EntryKind::Pdf);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn sibling_evt_is_the_same_stem_next_to_the_pdf_or_template() {
        let root = temp_dir("evt");
        touch(&root, "form.pdf");
        touch(&root, "form.evt");
        touch(&root, "plain.pdf");
        let with_evt = Entry {
            id: "resources/form.pdf".to_owned(),
            source: root.join("form.pdf"),
            kind: EntryKind::Pdf,
        };
        let without = Entry {
            id: "resources/plain.pdf".to_owned(),
            source: root.join("plain.pdf"),
            kind: EntryKind::Pdf,
        };
        assert_eq!(with_evt.sibling_evt(), Some(root.join("form.evt")));
        assert_eq!(without.sibling_evt(), None);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn ids_are_sorted_so_runs_are_reproducible() {
        let root = temp_dir("order");
        let corpus = root.join("corpus");
        for name in ["z.pdf", "a.pdf", "m/n.pdf"] {
            touch(&corpus, name);
        }
        let roots = Roots {
            corpus,
            resources: root.join("missing"),
        };
        let listing = list(&roots, &BTreeSet::new()).unwrap();
        let ids: Vec<&str> = listing.entries.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["corpus/a.pdf", "corpus/m/n.pdf", "corpus/z.pdf"]);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn xfa_directories_are_skipped() {
        let root = temp_dir("xfa");
        let corpus = root.join("corpus");
        touch(&corpus, "xfa_specific/fx/case.pdf");
        touch(&corpus, "fx/keep.pdf");
        let resources = root.join("resources");
        touch(&resources, "xfa/form.pdf");

        let roots = Roots { corpus, resources };
        let listing = list(&roots, &BTreeSet::new()).unwrap();
        assert_eq!(
            listing
                .entries
                .iter()
                .map(|e| e.id.as_str())
                .collect::<Vec<_>>(),
            ["corpus/fx/keep.pdf"]
        );
        assert_eq!(listing.skipped.len(), 2);
        assert!(listing.skipped.iter().all(|(_, r)| *r == SkipReason::Xfa));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn suppressed_names_are_skipped_including_their_templates() {
        let root = temp_dir("supp");
        let resources = root.join("resources");
        touch(&resources, "bad.pdf");
        touch(&resources, "bad.in");
        touch(&resources, "good.pdf");

        let suppressed = BTreeSet::from(["bad.pdf".to_owned()]);
        let roots = Roots {
            corpus: root.join("missing"),
            resources,
        };
        let listing = list(&roots, &suppressed).unwrap();
        assert_eq!(
            listing
                .entries
                .iter()
                .map(|e| e.id.as_str())
                .collect::<Vec<_>>(),
            ["resources/good.pdf"]
        );
        assert_eq!(listing.skipped.len(), 2);
        assert!(
            listing
                .skipped
                .iter()
                .all(|(_, r)| *r == SkipReason::Suppressed)
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn hidden_directories_like_dot_git_are_not_walked() {
        let root = temp_dir("hidden");
        let corpus = root.join("corpus");
        touch(&corpus, ".git/objects/stray.pdf");
        touch(&corpus, "real.pdf");
        let roots = Roots {
            corpus,
            resources: root.join("missing"),
        };
        let listing = list(&roots, &BTreeSet::new()).unwrap();
        assert_eq!(
            listing
                .entries
                .iter()
                .map(|e| e.id.as_str())
                .collect::<Vec<_>>(),
            ["corpus/real.pdf"]
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_missing_root_is_empty_not_an_error() {
        let roots = Roots {
            corpus: PathBuf::from("/nonexistent/corpus"),
            resources: PathBuf::from("/nonexistent/resources"),
        };
        assert!(list(&roots, &BTreeSet::new()).unwrap().entries.is_empty());
    }

    #[test]
    fn roots_derive_from_a_checkout() {
        let roots = Roots::under(Path::new("/oracle"));
        assert_eq!(roots.corpus, Path::new("/oracle/testing/corpus"));
        assert_eq!(roots.resources, Path::new("/oracle/testing/resources"));
    }
}
