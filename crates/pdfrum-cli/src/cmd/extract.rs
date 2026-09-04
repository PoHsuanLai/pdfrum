//! `pdfrum extract …`: text, links, the outline, attachments, annotations,
//! signatures.

use std::fmt::Write;
use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context, Result};
use pdfrum::{Document, LinkTarget};
use serde::Serialize;

use crate::out::{self, JsonRect, out, outln};
use crate::pages;
use crate::term::Term;

// ---- text -----------------------------------------------------------------

#[derive(Serialize)]
struct PageText {
    page: u32,
    text: String,
}

pub fn text(
    file: &Path,
    password: Option<&str>,
    spec: Option<&str>,
    layout: bool,
    json: bool,
) -> Result<ExitCode> {
    let doc = out::open(file, password)?;
    let selected = pages::select(spec, doc.page_count())?;
    let mut out_pages = Vec::with_capacity(selected.len());
    for index in selected {
        let page = doc.page(index)?;
        out_pages.push(PageText {
            page: out::page_number(page.index()),
            // The extractor keeps PDFium's `\r\n` line ends, which are the
            // oracle's contract; a command line speaks `\n`.
            text: if layout {
                page.layout_text()
            } else {
                page.text().slice(..).replace("\r\n", "\n")
            },
        });
    }
    if json {
        out::json(&out_pages)?;
    } else {
        for (i, p) in out_pages.iter().enumerate() {
            if i > 0 {
                // One form feed between pages, the convention `pdftotext`
                // set and every consumer of page-per-page text expects.
                out!("\u{c}");
            }
            out!("{}", p.text);
            if !p.text.ends_with('\n') {
                outln!();
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

// ---- markdown -------------------------------------------------------------

#[derive(Serialize)]
struct PageMarkdown {
    page: u32,
    markdown: String,
}

/// The pages as Markdown, one document, a horizontal rule between pages.
pub fn markdown(
    file: &Path,
    password: Option<&str>,
    spec: Option<&str>,
    json: bool,
) -> Result<ExitCode> {
    let doc = out::open(file, password)?;
    let mut out_pages = Vec::new();
    for index in pages::select(spec, doc.page_count())? {
        let page = doc.page(index)?;
        out_pages.push(PageMarkdown {
            page: out::page_number(page.index()),
            markdown: page.markdown(),
        });
    }
    if json {
        out::json(&out_pages)?;
    } else {
        for (i, p) in out_pages.iter().enumerate() {
            if i > 0 {
                outln!("\n---\n");
            }
            out!("{}", p.markdown);
        }
    }
    Ok(ExitCode::SUCCESS)
}

// ---- links ----------------------------------------------------------------

#[derive(Serialize)]
struct LinkRow {
    page: u32,
    rect: JsonRect,
    /// `page`, `uri`, `text` (a URL found in the text, not an annotation) or
    /// `other`.
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    target_page: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    uri: Option<String>,
}

pub fn links(
    file: &Path,
    password: Option<&str>,
    spec: Option<&str>,
    json: bool,
    term: Term,
) -> Result<ExitCode> {
    let doc = out::open(file, password)?;
    let mut rows = Vec::new();
    for index in pages::select(spec, doc.page_count())? {
        let page = doc.page(index)?;
        let number = out::page_number(page.index());
        for link in page.page_links() {
            let (kind, target_page, uri) = match link.target {
                LinkTarget::Page(p) => ("page", Some(out::page_number(p)), None),
                LinkTarget::Uri(u) => ("uri", None, Some(u)),
                LinkTarget::Other => ("other", None, None),
            };
            rows.push(LinkRow {
                page: number,
                rect: link.rect.into(),
                kind,
                target_page,
                uri,
            });
        }
        let text = page.text();
        for web in text.web_links() {
            for rect in text.rects(web.range.clone()) {
                rows.push(LinkRow {
                    page: number,
                    rect: rect.into(),
                    kind: "text",
                    target_page: None,
                    uri: Some(web.url.clone()),
                });
            }
        }
    }
    if json {
        out::json(&rows)?;
    } else {
        for r in &rows {
            let target = match (r.kind, r.target_page, &r.uri) {
                (_, Some(p), _) => term.link(&page_url(file, p), &format!("page {p}")),
                (_, _, Some(u)) => term.link(u, u),
                _ => "(action)".to_owned(),
            };
            outln!(
                "page {:<4} {:<5} {:<28} {target}",
                r.page,
                r.kind,
                out::rect(pdfrum::Rect::new(
                    r.rect.0[0],
                    r.rect.0[1],
                    r.rect.0[2],
                    r.rect.0[3]
                ))
            );
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// A `file://` URL opening `file` at `page`, the fragment viewers honour.
fn page_url(file: &Path, page: u32) -> String {
    let absolute = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    format!("file://{}#page={page}", absolute.display())
}

// ---- toc ------------------------------------------------------------------

#[derive(Serialize)]
struct TocRow {
    depth: usize,
    title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    page: Option<u32>,
}

pub fn toc(file: &Path, password: Option<&str>, json: bool, term: Term) -> Result<ExitCode> {
    let doc = out::open(file, password)?;
    let rows: Vec<TocRow> = doc
        .outline()
        .iter()
        .map(|b| TocRow {
            depth: b.depth(),
            title: b.title(),
            page: b.page_index().map(out::page_number),
        })
        .collect();
    if json {
        out::json(&rows)?;
    } else {
        for r in &rows {
            let page = r.page.map_or(String::new(), |p| {
                format!("  {}", term.link(&page_url(file, p), &format!("p.{p}")))
            });
            outln!("{}{}{page}", "  ".repeat(r.depth), r.title);
        }
    }
    Ok(ExitCode::SUCCESS)
}

// ---- attachments ----------------------------------------------------------

#[derive(Serialize)]
struct AttachmentRow {
    name: String,
    size: Option<usize>,
    #[serde(skip_serializing_if = "String::is_empty")]
    description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    subtype: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    written: Option<String>,
}

pub fn attachments(
    file: &Path,
    password: Option<&str>,
    dir: Option<&Path>,
    json: bool,
) -> Result<ExitCode> {
    let doc = out::open(file, password)?;
    let mut rows = Vec::new();
    for a in doc.attachments() {
        let data = a.data();
        let name = a.file_name();
        let written = match (dir, &data) {
            (Some(dir), Some(bytes)) => {
                std::fs::create_dir_all(dir)
                    .with_context(|| format!("cannot create {}", dir.display()))?;
                let leaf = Path::new(&name)
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| format!("attachment-{}", rows.len() + 1));
                let path = dir.join(leaf);
                std::fs::write(&path, bytes)
                    .with_context(|| format!("cannot write {}", path.display()))?;
                Some(path.display().to_string())
            }
            _ => None,
        };
        rows.push(AttachmentRow {
            name,
            size: data.as_ref().map(Vec::len),
            description: a.description(),
            subtype: a.subtype(),
            written,
        });
    }
    if json {
        out::json(&rows)?;
    } else if rows.is_empty() {
        outln!("no attachments");
    } else {
        for r in &rows {
            let size = r
                .size
                .map_or("(no data)".to_owned(), |n| format!("{n} bytes"));
            let mut line = format!("{:<32} {size:>14}", r.name);
            if !r.description.is_empty() {
                let _ = write!(line, "  {}", r.description);
            }
            if let Some(w) = &r.written {
                let _ = write!(line, "  -> {w}");
            }
            outln!("{line}");
        }
    }
    Ok(ExitCode::SUCCESS)
}

// ---- annotations ----------------------------------------------------------

#[derive(Serialize)]
struct AnnotationRow {
    page: u32,
    subtype: String,
    rect: JsonRect,
    hidden: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    contents: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    modified: Option<String>,
}

pub fn annotations(
    file: &Path,
    password: Option<&str>,
    spec: Option<&str>,
    json: bool,
) -> Result<ExitCode> {
    let doc = out::open(file, password)?;
    let mut rows = Vec::new();
    for index in pages::select(spec, doc.page_count())? {
        let page = doc.page(index)?;
        let number = out::page_number(page.index());
        for a in page.annotations() {
            rows.push(AnnotationRow {
                page: number,
                subtype: String::from_utf8_lossy(a.subtype().as_bytes()).into_owned(),
                rect: a.rect().into(),
                hidden: a.is_hidden(),
                name: a.name(),
                title: a.title(),
                contents: a.contents(),
                modified: a.modified(),
            });
        }
    }
    if json {
        out::json(&rows)?;
    } else {
        for r in &rows {
            let mut line = format!(
                "page {:<4} {:<12} {:<28}",
                r.page,
                r.subtype,
                out::rect(pdfrum::Rect::new(
                    r.rect.0[0],
                    r.rect.0[1],
                    r.rect.0[2],
                    r.rect.0[3]
                ))
            );
            if r.hidden {
                line.push_str(" hidden");
            }
            if let Some(t) = &r.title {
                let _ = write!(line, "  [{t}]");
            }
            if let Some(c) = &r.contents {
                let _ = write!(line, "  {}", c.replace(['\r', '\n'], " "));
            }
            outln!("{line}");
        }
    }
    Ok(ExitCode::SUCCESS)
}

// ---- signatures -----------------------------------------------------------

#[derive(Serialize)]
struct SignatureRow {
    sub_filter: Option<String>,
    reason: Option<String>,
    time: Option<String>,
    byte_range: Vec<i64>,
    doc_mdp_permission: u32,
    contents_len: usize,
}

pub fn signatures(file: &Path, password: Option<&str>, json: bool) -> Result<ExitCode> {
    let doc = out::open(file, password)?;
    let rows = signature_rows(&doc);
    if json {
        out::json(&rows)?;
    } else if rows.is_empty() {
        outln!("no signature fields");
    } else {
        for (i, s) in rows.iter().enumerate() {
            outln!(
                "signature {}: {}  contents {} bytes  byte range {:?}",
                i + 1,
                s.sub_filter.as_deref().unwrap_or("(no SubFilter)"),
                s.contents_len,
                s.byte_range
            );
            if let Some(t) = &s.time {
                outln!("  signed {t}");
            }
            if let Some(r) = &s.reason {
                outln!("  reason: {r}");
            }
            if s.doc_mdp_permission != 0 {
                outln!("  DocMDP permission {}", s.doc_mdp_permission);
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn signature_rows(doc: &Document) -> Vec<SignatureRow> {
    doc.signatures()
        .iter()
        .map(|s| SignatureRow {
            sub_filter: s.sub_filter(),
            reason: s.reason(),
            time: s.time(),
            byte_range: s.byte_range(),
            doc_mdp_permission: s.doc_mdp_permission(),
            contents_len: s.contents().len(),
        })
        .collect()
}
