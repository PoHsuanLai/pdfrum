//! Signature fields, read as the file wrote them.

use pdfrum_object::{Dict, Name, Resolve};

/// A signature field (ISO 32000-1 §12.8): its value dictionary's entries,
/// as written and unverified.
///
/// Read with [`signatures`], and every entry below is read through the same
/// resolver the fields were found with.
#[derive(Debug, Clone)]
pub struct Signature {
    field: Dict,
}

/// The document's signature fields: every top-level `/AcroForm /Fields`
/// entry whose `/FT` is `Sig`, in order.
///
/// ```
/// use pdfrum_doc::signature::signatures;
/// use pdfrum_object::{Array, Dict, Name, NoResolve, Object, PdfString};
///
/// let value = Dict::from_pairs([
///     (Name::from("SubFilter"), Object::Name(Name::from("ETSI.CAdES.detached"))),
///     (Name::from("Reason"), Object::Str(PdfString::literal(b"I agree"))),
/// ]);
/// let field = Dict::from_pairs([
///     (Name::from("FT"), Object::Name(Name::from("Sig"))),
///     (Name::from("V"), Object::Dict(value)),
/// ]);
/// // A field of another type is not a signature and is skipped.
/// let text = Dict::from_pairs([(Name::from("FT"), Object::Name(Name::from("Tx")))]);
/// let acro = Dict::from_pairs([(
///     Name::from("Fields"),
///     Object::Array(Array::of([Object::Dict(field), Object::Dict(text)])),
/// )]);
/// let catalog = Dict::from_pairs([(Name::from("AcroForm"), Object::Dict(acro))]);
///
/// let found = signatures(&catalog, &NoResolve);
/// assert_eq!(found.len(), 1);
/// assert_eq!(found[0].sub_filter(&NoResolve).as_deref(), Some("ETSI.CAdES.detached"));
/// assert_eq!(found[0].reason(&NoResolve).as_deref(), Some("I agree"));
/// // Nothing is verified: an absent `/Contents` simply reads empty.
/// assert!(found[0].contents(&NoResolve).is_empty());
///
/// // A catalog with no `/AcroForm` has no signatures.
/// assert!(signatures(&Dict::new(), &NoResolve).is_empty());
/// ```
#[must_use]
pub fn signatures(catalog: &Dict, r: &impl Resolve) -> Vec<Signature> {
    let Some(acro) = catalog.dict(&Name::from("AcroForm"), r) else {
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
        .map(|field| Signature { field })
        .collect()
}

impl Signature {
    /// The field's `/V` dictionary, where every entry below lives.
    fn value(&self, r: &impl Resolve) -> Option<Dict> {
        self.field.dict(&Name::from("V"), r)
    }

    /// The `/Contents` bytes — the signature itself, typically DER — empty
    /// when absent.
    ///
    /// ```
    /// use pdfrum_doc::signature::signatures;
    /// use pdfrum_object::{Array, Dict, Name, NoResolve, Object, PdfString};
    ///
    /// let value = Dict::from_pairs([(
    ///     Name::from("Contents"),
    ///     Object::Str(PdfString::literal(b"\x30\x82")),
    /// )]);
    /// let field = Dict::from_pairs([
    ///     (Name::from("FT"), Object::Name(Name::from("Sig"))),
    ///     (Name::from("V"), Object::Dict(value)),
    /// ]);
    /// let acro = Dict::from_pairs([(
    ///     Name::from("Fields"),
    ///     Object::Array(Array::of([Object::Dict(field)])),
    /// )]);
    /// let catalog = Dict::from_pairs([(Name::from("AcroForm"), Object::Dict(acro))]);
    ///
    /// assert_eq!(signatures(&catalog, &NoResolve)[0].contents(&NoResolve), b"\x30\x82");
    /// ```
    #[must_use]
    pub fn contents(&self, r: &impl Resolve) -> Vec<u8> {
        self.value(r)
            .and_then(|v| v.byte_string(&Name::from("Contents"), r))
            .unwrap_or_default()
    }

    /// The `/ByteRange` integers, the file ranges the signature covers; empty
    /// when absent, and an element that is not an integer reads as 0.
    ///
    /// ```
    /// use pdfrum_doc::signature::signatures;
    /// use pdfrum_object::{Array, Dict, Name, NoResolve, Object};
    ///
    /// let value = Dict::from_pairs([(
    ///     Name::from("ByteRange"),
    ///     // A non-integer element reads as 0 rather than shortening the list.
    ///     Object::Array(Array::of([
    ///         Object::Int(0),
    ///         Object::Int(10),
    ///         Object::Null,
    ///         Object::Int(10),
    ///     ])),
    /// )]);
    /// let field = Dict::from_pairs([
    ///     (Name::from("FT"), Object::Name(Name::from("Sig"))),
    ///     (Name::from("V"), Object::Dict(value)),
    /// ]);
    /// let acro = Dict::from_pairs([(
    ///     Name::from("Fields"),
    ///     Object::Array(Array::of([Object::Dict(field)])),
    /// )]);
    /// let catalog = Dict::from_pairs([(Name::from("AcroForm"), Object::Dict(acro))]);
    ///
    /// let found = signatures(&catalog, &NoResolve);
    /// assert_eq!(found[0].byte_range(&NoResolve), [0, 10, 0, 10]);
    /// ```
    #[must_use]
    pub fn byte_range(&self, r: &impl Resolve) -> Vec<i64> {
        let Some(array) = self
            .value(r)
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
    ///
    /// ```
    /// use pdfrum_doc::signature::signatures;
    /// use pdfrum_object::{Array, Dict, Name, NoResolve, Object};
    ///
    /// let sig = |value: Dict| {
    ///     let field = Dict::from_pairs([
    ///         (Name::from("FT"), Object::Name(Name::from("Sig"))),
    ///         (Name::from("V"), Object::Dict(value)),
    ///     ]);
    ///     let acro = Dict::from_pairs([(
    ///         Name::from("Fields"),
    ///         Object::Array(Array::of([Object::Dict(field)])),
    ///     )]);
    ///     Dict::from_pairs([(Name::from("AcroForm"), Object::Dict(acro))])
    /// };
    ///
    /// // Absent is `None`; present but not a name is an empty string.
    /// assert_eq!(signatures(&sig(Dict::new()), &NoResolve)[0].sub_filter(&NoResolve), None);
    /// let odd = Dict::from_pairs([(Name::from("SubFilter"), Object::Int(1))]);
    /// assert_eq!(
    ///     signatures(&sig(odd), &NoResolve)[0].sub_filter(&NoResolve).as_deref(),
    ///     Some(""),
    /// );
    /// ```
    #[must_use]
    pub fn sub_filter(&self, r: &impl Resolve) -> Option<String> {
        let value = self.value(r)?;
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
    pub fn reason(&self, r: &impl Resolve) -> Option<String> {
        let value = self.value(r)?;
        let object = value.get(&Name::from("Reason"), r)?;
        object.get().as_string().map(|s| s.as_text().into_owned())
    }

    /// The `/M` signing time as written, such as `D:20200624093114+02'00'`,
    /// when it is a string.
    ///
    /// ```
    /// use pdfrum_doc::signature::signatures;
    /// use pdfrum_object::{Array, Dict, Name, NoResolve, Object, PdfString};
    ///
    /// let value = Dict::from_pairs([(
    ///     Name::from("M"),
    ///     Object::Str(PdfString::literal(b"D:20200624093114+02'00'")),
    /// )]);
    /// let field = Dict::from_pairs([
    ///     (Name::from("FT"), Object::Name(Name::from("Sig"))),
    ///     (Name::from("V"), Object::Dict(value)),
    /// ]);
    /// let acro = Dict::from_pairs([(
    ///     Name::from("Fields"),
    ///     Object::Array(Array::of([Object::Dict(field)])),
    /// )]);
    /// let catalog = Dict::from_pairs([(Name::from("AcroForm"), Object::Dict(acro))]);
    ///
    /// assert_eq!(
    ///     signatures(&catalog, &NoResolve)[0].time(&NoResolve).as_deref(),
    ///     Some("D:20200624093114+02'00'"),
    /// );
    /// ```
    #[must_use]
    pub fn time(&self, r: &impl Resolve) -> Option<String> {
        let value = self.value(r)?;
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
    ///
    /// ```
    /// use pdfrum_doc::signature::signatures;
    /// use pdfrum_object::{Array, Dict, Name, NoResolve, Object};
    ///
    /// let params = Dict::from_pairs([(Name::from("P"), Object::Int(1))]);
    /// let reference = Dict::from_pairs([
    ///     (Name::from("TransformMethod"), Object::Name(Name::from("DocMDP"))),
    ///     (Name::from("TransformParams"), Object::Dict(params)),
    /// ]);
    /// let value = Dict::from_pairs([(
    ///     Name::from("Reference"),
    ///     Object::Array(Array::of([Object::Dict(reference)])),
    /// )]);
    /// let field = Dict::from_pairs([
    ///     (Name::from("FT"), Object::Name(Name::from("Sig"))),
    ///     (Name::from("V"), Object::Dict(value)),
    /// ]);
    /// let acro = Dict::from_pairs([(
    ///     Name::from("Fields"),
    ///     Object::Array(Array::of([Object::Dict(field)])),
    /// )]);
    /// let catalog = Dict::from_pairs([(Name::from("AcroForm"), Object::Dict(acro))]);
    ///
    /// assert_eq!(signatures(&catalog, &NoResolve)[0].doc_mdp_permission(&NoResolve), 1);
    /// ```
    #[must_use]
    pub fn doc_mdp_permission(&self, r: &impl Resolve) -> u32 {
        let Some(references) = self
            .value(r)
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
