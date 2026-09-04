//! `pdfrum hash`: three fingerprints of a document.
//!
//! * `file` — SHA-256 of the bytes, what `sha256sum` prints.
//! * `id` — the trailer's `/ID`, the identity the producer gave it.
//! * `semantic` — SHA-256 over every object in PDF syntax, in object-number
//!   order, with the volatile parts masked: `/ID`, the `/Info` dates and
//!   the XMP `/Metadata` stream. Two saves of the same content that differ
//!   only in timestamps, object order on disk or whitespace hash the same.

use std::fmt::Write;
use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context, Result};
use pdfrum::{Dict, Name, Object, Resolve};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::out::outln;
use crate::{out, syntax};

#[derive(Serialize)]
struct Report {
    file: String,
    sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<[String; 2]>,
    semantic: String,
    objects: usize,
}

pub fn run(file: &Path, password: Option<&str>, json: bool) -> Result<ExitCode> {
    let bytes = std::fs::read(file).with_context(|| format!("cannot read {}", file.display()))?;
    let sha256 = hex(&Sha256::digest(&bytes));
    let doc = out::open_quietly(file, password)?;
    let id = doc.id().map(|[a, b]| [hex(&a), hex(&b)]);
    let (semantic, objects) = semantic(&doc);
    let report = Report {
        file: file.display().to_string(),
        sha256,
        id,
        semantic,
        objects,
    };
    if json {
        out::json(&report)?;
    } else {
        outln!("file      {}", report.sha256);
        match &report.id {
            Some([a, b]) if a == b => outln!("id        {a}"),
            Some([a, b]) => outln!("id        {a} (created)\n          {b} (this revision)"),
            None => outln!("id        none"),
        }
        outln!(
            "semantic  {}  ({} objects)",
            report.semantic,
            report.objects
        );
    }
    Ok(ExitCode::SUCCESS)
}

/// The semantic digest and how many objects went into it.
fn semantic(doc: &pdfrum::Document) -> (String, usize) {
    let parser = doc.parser();
    let table = parser.xref();
    let info = parser.trailer().raw(&Name::from("Info")).and_then(as_ref);
    let metadata = parser
        .trailer()
        .raw(&Name::from("Root"))
        .and_then(as_ref)
        .and_then(|root| doc.fetch(root).ok())
        .and_then(|root| match &*root {
            Object::Dict(catalog) => catalog.raw(&Name::from("Metadata")).and_then(as_ref),
            _ => None,
        });
    let mut hasher = Sha256::new();
    let mut text = String::new();
    let mut count = 0;
    for num in table.object_numbers() {
        let reference = pdfrum::ObjRef::new(num, table.generation(num));
        let Ok(object) = doc.fetch(reference) else {
            continue;
        };
        if Some(reference) == metadata {
            // XMP carries the same dates and the same instance id as /Info.
            continue;
        }
        text.clear();
        let _ = writeln!(text, "{num} {} obj", reference.generation);
        if Some(reference) == info {
            if let Object::Dict(d) = &*object {
                syntax::object(&mut text, &Object::Dict(without_dates(d)), 0);
            }
        } else {
            syntax::object(&mut text, &object, 0);
        }
        hasher.update(text.as_bytes());
        hasher.update(b"\n");
        count += 1;
        if let Object::Stream(_) = &*object {
            // The data too: the dictionary alone would let two pages of
            // different words hash the same.
            if let Ok(data) = doc.stream_data(reference) {
                hasher.update(&data);
            }
        }
    }
    let mut trailer = syntax::effective(parser.trailer());
    trailer.remove(&Name::from("ID"));
    trailer.remove(&Name::from("Prev"));
    trailer.remove(&Name::from("XRefStm"));
    trailer.remove(&Name::from("Size"));
    text.clear();
    text.push_str("trailer\n");
    syntax::object(&mut text, &Object::Dict(trailer), 0);
    hasher.update(text.as_bytes());
    (hex(&hasher.finalize()), count)
}

fn as_ref(object: &Object) -> Option<pdfrum::ObjRef> {
    match object {
        Object::Ref(reference) => Some(*reference),
        _ => None,
    }
}

fn without_dates(info: &Dict) -> Dict {
    let mut d = info.clone();
    d.remove(&Name::from("CreationDate"));
    d.remove(&Name::from("ModDate"));
    d
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}
