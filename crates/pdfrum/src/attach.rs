//! The attachment writers: embedded files added, replaced, described and
//! removed through the document editor. The `/EmbeddedFiles` name tree is
//! rewritten flat on every change, a shape every reader accepts.

use crate::DocEdit;
use pdfrum_common::Diagnostics;
use pdfrum_object::{
    Array, ByteSpan, Dict, Name, ObjRef, Object, PdfString, Resolve, Stream, encode_text,
};

impl DocEdit<'_> {
    /// The current attachments as (name, value) pairs, read through the
    /// edits so far; empty without a tree.
    pub(crate) fn attachment_entries(&self) -> crate::Result<Vec<(String, Object)>> {
        let Some(root) = self.doc.inner.trailer().reference(&Name::from("Root")) else {
            return Err(pdfrum_edit::Error::NoDestinationCatalog.into());
        };
        let Ok(catalog) = self.inner.fetch(root) else {
            return Ok(Vec::new());
        };
        let Some(catalog) = catalog.as_dict() else {
            return Ok(Vec::new());
        };
        let Some(names) = catalog.dict(&Name::from("Names"), &self.inner) else {
            return Ok(Vec::new());
        };
        let Some(files) = names.dict(&Name::from("EmbeddedFiles"), &self.inner) else {
            return Ok(Vec::new());
        };
        let tree = pdfrum_doc::NameTree { root: files };
        let mut diags = Diagnostics::default();
        let count = tree.count(&self.inner, &self.doc.limits, &mut diags);
        Ok((0..count)
            .filter_map(|index| {
                tree.lookup_by_index(index, &self.inner, &self.doc.limits, &mut diags)
            })
            .collect())
    }

    /// The file specification of attachment `index` — its reference when it
    /// is indirect — and its dictionary; `None` when out of range or not a
    /// dictionary.
    pub(crate) fn attachment_spec(
        &self,
        index: usize,
    ) -> crate::Result<Option<(Option<ObjRef>, Dict)>> {
        let entries = self.attachment_entries()?;
        let Some(entry) = entries.get(index) else {
            return Ok(None);
        };
        Ok(match &entry.1 {
            Object::Ref(reference) => {
                let Ok(object) = self.inner.fetch(*reference) else {
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
        &mut self,
        entries: Vec<(String, Object)>,
    ) -> crate::Result<()> {
        let k_root = Name::from("Root");
        let k_names = Name::from("Names");
        let k_ef = Name::from("EmbeddedFiles");
        let k_kids = Name::from("Kids");
        let k_limits = Name::from("Limits");
        let Some(root) = self.doc.inner.trailer().reference(&k_root) else {
            return Err(pdfrum_edit::Error::NoDestinationCatalog.into());
        };
        let dict_at = |edit: &pdfrum_edit::EditDoc<'_>, reference: ObjRef| {
            edit.fetch(reference)
                .ok()
                .as_deref()
                .and_then(Object::as_dict)
                .cloned()
        };
        let mut catalog = dict_at(&self.inner, root).unwrap_or_default();
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
                let mut names = dict_at(&self.inner, names_ref).unwrap_or_default();
                match names.raw(&k_ef).cloned() {
                    Some(Object::Ref(tree_ref)) => {
                        let existing = dict_at(&self.inner, tree_ref);
                        self.inner
                            .replace(tree_ref, Object::Dict(tree_of(existing)));
                    }
                    Some(Object::Dict(inline)) => {
                        names.insert(k_ef.clone(), Object::Dict(tree_of(Some(inline))));
                        self.inner.replace(names_ref, Object::Dict(names));
                    }
                    _ => {
                        let tree_ref = self.inner.add(Object::Dict(tree_of(None)));
                        names.insert(k_ef.clone(), Object::Ref(tree_ref));
                        self.inner.replace(names_ref, Object::Dict(names));
                    }
                }
            }
            Some(Object::Dict(mut names)) => match names.raw(&k_ef).cloned() {
                Some(Object::Ref(tree_ref)) => {
                    let existing = dict_at(&self.inner, tree_ref);
                    self.inner
                        .replace(tree_ref, Object::Dict(tree_of(existing)));
                }
                Some(Object::Dict(inline)) => {
                    names.insert(k_ef.clone(), Object::Dict(tree_of(Some(inline))));
                    catalog.insert(k_names.clone(), Object::Dict(names));
                    self.inner.replace(root, Object::Dict(catalog));
                }
                _ => {
                    let tree_ref = self.inner.add(Object::Dict(tree_of(None)));
                    names.insert(k_ef.clone(), Object::Ref(tree_ref));
                    catalog.insert(k_names.clone(), Object::Dict(names));
                    self.inner.replace(root, Object::Dict(catalog));
                }
            },
            _ => {
                let tree_ref = self.inner.add(Object::Dict(tree_of(None)));
                let mut names = Dict::new();
                names.insert(k_ef.clone(), Object::Ref(tree_ref));
                let names_ref = self.inner.add(Object::Dict(names));
                catalog.insert(k_names.clone(), Object::Ref(names_ref));
                self.inner.replace(root, Object::Dict(catalog));
            }
        }
        Ok(())
    }

    /// Stores a rewritten file specification for attachment `index`:
    /// replaces it when it is indirect, otherwise rewrites the tree entry
    /// inline.
    pub(crate) fn store_attachment_spec(
        &mut self,
        index: usize,
        reference: Option<ObjRef>,
        spec: Dict,
    ) -> crate::Result<()> {
        if let Some(reference) = reference {
            self.inner.replace(reference, Object::Dict(spec));
            return Ok(());
        }
        let mut entries = self.attachment_entries()?;
        if let Some(entry) = entries.get_mut(index) {
            entry.1 = Object::Dict(spec);
        }
        self.write_attachment_entries(entries)
    }

    /// Removes attachment `index` from the name tree; `Ok(false)` when there
    /// is no such attachment.
    ///
    /// # Errors
    ///
    /// When the document has no catalog to hold the tree.
    pub fn delete_attachment(&mut self, index: usize) -> crate::Result<bool> {
        let mut entries = self.attachment_entries()?;
        if index >= entries.len() {
            return Ok(false);
        }
        entries.remove(index);
        self.write_attachment_entries(entries)?;
        Ok(true)
    }

    /// Adds an embedded file named `name`, sorted into the name tree by
    /// name, and returns its index among the attachments. The file
    /// specification is `<< /Type /Filespec /UF (name) /F (name) >>`, a new
    /// indirect object; `bytes` are stored as
    /// [`DocEdit::set_attachment_file`] stores them.
    ///
    /// # Errors
    ///
    /// When the document has no catalog to hold the tree.
    pub fn add_attachment(&mut self, name: &str, bytes: &[u8]) -> crate::Result<usize> {
        let mut entries = self.attachment_entries()?;
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
        let reference = self.inner.add(Object::Dict(spec));
        let index = entries
            .iter()
            .position(|(existing, _)| existing.as_str() > name)
            .unwrap_or(entries.len());
        entries.insert(index, (name.to_owned(), Object::Ref(reference)));
        self.write_attachment_entries(entries)?;
        self.set_attachment_file(index, bytes)?;
        Ok(index)
    }

    /// Replaces attachment `index`'s embedded file with a new stream carrying
    /// `/DL <len>` and `/Params << /Size <len> /CheckSum <md5> >>`, linked as
    /// `/EF << /F <ref> >>` on the file specification. `Ok(false)` when there
    /// is no such attachment.
    ///
    /// # Errors
    ///
    /// When the document has no catalog to hold the tree.
    pub fn set_attachment_file(&mut self, index: usize, bytes: &[u8]) -> crate::Result<bool> {
        let Some((reference, mut spec)) = self.attachment_spec(index)? else {
            return Ok(false);
        };
        let len = i64::try_from(bytes.len()).unwrap_or(i64::MAX);
        let mut params = Dict::new();
        params.insert(Name::from("Size"), Object::Int(len));
        params.insert(
            Name::from("CheckSum"),
            Object::Str(PdfString::hex(pdfrum_crypt::md5(bytes))),
        );
        let mut dict = Dict::new();
        dict.insert(Name::from("DL"), Object::Int(len));
        dict.insert(Name::from("Params"), Object::Dict(params));
        let stream_ref = self.inner.add(Object::Stream(Box::new(Stream::new(
            dict,
            ByteSpan::from(bytes.to_vec()),
        ))));
        let mut ef = Dict::new();
        ef.insert(Name::from("F"), Object::Ref(stream_ref));
        spec.insert(Name::from("EF"), Object::Dict(ef));
        self.store_attachment_spec(index, reference, spec)?;
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
        &mut self,
        index: usize,
        key: &str,
        text: &str,
    ) -> crate::Result<bool> {
        let Some((_, spec)) = self.attachment_spec(index)? else {
            return Ok(false);
        };
        let Some(ef) = spec.dict(&Name::from("EF"), &self.inner) else {
            return Ok(false);
        };
        let Some(stream_ref) = ef.reference(&Name::from("F")) else {
            return Ok(false);
        };
        let Some(stream) = self
            .inner
            .fetch(stream_ref)
            .ok()
            .and_then(|object| object.as_stream().cloned())
        else {
            return Ok(false);
        };
        let mut params = stream
            .dict
            .dict(&Name::from("Params"), &self.inner)
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
        self.inner.replace(
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
    pub fn set_attachment_description(&mut self, index: usize, text: &str) -> crate::Result<bool> {
        let Some((reference, mut spec)) = self.attachment_spec(index)? else {
            return Ok(false);
        };
        spec.insert(
            Name::from("Desc"),
            Object::Str(PdfString::literal(encode_text(text))),
        );
        self.store_attachment_spec(index, reference, spec)?;
        Ok(true)
    }
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
            .chunks_exact(2)
            .map(|pair| (pair[0] << 4) | pair[1])
            .collect(),
    )
}
