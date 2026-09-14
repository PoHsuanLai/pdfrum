//! The interactive form dictionary (ISO 32000-1 §12.7.2) on the write side.
//!
//! The catalog's `/AcroForm` is where a document says how its fields should be
//! presented, as opposed to what any one of them holds. Only the flag that
//! decides *who draws the field* lives here so far: the values themselves go
//! through `Document::save_form`, which lays each one out as it writes.

use pdfrum_object::{Dict, Name, Object, Resolve, names};

use crate::{doc::EditDoc, error::Error};

/// A form write either applies or names why it could not.
type Result<T> = core::result::Result<T, Error>;

/// Sets or clears `/AcroForm /NeedAppearances`.
///
/// The flag asks a reader to build every field's appearance from its value and
/// `/DA` rather than trust the `/AP` in the file. pdfrum draws appearances as
/// it fills, so a saved form shows its values without this — but a document
/// whose fields were filled by something else, or whose appearances are known
/// to be stale, wants it set, and setting it is the one way to make a reader
/// that disagrees with pdfrum's layout use its own.
///
/// `false` removes the key rather than writing `false`, which is the same
/// thing to a reader — [`pdfrum_doc`](../pdfrum_doc/index.html) reads it
/// strictly as a boolean, so a missing key and an explicit `false` both mean
/// "trust the `/AP`".
///
/// A document with no `/AcroForm` gains one, because a flag with no form to
/// hang on would be dropped by the next reader that rewrites the catalog.
///
/// # Errors
///
/// [`Error::NoDestinationCatalog`] when the document has no catalog to hold
/// the form.
///
/// ```
/// use std::sync::Arc;
/// use pdfrum_edit::{EditDoc, SaveOptions, save, set_need_appearances};
/// use pdfrum_object::{Resolve, names};
/// use pdfrum_parser::{LoadOptions, load};
///
/// let bytes: Arc<[u8]> = Arc::from(&include_bytes!("../tests/files/hello.pdf")[..]);
/// let doc = load(bytes, &LoadOptions::default())?;
/// let mut edit = EditDoc::new(&doc);
/// set_need_appearances(&mut edit, true)?;
///
/// let mut out = Vec::new();
/// save(&edit, &SaveOptions::default(), &mut out)?;
/// let reloaded = load(Arc::from(&out[..]), &LoadOptions::default())?;
/// let catalog = reloaded.catalog().expect("a catalog");
/// let form = catalog
///     .dict(&names::ACRO_FORM, &reloaded)
///     .expect("an /AcroForm");
/// assert_eq!(
///     form.get(&pdfrum_object::Name::from("NeedAppearances"), &reloaded)
///         .and_then(|v| v.as_direct().and_then(pdfrum_object::Object::as_bool)),
///     Some(true),
/// );
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn set_need_appearances(dest: &mut EditDoc<'_>, needed: bool) -> Result<()> {
    let key = Name::from("NeedAppearances");
    let Some(root) = dest.base().trailer().reference(names::ROOT) else {
        return Err(Error::NoDestinationCatalog);
    };
    let Ok(fetched) = dest.fetch(root) else {
        return Err(Error::NoDestinationCatalog);
    };
    let Some(mut catalog) = fetched.as_dict().cloned() else {
        return Err(Error::NoDestinationCatalog);
    };

    // `/AcroForm` is usually indirect, and editing it in place keeps every
    // other reference to it — the field tree's, most of all — pointing at the
    // dictionary that now carries the flag.
    match catalog.raw(names::ACRO_FORM).cloned() {
        Some(Object::Ref(form_ref)) => {
            let mut form = dest
                .fetch(form_ref)
                .ok()
                .and_then(|object| object.as_dict().cloned())
                .unwrap_or_default();
            set_flag(&mut form, &key, needed);
            dest.replace(form_ref, Object::Dict(form));
        }
        Some(Object::Dict(mut form)) => {
            set_flag(&mut form, &key, needed);
            catalog.insert(names::ACRO_FORM.clone(), Object::Dict(form));
            dest.replace(root, Object::Dict(catalog));
        }
        _ => {
            let mut form = Dict::new();
            set_flag(&mut form, &key, needed);
            catalog.insert(names::ACRO_FORM.clone(), Object::Dict(form));
            dest.replace(root, Object::Dict(catalog));
        }
    }
    Ok(())
}

/// Writes the flag, or removes it when it would say `false`.
fn set_flag(form: &mut Dict, key: &Name, needed: bool) {
    if needed {
        form.insert(key.clone(), Object::Bool(true));
    } else {
        form.remove(key);
    }
}
