//! Named destinations in the catalog `/Names /Dests` tree.
//!
//! Writers that emit a `GoTo` whose `/D` is a name must also ensure the name
//! tree can resolve it. When the existing tree is already a `/Kids` hierarchy,
//! entries are merged into that structure in place; otherwise the tree is
//! rewritten as a flat `/Names` leaf (the same shape [`crate::attach`] uses
//! for `/EmbeddedFiles`).

use pdfrum_common::{Diagnostics, Limits};
use pdfrum_object::{
    Array, Dict, Name, ObjRef, Object, PdfString, Resolve, decode_text, encode_text,
};

use crate::doc::EditDoc;
use crate::error::Error;
use crate::names;

type Result<T> = core::result::Result<T, Error>;

fn dict_at(edit: &EditDoc<'_>, reference: ObjRef) -> Option<Dict> {
    edit.fetch(reference)
        .ok()
        .as_deref()
        .and_then(Object::as_dict)
        .cloned()
}

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

/// Whether `name` is already present in `/Names /Dests`.
pub(crate) fn named_dest_exists(edit: &EditDoc<'_>, name: &str) -> Result<bool> {
    Ok(dest_entries(edit)?.iter().any(|(n, _)| n == name))
}

/// Builds a sorted `/Names` array and matching `/Limits` from pairs.
fn names_and_limits(entries: &[(String, Object)]) -> (Array, Array) {
    let mut sorted = entries.to_vec();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let mut names_array = Array::new();
    for (name, value) in &sorted {
        names_array.push(Object::Str(PdfString::literal(encode_text(name))));
        names_array.push(value.clone());
    }
    let mut limits = Array::new();
    if let (Some((first, _)), Some((last, _))) = (sorted.first(), sorted.last()) {
        limits.push(Object::Str(PdfString::literal(encode_text(first))));
        limits.push(Object::Str(PdfString::literal(encode_text(last))));
    }
    (names_array, limits)
}

fn leaf_entries<R: Resolve>(leaf: &Dict, r: &R) -> Vec<(String, Object)> {
    let Some(names_array) = leaf.array(names::NAMES, r) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for pair in 0..names_array.len() / 2 {
        let key = names_array
            .string_at(pair * 2)
            .map(|s| decode_text(s.as_bytes()).into_owned())
            .unwrap_or_default();
        let Some(value) = names_array.raw_at(pair * 2 + 1).cloned() else {
            continue;
        };
        out.push((key, value));
    }
    out
}

fn write_leaf(leaf: &mut Dict, entries: &[(String, Object)]) {
    let (names_array, limits) = names_and_limits(entries);
    leaf.insert(names::NAMES.clone(), Object::Array(names_array));
    if limits.is_empty() {
        leaf.remove(&Name::from("Limits"));
    } else {
        leaf.insert(Name::from("Limits"), Object::Array(limits));
    }
}

/// Locates the indirect `/Names /Dests` tree root, if any.
fn dests_tree_ref(edit: &EditDoc<'_>) -> Result<Option<ObjRef>> {
    let Some(root) = edit.base().trailer().reference(names::ROOT) else {
        return Err(Error::NoDestinationCatalog);
    };
    let Some(catalog) = dict_at(edit, root) else {
        return Ok(None);
    };
    let Some(names_obj) = catalog.raw(names::NAMES).cloned() else {
        return Ok(None);
    };
    let names_dict = match names_obj {
        Object::Ref(r) => match dict_at(edit, r) {
            Some(d) => d,
            None => return Ok(None),
        },
        Object::Dict(d) => d,
        _ => return Ok(None),
    };
    match names_dict.raw(names::DESTS).cloned() {
        Some(Object::Ref(r)) => Ok(Some(r)),
        _ => Ok(None),
    }
}

/// Tries to upsert into an existing `/Kids` tree without flattening it.
///
/// Returns `true` when the Kids path handled the write. Falls back (`false`)
/// when there is no Kids hierarchy (flat root leaf / missing tree).
fn try_upsert_into_kids(edit: &mut EditDoc<'_>, name: &str, dest: Object) -> Result<bool> {
    let Some(tree_ref) = dests_tree_ref(edit)? else {
        return Ok(false);
    };
    let Some(tree) = dict_at(edit, tree_ref) else {
        return Ok(false);
    };
    let k_kids = Name::from("Kids");
    // Flat root leaf → caller rewrites flat.
    if tree
        .array(names::NAMES, &pdfrum_object::NoResolve)
        .is_some()
        && tree.array(&k_kids, &pdfrum_object::NoResolve).is_none()
    {
        return Ok(false);
    }
    let Some(kids) = tree.array(&k_kids, edit) else {
        return Ok(false);
    };

    // Prefer updating a leaf that already holds `name`.
    for slot in 0..kids.len() {
        let Some(Object::Ref(leaf_ref)) = kids.raw_at(slot).cloned() else {
            // Inline kids cannot be replaced cleanly — flatten instead.
            return Ok(false);
        };
        let Some(mut leaf) = dict_at(edit, leaf_ref) else {
            continue;
        };
        let mut entries = leaf_entries(&leaf, edit);
        if let Some(existing) = entries.iter_mut().find(|(n, _)| n == name) {
            existing.1 = dest;
            write_leaf(&mut leaf, &entries);
            edit.replace(leaf_ref, Object::Dict(leaf));
            refresh_root_limits(edit, tree_ref);
            return Ok(true);
        }
    }

    // Append into the last referenced leaf when possible; else add a new kid.
    if let Some(Object::Ref(leaf_ref)) = kids.raw_at(kids.len().saturating_sub(1)).cloned()
        && let Some(mut leaf) = dict_at(edit, leaf_ref)
    {
        let mut entries = leaf_entries(&leaf, edit);
        entries.push((name.to_owned(), dest));
        write_leaf(&mut leaf, &entries);
        edit.replace(leaf_ref, Object::Dict(leaf));
        refresh_root_limits(edit, tree_ref);
        return Ok(true);
    }

    let mut leaf = Dict::new();
    write_leaf(&mut leaf, &[(name.to_owned(), dest)]);
    let leaf_ref = edit.add(Object::Dict(leaf));
    let mut tree = dict_at(edit, tree_ref).unwrap_or_default();
    let mut kids = tree
        .array(&k_kids, &pdfrum_object::NoResolve)
        .unwrap_or_default();
    kids.push(Object::Ref(leaf_ref));
    tree.insert(k_kids, Object::Array(kids));
    edit.replace(tree_ref, Object::Dict(tree));
    refresh_root_limits(edit, tree_ref);
    Ok(true)
}

/// Recomputes the root `/Limits` from every leaf under `/Kids`.
fn refresh_root_limits(edit: &mut EditDoc<'_>, tree_ref: ObjRef) {
    let Some(mut tree) = dict_at(edit, tree_ref) else {
        return;
    };
    let k_kids = Name::from("Kids");
    let k_limits = Name::from("Limits");
    let Some(kids) = tree.array(&k_kids, edit) else {
        return;
    };
    let mut low: Option<String> = None;
    let mut high: Option<String> = None;
    for slot in 0..kids.len() {
        let Some(leaf) = kids.dict_at(slot, edit) else {
            continue;
        };
        for (key, _) in leaf_entries(&leaf, edit) {
            if low.as_ref().is_none_or(|l| &key < l) {
                low = Some(key.clone());
            }
            if high.as_ref().is_none_or(|h| &key > h) {
                high = Some(key);
            }
        }
    }
    match (low, high) {
        (Some(l), Some(h)) => {
            tree.insert(
                k_limits,
                Object::Array(Array::of([
                    Object::Str(PdfString::literal(encode_text(&l))),
                    Object::Str(PdfString::literal(encode_text(&h))),
                ])),
            );
        }
        _ => {
            tree.remove(&k_limits);
        }
    }
    edit.replace(tree_ref, Object::Dict(tree));
}

/// Writes `entries` back as a flat `/Names` array under `/Names /Dests`.
///
/// `/Names` and `/Dests` are created as new indirect objects when missing.
/// Existing `/Kids` are removed — only used when the tree was already flat
/// or absent.
fn write_dest_entries_flat(edit: &mut EditDoc<'_>, entries: &[(String, Object)]) -> Result<()> {
    let k_names = names::NAMES.clone();
    let k_dests = names::DESTS.clone();
    let k_kids = Name::from("Kids");
    let k_limits = Name::from("Limits");
    let Some(root) = edit.base().trailer().reference(names::ROOT) else {
        return Err(Error::NoDestinationCatalog);
    };
    let mut catalog = dict_at(edit, root).unwrap_or_default();
    let (names_array, _) = names_and_limits(entries);
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
/// appended. A `/Kids` tree is updated in place when possible; otherwise the
/// tree is rewritten flat.
///
/// # Errors
///
/// [`Error::NoDestinationCatalog`] when the document has no catalog.
pub(crate) fn upsert_named_dest(edit: &mut EditDoc<'_>, name: &str, dest: Object) -> Result<()> {
    if try_upsert_into_kids(edit, name, dest.clone())? {
        return Ok(());
    }
    let mut entries = dest_entries(edit)?;
    if let Some(slot) = entries.iter_mut().find(|(n, _)| n == name) {
        slot.1 = dest;
    } else {
        entries.push((name.to_owned(), dest));
    }
    write_dest_entries_flat(edit, &entries)
}

/// Inserts `name` → `dest` only when the name is not already present.
///
/// Useful when a link should resolve an existing destination without
/// overwriting it. Returns whether a new entry was written.
///
/// # Errors
///
/// [`Error::NoDestinationCatalog`] when the document has no catalog.
pub(crate) fn ensure_named_dest(edit: &mut EditDoc<'_>, name: &str, dest: Object) -> Result<bool> {
    if named_dest_exists(edit, name)? {
        return Ok(false);
    }
    upsert_named_dest(edit, name, dest)?;
    Ok(true)
}
