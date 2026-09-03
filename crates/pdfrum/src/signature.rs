//! Signature fields, read as the file wrote them.

use crate::Document;
use pdfrum_object::{Dict, Name};

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
    field: Dict,
    doc: &'a Document,
}

impl Document {
    /// The document's signature fields: every top-level `/AcroForm /Fields`
    /// entry whose `/FT` is `Sig`, in order.
    #[must_use]
    pub fn signatures(&self) -> Vec<Signature<'_>> {
        let r = &self.inner;
        let Some(acro) = self.catalog().dict(&Name::from("AcroForm"), r) else {
            return Vec::new();
        };
        let Some(fields) = acro.array(&Name::from("Fields"), r) else {
            return Vec::new();
        };
        let ft = Name::from("FT");
        fields
            .iter()
            .filter_map(|object| object.resolve(r).ok()?.get().as_dict().cloned())
            .filter(|dict| {
                dict.get(&ft, r).is_some_and(|value| {
                    value
                        .get()
                        .as_name()
                        .is_some_and(|n| n.as_bytes() == b"Sig")
                })
            })
            .map(|field| Signature { field, doc: self })
            .collect()
    }
}

impl Signature<'_> {
    /// The field's `/V` dictionary, where every entry below lives.
    fn value(&self) -> Option<Dict> {
        self.field.dict(&Name::from("V"), &self.doc.inner)
    }

    /// The `/Contents` bytes — the signature itself, typically DER — empty
    /// when absent.
    #[must_use]
    pub fn contents(&self) -> Vec<u8> {
        self.value()
            .and_then(|v| v.byte_string(&Name::from("Contents"), &self.doc.inner))
            .unwrap_or_default()
    }

    /// The `/ByteRange` integers, the file ranges the signature covers; empty
    /// when absent, and an element that is not an integer reads as 0.
    #[must_use]
    pub fn byte_range(&self) -> Vec<i64> {
        let r = &self.doc.inner;
        let Some(array) = self
            .value()
            .and_then(|v| v.array(&Name::from("ByteRange"), r))
        else {
            return Vec::new();
        };
        array
            .iter()
            .map(|object| {
                object
                    .resolve(r)
                    .ok()
                    .and_then(|o| o.get().as_int())
                    .unwrap_or(0)
            })
            .collect()
    }

    /// The `/SubFilter` name, such as `ETSI.CAdES.detached`: `None` when the
    /// key is absent, empty when it is present but not a name.
    #[must_use]
    pub fn sub_filter(&self) -> Option<String> {
        let r = &self.doc.inner;
        let value = self.value()?;
        let key = Name::from("SubFilter");
        if !value.contains_key(&key) {
            return None;
        }
        let name = value.get(&key, r).and_then(|o| {
            o.get()
                .as_name()
                .map(|n| String::from_utf8_lossy(n.as_bytes()).into_owned())
        });
        Some(name.unwrap_or_default())
    }

    /// The `/Reason` text, when it is a string.
    #[must_use]
    pub fn reason(&self) -> Option<String> {
        let r = &self.doc.inner;
        let value = self.value()?;
        let object = value.get(&Name::from("Reason"), r)?;
        object.get().as_string().map(|s| s.as_text().into_owned())
    }

    /// The `/M` signing time as written, such as `D:20200624093114+02'00'`,
    /// when it is a string.
    #[must_use]
    pub fn time(&self) -> Option<String> {
        let r = &self.doc.inner;
        let value = self.value()?;
        let object = value.get(&Name::from("M"), r)?;
        object
            .get()
            .as_string()
            .map(|s| String::from_utf8_lossy(&s.bytes).into_owned())
    }

    /// The `DocMDP` permission level (ISO 32000-1 §12.8.2.2): 1, 2 or 3 from
    /// the first `/Reference` whose transform method is `DocMDP` (its
    /// `/TransformParams /P`, 2 when unstated), and 0 when there is none or
    /// the value is out of range.
    #[must_use]
    pub fn doc_mdp_permission(&self) -> u32 {
        let r = &self.doc.inner;
        let Some(references) = self
            .value()
            .and_then(|v| v.array(&Name::from("Reference"), r))
        else {
            return 0;
        };
        for object in references.iter() {
            let Some(reference) = object
                .resolve(r)
                .ok()
                .and_then(|o| o.get().as_dict().cloned())
            else {
                continue;
            };
            let is_doc_mdp = reference
                .get(&Name::from("TransformMethod"), r)
                .is_some_and(|o| o.get().as_name().is_some_and(|n| n.as_bytes() == b"DocMDP"));
            if !is_doc_mdp {
                continue;
            }
            let Some(params) = reference.dict(&Name::from("TransformParams"), r) else {
                continue;
            };
            let permission = params.int(&Name::from("P"), r).unwrap_or(2);
            return if (1..=3).contains(&permission) {
                u32::try_from(permission).unwrap_or(0)
            } else {
                0
            };
        }
        0
    }
}
