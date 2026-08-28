//! The crate's error type.
//!
//! Almost nothing here can fail. Reading a CMap program, resolving a
//! predefined name and decoding a string are all total: damage is recorded
//! through [`Diagnostics`](pdfrum_common::Diagnostics) and a best-effort value
//! comes back. The one genuinely fallible operation is looking a name up
//! against the built-in tables and finding nothing — and even that has a
//! total counterpart in [`from_encoding_name`](crate::from_encoding_name),
//! which is what callers normally want.

/// Something a CMap operation could not do.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The `/Encoding` name is not one of the built-in CMaps and is not
    /// `Identity-H` or `Identity-V`.
    ///
    /// This is rarely fatal: a file naming an unknown CMap still renders,
    /// because the fallback decodes two-byte codes and maps each to itself.
    /// [`from_encoding_name`](crate::from_encoding_name) builds that fallback
    /// instead of returning this.
    #[error("no predefined CMap is named {name:?}")]
    UnknownPredefinedCMap {
        /// The name as it was asked for, before any suffix handling.
        name: Vec<u8>,
    },
}
