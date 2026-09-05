//! The cost of adoption, measured on a minimal consumer crate per engine:
//! crates in the tree, C in the build, clean build time, stripped binary
//! size, `unsafe` in the tree, licence. Every number is what `cargo` and a
//! grep say on this machine, not what a README says.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};

/// The adoption row for one engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Adoption {
    pub engine: String,
    pub version: String,
    /// The `[dependencies]` line the consumer crate used.
    pub dependency: String,
    pub license: String,
    /// Unique crates in `cargo tree -e normal`, the consumer itself included.
    pub crates: usize,
    /// Crates in `cargo tree -e build` named `cc`, `cmake`, `bindgen`,
    /// `pkg-config` or `*-sys`.
    pub native_build_crates: Vec<String>,
    /// Wall seconds of `cargo build --release` from an empty target dir.
    pub clean_build_secs: f64,
    /// Bytes of the release binary after `strip`.
    pub stripped_bytes: u64,
    /// Occurrences of the token `unsafe` in the `.rs` files of the crate itself.
    pub unsafe_in_crate: usize,
    /// The same over every crate in the normal tree (std excluded).
    pub unsafe_in_tree: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

struct Consumer {
    engine: &'static str,
    version: &'static str,
    dependency: String,
    main: &'static str,
}

fn consumers(repo_root: &Path, pdfium_lib: Option<&Path>) -> Vec<Consumer> {
    let pdfrum_path = repo_root.join("crates/pdfrum");
    let lib = pdfium_lib
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    vec![
        Consumer {
            engine: "pdfrum",
            version: "0.1.0",
            dependency: format!("pdfrum = {{ path = \"{}\" }}", pdfrum_path.display()),
            main: "fn main() { let p = std::env::args().nth(1).unwrap_or_default(); let d = pdfrum::Document::open(&p).unwrap_or_else(|e| { eprintln!(\"{e}\"); std::process::exit(1) }); println!(\"{}\", d.page_count()); }",
        },
        Consumer {
            engine: "hayro",
            version: "0.7.1",
            dependency: "hayro = \"=0.7.1\"".to_owned(),
            main: "fn main() { let p = std::env::args().nth(1).unwrap_or_default(); let b = std::fs::read(&p).unwrap_or_default(); let d = hayro::hayro_interpret::hayro_syntax::Pdf::new(b).unwrap_or_else(|e| { eprintln!(\"{e:?}\"); std::process::exit(1) }); println!(\"{}\", d.pages().len()); }",
        },
        Consumer {
            engine: "pdf-extract",
            version: "0.12.0",
            dependency: "pdf-extract = \"=0.12.0\"".to_owned(),
            main: "fn main() { let p = std::env::args().nth(1).unwrap_or_default(); let t = pdf_extract::extract_text(&p).unwrap_or_else(|e| { eprintln!(\"{e:?}\"); std::process::exit(1) }); println!(\"{}\", t.len()); }",
        },
        Consumer {
            engine: "lopdf",
            version: "0.44.0",
            dependency: "lopdf = \"=0.44.0\"".to_owned(),
            main: "fn main() { let p = std::env::args().nth(1).unwrap_or_default(); let d = lopdf::Document::load(&p).unwrap_or_else(|e| { eprintln!(\"{e}\"); std::process::exit(1) }); println!(\"{}\", d.get_pages().len()); }",
        },
        Consumer {
            engine: "pdf",
            version: "0.10.0",
            dependency: "pdf = \"=0.10.0\"".to_owned(),
            main: "fn main() { let p = std::env::args().nth(1).unwrap_or_default(); let f = pdf::file::FileOptions::cached().open(&p).unwrap_or_else(|e| { eprintln!(\"{e}\"); std::process::exit(1) }); println!(\"{}\", f.num_pages()); }",
        },
        Consumer {
            engine: "pdf_oxide",
            version: "0.3.77",
            dependency: "pdf_oxide = \"=0.3.77\"".to_owned(),
            main: "fn main() { let p = std::env::args().nth(1).unwrap_or_default(); let d = pdf_oxide::PdfDocument::open(&p).unwrap_or_else(|e| { eprintln!(\"{e}\"); std::process::exit(1) }); println!(\"{}\", d.page_count().unwrap_or(0)); }",
        },
        Consumer {
            engine: "pdfium-render",
            version: "0.9.3",
            dependency: "pdfium-render = \"=0.9.3\"".to_owned(),
            main: Box::leak(format!(
                "use pdfium_render::prelude::*; fn main() {{ let p = std::env::args().nth(1).unwrap_or_default(); let pdfium = Pdfium::new(Pdfium::bind_to_library(\"{lib}\").unwrap_or_else(|e| {{ eprintln!(\"{{e:?}}\"); std::process::exit(1) }})); let d = pdfium.load_pdf_from_file(&p, None).unwrap_or_else(|e| {{ eprintln!(\"{{e:?}}\"); std::process::exit(1) }}); println!(\"{{}}\", d.pages().len()); }}"
            ).into_boxed_str()),
        },
        Consumer {
            engine: "mupdf",
            version: "0.8.0",
            dependency: "mupdf = { version = \"=0.8.0\", default-features = false, features = [\"base14-fonts\"] }".to_owned(),
            main: "fn main() { let p = std::env::args().nth(1).unwrap_or_default(); let d = mupdf::Document::open(&p).unwrap_or_else(|e| { eprintln!(\"{e}\"); std::process::exit(1) }); println!(\"{}\", d.page_count().unwrap_or(0)); }",
        },
    ]
}

fn cargo(dir: &Path, target: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("cargo")
        .args(args)
        .current_dir(dir)
        .env("CARGO_TARGET_DIR", target)
        .output()
        .with_context(|| format!("cargo {}", args.join(" ")))?;
    if !output.status.success() {
        bail!(
            "cargo {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
                .lines()
                .rev()
                .take(15)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Unique `name version` pairs from a `--prefix none` tree.
fn tree_crates(listing: &str) -> BTreeSet<(String, String)> {
    listing
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let name = parts.next()?;
            let version = parts.next()?.trim_start_matches('v');
            Some((name.to_owned(), version.to_owned()))
        })
        .collect()
}

/// Counts the token `unsafe` in every `.rs` file under `dir`.
fn count_unsafe(dir: &Path) -> usize {
    fn walk(dir: &Path, total: &mut usize) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().is_none_or(|n| n != "target") {
                    walk(&path, total);
                }
            } else if path.extension().is_some_and(|ext| ext == "rs")
                && let Ok(text) = std::fs::read_to_string(&path)
            {
                *total += text
                    .split(|c: char| !(c.is_alphanumeric() || c == '_'))
                    .filter(|word| *word == "unsafe")
                    .count();
            }
        }
    }
    let mut total = 0;
    walk(dir, &mut total);
    total
}

/// Where a crate's source is: the registry cache for a published crate, the
/// workspace for a pdfrum member.
fn source_dir(name: &str, version: &str, repo_root: &Path) -> Option<PathBuf> {
    if let Some(member) = name.strip_prefix("pdfrum") {
        let candidate = repo_root.join("crates").join(format!("pdfrum{member}"));
        if candidate.is_dir() {
            return Some(candidate);
        }
        let corpus = repo_root.join("benches/corpus-list");
        if name == "pdfrum-corpus" && corpus.is_dir() {
            return Some(corpus);
        }
    }
    let home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cargo")))?;
    let registry = home.join("registry/src");
    for index in std::fs::read_dir(registry).ok()?.flatten() {
        let candidate = index.path().join(format!("{name}-{version}"));
        if candidate.is_dir() {
            return Some(candidate);
        }
    }
    None
}

fn license_of(metadata: &serde_json::Value, name: &str) -> String {
    metadata["packages"]
        .as_array()
        .and_then(|packages| packages.iter().find(|p| p["name"].as_str() == Some(name)))
        .and_then(|p| p["license"].as_str())
        .unwrap_or("?")
        .to_owned()
}

/// Measures one consumer.
fn measure(consumer: &Consumer, scratch: &Path, repo_root: &Path) -> Result<Adoption> {
    let dir = scratch.join("adoption").join(consumer.engine);
    let target = dir.join("target");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src"))?;
    std::fs::write(
        dir.join("Cargo.toml"),
        format!(
            "[package]\nname = \"consumer\"\nversion = \"0.0.0\"\nedition = \"2024\"\n[workspace]\n[dependencies]\n{}\n[profile.release]\nstrip = false\n",
            consumer.dependency
        ),
    )?;
    std::fs::write(dir.join("src/main.rs"), consumer.main)?;

    let normal = cargo(&dir, &target, &["tree", "-e", "normal", "--prefix", "none"])?;
    let build = cargo(&dir, &target, &["tree", "-e", "build", "--prefix", "none"])?;
    let normal_crates = tree_crates(&normal);
    let native: Vec<String> = tree_crates(&build)
        .into_iter()
        .map(|(name, _)| name)
        .filter(|name| {
            name.ends_with("-sys")
                || ["cc", "cmake", "bindgen", "pkg-config"].contains(&name.as_str())
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let metadata: serde_json::Value = serde_json::from_str(&cargo(
        &dir,
        &target,
        &["metadata", "--format-version", "1"],
    )?)?;
    let license = license_of(&metadata, consumer.engine);

    let clock = Instant::now();
    cargo(&dir, &target, &["build", "--release"])?;
    let clean_build_secs = clock.elapsed().as_secs_f64();
    let binary = target.join("release/consumer");
    let stripped = target.join("release/consumer.stripped");
    let status = Command::new("strip")
        .arg("-o")
        .arg(&stripped)
        .arg(&binary)
        .status()
        .context("strip")?;
    if !status.success() {
        bail!("strip failed");
    }
    let stripped_bytes = std::fs::metadata(&stripped)?.len();

    let mut unsafe_in_tree = 0;
    let mut unsafe_in_crate = 0;
    for (name, version) in &normal_crates {
        if name == "consumer" {
            continue;
        }
        let Some(source) = source_dir(name, version, repo_root) else {
            eprintln!("  (no source for {name} {version}; unsafe count skipped)");
            continue;
        };
        let count = count_unsafe(&source);
        unsafe_in_tree += count;
        if name == consumer.engine || name.starts_with("pdfrum") && consumer.engine == "pdfrum" {
            unsafe_in_crate += count;
        }
    }

    Ok(Adoption {
        engine: consumer.engine.to_owned(),
        version: consumer.version.to_owned(),
        dependency: consumer.dependency.clone(),
        license,
        crates: normal_crates.len(),
        native_build_crates: native,
        clean_build_secs,
        stripped_bytes,
        unsafe_in_crate,
        unsafe_in_tree,
        error: None,
    })
}

/// Measures every named engine; a failure is a row with `error`, not a
/// missing row.
pub fn run(
    engines: &[String],
    scratch: &Path,
    repo_root: &Path,
    pdfium_lib: Option<&Path>,
) -> Result<Vec<Adoption>> {
    let consumers = consumers(repo_root, pdfium_lib);
    let mut out = Vec::new();
    for name in engines {
        let Some(consumer) = consumers.iter().find(|c| c.engine == name) else {
            return Err(anyhow!("no consumer template for engine {name}"));
        };
        eprintln!("adoption: {name}");
        match measure(consumer, scratch, repo_root) {
            Ok(row) => out.push(row),
            Err(err) => out.push(Adoption {
                engine: consumer.engine.to_owned(),
                version: consumer.version.to_owned(),
                dependency: consumer.dependency.clone(),
                license: "?".to_owned(),
                crates: 0,
                native_build_crates: Vec::new(),
                clean_build_secs: 0.0,
                stripped_bytes: 0,
                unsafe_in_crate: 0,
                unsafe_in_tree: 0,
                error: Some(format!("{err:#}")),
            }),
        }
    }
    Ok(out)
}

/// The adoption table as Markdown.
pub fn render(rows: &[Adoption]) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    out.push_str("| engine | version | licence | crates in tree | C in build | clean release build | stripped hello-world | `unsafe` in crate | `unsafe` in tree |\n");
    out.push_str("|---|---|---|---|---|---|---|---|---|\n");
    for row in rows {
        if let Some(err) = &row.error {
            let _ = writeln!(
                out,
                "| {} | {} | not measured: {} | | | | | | |",
                row.engine,
                row.version,
                err.lines().next().unwrap_or("")
            );
            continue;
        }
        let _ = writeln!(
            out,
            "| {} | {} | {} | {} | {} | {:.0} s | {:.1} MiB | {} | {} |",
            row.engine,
            row.version,
            row.license,
            row.crates,
            if row.native_build_crates.is_empty() {
                "none".to_owned()
            } else {
                row.native_build_crates.join(", ")
            },
            row.clean_build_secs,
            row.stripped_bytes as f64 / (1024.0 * 1024.0),
            row.unsafe_in_crate,
            row.unsafe_in_tree
        );
    }
    out
}
