//! The document's XMP metadata packet, as bytes.
//!
//! There is no XML parsing here and none is planned. Upstream's only use of
//! `/Metadata` is a scan for an Acrobat shared-review workflow marker, whose
//! sole consumer prints an "unsupported feature" line to **stderr** — it
//! reaches no output any consumer diffs. Callers that want the packet's
//! contents parse it themselves.

use pdfrum_common::{Diagnostics, Limits};
use pdfrum_object::{Dict, Resolve};

use crate::names;

/// The catalog's `/Metadata` stream, decoded.
///
/// Returns nothing when the key is absent or does not hold a stream.
#[must_use]
pub fn xmp<R: Resolve>(
    catalog: &Dict,
    r: &R,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> Option<Vec<u8>> {
    let stream = catalog.stream(names::METADATA, r)?;
    Some(pdfrum_filters::decode_chain(&stream, 0, r, limits, diags).data)
}
