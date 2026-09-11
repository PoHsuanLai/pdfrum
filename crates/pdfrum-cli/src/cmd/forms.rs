//! `pdfrum forms …`: dump, fill, flatten.

use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use pdfrum::{Document, FlattenMode, Flattened};
use serde::Serialize;

use crate::cmd::pages::save_options;
use crate::out;
use crate::out::{Align, Table};
use crate::term::{Style, Term};
#[cfg(feature = "javascript")]
use pdfrum::{Diagnostics, FormSession, ScriptConfig};

#[derive(Serialize)]
pub struct FieldRow {
    name: String,
    kind: String,
    value: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    checked: Option<bool>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    options: Vec<String>,
    read_only: bool,
    required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    tooltip: Option<String>,
    widgets: usize,
}

/// The form's fields with their kinds and values; none when the document
/// has no interactive form.
pub fn field_rows(doc: &Document) -> Vec<FieldRow> {
    let Some(form) = doc.form() else {
        return Vec::new();
    };
    form.fields()
        .map(|f| {
            let kind = format!("{:?}", f.kind()).to_ascii_lowercase();
            let checkable = matches!(
                kind.as_str(),
                "check_box" | "checkbox" | "radio_button" | "radio"
            );
            FieldRow {
                name: f.name().to_owned(),
                checked: checkable.then(|| f.is_checked()),
                kind,
                value: f.value(),
                options: f.options(),
                read_only: f.is_read_only(),
                required: f.is_required(),
                tooltip: f.tooltip(),
                widgets: f.widget_count(),
            }
        })
        .collect()
}

pub fn dump(file: &Path, password: Option<&str>, json: out::Json, term: Term) -> Result<ExitCode> {
    let doc = out::open(file, password)?;
    let rows = field_rows(&doc);
    if json.is_on() {
        out::items(&rows, json)?;
    } else if rows.is_empty() {
        out::none("form fields");
    } else {
        let mut table = Table::new(&[
            ("NAME", Align::Left),
            ("KIND", Align::Left),
            ("VALUE", Align::Left),
            ("OPTIONS", Align::Left),
            ("FLAGS", Align::Left),
        ]);
        for r in &rows {
            let mut flags = Vec::new();
            if r.read_only {
                flags.push("read-only");
            }
            if r.required {
                flags.push("required");
            }
            let value = match r.checked {
                Some(true) => "[x]".to_owned(),
                Some(false) => "[ ]".to_owned(),
                None => r.value.clone(),
            };
            table.row(vec![
                term.paint(Style::Ident, &r.name),
                r.kind.clone(),
                value,
                r.options.join(" | "),
                flags.join(", "),
            ]);
        }
        table.print(term, 0);
    }
    Ok(ExitCode::SUCCESS)
}

/// Fill fields from a JSON object: `{"name": "text", "box": true}`. A
/// boolean checks or clears; anything else is set as the field's text.
pub fn fill(
    file: &Path,
    password: Option<&str>,
    data: &Path,
    output: &Path,
    deterministic: bool,
    #[cfg(feature = "javascript")] scripts: bool,
    term: Term,
) -> Result<ExitCode> {
    let sink = out::Sink::new(output, "PDF")?;
    let doc = out::open(file, password)?;
    let text =
        std::fs::read_to_string(data).with_context(|| format!("cannot read {}", data.display()))?;
    let values: serde_json::Map<String, serde_json::Value> = serde_json::from_str(&text)
        .with_context(|| format!("{} is not a JSON object of field values", data.display()))?;
    let (bytes, set) = fill_values(&doc, &file.display().to_string(), &values, deterministic)?;
    let what = format!("{set} field{} set", if set == 1 { "" } else { "s" });
    #[cfg(feature = "javascript")]
    if scripts {
        let (bytes, by_scripts) = apply_open_scripts(bytes, password, deterministic)?;
        sink.finish(
            term,
            &bytes,
            &what,
            Some(&format!("{by_scripts} by scripts")),
        )?;
        return Ok(ExitCode::SUCCESS);
    }
    sink.finish(term, &bytes, &what, None)?;
    Ok(ExitCode::SUCCESS)
}

/// The document with `values` set on its fields, as the bytes of a saved
/// file, and how many fields were set. A boolean checks or clears; a
/// string is the field's text; `null` clears it; anything else is set as
/// its JSON text. `name` is what the document is called in the error when
/// it has no form.
pub fn fill_values(
    doc: &Document,
    name: &str,
    values: &serde_json::Map<String, serde_json::Value>,
    deterministic: bool,
) -> Result<(Vec<u8>, usize)> {
    let Some(mut form) = doc.form() else {
        bail!("{name} has no interactive form");
    };
    let mut set = 0;
    for (field, value) in values {
        let result = match value {
            serde_json::Value::Bool(checked) => form.set_checked(field, *checked),
            serde_json::Value::String(s) => form.set(field, s.clone()),
            serde_json::Value::Null => form.set(field, ""),
            other => form.set(field, other.to_string()),
        };
        result.with_context(|| format!("no field named {field:?}"))?;
        set += 1;
    }
    let mut bytes = Vec::new();
    doc.write_form_to(&mut bytes, &form, &save_options(deterministic, doc.bytes()))?;
    Ok((bytes, set))
}

/// Open the saved bytes the way a viewer would — the document's open
/// scripts, then every page, whose formatters run on load — and write
/// back whatever the scripts assigned to fields. Scripts that threw are
/// reported on stderr. The count is how many fields the scripts changed;
/// the bytes come back untouched when it is zero.
#[cfg(feature = "javascript")]
fn apply_open_scripts(
    bytes: Vec<u8>,
    password: Option<&str>,
    deterministic: bool,
) -> Result<(Vec<u8>, usize)> {
    let saved = out::open_bytes(bytes, password).context("cannot reopen the filled form")?;
    let Some(mut form) = saved.form() else {
        return Ok((saved.bytes().to_vec(), 0));
    };
    let mut session = FormSession::with_scripts(&saved, &ScriptConfig::wall_clock())
        .context("cannot start the script engine for the filled form")?;
    session.open_document();
    for page in 0..saved.page_count() {
        session.load_page(page);
    }
    session.honour_focus_requests();
    let writes = session
        .scripts_mut()
        .map(pdfrum::ScriptCascade::drain_field_writes)
        .unwrap_or_default();
    for failure in session.script_failures(&mut Diagnostics::default()) {
        eprintln!("pdfrum: {}", failure.line());
    }
    let mut applied = 0;
    for (index, value) in writes {
        let Some(name) = form
            .fields()
            .nth(usize::try_from(index).unwrap_or(usize::MAX))
            .map(|f| f.name().to_owned())
        else {
            continue;
        };
        if form.set(&name, value).is_ok() {
            applied += 1;
        }
    }
    if applied == 0 {
        return Ok((saved.bytes().to_vec(), 0));
    }
    let mut bytes = Vec::new();
    saved.write_form_to(
        &mut bytes,
        &form,
        &save_options(deterministic, saved.bytes()),
    )?;
    Ok((bytes, applied))
}

/// Bake every page's annotations into its content.
pub fn flatten(
    file: &Path,
    password: Option<&str>,
    print: bool,
    output: &Path,
    deterministic: bool,
    term: Term,
) -> Result<ExitCode> {
    let sink = out::Sink::new(output, "PDF")?;
    let doc = out::open(file, password)?;
    let mode = if print {
        FlattenMode::Print
    } else {
        FlattenMode::Display
    };
    let mut edit = doc.edit();
    let mut flattened = 0;
    for index in 0..doc.page_count() {
        if edit.flatten(index, mode)? == Flattened::Done {
            flattened += 1;
        }
    }
    let mut bytes = Vec::new();
    edit.write_to(&mut bytes, &save_options(deterministic, doc.bytes()))?;
    sink.finish(
        term,
        &bytes,
        "flattened",
        Some(&format!(
            "{flattened} of {} pages had something to flatten",
            doc.page_count()
        )),
    )?;
    Ok(ExitCode::SUCCESS)
}
