//! The attachment writers: embedded files added, replaced, described and
//! removed through the document editor. The `/EmbeddedFiles` name tree is
//! rewritten flat on every change, a shape every reader accepts.

use crate::{EditDoc, Error};
use pdfrum_common::{Diagnostics, Limits};
use pdfrum_object::{
    Array, ByteSpan, Dict, Name, ObjRef, Object, PdfString, Resolve, Stream, encode_text,
};

/// An attachment write either applies or names why it could not.
type Result<T> = core::result::Result<T, Error>;

/// What an attachment carries besides its name and bytes.
///
/// A config struct with [`Default`]. `#[non_exhaustive]` so a field added
/// later is not a major break; fill one in with [`AttachmentOptions::builder`].
/// Every field is optional and an absent one writes no key.
///
/// ```
/// use pdfrum_edit::AttachmentOptions;
///
/// let options = AttachmentOptions::builder()
///     .description("The source data")
///     .mime_type("text/csv")
///     .build();
/// assert!(options.modified.is_none());
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct AttachmentOptions {
    /// The file specification's `/Desc`, the text a viewer shows beside the
    /// name. Read back by `Attachment::description`.
    pub description: Option<String>,
    /// The embedded file's MIME type — `text/plain`, `application/pdf` —
    /// written as the stream's `/Subtype` name. Read back by
    /// [`Attachment::subtype`](crate::Attachment::subtype).
    pub mime_type: Option<String>,
    /// The file's own modification time as a PDF date string
    /// (`D:YYYYMMDDHHmmSS…`, ISO 32000-1 §7.9.4), written to `/Params
    /// /ModDate`. [`pdf_date`](crate::pdf_date) spells a `SystemTime` that
    /// way. Read back by [`Attachment::param`](crate::Attachment::param).
    pub modified: Option<String>,
}

/// Builds an [`AttachmentOptions`] a setting at a time.
///
/// The way to change one field from outside this crate: the type is
/// `#[non_exhaustive]`, so struct-update syntax is a same-crate spelling.
/// Each method takes an `impl Into<String>`, so the `Some(…into())` the
/// fields need is written once here rather than at every call site.
///
/// ```
/// use pdfrum_edit::AttachmentOptions;
///
/// let options = AttachmentOptions::builder()
///     .description("The source data")
///     .mime_type("text/csv")
///     .build();
///
/// assert_eq!(options.description.as_deref(), Some("The source data"));
/// assert!(options.modified.is_none());
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[must_use]
pub struct AttachmentOptionsBuilder(AttachmentOptions);

impl AttachmentOptionsBuilder {
    /// The text a viewer shows beside the name —
    /// [`AttachmentOptions::description`].
    ///
    /// ```
    /// let options = pdfrum::AttachmentOptions::builder().description("notes").build();
    /// assert_eq!(options.description.as_deref(), Some("notes"));
    /// ```
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.0.description = Some(description.into());
        self
    }

    /// The embedded file's MIME type — [`AttachmentOptions::mime_type`].
    ///
    /// ```
    /// let options = pdfrum::AttachmentOptions::builder().mime_type("text/csv").build();
    /// assert_eq!(options.mime_type.as_deref(), Some("text/csv"));
    /// ```
    pub fn mime_type(mut self, mime_type: impl Into<String>) -> Self {
        self.0.mime_type = Some(mime_type.into());
        self
    }

    /// The file's modification time as a PDF date string —
    /// [`AttachmentOptions::modified`]. [`pdf_date`](crate::pdf_date) spells
    /// a `SystemTime` that way.
    ///
    /// ```
    /// let options = pdfrum::AttachmentOptions::builder()
    ///     .modified("D:20260906120000Z")
    ///     .build();
    /// assert!(options.modified.is_some());
    /// ```
    pub fn modified(mut self, modified: impl Into<String>) -> Self {
        self.0.modified = Some(modified.into());
        self
    }

    /// The options as built.
    ///
    /// ```
    /// let options = pdfrum::AttachmentOptions::builder().build();
    /// assert_eq!(options, pdfrum::AttachmentOptions::default());
    /// ```
    #[must_use]
    pub fn build(self) -> AttachmentOptions {
        self.0
    }
}

impl AttachmentOptions {
    /// A builder starting from the defaults.
    ///
    /// ```
    /// let options = pdfrum::AttachmentOptions::builder().mime_type("text/csv").build();
    /// ```
    pub fn builder() -> AttachmentOptionsBuilder {
        AttachmentOptionsBuilder::default()
    }
}

/// The embedded file stream (ISO 32000-1 §7.11.4): `/Type /EmbeddedFile`,
/// the MIME type as `/Subtype`, `/DL` and `/Params` with the size, an MD5
/// `/CheckSum` and the modification date when one was given. The save
/// flate-compresses it like every other filterless stream it writes.
fn embedded_file(bytes: &[u8], mime_type: Option<&str>, modified: Option<&str>) -> Stream {
    let len = i64::try_from(bytes.len()).unwrap_or(i64::MAX);
    let mut params = Dict::new();
    params.insert(Name::from("Size"), Object::Int(len));
    params.insert(
        Name::from("CheckSum"),
        Object::Str(PdfString::hex(pdfrum_crypt::md5(bytes))),
    );
    if let Some(modified) = modified.filter(|date| !date.is_empty()) {
        params.insert(
            Name::from("ModDate"),
            Object::Str(PdfString::literal(encode_text(modified))),
        );
    }
    let mut dict = Dict::new();
    dict.insert(Name::from("Type"), Object::Name(Name::from("EmbeddedFile")));
    if let Some(mime_type) = mime_type.filter(|mime| !mime.is_empty()) {
        dict.insert(
            Name::from("Subtype"),
            Object::Name(Name::from(mime_type.as_bytes())),
        );
    }
    dict.insert(Name::from("DL"), Object::Int(len));
    dict.insert(Name::from("Params"), Object::Dict(params));
    Stream::new(dict, ByteSpan::from(bytes.to_vec()))
}

/// The current attachments as (name, value) pairs, read through the
/// edits so far; empty without a tree.
pub(crate) fn attachment_entries(
    edit: &EditDoc<'_>,
    limits: &Limits,
) -> Result<Vec<(String, Object)>> {
    let Some(root) = edit.base().trailer().reference(&Name::from("Root")) else {
        return Err(Error::NoDestinationCatalog);
    };
    let Ok(catalog) = edit.fetch(root) else {
        return Ok(Vec::new());
    };
    let Some(catalog) = catalog.as_dict() else {
        return Ok(Vec::new());
    };
    let Some(names) = catalog.dict(&Name::from("Names"), edit) else {
        return Ok(Vec::new());
    };
    let Some(files) = names.dict(&Name::from("EmbeddedFiles"), edit) else {
        return Ok(Vec::new());
    };
    let tree = pdfrum_doc::NameTree { root: files };
    let mut diags = Diagnostics::default();
    let count = tree.count(edit, limits, &mut diags);
    Ok((0..count)
        .filter_map(|index| tree.lookup_by_index(index, edit, limits, &mut diags))
        .collect())
}

/// The file specification of attachment `index` — its reference when it
/// is indirect — and its dictionary; `None` when out of range or not a
/// dictionary.
pub(crate) fn attachment_spec(
    edit: &EditDoc<'_>,
    limits: &Limits,
    index: usize,
) -> Result<Option<(Option<ObjRef>, Dict)>> {
    let entries = attachment_entries(edit, limits)?;
    let Some(entry) = entries.get(index) else {
        return Ok(None);
    };
    Ok(match &entry.1 {
        Object::Ref(reference) => {
            let Ok(object) = edit.fetch(*reference) else {
                return Ok(None);
            };
            object
                .as_dict()
                .map(|dict| (Some(*reference), dict.clone()))
        }
        Object::Dict(dict) => Some((None, dict.clone())),
        _ => None,
    })
}

/// Writes `entries` back as a flat `/Names` array. `/Names` and
/// `/EmbeddedFiles` are created as new indirect objects when missing,
/// referenced from their parent; the innermost *indirect* holder is
/// replaced, and an inline holder is rewritten inside its parent, outward
/// to the catalog.
pub(crate) fn write_attachment_entries(
    edit: &mut EditDoc<'_>,
    entries: Vec<(String, Object)>,
) -> Result<()> {
    let k_root = Name::from("Root");
    let k_names = Name::from("Names");
    let k_ef = Name::from("EmbeddedFiles");
    let k_kids = Name::from("Kids");
    let k_limits = Name::from("Limits");
    let Some(root) = edit.base().trailer().reference(&k_root) else {
        return Err(Error::NoDestinationCatalog);
    };
    let dict_at = |edit: &EditDoc<'_>, reference: ObjRef| {
        edit.fetch(reference)
            .ok()
            .as_deref()
            .and_then(Object::as_dict)
            .cloned()
    };
    let mut catalog = dict_at(edit, root).unwrap_or_default();
    let mut names_array = Array::new();
    for (name, value) in entries {
        names_array.push(Object::Str(PdfString::literal(encode_text(&name))));
        names_array.push(value);
    }
    let tree_of = |existing: Option<Dict>| {
        let mut tree = existing.unwrap_or_default();
        tree.remove(&k_kids);
        tree.remove(&k_limits);
        tree.insert(k_names.clone(), Object::Array(names_array.clone()));
        tree
    };
    match catalog.raw(&k_names).cloned() {
        Some(Object::Ref(names_ref)) => {
            let mut names = dict_at(edit, names_ref).unwrap_or_default();
            match names.raw(&k_ef).cloned() {
                Some(Object::Ref(tree_ref)) => {
                    let existing = dict_at(edit, tree_ref);
                    edit.replace(tree_ref, Object::Dict(tree_of(existing)));
                }
                Some(Object::Dict(inline)) => {
                    names.insert(k_ef.clone(), Object::Dict(tree_of(Some(inline))));
                    edit.replace(names_ref, Object::Dict(names));
                }
                _ => {
                    let tree_ref = edit.add(Object::Dict(tree_of(None)));
                    names.insert(k_ef.clone(), Object::Ref(tree_ref));
                    edit.replace(names_ref, Object::Dict(names));
                }
            }
        }
        Some(Object::Dict(mut names)) => match names.raw(&k_ef).cloned() {
            Some(Object::Ref(tree_ref)) => {
                let existing = dict_at(edit, tree_ref);
                edit.replace(tree_ref, Object::Dict(tree_of(existing)));
            }
            Some(Object::Dict(inline)) => {
                names.insert(k_ef.clone(), Object::Dict(tree_of(Some(inline))));
                catalog.insert(k_names.clone(), Object::Dict(names));
                edit.replace(root, Object::Dict(catalog));
            }
            _ => {
                let tree_ref = edit.add(Object::Dict(tree_of(None)));
                names.insert(k_ef.clone(), Object::Ref(tree_ref));
                catalog.insert(k_names.clone(), Object::Dict(names));
                edit.replace(root, Object::Dict(catalog));
            }
        },
        _ => {
            let tree_ref = edit.add(Object::Dict(tree_of(None)));
            let mut names = Dict::new();
            names.insert(k_ef.clone(), Object::Ref(tree_ref));
            let names_ref = edit.add(Object::Dict(names));
            catalog.insert(k_names.clone(), Object::Ref(names_ref));
            edit.replace(root, Object::Dict(catalog));
        }
    }
    Ok(())
}

/// Stores a rewritten file specification for attachment `index`:
/// replaces it when it is indirect, otherwise rewrites the tree entry
/// inline.
pub(crate) fn store_attachment_spec(
    edit: &mut EditDoc<'_>,
    limits: &Limits,
    index: usize,
    reference: Option<ObjRef>,
    spec: Dict,
) -> Result<()> {
    if let Some(reference) = reference {
        edit.replace(reference, Object::Dict(spec));
        return Ok(());
    }
    let mut entries = attachment_entries(edit, limits)?;
    if let Some(entry) = entries.get_mut(index) {
        entry.1 = Object::Dict(spec);
    }
    write_attachment_entries(edit, entries)
}

/// Removes attachment `index` from the name tree; `Ok(false)` when there
/// is no such attachment.
///
/// # Errors
///
/// When the document has no catalog to hold the tree.
pub fn delete_attachment(edit: &mut EditDoc<'_>, limits: &Limits, index: usize) -> Result<bool> {
    let mut entries = attachment_entries(edit, limits)?;
    if index >= entries.len() {
        return Ok(false);
    }
    entries.remove(index);
    write_attachment_entries(edit, entries)?;
    Ok(true)
}

/// Adds an embedded file named `name`, sorted into the `/EmbeddedFiles`
/// name tree by name — creating the tree when the document has none —
/// and returns its index among the attachments.
///
/// The file specification is `<< /Type /Filespec /UF (name) /F (name)
/// /Desc (…) /EF << /F stream >> >>`, a new indirect object; the stream
/// is `/Type /EmbeddedFile` with `/Subtype` as the MIME type, `/DL`, and
/// `/Params` holding `/Size`, an MD5 `/CheckSum` and `/ModDate`, and the
/// save flate-compresses it. A second attachment with the same name is
/// a second entry, not a replacement.
///
/// ```
/// use pdfrum::{AttachmentOptions, Document, SaveOptions};
///
/// let doc = Document::open("../pdfrum/tests/fixtures/hello_world.pdf")?;
/// let mut edit = doc.edit();
/// edit.add_attachment(
///     "notes.txt",
///     b"Read me",
///     &AttachmentOptions::builder().mime_type("text/plain").build(),
/// )?;
/// let mut bytes = Vec::new();
/// edit.write_to(&mut bytes, &SaveOptions::default())?;
///
/// let saved = Document::from_bytes(bytes)?;
/// let attachment = &saved.attachments()[0];
/// assert_eq!(attachment.file_name(), "notes.txt");
/// assert_eq!(attachment.data().as_deref(), Some(&b"Read me"[..]));
/// assert_eq!(attachment.subtype().as_deref(), Some("text/plain"));
/// # Ok::<(), pdfrum::Error>(())
/// ```
///
/// # Errors
///
/// When the document has no catalog to hold the tree.
pub fn add_attachment(
    edit: &mut EditDoc<'_>,
    limits: &Limits,
    name: &str,
    bytes: &[u8],
    options: &AttachmentOptions,
) -> Result<usize> {
    let mut entries = attachment_entries(edit, limits)?;
    let stream_ref = edit.add(Object::Stream(Box::new(embedded_file(
        bytes,
        options.mime_type.as_deref(),
        options.modified.as_deref(),
    ))));
    let mut spec = Dict::new();
    spec.insert(Name::from("Type"), Object::Name(Name::from("Filespec")));
    spec.insert(
        Name::from("UF"),
        Object::Str(PdfString::literal(encode_text(name))),
    );
    spec.insert(
        Name::from("F"),
        Object::Str(PdfString::literal(encode_text(name))),
    );
    if let Some(description) = options.description.as_deref().filter(|d| !d.is_empty()) {
        spec.insert(
            Name::from("Desc"),
            Object::Str(PdfString::literal(encode_text(description))),
        );
    }
    let mut ef = Dict::new();
    ef.insert(Name::from("F"), Object::Ref(stream_ref));
    spec.insert(Name::from("EF"), Object::Dict(ef));
    let reference = edit.add(Object::Dict(spec));
    let index = entries
        .iter()
        .position(|(existing, _)| existing.as_str() > name)
        .unwrap_or(entries.len());
    entries.insert(index, (name.to_owned(), Object::Ref(reference)));
    write_attachment_entries(edit, entries)?;
    Ok(index)
}

/// Removes every attachment named `name` from the name tree; `Ok(false)`
/// when there is none. The objects go with the next full save's garbage
/// collection.
///
/// ```
/// use pdfrum::Document;
///
/// let doc = Document::open("../pdfrum/tests/fixtures/embedded_attachments.pdf")?;
/// let mut edit = doc.edit();
/// assert!(edit.remove_attachment("1.txt")?);
/// assert!(!edit.remove_attachment("1.txt")?, "already gone");
/// # Ok::<(), pdfrum::Error>(())
/// ```
///
/// # Errors
///
/// When the document has no catalog to hold the tree.
pub fn remove_attachment(edit: &mut EditDoc<'_>, limits: &Limits, name: &str) -> Result<bool> {
    let mut entries = attachment_entries(edit, limits)?;
    let before = entries.len();
    entries.retain(|(existing, _)| existing != name);
    if entries.len() == before {
        return Ok(false);
    }
    write_attachment_entries(edit, entries)?;
    Ok(true)
}

/// Replaces attachment `index`'s embedded file with a new stream carrying
/// `/Type /EmbeddedFile`, `/DL <len>` and `/Params << /Size <len>
/// /CheckSum <md5> >>`, linked as `/EF << /F <ref> >>` on the file
/// specification; a MIME type or date the old stream had is not carried
/// over. `Ok(false)` when there is no such attachment.
///
/// # Errors
///
/// When the document has no catalog to hold the tree.
pub fn set_attachment_file(
    edit: &mut EditDoc<'_>,
    limits: &Limits,
    index: usize,
    bytes: &[u8],
) -> Result<bool> {
    let Some((reference, mut spec)) = attachment_spec(edit, limits, index)? else {
        return Ok(false);
    };
    let stream_ref = edit.add(Object::Stream(Box::new(embedded_file(bytes, None, None))));
    let mut ef = Dict::new();
    ef.insert(Name::from("F"), Object::Ref(stream_ref));
    spec.insert(Name::from("EF"), Object::Dict(ef));
    store_attachment_spec(edit, limits, index, reference, spec)?;
    Ok(true)
}

/// Sets a `/Params` text entry on attachment `index`'s embedded file —
/// `CreationDate`, `ModDate`, any key — creating `/Params` when missing;
/// a `CheckSum` given as `<HEX…>` is stored as that hex string.
/// `Ok(false)` when the attachment has no embedded file.
///
/// # Errors
///
/// When the document has no catalog to hold the tree.
pub fn set_attachment_param(
    edit: &mut EditDoc<'_>,
    limits: &Limits,
    index: usize,
    key: &str,
    text: &str,
) -> Result<bool> {
    let Some((_, spec)) = attachment_spec(edit, limits, index)? else {
        return Ok(false);
    };
    let Some(ef) = spec.dict(&Name::from("EF"), edit) else {
        return Ok(false);
    };
    let Some(stream_ref) = ef.reference(&Name::from("F")) else {
        return Ok(false);
    };
    let Some(stream) = edit
        .fetch(stream_ref)
        .ok()
        .and_then(|object| object.as_stream().cloned())
    else {
        return Ok(false);
    };
    let mut params = stream
        .dict
        .dict(&Name::from("Params"), edit)
        .unwrap_or_default();
    let hex = if key == "CheckSum" {
        hex_bytes(text)
    } else {
        None
    };
    let value = hex.map_or_else(
        || Object::Str(PdfString::literal(encode_text(text))),
        |bytes| Object::Str(PdfString::hex(bytes)),
    );
    params.insert(Name::from(key), value);
    let mut dict = stream.dict.clone();
    dict.insert(Name::from("Params"), Object::Dict(params));
    edit.replace(
        stream_ref,
        Object::Stream(Box::new(Stream::new(dict, stream.data.clone()))),
    );
    Ok(true)
}

/// Sets the file specification's `/Desc`; `Ok(false)` when there is no
/// such attachment.
///
/// # Errors
///
/// When the document has no catalog to hold the tree.
pub fn set_attachment_description(
    edit: &mut EditDoc<'_>,
    limits: &Limits,
    index: usize,
    text: &str,
) -> Result<bool> {
    let Some((reference, mut spec)) = attachment_spec(edit, limits, index)? else {
        return Ok(false);
    };
    spec.insert(
        Name::from("Desc"),
        Object::Str(PdfString::literal(encode_text(text))),
    );
    store_attachment_spec(edit, limits, index, reference, spec)?;
    Ok(true)
}

/// `<HEXPAIRS>` — angle brackets, an even count of hex digits, whitespace
/// ignored — as bytes; `None` for anything else.
fn hex_bytes(text: &str) -> Option<Vec<u8>> {
    let mut digits = Vec::new();
    for c in text.strip_prefix('<')?.strip_suffix('>')?.chars() {
        if c.is_ascii_whitespace() {
            continue;
        }
        digits.push(u8::try_from(c.to_digit(16)?).ok()?);
    }
    if !digits.len().is_multiple_of(2) {
        return None;
    }
    Some(
        digits
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&[hi, lo]| (hi << 4) | lo)
            .collect(),
    )
}
