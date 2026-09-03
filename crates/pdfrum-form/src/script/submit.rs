//! What `Doc.submitForm` hands back: an FDF file, or a URL-encoded query.
//!
//! **Nothing is sent.** The bytes go onto the transcript for a host to decide
//! about, which is the whole of this crate's position on the most dangerous
//! entry point in the object model.
//!
//! # Two shapes over one field walk
//!
//! `bFDF` picks between them, and they are not two formats of the same thing:
//! the URL-encoded form is produced by *re-reading* the FDF (upstream
//! literally parses its own output back), so a field the FDF walk skipped is
//! absent from both, and the ordering is the FDF's.

use boa_engine::Context;

use super::doc::SubmitRequest;
use super::model::{FieldModel, FieldModelKind};

/// Fields the FDF walk never carries, whatever was asked for.
///
/// A push button holds no value, and `/Ff` bit 3 (`NoExport`) is the
/// document's own instruction not to submit the field.
fn exportable(field: &FieldModel) -> bool {
    field.kind != FieldModelKind::Button && !field.flags.no_export
}

/// The bytes `submitForm` hands back for one request.
pub(super) fn serialize(request: &SubmitRequest, context: &mut Context) -> Vec<u8> {
    let selected = select(request, context);
    let fdf = write_fdf(&selected, &document_path(context));
    if request.fdf {
        return fdf;
    }
    url_encoded(&selected)
}

/// One field as the FDF walk keeps it: its fully-qualified name and its
/// value.
struct Submitted {
    name: String,
    value: String,
}

/// The fields a request selects, in the form's own `/Fields` order.
///
/// # Two paths, and they disagree about what "no names" means
///
/// `aFields` empty **and** `bEmpty` true is a shortcut: it submits the whole
/// form, every exportable field, valueless ones included. Any other request
/// walks the caller's names — and a request that names nothing then selects
/// nothing, because the walk is over `aFields` rather than over the form.
///
/// Along the named path a field with no value is dropped unless `bEmpty`, and
/// the FDF walk then drops a `/Ff` *required* field whose `/V` is empty
/// whichever path selected it.
fn select(request: &SubmitRequest, context: &mut Context) -> Vec<Submitted> {
    let Some(host) = super::bind::host(context) else {
        return Vec::new();
    };
    let state = host.borrow();
    let whole_form = request.fields.is_empty() && request.empty;
    let mut out = Vec::new();
    for field in &state.document.fields {
        if !exportable(field) {
            continue;
        }
        if !whole_form {
            if !request.fields.contains(&field.name) {
                continue;
            }
            if !request.empty && field.value.is_empty() {
                continue;
            }
        }
        // `ExportToFDF`'s own gate, applied on both paths: a required field
        // with an empty `/V` is never written.
        if field.flags.required && field.value.is_empty() {
            continue;
        }
        out.push(Submitted {
            name: field.name.clone(),
            value: field.value.clone(),
        });
    }
    out
}

/// The document's path as the FDF's `/F` filespec carries it.
///
/// **`Doc.URL`'s spelling, not `Doc.path`'s.** Both come from the same
/// `GetFilePath()`, and `Doc.path` is the one that prepends a separator
/// (`cjs_document.cpp`); `ExportToFDF` takes the raw
/// `form_fill_env_->GetFilePath()`, so the filespec reads `myfile.pdf` and
/// not `/myfile.pdf`. Two bytes, and they are in the golden's byte count.
fn document_path(context: &Context) -> String {
    super::bind::host(context)
        .map(|host| host.borrow().document.url.clone())
        .unwrap_or_default()
}

/// The FDF file, byte for byte as `CFDF_Document::WriteToString` writes one.
///
/// **Line endings are CRLF and the dictionaries have no whitespace at all** —
/// `<</T(name)/V(Tralfaz)>>`, not a pretty-printed one. Keys come out in the
/// order PDFium's `std::map` holds them, which is ASCII order over the key
/// name: `F` before `Fields`, and `F` before `Type` before `UF` inside the
/// filespec.
fn write_fdf(fields: &[Submitted], path: &str) -> Vec<u8> {
    let mut out = String::from("%FDF-1.2\r\n1 0 obj\r\n<</FDF<<");
    if !path.is_empty() {
        out.push_str("/F<</F(");
        out.push_str(&escape(path));
        out.push_str(")/Type/Filespec/UF(");
        out.push_str(&escape(path));
        out.push_str(")>>");
    }
    out.push_str("/Fields[");
    for field in fields {
        out.push_str("<</T(");
        out.push_str(&escape(&field.name));
        out.push_str(")/V(");
        out.push_str(&escape(&field.value));
        out.push_str(")>>");
    }
    out.push_str("]>>>>\r\nendobj\r\n\r\ntrailer\r\n<</Root 1 0 R>>\r\n%%EOF\r\n");
    out.into_bytes()
}

/// A literal string's body, with the three characters that would end or
/// unbalance it escaped.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if matches!(ch, '(' | ')' | '\\') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// `name=value&name=value`, with **no percent-encoding at all**.
///
/// That is upstream's own behaviour and not a simplification: the encoder
/// concatenates `name_b << "=" << csValue_b` with an `&` between, and neither
/// half is escaped — so a value containing `&` or `=` produces a query string
/// that cannot be parsed back. Reproduced rather than corrected, because
/// correcting it would change what a host receives without any evidence about
/// what a receiver expects; the escaping belongs to whoever posts the bytes.
fn url_encoded(fields: &[Submitted]) -> Vec<u8> {
    let mut out = String::new();
    for (index, field) in fields.iter().enumerate() {
        if index > 0 {
            out.push('&');
        }
        out.push_str(&field.name);
        out.push('=');
        out.push_str(&field.value);
    }
    out.into_bytes()
}
