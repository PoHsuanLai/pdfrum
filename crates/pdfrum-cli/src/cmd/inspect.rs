//! `pdfrum inspect …`: the file's insides — objects, the cross-reference
//! table, revisions, the structure tree.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use pdfrum::{Dict, Document, Kid, ObjRef, Object, Resolve, StructTree};
use serde::Serialize;

use crate::out::{Align, Table, out, outln};
use crate::term::{Style, Term};
use crate::{out, syntax};

// ---- object ---------------------------------------------------------------

/// `inspect object --json`: the object in [`syntax::json`]'s encoding,
/// with the same hints the text form prints after each top-level
/// reference, keyed by the dictionary key.
#[derive(Serialize)]
pub struct ObjectReport {
    object: u32,
    generation: u16,
    value: serde_json::Value,
    hints: BTreeMap<String, String>,
}

/// Object `num generation` as JSON, with its hints.
pub fn object_report(doc: &Document, num: u32, generation: u16) -> Result<ObjectReport> {
    let object = doc
        .fetch(ObjRef::new(num, generation))
        .with_context(|| format!("object {num} {generation} is not in the document"))?;
    let entries = match &*object {
        Object::Dict(d) => Some(d),
        Object::Stream(s) => Some(&s.dict),
        _ => None,
    };
    let hints = entries
        .into_iter()
        .flat_map(Dict::iter)
        .filter_map(|(key, value)| match value {
            Object::Ref(r) => {
                let target = doc.fetch(*r).ok()?;
                Some((key.as_text().into_owned(), syntax::describe(&target)))
            }
            _ => None,
        })
        .collect();
    Ok(ObjectReport {
        object: num,
        generation,
        value: syntax::json(&object),
        hints,
    })
}

/// What `inspect object` does with the object: prints it in PDF syntax
/// for a person, as JSON, or — for a stream — writes its decoded data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectForm {
    Syntax,
    Json,
    Decode,
}

/// One indirect object, printed in PDF syntax. `--decode` writes a
/// stream's decoded data to stdout instead.
pub fn object(
    file: &Path,
    password: Option<&str>,
    num: u32,
    generation: u16,
    form: ObjectForm,
    term: Term,
) -> Result<ExitCode> {
    let doc = out::open(file, password)?;
    let reference = ObjRef::new(num, generation);
    let object = doc
        .fetch(reference)
        .with_context(|| format!("object {num} {generation} is not in the document"))?;
    match form {
        ObjectForm::Decode => {
            let Object::Stream(_) = &*object else {
                bail!("object {num} {generation} is not a stream; nothing to decode");
            };
            let data = doc.stream_data(reference)?;
            out::write_bytes(&data);
        }
        ObjectForm::Json => out::json(&object_report(&doc, num, generation)?)?,
        ObjectForm::Syntax => {
            let mut text = format!("{num} {generation} obj\n");
            syntax::object(&mut text, &object, 0);
            text.push_str("\nendobj");
            outln!("{}", syntax::highlight(&with_hints(&text, &doc), term));
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// The dump with a `% …` hint after each top-level entry whose value is a
/// reference — what the object it points at is — so `/Pages 2 0 R` reads
/// `/Pages 2 0 R  % Pages` without a second command.
fn with_hints(text: &str, doc: &pdfrum::Document) -> String {
    let mut out = String::with_capacity(text.len() + 64);
    for (i, line) in text.lines().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(line);
        // Exactly one level in, `/Key N G R`.
        let entry = line.strip_prefix("  ").filter(|l| !l.starts_with(' '));
        if let Some(entry) = entry
            && let Some(hint) = reference_hint(entry, doc)
        {
            out.push_str("  % ");
            out.push_str(&hint);
        }
    }
    out
}

fn reference_hint(entry: &str, doc: &pdfrum::Document) -> Option<String> {
    let mut parts = entry.split(' ');
    let key = parts.next()?;
    if !key.starts_with('/') {
        return None;
    }
    let num: u32 = parts.next()?.parse().ok()?;
    let generation: u16 = parts.next()?.parse().ok()?;
    if parts.next()? != "R" || parts.next().is_some() {
        return None;
    }
    let target = doc.fetch(ObjRef::new(num, generation)).ok()?;
    Some(syntax::describe(&target))
}

// ---- xref -----------------------------------------------------------------

#[derive(Serialize)]
struct XrefRow {
    object: u32,
    generation: u16,
    /// `offset`, `in_stream` or `free`.
    kind: &'static str,
    /// What the object is: its `/Type`, a stream's filter and size, an
    /// array's length, or the scalar.
    #[serde(skip_serializing_if = "Option::is_none")]
    what: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    offset: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    index: Option<u32>,
}

#[derive(Serialize)]
pub struct XrefReport {
    file: String,
    rebuilt: bool,
    entries: usize,
    trailer: String,
    rows: Vec<XrefRow>,
}

/// The cross-reference table as the parser holds it, with the trailer.
pub fn xref_report(doc: &Document, file: &Path) -> XrefReport {
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
                what: what(doc, num, generation),
                offset: Some(offset),
                stream: None,
                index: None,
            },
            Some(pdfrum::XrefEntry::InObjStream { stream, index }) => XrefRow {
                object: num,
                generation,
                kind: "in_stream",
                what: what(doc, num, generation),
                offset: None,
                stream: Some(stream.num),
                index: Some(index),
            },
            _ => XrefRow {
                object: num,
                generation,
                kind: "free",
                what: None,
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
    XrefReport {
        file: file.display().to_string(),
        rebuilt: doc.xref_was_rebuilt(),
        entries: rows.len(),
        trailer,
        rows,
    }
}

pub fn xref(file: &Path, password: Option<&str>, json: out::Json, term: Term) -> Result<ExitCode> {
    let doc = out::open_quietly(file, password)?;
    let report = xref_report(&doc, file);
    if json == out::Json::Document {
        out::json(&report)?;
    } else if json == out::Json::Lines {
        // The entries, one per line; the trailer is `inspect object`'s.
        out::items(&report.rows, json)?;
    } else {
        let mut rows = vec![
            ("file", Some(report.file.clone())),
            ("objects", Some(report.entries.to_string())),
        ];
        // Only worth a line when the file's own table could not be used.
        if report.rebuilt {
            rows.push((
                "table",
                Some(term.paint(Style::Warn, "rebuilt by scanning the file")),
            ));
        }
        out::record(term, &rows);
        out::heading(term, "trailer");
        let mut trailer = String::new();
        for line in with_hints(&report.trailer, &doc).lines() {
            let _ = writeln!(trailer, "  {line}");
        }
        out!("{}", syntax::highlight(&trailer, term));
        let mut table = Table::new(&[
            ("OBJECT", Align::Right),
            ("GENERATION", Align::Right),
            ("WHERE", Align::Left),
            ("WHAT", Align::Left),
        ]);
        for r in &report.rows {
            let place = match (r.offset, r.stream, r.index) {
                (Some(o), _, _) => format!("byte {o}"),
                (_, Some(s), Some(i)) => format!("in object {s}, item {i}"),
                _ => "free".to_owned(),
            };
            table.row(vec![
                term.paint(Style::Ident, &r.object.to_string()),
                // Nearly always 0, and then the column goes.
                if r.generation == 0 {
                    String::new()
                } else {
                    r.generation.to_string()
                },
                place,
                r.what.clone().unwrap_or_default(),
            ]);
        }
        table.print(term, 0);
    }
    Ok(ExitCode::SUCCESS)
}

/// [`syntax::describe`] of one object, or nothing when it cannot be read.
fn what(doc: &pdfrum::Document, num: u32, generation: u16) -> Option<String> {
    doc.fetch(ObjRef::new(num, generation))
        .ok()
        .map(|o| syntax::describe(&o))
}

// ---- revisions ------------------------------------------------------------

#[derive(Serialize)]
pub struct RevisionRow {
    revision: usize,
    xref_offset: u64,
    xref_stream: bool,
    /// The byte after this revision's `%%EOF`: its length as a file.
    end: usize,
}

/// The incremental-update history, one row per saved revision.
pub fn revision_rows(doc: &Document) -> Vec<RevisionRow> {
    doc.revisions()
        .iter()
        .map(|r| RevisionRow {
            revision: r.index + 1,
            xref_offset: r.xref_offset,
            xref_stream: r.is_stream,
            end: r.end,
        })
        .collect()
}

pub fn revisions(
    file: &Path,
    password: Option<&str>,
    json: out::Json,
    term: Term,
) -> Result<ExitCode> {
    let doc = out::open(file, password)?;
    let rows = revision_rows(&doc);
    if json.is_on() {
        out::items(&rows, json)?;
    } else if rows.is_empty() {
        out::none("revisions");
    } else {
        let mut table = Table::new(&[
            ("REVISION", Align::Right),
            ("TABLE AT", Align::Right),
            ("FORMAT", Align::Left),
            ("SIZE", Align::Right),
        ]);
        for r in &rows {
            table.row(vec![
                term.paint(Style::Ident, &r.revision.to_string()),
                format!("byte {}", r.xref_offset),
                if r.xref_stream {
                    "stream"
                } else {
                    "classic table"
                }
                .to_owned(),
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
    let sink = out::Sink::new(output, "PDF")?;
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
    sink.finish(
        term,
        bytes,
        &format!("revision {rev} of {count}"),
        Some(&out::bytes(bytes.len() as u64)),
    )?;
    Ok(ExitCode::SUCCESS)
}

// ---- structure ------------------------------------------------------------

#[derive(Serialize)]
pub struct StructRow {
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

/// The structure tree of the selected pages, one row per element in
/// depth-first order, and whether any page had one at all.
pub fn structure_rows(doc: &Document, spec: Option<&str>) -> Result<(Vec<StructRow>, bool)> {
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
                walk(&tree, i, 0, number, doc, &mut rows);
            }
        }
    }
    Ok((rows, tagged))
}

pub fn structure(
    file: &Path,
    password: Option<&str>,
    spec: Option<&str>,
    json: out::Json,
    term: Term,
) -> Result<ExitCode> {
    let doc = out::open(file, password)?;
    let (rows, tagged) = structure_rows(&doc, spec)?;
    if json.is_on() {
        out::items(&rows, json)?;
    } else if !tagged {
        out::none("structure tree");
    } else {
        let columns = [
            ("ELEMENT", Align::Left),
            ("CONTENT ID", Align::Left),
            ("ALT TEXT", Align::Left),
            ("ACTUAL TEXT", Align::Left),
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
