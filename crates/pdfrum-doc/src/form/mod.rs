//! The AcroForm data model (ISO 32000-1 §12.7): fields, their inherited
//! attributes, their values, and the edits that write a filled form back out.
//!
//! No JavaScript. A `/AA` script action on a field is data this crate will
//! hand you ([`Action::javascript`](crate::Action::javascript)); executing it
//! is out of scope permanently, which is what "fill, no JS" means.

pub(crate) mod attr;
mod field;

pub use attr::{field_attr, full_name};
pub use field::{
    Field, FieldFlags, FieldKind, FieldValues, Form, apply, selected_indices_for_interaction,
};
