//! Named destinations in the catalog `/Names /Dests` tree.
//!
//! Writers that emit a `GoTo` whose `/D` is a name must also ensure the name
//! tree can resolve it. The tree is rewritten flat on every change — the same
//! shape [`crate::attach`] uses for `/EmbeddedFiles`, and every reader accepts.

use pdfrum_common::{Diagnostics, Limits};
use pdfrum_object::{Array, Dict, Name, ObjRef, Object, PdfString, Resolve, encode_text};

use crate::doc::EditDoc;
use crate::error::Error;
use crate::names;

type Result<T> = core::result::Result<T, Error>;

/// Current named destinations as `(name, value)` pairs, read through the
/// edits so far; empty without a tree.
fn dest_entries(edit: &EditDoc<'_>) -> Result<Vec<(String, Object)>> {
    let Some(root) = edit.base().trailer().reference(names::ROOT) else {
        return Err(Error::NoDestinationCatalog);
    };
    let Ok(catalog) = edit.fetch(root) else {
        return Ok(Vec::new());
    };
    let Some(catalog) = catalog.as_dict() else {
        return Ok(Vec::new());
    };
    let Some(names_dict) = catalog.dict(names::NAMES, edit) else {
        return Ok(Vec::new());
    };
    let Some(dests) = names_dict.dict(names::DESTS, edit) else {
        return Ok(Vec::new());
    };
    let tree = pdfrum_doc::NameTree { root: dests };
    let limits = Limits::default();
    let mut diags = Diagnostics::default();
    let count = tree.count(edit, &limits, &mut diags);
    Ok((0..count)
        .filter_map(|index| tree.lookup_by_index(index, edit, &limits, &mut diags))
        .collect())
}

/// Writes `entries` back as a flat `/Names` array under `/Names /Dests`.
///
/// `/Names` and `/Dests` are created as new indirect objects when missing.
fn write_dest_entries(edit: &mut EditDoc<'_>, entries: Vec<(String, Object)>) -> Result<()> {
    let k_names = names::NAMES.clone();
    let k_dests = names::DESTS.clone();
    let k_kids = Name::from("Kids");
    let k_limits = Name::from("Limits");
    let Some(root) = edit.base().trailer().reference(names::ROOT) else {
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
            let mut names_dict = dict_at(edit, names_ref).unwrap_or_default();
            match names_dict.raw(&k_dests).cloned() {
                Some(Object::Ref(tree_ref)) => {
                    let existing = dict_at(edit, tree_ref);
                    edit.replace(tree_ref, Object::Dict(tree_of(existing)));
                }
                Some(Object::Dict(inline)) => {
                    names_dict.insert(k_dests.clone(), Object::Dict(tree_of(Some(inline))));
                    edit.replace(names_ref, Object::Dict(names_dict));
                }
                _ => {
                    let tree_ref = edit.add(Object::Dict(tree_of(None)));
                    names_dict.insert(k_dests.clone(), Object::Ref(tree_ref));
                    edit.replace(names_ref, Object::Dict(names_dict));
                }
            }
        }
        Some(Object::Dict(mut names_dict)) => match names_dict.raw(&k_dests).cloned() {
            Some(Object::Ref(tree_ref)) => {
                let existing = dict_at(edit, tree_ref);
                edit.replace(tree_ref, Object::Dict(tree_of(existing)));
            }
            Some(Object::Dict(inline)) => {
                names_dict.insert(k_dests.clone(), Object::Dict(tree_of(Some(inline))));
                catalog.insert(k_names.clone(), Object::Dict(names_dict));
                edit.replace(root, Object::Dict(catalog));
            }
            _ => {
                let tree_ref = edit.add(Object::Dict(tree_of(None)));
                names_dict.insert(k_dests.clone(), Object::Ref(tree_ref));
                catalog.insert(k_names.clone(), Object::Dict(names_dict));
                edit.replace(root, Object::Dict(catalog));
            }
        },
        _ => {
            let tree_ref = edit.add(Object::Dict(tree_of(None)));
            let mut names_dict = Dict::new();
            names_dict.insert(k_dests.clone(), Object::Ref(tree_ref));
            let names_ref = edit.add(Object::Dict(names_dict));
            catalog.insert(k_names.clone(), Object::Ref(names_ref));
            edit.replace(root, Object::Dict(catalog));
        }
    }
    Ok(())
}

/// Upserts a named destination so `/Names /Dests` (or a later lookup) can
/// resolve `name` to `dest`.
///
/// `dest` is typically an explicit destination array (`[page /Fit]`, …). An
/// existing entry with the same name is replaced; otherwise a new entry is
/// appended. The tree is rewritten flat.
///
/// # Errors
///
/// [`Error::NoDestinationCatalog`] when the document has no catalog.
pub(crate) fn upsert_named_dest(edit: &mut EditDoc<'_>, name: &str, dest: Object) -> Result<()> {
    let mut entries = dest_entries(edit)?;
    if let Some(slot) = entries.iter_mut().find(|(n, _)| n == name) {
        slot.1 = dest;
    } else {
        entries.push((name.to_owned(), dest));
    }
    write_dest_entries(edit, entries)
}
