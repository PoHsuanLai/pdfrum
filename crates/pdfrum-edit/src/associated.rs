//! Associated files (ISO 32000-2 §14.13): `/AF`, the array that says an
//! embedded file *belongs to* something rather than merely riding along in
//! the document.
//!
//! An attachment in `/Names /EmbeddedFiles` is a file a reader offers to save.
//! The same file listed in an `/AF` is a file a reader is told relates to the
//! document, or to one page of it, with `/AFRelationship` saying how. That
//! distinction is what PDF/A-3 and the electronic-invoice profiles built on
//! it — Factur-X and its predecessors — are about: the XML invoice has to be
//! **the source** of the rendered page, not an attachment that happens to sit
//! beside it.
//!
//! So this does not embed anything. It associates a file the document already
//! carries, which is why every entry here names an attachment by index.

use pdfrum_common::{Limits, PageIndex};
use pdfrum_object::{Array, Dict, Name, ObjRef, Object, Resolve, names};

use crate::{attach, doc::EditDoc, error::Error};

/// An association write either applies or names why it could not.
type Result<T> = core::result::Result<T, Error>;

/// What an associated file is to the thing it is associated with
/// (`/AFRelationship`, ISO 32000-2 table 404).
///
/// The relationship is not decoration: a PDF/A-3 validator reads it, and an
/// electronic-invoice profile requires the invoice XML to be
/// [`Alternative`](Self::Alternative) — the machine-readable form of what the
/// page shows — rather than [`Supplement`](Self::Supplement) or
/// [`Unspecified`](Self::Unspecified).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Relationship {
    /// `/Source`: the file the visible content was generated from.
    Source,
    /// `/Data`: data the content presents — a spreadsheet behind a chart.
    Data,
    /// `/Alternative`: an alternative representation of the same content.
    /// This is what an electronic invoice's XML is.
    Alternative,
    /// `/Supplement`: additional material the content does not itself show.
    Supplement,
    /// `/EncryptedPayload`: the file is an encrypted payload, and the
    /// document around it is the wrapper.
    EncryptedPayload,
    /// `/FormData`: the data of a form the document presents.
    FormData,
    /// `/Schema`: a schema the associated data conforms to.
    Schema,
    /// `/Unspecified`: the relationship is not stated. The default, and what
    /// a validator that requires one will reject.
    #[default]
    Unspecified,
}

impl Relationship {
    /// The `/AFRelationship` name this relationship writes.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Source => "Source",
            Self::Data => "Data",
            Self::Alternative => "Alternative",
            Self::Supplement => "Supplement",
            Self::EncryptedPayload => "EncryptedPayload",
            Self::FormData => "FormData",
            Self::Schema => "Schema",
            Self::Unspecified => "Unspecified",
        }
    }

    /// The relationship an `/AFRelationship` name means, or nothing for one
    /// outside the table.
    #[must_use]
    pub fn from_bytes(bytes: &[u8]) -> Option<Relationship> {
        match bytes {
            b"Source" => Some(Self::Source),
            b"Data" => Some(Self::Data),
            b"Alternative" => Some(Self::Alternative),
            b"Supplement" => Some(Self::Supplement),
            b"EncryptedPayload" => Some(Self::EncryptedPayload),
            b"FormData" => Some(Self::FormData),
            b"Schema" => Some(Self::Schema),
            b"Unspecified" => Some(Self::Unspecified),
            _ => None,
        }
    }
}

/// Associates the attachment at `index` with the **document**, by adding it to
/// the catalog's `/AF`.
///
/// The attachment stays where it is; this adds a second reference to the same
/// file specification, which is what makes the file both savable from the
/// attachments pane and declared as related to the document. `relationship`
/// is written onto the specification as `/AFRelationship`.
///
/// Answers `false` when there is no attachment at `index`. An attachment
/// already in the catalog's `/AF` has only its relationship updated rather
/// than being listed twice.
///
/// # Errors
///
/// [`Error::NoDestinationCatalog`] when the document has no catalog.
///
/// ```
/// use std::sync::Arc;
/// use pdfrum_common::Limits;
/// use pdfrum_edit::{
///     AttachmentOptions, EditDoc, Relationship, SaveOptions, add_attachment,
///     associate_file_with_document, save,
/// };
/// use pdfrum_parser::{LoadOptions, load};
///
/// let bytes: Arc<[u8]> = Arc::from(&include_bytes!("../tests/files/hello.pdf")[..]);
/// let doc = load(bytes, &LoadOptions::default())?;
/// let mut edit = EditDoc::new(&doc);
/// let limits = Limits::default();
///
/// let index = add_attachment(
///     &mut edit,
///     &limits,
///     "invoice.xml",
///     b"<invoice/>",
///     &AttachmentOptions::builder().mime_type("text/xml").build(),
/// )?;
/// associate_file_with_document(&mut edit, &limits, index, Relationship::Alternative)?;
///
/// let mut out = Vec::new();
/// save(&edit, &SaveOptions::default(), &mut out)?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn associate_file_with_document(
    dest: &mut EditDoc<'_>,
    limits: &Limits,
    index: usize,
    relationship: Relationship,
) -> Result<bool> {
    let Some(spec_ref) = tag_relationship(dest, limits, index, relationship)? else {
        return Ok(false);
    };
    let Some(root) = dest.base().trailer().reference(names::ROOT) else {
        return Err(Error::NoDestinationCatalog);
    };
    let Some(mut catalog) = dict_at(dest, root) else {
        return Err(Error::NoDestinationCatalog);
    };
    let af = push_unique(dest, &catalog, spec_ref);
    catalog.insert(af_key(), Object::Array(af));
    dest.replace(root, Object::Dict(catalog));
    Ok(true)
}

/// Associates the attachment at `index` with one **page**, by adding it to
/// that page's `/AF`.
///
/// A page-level association says the file relates to that page in particular —
/// the measurement data behind one chart, say — where the catalog-level one
/// speaks for the document. A file may be in both.
///
/// Answers `false` when there is no attachment at `index`.
///
/// # Errors
///
/// - [`Error::PageIndexOutOfRange`] / [`Error::InlinePage`] for a bad page.
/// - [`Error::NoDestinationCatalog`] when the document has no catalog.
pub fn associate_file_with_page(
    dest: &mut EditDoc<'_>,
    limits: &Limits,
    page: impl Into<PageIndex>,
    index: usize,
    relationship: Relationship,
) -> Result<bool> {
    let page = page.into();
    let Some((page_ref, page_dict, _)) = dest.page_state(page)? else {
        return Err(Error::InlinePage(page));
    };
    let Some(spec_ref) = tag_relationship(dest, limits, index, relationship)? else {
        return Ok(false);
    };
    let mut page_dict = page_dict;
    let af = push_unique(dest, &page_dict, spec_ref);
    page_dict.insert(af_key(), Object::Array(af));
    dest.replace(page_ref, Object::Dict(page_dict));
    Ok(true)
}

/// The attachments the catalog's `/AF` names, by index into
/// [`attachments`](crate::add_attachment)'s ordering, each with the
/// relationship its specification states.
///
/// A specification in `/AF` that is not one of the document's attachments is
/// skipped: there is no index to report it under, and the read side addresses
/// attachments by index.
///
/// # Errors
///
/// [`Error::NoDestinationCatalog`] when the document has no catalog.
pub fn document_associated_files(
    dest: &EditDoc<'_>,
    limits: &Limits,
) -> Result<Vec<(usize, Relationship)>> {
    let Some(root) = dest.base().trailer().reference(names::ROOT) else {
        return Err(Error::NoDestinationCatalog);
    };
    let Some(catalog) = dict_at(dest, root) else {
        return Err(Error::NoDestinationCatalog);
    };
    associated_in(dest, limits, &catalog)
}

/// As [`document_associated_files`], for one page's `/AF`.
///
/// # Errors
///
/// - [`Error::PageIndexOutOfRange`] / [`Error::InlinePage`] for a bad page.
/// - [`Error::NoDestinationCatalog`] when the document has no catalog.
pub fn page_associated_files(
    dest: &EditDoc<'_>,
    limits: &Limits,
    page: impl Into<PageIndex>,
) -> Result<Vec<(usize, Relationship)>> {
    let page = page.into();
    let Some((_, page_dict, _)) = dest.page_state(page)? else {
        return Err(Error::InlinePage(page));
    };
    associated_in(dest, limits, &page_dict)
}

/// The `/AF` entries of one dictionary, resolved to attachment indices.
fn associated_in(
    dest: &EditDoc<'_>,
    limits: &Limits,
    holder: &Dict,
) -> Result<Vec<(usize, Relationship)>> {
    let Some(af) = holder.array(&af_key(), dest) else {
        return Ok(Vec::new());
    };
    let specs = attachment_spec_refs(dest, limits)?;
    let mut out = Vec::new();
    for position in 0..af.len() {
        let Some(reference) = af.reference_at(position) else {
            continue;
        };
        let Some(index) = specs.iter().position(|spec| *spec == Some(reference)) else {
            continue;
        };
        let relationship = dest
            .fetch(reference)
            .ok()
            .and_then(|object| object.as_dict().cloned())
            .and_then(|spec| {
                spec.raw(&relationship_key())
                    .and_then(Object::as_name)
                    .cloned()
            })
            .and_then(|name| Relationship::from_bytes(name.as_bytes()))
            .unwrap_or_default();
        out.push((index, relationship));
    }
    Ok(out)
}

/// Writes `/AFRelationship` onto the attachment's specification and answers
/// the reference that names it.
///
/// `None` when there is no attachment at `index`, or when its specification is
/// written inline — an inline specification has no reference for an `/AF` to
/// hold, which is the same limitation the rest of the writer has for inline
/// pages.
fn tag_relationship(
    dest: &mut EditDoc<'_>,
    limits: &Limits,
    index: usize,
    relationship: Relationship,
) -> Result<Option<ObjRef>> {
    let Some(spec_ref) = attach::attachment_spec_ref(dest, limits, index)? else {
        return Ok(None);
    };
    let Some(mut spec) = dict_at(dest, spec_ref) else {
        return Ok(None);
    };
    spec.insert(
        relationship_key(),
        Object::Name(Name::from(relationship.as_str().as_bytes())),
    );
    dest.replace(spec_ref, Object::Dict(spec));
    Ok(Some(spec_ref))
}

/// Every attachment's specification reference, in attachment order.
fn attachment_spec_refs(dest: &EditDoc<'_>, limits: &Limits) -> Result<Vec<Option<ObjRef>>> {
    let count = attach::attachment_entries(dest, limits)?.len();
    (0..count)
        .map(|index| attach::attachment_spec_ref(dest, limits, index))
        .collect()
}

/// `holder`'s `/AF` with `spec_ref` in it, added only if it is not already.
///
/// Listing one specification twice would have a reader offer the same file
/// twice, and the relationship is on the specification rather than on the
/// entry, so a second entry could not say anything the first does not.
fn push_unique(dest: &EditDoc<'_>, holder: &Dict, spec_ref: ObjRef) -> Array {
    let mut af: Array = holder
        .array(&af_key(), dest)
        .map(|existing| existing.iter().cloned().collect())
        .unwrap_or_default();
    let already = af
        .iter()
        .any(|entry| matches!(entry, Object::Ref(reference) if *reference == spec_ref));
    if !already {
        af.push(Object::Ref(spec_ref));
    }
    af
}

/// A dictionary as the edits leave it.
fn dict_at(dest: &EditDoc<'_>, reference: ObjRef) -> Option<Dict> {
    dest.fetch(reference)
        .ok()
        .and_then(|object| object.as_dict().cloned())
}

/// The `/AF` key.
fn af_key() -> Name {
    Name::from("AF")
}

/// The `/AFRelationship` key.
fn relationship_key() -> Name {
    Name::from("AFRelationship")
}
