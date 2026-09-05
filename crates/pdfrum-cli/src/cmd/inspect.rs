//! `pdfrum inspect …`: the file's insides — objects, the cross-reference
//! table, revisions, the structure tree.

use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use pdfrum::{Kid, ObjRef, Object, Resolve, StructTree};
use serde::Serialize;

use crate::out::{Align, Table, outln};
use crate::term::{Style, Term};
use crate::{out, syntax};

// ---- object ---------------------------------------------------------------

/// One indirect object, printed in PDF syntax. `--decode` writes a
/// stream's decoded data to stdout instead.
pub fn object(
    file: &Path,
    password: Option<&str>,
    num: u32,
    generation: u16,
    decode: bool,
) -> Result<ExitCode> {
    let doc = out::open(file, password)?;
    let reference = ObjRef::new(num, generation);
    let object = doc
        .fetch(reference)
        .with_context(|| format!("object {num} {generation} is not in the document"))?;
    if decode {
        let Object::Stream(_) = &*object else {
            bail!("object {num} {generation} is not a stream; nothing to decode");
        };
        let data = doc.stream_data(reference)?;
        out::write_bytes(&data);
        return Ok(ExitCode::SUCCESS);
    }
    let mut text = format!("{num} {generation} obj\n");
    syntax::object(&mut text, &object, 0);
    text.push_str("\nendobj");
    outln!("{text}");
    Ok(ExitCode::SUCCESS)
}

// ---- xref -----------------------------------------------------------------

#[derive(Serialize)]
struct XrefRow {
    object: u32,
    generation: u16,
    /// `offset`, `in_stream` or `free`.
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    offset: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    index: Option<u32>,
}

#[derive(Serialize)]
struct XrefReport {
    file: String,
    rebuilt: bool,
    entries: usize,
    trailer: String,
    rows: Vec<XrefRow>,
}

pub fn xref(file: &Path, password: Option<&str>, json: bool, term: Term) -> Result<ExitCode> {
    let doc = out::open_quietly(file, password)?;
    let parser = doc.parser();
    let table = parser.xref();
    let mut rows = Vec::with_capacity(table.len());
    for num in table.object_numbers() {
        let generation = table.generation(num);
        let row = match table.entry(num) {
            Some(pdfrum::XrefEntry::Offset(offset)) => XrefRow {
                object: num,
                generation,
                kind: "offset",
                offset: Some(offset),
                stream: None,
                index: None,
            },
            Some(pdfrum::XrefEntry::InObjStream { stream, index }) => XrefRow {
                object: num,
                generation,
                kind: "in_stream",
                offset: None,
                stream: Some(stream.num),
                index: Some(index),
            },
            _ => XrefRow {
                object: num,
                generation,
                kind: "free",
                offset: None,
                stream: None,
                index: None,
            },
        };
        rows.push(row);
    }
    let mut trailer = String::new();
    syntax::object(
        &mut trailer,
        &Object::Dict(syntax::effective(parser.trailer())),
        0,
    );
    let report = XrefReport {
        file: file.display().to_string(),
        rebuilt: doc.xref_was_rebuilt(),
        entries: rows.len(),
        trailer,
        rows,
    };
    if json {
        out::json(&report)?;
    } else {
        out::record(
            term,
            &[
                ("file", Some(report.file.clone())),
                ("objects", Some(report.entries.to_string())),
                (
                    "xref",
                    Some(if report.rebuilt {
                        term.paint(Style::Warn, "rebuilt by scanning the file")
                    } else {
                        "as written".to_owned()
                    }),
                ),
            ],
        );
        out::heading(term, "trailer");
        for line in report.trailer.lines() {
            outln!("  {line}");
        }
        let mut table = Table::new(&[
            ("OBJ", Align::Right),
            ("GEN", Align::Right),
            ("PLACE", Align::Left),
        ]);
        for r in &report.rows {
            let place = match (r.offset, r.stream, r.index) {
                (Some(o), _, _) => format!("@{o}"),
                (_, Some(s), Some(i)) => format!("in {s} 0 R [{i}]"),
                _ => "free".to_owned(),
            };
            table.row(vec![
                term.paint(Style::Ident, &r.object.to_string()),
                r.generation.to_string(),
                place,
            ]);
        }
        table.print(term, 0);
    }
    Ok(ExitCode::SUCCESS)
}

// ---- revisions ------------------------------------------------------------

#[derive(Serialize)]
struct RevisionRow {
    revision: usize,
    xref_offset: u64,
    xref_stream: bool,
    /// The byte after this revision's `%%EOF`: its length as a file.
    end: usize,
}

pub fn revisions(file: &Path, password: Option<&str>, json: bool, term: Term) -> Result<ExitCode> {
    let doc = out::open(file, password)?;
    let rows: Vec<RevisionRow> = doc
        .revisions()
        .iter()
        .map(|r| RevisionRow {
            revision: r.index + 1,
            xref_offset: r.xref_offset,
            xref_stream: r.is_stream,
            end: r.end,
        })
        .collect();
    if json {
        out::json(&rows)?;
    } else if rows.is_empty() {
        out::none("revisions");
    } else {
        let mut table = Table::new(&[
            ("REV", Align::Right),
            ("XREF", Align::Right),
            ("KIND", Align::Left),
            ("SIZE", Align::Right),
        ]);
        for r in &rows {
            table.row(vec![
                term.paint(Style::Ident, &r.revision.to_string()),
                format!("@{}", r.xref_offset),
                if r.xref_stream { "stream" } else { "table" }.to_owned(),
                out::bytes(r.end as u64),
            ]);
        }
        table.print(term, 0);
    }
    Ok(ExitCode::SUCCESS)
}

/// Write the file as it stood at revision `rev` (1-based).
pub fn revision(
    file: &Path,
    password: Option<&str>,
    rev: usize,
    output: &Path,
    term: Term,
) -> Result<ExitCode> {
    let doc = out::open(file, password)?;
    let count = doc.revisions().len();
    if rev == 0 || rev > count {
        bail!(
            "revision {rev} is not in 1..={count}{}",
            if count == 0 {
                " (no revision chain)"
            } else {
                ""
            }
        );
    }
    let bytes = doc
        .revision_bytes(rev - 1)
        .context("the revision's end could not be found")?;
    std::fs::write(output, bytes).with_context(|| format!("cannot write {}", output.display()))?;
    out::summary(
        term,
        output,
        &format!("revision {rev} of {count}"),
        Some(&out::bytes(bytes.len() as u64)),
    );
    Ok(ExitCode::SUCCESS)
}

// ---- structure ------------------------------------------------------------

#[derive(Serialize)]
struct StructRow {
    page: u32,
    depth: usize,
    kind: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    content_ids: Vec<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    alt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    actual_text: Option<String>,
}

pub fn structure(
    file: &Path,
    password: Option<&str>,
    spec: Option<&str>,
    json: bool,
    term: Term,
) -> Result<ExitCode> {
    let doc = out::open(file, password)?;
    let mut rows = Vec::new();
    let mut tagged = false;
    for index in crate::pages::select(spec, doc.page_count())? {
        let page = doc.page(index)?;
        let Some(tree) = page.structure() else {
            continue;
        };
        tagged = true;
        let number = out::page_number(page.index());
        for (i, element) in tree.elements.iter().enumerate() {
            if element.parent.is_none() {
                walk(&tree, i, 0, number, &doc, &mut rows);
            }
        }
    }
    if json {
        out::json(&rows)?;
    } else if !tagged {
        out::none("structure tree");
    } else {
        let columns = [
            ("ELEMENT", Align::Left),
            ("MCID", Align::Left),
            ("ALT", Align::Left),
            ("TEXT", Align::Left),
        ];
        let mut last_page = 0;
        let mut table = Table::new(&columns);
        for r in &rows {
            if r.page != last_page {
                if !table.is_empty() {
                    table.print(term, 2);
                    table = Table::new(&columns);
                }
                out::heading(term, &format!("page {}", r.page));
                last_page = r.page;
            }
            table.row(vec![
                format!("{}{}", "  ".repeat(r.depth), r.kind),
                r.content_ids
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(","),
                r.alt.clone().unwrap_or_default(),
                r.actual_text.clone().unwrap_or_default(),
            ]);
        }
        if !table.is_empty() {
            table.print(term, 2);
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn walk(
    tree: &StructTree,
    index: usize,
    depth: usize,
    page: u32,
    doc: &pdfrum::Document,
    rows: &mut Vec<StructRow>,
) {
    let Some(element) = tree.elements.get(index) else {
        return;
    };
    let content_ids = element
        .kids
        .iter()
        .filter_map(|k| match k {
            Kid::PageContent { content_id } | Kid::StreamContent { content_id, .. } => {
                Some(*content_id)
            }
            _ => None,
        })
        .collect();
    let alt = element.alt_text(doc);
    let actual = element.actual_text(doc);
    rows.push(StructRow {
        page,
        depth,
        kind: String::from_utf8_lossy(&element.kind).into_owned(),
        content_ids,
        alt: (!alt.is_empty()).then_some(alt),
        actual_text: (!actual.is_empty()).then_some(actual),
    });
    for kid in &element.kids {
        if let Kid::Element {
            linked: Some(i), ..
        } = kid
        {
            walk(tree, *i, depth + 1, page, doc, rows);
        }
    }
}
