//! List a PDF form's fields, fill one, and save the result.
//!
//! ```text
//! # list what the form has
//! cargo run --example fill-form-and-save -- tests/fixtures/text_form.pdf
//!
//! # fill a field and write a new file
//! cargo run --example fill-form-and-save -- \
//!     tests/fixtures/text_form.pdf out.pdf "Text Box" "Hello, form"
//! ```
//!
//! The shape to notice: the document is never mutable. Values are buffered on
//! the `Form` and turned into object replacements only at save time, so the
//! `Document` stays shareable — and immutable — the whole way through.

use std::path::Path;
use std::process::ExitCode;

use pdfrum::{Document, FieldKind, SaveOptions};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(input) = args.first() else {
        eprintln!("usage: fill-form-and-save <input.pdf> [output.pdf <field> <value>]");
        return ExitCode::FAILURE;
    };
    let fill = match (args.get(1), args.get(2), args.get(3)) {
        (Some(out), Some(field), Some(value)) => {
            Some((out.as_str(), field.as_str(), value.as_str()))
        }
        _ => None,
    };

    match run(Path::new(input), fill) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}

fn run(input: &Path, fill: Option<(&str, &str, &str)>) -> Result<(), pdfrum::Error> {
    let doc = Document::open(input)?;
    let Some(mut form) = doc.form() else {
        println!("this document has no interactive form");
        return Ok(());
    };

    println!("{} field(s):", form.field_count());
    for field in form.fields() {
        let kind = match field.kind() {
            FieldKind::Text => "text",
            FieldKind::Check => "checkbox",
            FieldKind::Radio => "radio",
            FieldKind::Button => "button",
            FieldKind::Combo => "combo",
            FieldKind::List => "list",
            FieldKind::Signature => "signature",
        };
        print!("  {:<24} {kind:<10} = {:?}", field.name(), field.value());
        if field.is_read_only() {
            print!("  [read-only]");
        }
        if field.is_required() {
            print!("  [required]");
        }
        // A toggle's value is a state name, so show which names it accepts.
        let states = field.states();
        if !states.is_empty() {
            print!("  states: {}", states.join(", "));
        }
        // A choice field's selectable labels.
        let options = field.options();
        if !options.is_empty() {
            print!("  options: {}", options.join(", "));
        }
        println!();
    }

    let Some((out, name, value)) = fill else {
        return Ok(());
    };

    if let Err(err) = form.set(name, value) {
        eprintln!("warning: {err}; saving anyway");
    }

    // The value reads back before anything is written, because the form
    // consults its own edit buffer.
    if let Some(field) = form.field(name) {
        println!(
            "\n{name:?} is now {:?} (file still holds {:?})",
            field.value(),
            field.stored_value()
        );
    }

    doc.save_form(out, &form, &SaveOptions::default())?;
    println!("wrote {out}");

    // Prove it round-trips: reopen and read the value back out of the file.
    let saved = Document::open(out)?;
    if let Some(field) = saved.form().and_then(|f| f.field(name)) {
        println!("reopened: {name:?} = {:?}", field.value());
    }
    Ok(())
}
