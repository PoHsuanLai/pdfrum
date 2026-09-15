//! The interactive form dictionary (ISO 32000-1 §12.7.2) on the write side,
//! and the appearance characteristics (§12.5.6.19) a widget is drawn from.
//!
//! The catalog's `/AcroForm` is where a document says how its fields should be
//! presented, as opposed to what any one of them holds; `/MK` is where one
//! widget says how it should look. Values themselves go through
//! `Document::save_form`, which lays each one out as it writes.

use pdfrum_object::{Array, Dict, Name, ObjRef, Object, Resolve, names};
use peniko::Color;

use crate::{doc::EditDoc, error::Error};

/// A form write either applies or names why it could not.
type Result<T> = core::result::Result<T, Error>;

/// A widget's appearance characteristics — its `/MK` dictionary.
///
/// The generator reads `/MK` on **every** regeneration, so writing one is
/// enough to change how a field is drawn: there is no separate appearance to
/// keep in step. An unset field leaves that key alone rather than writing a
/// default, which is what lets this edit one characteristic of a widget
/// without flattening the rest.
///
/// ```
/// use pdfrum_edit::WidgetAppearance;
/// use peniko::Color;
///
/// let mk = WidgetAppearance::new()
///     .background(Color::from_rgb8(240, 240, 240))
///     .border(Color::from_rgb8(0, 0, 0));
/// // Characteristics left unset keep whatever the widget already had.
/// assert_eq!(mk, mk.clone());
/// ```
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WidgetAppearance {
    background: Option<Color>,
    border: Option<Color>,
    rotation: Option<i64>,
    caption: Option<String>,
}

impl WidgetAppearance {
    /// Characteristics that change nothing.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// `/BG`: the colour behind the field.
    #[must_use]
    pub fn background(mut self, color: Color) -> Self {
        self.background = Some(color);
        self
    }

    /// `/BC`: the border colour.
    ///
    /// A widget's border is drawn only when this names one — the `/BS` width
    /// alone does not produce a visible edge.
    #[must_use]
    pub fn border(mut self, color: Color) -> Self {
        self.border = Some(color);
        self
    }

    /// `/R`: the widget's rotation within its rectangle, in degrees.
    ///
    /// Read as a quarter turn; anything else rounds down to one.
    #[must_use]
    pub fn rotation(mut self, degrees: i64) -> Self {
        self.rotation = Some(degrees);
        self
    }

    /// `/CA`: the caption a push button shows.
    #[must_use]
    pub fn caption(mut self, text: impl Into<String>) -> Self {
        self.caption = Some(text.into());
        self
    }

    /// These characteristics alone, as a fresh `/MK`.
    ///
    /// A created widget has nothing to merge into, so it takes this; an
    /// existing one goes through [`Self::merge_into`] instead and keeps the
    /// keys the builder says nothing about.
    pub(crate) fn to_dict(&self) -> Dict {
        self.merge_into(Dict::new())
    }

    /// Merges these characteristics into an existing `/MK`, keeping keys the
    /// builder says nothing about.
    fn merge_into(&self, mut mk: Dict) -> Dict {
        if let Some(color) = self.background {
            mk.insert(names::BG.clone(), color_object(color));
        }
        if let Some(color) = self.border {
            mk.insert(names::BC.clone(), color_object(color));
        }
        if let Some(degrees) = self.rotation {
            mk.insert(Name::from("R"), Object::Int(degrees));
        }
        if let Some(caption) = &self.caption {
            mk.insert(
                names::CA.clone(),
                Object::Str(pdfrum_object::PdfString::literal(caption.as_bytes())),
            );
        }
        mk
    }
}

/// The `/MK` key. `pdfrum-doc` owns the constant; this crate spells it here
/// rather than depending on that module's name table for one entry.
pub(crate) fn mk_key() -> Name {
    Name::from("MK")
}

/// An `/MK` colour: three clamped components, as the reader's `from_array`
/// expects.
fn color_object(color: Color) -> Object {
    let [r, g, b, _] = color.components;
    Object::Array(Array::of([
        Object::Real(r.clamp(0.0, 1.0)),
        Object::Real(g.clamp(0.0, 1.0)),
        Object::Real(b.clamp(0.0, 1.0)),
    ]))
}

/// Sets a widget annotation's `/MK` appearance characteristics.
///
/// `annot` is the widget's own object — the reference an
/// [`add_annotation`](crate::add_annotation) returned, or one found by walking
/// a page's `/Annots`. Characteristics the builder leaves unset keep whatever
/// the widget already had.
///
/// The generated appearance follows automatically: `/BG` and `/BC` are read
/// every time a widget's appearance is rebuilt, so this needs no companion
/// call to redraw. A widget whose `/AP` is stale and which the document does
/// not mark with [`set_need_appearances`] may still show its old face in a
/// reader that trusts the stream, which is the ordinary appearance-staleness
/// question rather than anything specific to `/MK`.
///
/// # Errors
///
/// [`Error::UnresolvedRef`](crate::Error) when `annot` names no dictionary.
pub fn set_widget_appearance(
    dest: &mut EditDoc<'_>,
    annot: ObjRef,
    appearance: &WidgetAppearance,
) -> Result<()> {
    let Some(dict) = dest
        .fetch(annot)
        .ok()
        .and_then(|object| object.as_dict().cloned())
    else {
        return Err(Error::Object(pdfrum_object::Error::UnresolvedRef(annot)));
    };

    // `/MK` is usually direct, but an indirect one is edited in place so any
    // other widget sharing it keeps the same characteristics.
    match dict.raw(&mk_key()).cloned() {
        Some(Object::Ref(mk_ref)) => {
            let mk = dest
                .fetch(mk_ref)
                .ok()
                .and_then(|object| object.as_dict().cloned())
                .unwrap_or_default();
            dest.replace(mk_ref, Object::Dict(appearance.merge_into(mk)));
        }
        existing => {
            let mk = match existing {
                Some(Object::Dict(mk)) => mk,
                _ => Dict::new(),
            };
            let mut dict = dict;
            dict.insert(mk_key(), Object::Dict(appearance.merge_into(mk)));
            dest.replace(annot, Object::Dict(dict));
        }
    }
    Ok(())
}

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
