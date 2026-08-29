//! Opening a file and everything that belongs to the document as a whole.

use std::path::Path;
use std::sync::Arc;

use pdfrum_common::{Diagnostics, Limits};
use pdfrum_object::{Dict, Name, Object, Resolve};

use crate::form::Form;
use crate::{Outline, Page, Result};

/// How to open a document.
///
/// A plain config struct with [`Default`], filled in with struct-update
/// syntax (STYLE.md §4):
///
/// ```
/// use pdfrum::OpenOptions;
///
/// let opts = OpenOptions {
///     password: Some(b"secret".to_vec()),
///     ..OpenOptions::default()
/// };
/// assert_eq!(opts.password.as_deref(), Some(b"secret".as_slice()));
/// ```
#[derive(Debug, Clone, Default)]
pub struct OpenOptions {
    /// The password to try, as raw bytes rather than a `str`: PDF passwords
    /// are byte strings and need not be UTF-8.
    ///
    /// Both the user and the owner password are accepted; which one opened
    /// the file decides what [`Document::permissions`] reports.
    pub password: Option<Vec<u8>>,
    /// The depth and size caps recovery enforces while reading.
    pub limits: Limits,
}

/// An open PDF document.
///
/// Holds the file's bytes, the cross-reference the reader recovered from
/// them, and a lazy object store. Pages are **not** parsed at open time —
/// [`Document::page`] interprets one on demand, so opening a thousand-page
/// file costs the same as opening a one-page file.
///
/// `Send + Sync`, deliberately: the type is what a rayon `par_iter` shares
/// across threads. See the crate docs for the parallel-rendering pattern.
///
/// ```
/// let doc = pdfrum::Document::open("tests/fixtures/hello_world.pdf")?;
/// assert_eq!(doc.page_count(), 1);
/// # Ok::<(), pdfrum::Error>(())
/// ```
#[derive(Debug)]
pub struct Document {
    pub(crate) inner: pdfrum_parser::Document,
    pub(crate) limits: Limits,
}

impl Document {
    /// Opens the file at `path`.
    ///
    /// Reads it into memory whole — PDF is a random-access format whose
    /// cross-reference table lives at the *end*, so there is no useful
    /// streaming read.
    ///
    /// # Errors
    ///
    /// [`Error::Io`](crate::Error::Io) if the file cannot be read, [`Error::Open`](crate::Error::Open) if what was
    /// read is not a PDF this reader can recover.
    ///
    /// ```
    /// let doc = pdfrum::Document::open("tests/fixtures/bookmarks.pdf")?;
    /// assert_eq!(doc.page_count(), 2);
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    pub fn open(path: impl AsRef<Path>) -> Result<Document> {
        Document::open_with(path, &OpenOptions::default())
    }

    /// Opens the file at `path`, trying `password`.
    ///
    /// # Errors
    ///
    /// [`Error::Open`](crate::Error::Open) wrapping
    /// [`LoadError::WrongPassword`](pdfrum_parser::LoadError::WrongPassword)
    /// when the password does not open the file — the variant to match on to
    /// decide whether to prompt again.
    pub fn open_with_password(path: impl AsRef<Path>, password: &[u8]) -> Result<Document> {
        Document::open_with(
            path,
            &OpenOptions {
                password: Some(password.to_vec()),
                ..OpenOptions::default()
            },
        )
    }

    /// Opens the file at `path` with explicit options.
    ///
    /// # Errors
    ///
    /// [`Error::Io`](crate::Error::Io) or [`Error::Open`](crate::Error::Open), as [`Document::open`].
    pub fn open_with(path: impl AsRef<Path>, options: &OpenOptions) -> Result<Document> {
        let bytes = std::fs::read(path.as_ref())?;
        Document::from_bytes_with(Arc::from(bytes), options)
    }

    /// Opens a document already in memory.
    ///
    /// Takes `Arc<[u8]>` rather than `Vec<u8>` because the document keeps the
    /// bytes for as long as it lives — every object is parsed lazily out of
    /// them — and an `Arc` lets a caller who already has the file share it
    /// instead of copying.
    ///
    /// # Errors
    ///
    /// [`Error::Open`](crate::Error::Open) when the bytes are not a recoverable PDF.
    ///
    /// ```
    /// use std::sync::Arc;
    ///
    /// let bytes = std::fs::read("tests/fixtures/hello_world.pdf")?;
    /// let doc = pdfrum::Document::from_bytes(Arc::from(bytes))?;
    /// assert_eq!(doc.page_count(), 1);
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    pub fn from_bytes(bytes: Arc<[u8]>) -> Result<Document> {
        Document::from_bytes_with(bytes, &OpenOptions::default())
    }

    /// Opens a document already in memory, with explicit options.
    ///
    /// # Errors
    ///
    /// [`Error::Open`](crate::Error::Open) when the bytes are not a recoverable PDF.
    pub fn from_bytes_with(bytes: Arc<[u8]>, options: &OpenOptions) -> Result<Document> {
        let load = pdfrum_parser::LoadOptions {
            password: options.password.clone(),
            limits: options.limits,
        };
        Ok(Document {
            inner: pdfrum_parser::load(bytes, &load)?,
            limits: options.limits,
        })
    }

    /// How many pages the document has.
    #[must_use]
    pub fn page_count(&self) -> u32 {
        self.inner.page_count()
    }

    /// One page, interpreted on demand.
    ///
    /// # Errors
    ///
    /// [`Error::Read`](crate::Error::Read) when `index` is past the end or the page tree has no
    /// page there.
    ///
    /// ```
    /// let doc = pdfrum::Document::open("tests/fixtures/hello_world.pdf")?;
    /// let page = doc.page(0)?;
    /// assert_eq!(page.width().round(), 200.0);
    /// assert!(doc.page(1).is_err(), "there is only one page");
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    pub fn page(&self, index: u32) -> Result<Page<'_>> {
        Page::load(self, index)
    }

    /// Every page in order, interpreted lazily as the iterator advances.
    ///
    /// A page that will not load is skipped rather than ending the walk,
    /// which is what a viewer does: one broken page does not hide the rest of
    /// the document. Use [`Document::page`] when you need to know.
    ///
    /// ```
    /// let doc = pdfrum::Document::open("tests/fixtures/bookmarks.pdf")?;
    /// let widths: Vec<f64> = doc.pages().map(|p| p.width().round()).collect();
    /// assert_eq!(widths, [612.0, 612.0]);
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    pub fn pages(&self) -> impl Iterator<Item = Page<'_>> + '_ {
        (0..self.page_count()).filter_map(|index| self.page(index).ok())
    }

    /// The label a page carries in `/PageLabels` — the "iv" or "A-3" a reader
    /// shows instead of the page's ordinal (ISO 32000-1 §12.4.2).
    ///
    /// `None` when the document numbers its pages plainly, which most do.
    #[must_use]
    pub fn page_label(&self, index: u32) -> Option<String> {
        let catalog = self.catalog();
        let mut diags = Diagnostics::default();
        pdfrum_doc::page_label(
            &catalog,
            i64::from(index),
            self.page_count(),
            &self.inner,
            &self.limits,
            &mut diags,
        )
    }

    /// The document's outline — its bookmarks, flattened to a pre-order walk
    /// that carries each entry's depth.
    ///
    /// Empty when the document has no `/Outlines`.
    ///
    /// ```
    /// let doc = pdfrum::Document::open("tests/fixtures/bookmarks.pdf")?;
    /// let titles: Vec<String> = doc.outline().iter().map(|b| b.title()).collect();
    /// assert_eq!(titles[0], "A Good Beginning");
    /// assert!(titles.iter().any(|t| t == "Open Middle Descendant"));
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    #[must_use]
    pub fn outline(&self) -> Outline<'_> {
        Outline::load(self)
    }

    /// The document's `/Info` metadata, as a value.
    ///
    /// ```
    /// let doc = pdfrum::Document::open("tests/fixtures/hello_world.pdf")?;
    /// // This fixture carries no /Info dictionary at all.
    /// assert_eq!(doc.metadata().title, None);
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    #[must_use]
    pub fn metadata(&self) -> Metadata {
        Metadata::read(self)
    }

    /// The catalog's `/Metadata` stream — the document's XMP packet, decoded,
    /// exactly as the file stored it.
    ///
    /// Returned as bytes rather than parsed: XMP is RDF/XML, and this crate
    /// does not carry an XML parser.
    #[must_use]
    pub fn xmp_metadata(&self) -> Option<Vec<u8>> {
        let mut diags = Diagnostics::default();
        pdfrum_doc::xmp(&self.catalog(), &self.inner, &self.limits, &mut diags)
    }

    /// The document's interactive form, if it has one.
    ///
    /// ```
    /// let doc = pdfrum::Document::open("tests/fixtures/text_form.pdf")?;
    /// let form = doc.form().expect("this fixture has an AcroForm");
    /// assert_eq!(form.field_count(), 1);
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    #[must_use]
    pub fn form(&self) -> Option<Form<'_>> {
        Form::load(self)
    }

    /// The files attached to the document as a whole, from the catalog's
    /// `/Names /EmbeddedFiles` name tree (ISO 32000-1 §7.11.4).
    ///
    /// Page-level file attachments are annotations instead, and are reached
    /// through [`Page::annotations`].
    #[must_use]
    pub fn attachments(&self) -> Vec<Attachment<'_>> {
        let mut diags = Diagnostics::default();
        let Some(names) = self.catalog().dict(&Name::from("Names"), &self.inner) else {
            return Vec::new();
        };
        let Some(tree) = names.dict(&Name::from("EmbeddedFiles"), &self.inner) else {
            return Vec::new();
        };
        let tree = pdfrum_doc::NameTree { root: tree };
        let count = tree.count(&self.inner, &self.limits, &mut diags);
        (0..count)
            .filter_map(|index| {
                let (name, value) =
                    tree.lookup_by_index(index, &self.inner, &self.limits, &mut diags)?;
                Some(Attachment {
                    name,
                    spec: pdfrum_doc::FileSpec::new(value),
                    doc: self,
                })
            })
            .collect()
    }

    /// Everything the reader repaired, worked around, or refused while
    /// opening the file.
    ///
    /// Damage tolerance is a channel of its own, not an error (STYLE.md §3):
    /// a document with a rebuilt cross-reference table opens successfully and
    /// says so here.
    ///
    /// ```
    /// let doc = pdfrum::Document::open("tests/fixtures/hello_world.pdf")?;
    /// // This fixture's content stream deliberately omits its /Length, which
    /// // the parser recovers from and records.
    /// for entry in doc.diagnostics().entries() {
    ///     println!("{:?} at {:?}", entry.what, entry.at);
    /// }
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    #[must_use]
    pub fn diagnostics(&self) -> &Diagnostics {
        &self.inner.diags
    }

    /// The PDF version the header declares, as major × 10 + minor: `17` for
    /// `%PDF-1.7`.
    #[must_use]
    pub fn version(&self) -> u8 {
        self.inner.version()
    }

    /// Whether the file was encrypted.
    ///
    /// A document that opened is already decrypted in memory; this reports
    /// how it arrived, which decides whether a save must drop the security
    /// handler.
    #[must_use]
    pub fn is_encrypted(&self) -> bool {
        self.inner.is_encrypted()
    }

    /// The permission bits the security handler grants (ISO 32000-1 §7.6.4).
    ///
    /// `owner` asks for the owner's permissions rather than the user's. An
    /// unencrypted document grants everything.
    #[must_use]
    pub fn permissions(&self, owner: bool) -> u32 {
        self.inner.permissions(owner)
    }

    /// Whether the cross-reference table had to be rebuilt by scanning the
    /// file for objects — the strongest single signal that a document is
    /// damaged.
    #[must_use]
    pub fn xref_was_rebuilt(&self) -> bool {
        self.inner.xref_was_rebuilt()
    }

    /// The document's raw bytes, shared.
    #[must_use]
    pub fn bytes(&self) -> &Arc<[u8]> {
        self.inner.bytes()
    }

    /// The document catalog (`/Root`), or an empty dictionary when the file
    /// has none this reader could reach.
    pub(crate) fn catalog(&self) -> Dict {
        self.inner.catalog().unwrap_or_default()
    }

    /// The parser document underneath, for a caller who needs the object
    /// model this crate deliberately does not re-export.
    ///
    /// The escape hatch, and it is meant to be one: everything below the
    /// facade is a stable public API of its own, so reaching for a `/Dict`
    /// key nothing here surfaces does not require forking anything.
    #[must_use]
    pub fn parser(&self) -> &pdfrum_parser::Document {
        &self.inner
    }
}

impl Resolve for Document {
    fn fetch(
        &self,
        r: pdfrum_object::ObjRef,
    ) -> std::result::Result<Arc<Object>, pdfrum_object::Error> {
        self.inner.fetch(r)
    }
}

/// A document's `/Info` metadata (ISO 32000-1 §14.3.3).
///
/// Every field is optional because every key is: a document may carry all of
/// them, some, or no `/Info` dictionary at all. The two date fields are the
/// raw PDF date strings (`D:YYYYMMDDHHmmSS`), unparsed — this crate does not
/// carry a calendar.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Metadata {
    /// `/Title`.
    pub title: Option<String>,
    /// `/Author`.
    pub author: Option<String>,
    /// `/Subject`.
    pub subject: Option<String>,
    /// `/Keywords`.
    pub keywords: Option<String>,
    /// `/Creator` — the application the content came from.
    pub creator: Option<String>,
    /// `/Producer` — the library that wrote the PDF.
    pub producer: Option<String>,
    /// `/CreationDate`, as written.
    pub creation_date: Option<String>,
    /// `/ModDate`, as written.
    pub modification_date: Option<String>,
}

impl Metadata {
    fn read(doc: &Document) -> Metadata {
        let Some(info) = doc
            .inner
            .trailer()
            .dict(&Name::from("Info"), &doc.inner)
            .filter(|info| !info.is_empty())
        else {
            return Metadata::default();
        };
        let text = |key: &str| info.text(&Name::from(key), &doc.inner);
        Metadata {
            title: text("Title"),
            author: text("Author"),
            subject: text("Subject"),
            keywords: text("Keywords"),
            creator: text("Creator"),
            producer: text("Producer"),
            creation_date: text("CreationDate"),
            modification_date: text("ModDate"),
        }
    }
}

/// One file attached to the document.
#[derive(Debug, Clone)]
pub struct Attachment<'a> {
    /// The name the document's embedded-files tree filed it under.
    pub name: String,
    spec: pdfrum_doc::FileSpec,
    doc: &'a Document,
}

impl Attachment<'_> {
    /// The attachment's own file name, from its file specification — which
    /// need not match the name it is filed under.
    #[must_use]
    pub fn file_name(&self) -> String {
        self.spec.file_name(&self.doc.inner)
    }

    /// The attached file's bytes, decoded.
    ///
    /// `None` when the specification names no embedded stream — a file
    /// specification may point at a path on disk instead.
    #[must_use]
    pub fn data(&self) -> Option<Vec<u8>> {
        let stream = self.spec.file_stream(&self.doc.inner)?;
        let mut diags = Diagnostics::default();
        Some(
            pdfrum_filters::decode_chain(&stream, 0, &self.doc.inner, &self.doc.limits, &mut diags)
                .data,
        )
    }
}
