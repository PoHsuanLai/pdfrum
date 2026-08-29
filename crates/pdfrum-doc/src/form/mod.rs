//! The AcroForm data model (ISO 32000-1 §12.7): fields, their inherited
//! attributes, and the widget annotations that present them.

pub mod attr;

pub use attr::{field_attr, full_name};
