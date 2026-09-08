//! `pdfrum attach add|remove`: embedded files put in and taken out.
//!
//! `extract attachments` is the reader. `add` names each attachment after
//! its file, guesses the MIME type from the extension unless `--mime` says,
//! and records the file's modification time unless the save is
//! reproducible; `remove` takes attachments out by name.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use pdfrum::{Attachment, AttachmentOptions, Document};

use crate::cmd::pages::save_options;
use crate::out;
use crate::term::Term;

/// The MIME type an extension implies, for the common ones;
/// `application/octet-stream` for the rest.
pub fn guess_mime(name: &str) -> &'static str {
    let extension = Path::new(name)
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase());
    match extension.as_deref() {
        Some("pdf") => "application/pdf",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("txt") => "text/plain",
        Some("json") => "application/json",
        Some("csv") => "text/csv",
        Some("xml") => "application/xml",
        Some("zip") => "application/zip",
        _ => "application/octet-stream",
    }
}

/// One file to attach.
pub struct NewAttachment {
    pub name: String,
    pub bytes: Vec<u8>,
    pub description: Option<String>,
    /// The MIME type; guessed from the name when `None`.
    pub mime: Option<String>,
    /// The file's modification time as a PDF date, when known.
    pub modified: Option<String>,
}

/// `doc` with `items` attached, as the bytes of a saved file. A name
/// already attached, or given twice, is refused before anything is added.
pub fn add_bytes(doc: &Document, items: &[NewAttachment], deterministic: bool) -> Result<Vec<u8>> {
    if items.is_empty() {
        bail!("attach add needs at least one file");
    }
    let mut names: Vec<String> = doc
        .attachments()
        .iter()
        .map(Attachment::file_name)
        .collect();
    for item in items {
        if names.contains(&item.name) {
            bail!(
                "{:?} is already attached; `attach remove` it first, or attach it under another name",
                item.name
            );
        }
        names.push(item.name.clone());
    }
    let mut edit = doc.edit();
    for item in items {
        let mut options = AttachmentOptions::default();
        options.description.clone_from(&item.description);
        options.mime_type = Some(
            item.mime
                .clone()
                .unwrap_or_else(|| guess_mime(&item.name).to_owned()),
        );
        options.modified.clone_from(&item.modified);
        edit.add_attachment(&item.name, &item.bytes, &options)?;
    }
    let mut bytes = Vec::new();
    edit.write_to(&mut bytes, &save_options(deterministic, doc.bytes()))?;
    Ok(bytes)
}

/// `doc` without the attachments named, as the bytes of a saved file, and
/// how many were removed. A name that is not attached is refused before
/// anything is removed.
pub fn remove_bytes(
    doc: &Document,
    names: &[String],
    deterministic: bool,
) -> Result<(Vec<u8>, usize)> {
    if names.is_empty() {
        bail!("attach remove needs at least one name");
    }
    let attached: Vec<String> = doc
        .attachments()
        .iter()
        .map(Attachment::file_name)
        .collect();
    for name in names {
        if !attached.contains(name) {
            bail!("no attachment named {name:?}; `extract attachments` lists them");
        }
    }
    let mut edit = doc.edit();
    let mut removed = 0;
    for name in names {
        if edit.remove_attachment(name)? {
            removed += 1;
        }
    }
    let mut bytes = Vec::new();
    edit.write_to(&mut bytes, &save_options(deterministic, doc.bytes()))?;
    Ok((bytes, removed))
}

/// What `attach add` was asked for.
pub struct Add<'a> {
    pub file: &'a Path,
    pub password: Option<&'a str>,
    pub paths: &'a [PathBuf],
    pub description: Option<&'a str>,
    /// `--mime`, for a single file.
    pub mime: Option<&'a str>,
    pub output: &'a Path,
    pub deterministic: bool,
}

/// A file on disk as an attachment: named after the file, its modification
/// time recorded unless the save is reproducible.
fn read(path: &Path, req: &Add<'_>) -> Result<NewAttachment> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty())
        .with_context(|| format!("{}: not a file name", path.display()))?;
    let bytes = std::fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
    let modified = if req.deterministic {
        None
    } else {
        std::fs::metadata(path)
            .and_then(|m| m.modified())
            .ok()
            .map(pdfrum::pdf_date)
    };
    Ok(NewAttachment {
        name,
        bytes,
        description: req.description.map(str::to_owned),
        mime: req.mime.map(str::to_owned),
        modified,
    })
}

/// `attach add`: every path attached under its file name.
pub fn add(req: &Add<'_>, term: Term) -> Result<ExitCode> {
    if req.mime.is_some() && req.paths.len() > 1 {
        bail!("--mime names one type; give one file with it");
    }
    let sink = out::Sink::new(req.output, "PDF")?;
    let doc = out::open(req.file, req.password)?;
    let items: Vec<NewAttachment> = req
        .paths
        .iter()
        .map(|p| read(p, req))
        .collect::<Result<_>>()?;
    let bytes = add_bytes(&doc, &items, req.deterministic)?;
    sink.finish(
        term,
        &bytes,
        &format!(
            "{} attachment{} added",
            items.len(),
            if items.len() == 1 { "" } else { "s" }
        ),
        None,
    )?;
    Ok(ExitCode::SUCCESS)
}

/// `attach remove`: the attachments named taken out.
pub fn remove(
    file: &Path,
    password: Option<&str>,
    names: &[String],
    output: &Path,
    deterministic: bool,
    term: Term,
) -> Result<ExitCode> {
    let sink = out::Sink::new(output, "PDF")?;
    let doc = out::open(file, password)?;
    let (bytes, removed) = remove_bytes(&doc, names, deterministic)?;
    sink.finish(
        term,
        &bytes,
        &format!(
            "{removed} attachment{} removed",
            if removed == 1 { "" } else { "s" }
        ),
        None,
    )?;
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::guess_mime;

    #[test]
    fn the_common_extensions_are_known_and_the_rest_are_bytes() {
        assert_eq!(guess_mime("report.PDF"), "application/pdf");
        assert_eq!(guess_mime("a.jpeg"), "image/jpeg");
        assert_eq!(guess_mime("data.csv"), "text/csv");
        assert_eq!(guess_mime("notes"), "application/octet-stream");
        assert_eq!(guess_mime("x.tar.gz"), "application/octet-stream");
    }
}
