//! `pdfrum extract …`: text, links, the outline, attachments, annotations,
//! signatures.

use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context, Result};
use pdfrum::{Document, LinkTarget};
use serde::Serialize;

use crate::out::{self, Align, JsonRect, Table, out, outln};
use crate::pages;
use crate::term::{Style, Term};

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
    } else if rows.is_empty() {
        out::none("links");
    } else {
        let mut table = Table::new(&[
            ("PAGE", Align::Right),
            ("KIND", Align::Left),
            ("AREA", Align::Left),
            ("TARGET", Align::Left),
        ]);
        for r in &rows {
            let target = match (r.kind, r.target_page, &r.uri) {
                (_, Some(p), _) => term.link(&page_url(file, p), &format!("page {p}")),
                (_, _, Some(u)) => term.link(u, u),
                _ => "action".to_owned(),
            };
            table.row(vec![
                out::page(term, r.page),
                r.kind.to_owned(),
                out::rect(pdfrum::Rect::new(
                    r.rect.0[0],
                    r.rect.0[1],
                    r.rect.0[2],
                    r.rect.0[3],
                )),
                target,
            ]);
        }
        table.print(term, 0);
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
    } else if rows.is_empty() {
        out::none("bookmarks");
    } else {
        let mut table = Table::new(&[("TITLE", Align::Left), ("PAGE", Align::Right)]);
        for r in &rows {
            let page = r.page.map_or(String::new(), |p| {
                term.link(&page_url(file, p), &p.to_string())
            });
            table.row(vec![format!("{}{}", "  ".repeat(r.depth), r.title), page]);
        }
        table.print(term, 0);
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
    term: Term,
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
        out::none("attachments");
    } else {
        let mut table = Table::new(&[
            ("NAME", Align::Left),
            ("SIZE", Align::Right),
            ("DESCRIPTION", Align::Left),
            ("WRITTEN", Align::Left),
        ]);
        for r in &rows {
            table.row(vec![
                term.paint(Style::Ident, &r.name),
                r.size
                    .map_or("no data".to_owned(), |n| out::bytes(n as u64)),
                r.description.clone(),
                r.written.clone().unwrap_or_default(),
            ]);
        }
        table.print(term, 0);
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
    term: Term,
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
    } else if rows.is_empty() {
        out::none("annotations");
    } else {
        let mut table = Table::new(&[
            ("PAGE", Align::Right),
            ("KIND", Align::Left),
            ("AREA", Align::Left),
            ("FLAGS", Align::Left),
            ("TITLE", Align::Left),
            ("CONTENTS", Align::Left),
        ]);
        for r in &rows {
            table.row(vec![
                out::page(term, r.page),
                r.subtype.clone(),
                out::rect(pdfrum::Rect::new(
                    r.rect.0[0],
                    r.rect.0[1],
                    r.rect.0[2],
                    r.rect.0[3],
                )),
                if r.hidden {
                    "hidden".to_owned()
                } else {
                    String::new()
                },
                r.title.clone().unwrap_or_default(),
                r.contents
                    .as_ref()
                    .map(|c| c.replace(['\r', '\n'], " "))
                    .unwrap_or_default(),
            ]);
        }
        table.print(term, 0);
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

pub fn signatures(file: &Path, password: Option<&str>, json: bool, term: Term) -> Result<ExitCode> {
    let doc = out::open(file, password)?;
    let rows = signature_rows(&doc);
    if json {
        out::json(&rows)?;
    } else if rows.is_empty() {
        out::none("signatures");
    } else {
        for (i, s) in rows.iter().enumerate() {
            out::heading(term, &format!("signature {}", i + 1));
            let mut record = vec![
                ("  format", s.sub_filter.clone()),
                ("  signed", s.time.clone()),
            ];
            if let Some(r) = &s.reason {
                record.push(("  reason", Some(r.clone())));
            }
            record.push(("  contents", Some(out::bytes(s.contents_len as u64))));
            record.push((
                "  byte range",
                Some(
                    s.byte_range
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(" "),
                ),
            ));
            if s.doc_mdp_permission != 0 {
                record.push((
                    "  certification",
                    Some(format!("level {}", s.doc_mdp_permission)),
                ));
            }
            out::record(term, &record);
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

// ---- images ---------------------------------------------------------------

/// Below this many pixels on a side an image is a spacer, a rule or a
/// tracking dot, not a picture; `--all` lists them anyway.
const TINY: u32 = 4;

#[derive(Serialize)]
struct ImageRow {
    page: u32,
    index: usize,
    /// The image `XObject`'s object number, when it has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    object: Option<u32>,
    /// How many times the same `XObject` is drawn on the selected pages.
    uses: usize,
    width: u32,
    height: u32,
    is_mask: bool,
    /// The file's own encoding when it is one a file can hold as is
    /// (`jpeg`, `jp2`, `jb2`, `ccitt`), else `png` for the decoded pixels.
    format: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    written: Option<String>,
}

/// Every image on the selected pages; `-o DIR` writes them out, JPEG and
/// JPEG 2000 data untouched, everything else decoded to PNG.
pub fn images(
    file: &Path,
    password: Option<&str>,
    spec: Option<&str>,
    dir: Option<&Path>,
    all: bool,
    json: bool,
    term: Term,
) -> Result<ExitCode> {
    let doc = out::open(file, password)?;
    if let Some(dir) = dir {
        std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    }
    let stem = file
        .file_stem()
        .map_or_else(|| "page".to_owned(), |s| s.to_string_lossy().into_owned());
    // One row per picture, not per draw: the same XObject placed on ten
    // pages is one image with ten uses. Only what has no object (an
    // inline image) is listed per draw.
    let mut seen: Vec<(Option<pdfrum::ObjRef>, usize)> = Vec::new();
    let mut rows = Vec::new();
    let mut pictures = Vec::new();
    for index in pages::select(spec, doc.page_count())? {
        let page = doc.page(index)?;
        let number = out::page_number(page.index());
        for image in page.images() {
            if !all && (image.width < TINY || image.height < TINY) {
                continue;
            }
            if let Some(source) = image.source
                && !all
                && let Some((_, at)) = seen.iter().find(|(s, _)| *s == Some(source))
            {
                let row: &mut ImageRow = &mut rows[*at];
                row.uses += 1;
                continue;
            }
            seen.push((image.source, rows.len()));
            rows.push(ImageRow {
                page: number,
                index: rows.len() + 1,
                object: image.source.map(|r| r.num),
                uses: 1,
                width: image.width,
                height: image.height,
                is_mask: image.is_mask,
                format: "png",
                written: None,
            });
            pictures.push(image);
        }
    }
    for (row, image) in rows.iter_mut().zip(&pictures) {
        let native = image
            .raw
            .as_ref()
            .map(|r| (r.encoding.extension(), &r.data));
        row.format = native.map_or("png", |(ext, _)| ext);
        if let Some(dir) = dir {
            let path = dir.join(format!("{stem}-{}.{}", row.index, row.format));
            match native {
                Some((_, data)) => std::fs::write(&path, data).map_err(anyhow::Error::from),
                None => image.pixmap().save_png(&path).map_err(anyhow::Error::from),
            }
            .with_context(|| format!("cannot write {}", path.display()))?;
            row.written = Some(path.display().to_string());
        }
    }
    if json {
        out::json(&rows)?;
    } else if rows.is_empty() {
        out::none("images");
    } else {
        let mut table = Table::new(&[
            ("IMAGE", Align::Right),
            ("PAGE", Align::Right),
            ("OBJECT", Align::Right),
            ("PIXELS", Align::Left),
            ("FORMAT", Align::Left),
            ("USES", Align::Right),
            ("MASK", Align::Left),
            ("WRITTEN", Align::Left),
        ]);
        for r in &rows {
            table.row(vec![
                term.paint(Style::Ident, &r.index.to_string()),
                out::page(term, r.page),
                r.object.map(|o| o.to_string()).unwrap_or_default(),
                format!("{}x{}", r.width, r.height),
                r.format.to_owned(),
                if r.uses > 1 {
                    r.uses.to_string()
                } else {
                    String::new()
                },
                if r.is_mask {
                    "mask".to_owned()
                } else {
                    String::new()
                },
                r.written.clone().unwrap_or_default(),
            ]);
        }
        table.print(term, 0);
    }
    Ok(ExitCode::SUCCESS)
}

// ---- fonts ----------------------------------------------------------------

#[derive(Serialize)]
struct FontRow {
    name: String,
    /// `type1`, `truetype`, `cff` or `opentype`.
    kind: &'static str,
    object: u32,
    size: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    written: Option<String>,
}

/// The font programs embedded in the document; `-o DIR` writes each one
/// out under its base name with the extension its format takes.
pub fn fonts(
    file: &Path,
    password: Option<&str>,
    dir: Option<&Path>,
    json: bool,
    term: Term,
) -> Result<ExitCode> {
    let doc = out::open(file, password)?;
    if let Some(dir) = dir {
        std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    }
    let mut rows = Vec::new();
    for font in doc.embedded_fonts() {
        let written = match dir {
            Some(dir) => {
                let safe: String = font
                    .name
                    .chars()
                    .map(|c| {
                        if c.is_alphanumeric() || matches!(c, '-' | '_' | '+') {
                            c
                        } else {
                            '_'
                        }
                    })
                    .collect();
                let path = dir.join(format!(
                    "{safe}-{}.{}",
                    font.object.num,
                    font.kind.extension()
                ));
                std::fs::write(&path, &font.data)
                    .with_context(|| format!("cannot write {}", path.display()))?;
                Some(path.display().to_string())
            }
            None => None,
        };
        rows.push(FontRow {
            name: font.name,
            kind: font.kind.name(),
            object: font.object.num,
            size: font.data.len(),
            written,
        });
    }
    if json {
        out::json(&rows)?;
    } else if rows.is_empty() {
        out::none("fonts");
    } else {
        let mut table = Table::new(&[
            ("NAME", Align::Left),
            ("KIND", Align::Left),
            ("OBJECT", Align::Right),
            ("SIZE", Align::Right),
            ("WRITTEN", Align::Left),
        ]);
        for r in &rows {
            table.row(vec![
                term.paint(Style::Ident, &r.name),
                r.kind.to_owned(),
                r.object.to_string(),
                out::bytes(r.size as u64),
                r.written.clone().unwrap_or_default(),
            ]);
        }
        table.print(term, 0);
    }
    Ok(ExitCode::SUCCESS)
}
