//! `pdfrum info`: what a document is, on one screen.

use std::fmt::Write;
use std::path::Path;
use std::process::ExitCode;

use anyhow::Result;
use pdfrum::{Document, Rotation};
use serde::Serialize;

use crate::out::{self, JsonRect};
use crate::term::{Style, Term};

/// The whole report, which is also the JSON schema.
#[derive(Serialize)]
struct Report {
    file: String,
    version: Option<String>,
    pages: u32,
    encrypted: bool,
    xref_rebuilt: bool,
    permissions: Permissions,
    metadata: Metadata,
    id: Option<Identity>,
    outline_entries: usize,
    attachments: usize,
    signatures: Vec<Signature>,
    page_boxes: Vec<PageBoxes>,
}

#[derive(Serialize)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "the JSON schema is five named booleans, one per permission bit"
)]
struct Permissions {
    print: bool,
    modify: bool,
    copy: bool,
    annotate: bool,
    fill_form: bool,
}

#[derive(Serialize)]
struct Metadata {
    title: Option<String>,
    author: Option<String>,
    subject: Option<String>,
    keywords: Option<String>,
    creator: Option<String>,
    producer: Option<String>,
    creation_date: Option<String>,
    modification_date: Option<String>,
}

/// The trailer's `/ID` pair, and whether the file is the generation that
/// first wrote it.
#[derive(Serialize)]
struct Identity {
    permanent: String,
    revision: String,
    pristine: bool,
}

#[derive(Serialize)]
struct Signature {
    sub_filter: Option<String>,
    reason: Option<String>,
    time: Option<String>,
    byte_range: Vec<i64>,
    doc_mdp_permission: u32,
}

#[derive(Serialize)]
struct PageBoxes {
    page: u32,
    width: f64,
    height: f64,
    rotation: u32,
    media_box: JsonRect,
    crop_box: JsonRect,
    #[serde(skip_serializing_if = "Option::is_none")]
    bleed_box: Option<JsonRect>,
    #[serde(skip_serializing_if = "Option::is_none")]
    trim_box: Option<JsonRect>,
    #[serde(skip_serializing_if = "Option::is_none")]
    art_box: Option<JsonRect>,
}

pub fn run(file: &Path, password: Option<&str>, json: bool, term: Term) -> Result<ExitCode> {
    let doc = out::open(file, password)?;
    let report = report(&doc, file);
    if json {
        out::json(&report)?;
    } else {
        print(&report, term);
    }
    Ok(ExitCode::SUCCESS)
}

fn report(doc: &Document, file: &Path) -> Report {
    let p = doc.permissions();
    let m = doc.metadata();
    let page_boxes = doc
        .pages()
        .map(|page| PageBoxes {
            page: out::page_number(page.index()),
            width: page.width(),
            height: page.height(),
            rotation: rotation_degrees(page.rotation()),
            media_box: page.media_box().into(),
            crop_box: page.crop_box().into(),
            bleed_box: page.bleed_box().map(Into::into),
            trim_box: page.trim_box().map(Into::into),
            art_box: page.art_box().map(Into::into),
        })
        .collect();
    Report {
        file: file.display().to_string(),
        version: doc.version().map(|v| v.to_string()),
        pages: doc.page_count(),
        encrypted: doc.is_encrypted(),
        xref_rebuilt: doc.xref_was_rebuilt(),
        permissions: Permissions {
            print: p.print,
            modify: p.modify,
            copy: p.copy,
            annotate: p.annotate,
            fill_form: p.fill_form,
        },
        metadata: Metadata {
            title: m.title,
            author: m.author,
            subject: m.subject,
            keywords: m.keywords,
            creator: m.creator,
            producer: m.producer,
            creation_date: m.creation_date,
            modification_date: m.modification_date,
        },
        id: doc.id().map(|[permanent, revision]| Identity {
            pristine: permanent == revision,
            permanent: out::hex(&permanent),
            revision: out::hex(&revision),
        }),
        outline_entries: doc.outline().len(),
        attachments: doc.attachments().len(),
        signatures: doc
            .signatures()
            .iter()
            .map(|s| Signature {
                sub_filter: s.sub_filter(),
                reason: s.reason(),
                time: s.time(),
                byte_range: s.byte_range(),
                doc_mdp_permission: s.doc_mdp_permission(),
            })
            .collect(),
        page_boxes,
    }
}

fn rotation_degrees(rotation: Rotation) -> u32 {
    rotation.degrees()
}

fn print(r: &Report, term: Term) {
    let mut rows: Vec<(&str, Option<String>)> = Vec::new();
    let mut row = |key: &'static str, value: Option<String>| rows.push((key, value));
    row("file", Some(r.file.clone()));
    row("version", r.version.clone());
    row("pages", Some(r.pages.to_string()));
    if let Some(first) = r.page_boxes.first() {
        let mut size = format!("{:.2} x {:.2} pt", first.width, first.height);
        if first.rotation != 0 {
            let _ = write!(size, ", rotated {}", first.rotation);
        }
        if r.page_boxes
            .iter()
            .any(|p| (p.width, p.height) != (first.width, first.height))
        {
            size.push_str(" (first page; sizes vary)");
        }
        row("page size", Some(size));
    }
    row(
        "security",
        r.encrypted.then(|| {
            let allowed = [
                ("print", r.permissions.print),
                ("modify", r.permissions.modify),
                ("copy", r.permissions.copy),
                ("annotate", r.permissions.annotate),
                ("fill forms", r.permissions.fill_form),
            ]
            .iter()
            .map(|(name, ok)| {
                format!(
                    "{name} {}",
                    if *ok {
                        term.paint(Style::Ok, "allowed")
                    } else {
                        term.paint(Style::Warn, "denied")
                    }
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
            format!("encrypted; {allowed}")
        }),
    );
    if r.xref_rebuilt {
        row(
            "structure",
            Some(term.paint(Style::Warn, "cross-reference table rebuilt on open")),
        );
    }
    for (key, value) in [
        ("title", &r.metadata.title),
        ("author", &r.metadata.author),
        ("subject", &r.metadata.subject),
        ("keywords", &r.metadata.keywords),
        ("creator", &r.metadata.creator),
        ("producer", &r.metadata.producer),
        ("created", &r.metadata.creation_date),
        ("modified", &r.metadata.modification_date),
    ] {
        if let Some(value) = value {
            row(key, Some(value.clone()));
        }
    }
    match &r.id {
        Some(id) => {
            row("id", Some(id.permanent.clone()));
            row(
                "identity",
                Some(
                    if id.pristine {
                        "original generation (both /ID elements equal)"
                    } else {
                        "re-saved since first written (/ID elements differ)"
                    }
                    .to_owned(),
                ),
            );
        }
        None => row("id", None),
    }
    if r.outline_entries > 0 {
        row("outline", Some(format!("{} entries", r.outline_entries)));
    }
    if r.attachments > 0 {
        row("attachments", Some(r.attachments.to_string()));
    }
    let signatures = signature_lines(r);
    if !signatures.is_empty() {
        row("signatures", Some(signatures.join("\n")));
    }
    out::record(term, &rows);
}

/// One line per signature: sub-filter, time, reason, `DocMDP`.
fn signature_lines(r: &Report) -> Vec<String> {
    r.signatures
        .iter()
        .map(|s| {
            let mut parts = Vec::new();
            if let Some(f) = &s.sub_filter {
                parts.push(f.clone());
            }
            if let Some(t) = &s.time {
                parts.push(format!("signed {t}"));
            }
            if let Some(reason) = &s.reason {
                parts.push(format!("reason {reason}"));
            }
            if s.doc_mdp_permission != 0 {
                parts.push(format!("DocMDP P={}", s.doc_mdp_permission));
            }
            parts.join("; ")
        })
        .collect()
}
