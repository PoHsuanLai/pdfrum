//! Signature fields, read as the file wrote them.

use crate::Document;

/// A signature field (ISO 32000-1 §12.8): its value dictionary's entries,
/// as written and unverified.
///
/// ```
/// let doc = pdfrum::Document::open("tests/fixtures/two_signatures.pdf")?;
/// let signatures = doc.signatures();
/// assert_eq!(signatures.len(), 2);
/// assert_eq!(signatures[0].sub_filter().as_deref(), Some("ETSI.CAdES.detached"));
/// assert_eq!(signatures[0].byte_range(), [0, 10, 30, 10]);
/// # Ok::<(), pdfrum::Error>(())
/// ```
#[derive(Debug, Clone)]
pub struct Signature<'a> {
    inner: pdfrum_doc::signature::Signature,
    doc: &'a Document,
}

impl Document {
    /// The document's signature fields: every top-level `/AcroForm /Fields`
    /// entry whose `/FT` is `Sig`, in order.
    #[must_use]
    pub fn signatures(&self) -> Vec<Signature<'_>> {
        pdfrum_doc::signature::signatures(&self.catalog(), &self.inner)
            .into_iter()
            .map(|inner| Signature { inner, doc: self })
            .collect()
    }
}

impl Signature<'_> {
    /// The `/Contents` bytes — the signature itself, typically DER — empty
    /// when absent.
    #[must_use]
    pub fn contents(&self) -> Vec<u8> {
        self.inner.contents(&self.doc.inner)
    }

    /// The `/ByteRange` as `[start, len, start, len, ...]`; empty when
    /// absent, and an element that is not an integer reads as 0.
    #[must_use]
    pub fn byte_range(&self) -> Vec<i64> {
        self.inner.byte_range(&self.doc.inner)
    }

    /// The `/SubFilter` name, such as `ETSI.CAdES.detached`.
    #[must_use]
    pub fn sub_filter(&self) -> Option<String> {
        self.inner.sub_filter(&self.doc.inner)
    }

    /// The `/Reason` text the signer gave.
    #[must_use]
    pub fn reason(&self) -> Option<String> {
        self.inner.reason(&self.doc.inner)
    }

    /// The `/M` signing time, as the PDF date string the file carries.
    #[must_use]
    pub fn time(&self) -> Option<String> {
        self.inner.time(&self.doc.inner)
    }

    /// The `DocMDP` `/P` permission level (ISO 32000-1 §12.8.2.2); 0 when the
    /// signature declares no `DocMDP` transform.
    #[must_use]
    pub fn doc_mdp_permission(&self) -> u32 {
        self.inner.doc_mdp_permission(&self.doc.inner)
    }
}
