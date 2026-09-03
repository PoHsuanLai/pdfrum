//! Opening a file and everything that belongs to the document as a whole.

use std::path::Path;
use std::sync::{Arc, Mutex};

use pdfrum_common::{Diagnostics, Limits, PageIndex, PdfVersion};
use pdfrum_crypt::Permissions;
use pdfrum_object::{Dict, Name, Object, Resolve};

use crate::form::Form;
use crate::{Outline, Page, Result};

/// How to open a document.
///
/// A config struct with [`Default`], filled in with struct-update syntax:
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
#[derive(Debug)]
pub struct Document {
    pub(crate) inner: pdfrum_parser::Document,
    pub(crate) limits: Limits,
    /// What *this crate's* reads have repaired since the file opened.
    ///
    /// Interior mutability for a lazy record: every read below this crate
    /// takes a `&mut Diagnostics` while this crate's methods take `&self`, so
    /// what those reads recover has nowhere else to go.
    ///
    /// It is deliberately *not* what [`Document::diagnostics`] returns: that
    /// stays the load-time snapshot, so an existing caller's answer does not
    /// start changing under it. [`Document::all_diagnostics`] is the
    /// document-wide view.
    session: Mutex<Diagnostics>,
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
    pub fn open(path: impl AsRef<Path>) -> Result<Document> {
        Document::open_with(path, &OpenOptions::default())
    }

    /// Opens the file at `path`, trying `password`.
    ///
    /// ```
    /// use pdfrum::{Document, Error};
    ///
    /// const ENCRYPTED: &str = "tests/fixtures/encrypted.pdf";
    ///
    /// // The user password opens it.
    /// let doc = Document::open_with_password(ENCRYPTED, b"1234")?;
    /// assert_eq!(doc.page_count(), 1);
    ///
    /// // Anything else is the one error worth prompting again on.
    /// match Document::open_with_password(ENCRYPTED, b"nope") {
    ///     Err(Error::WrongPassword) => {} // ask the user again
    ///     other => panic!("expected a wrong password, got {other:?}"),
    /// }
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    ///
    /// # Errors
    ///
    /// [`Error::WrongPassword`](crate::Error::WrongPassword) when the password
    /// does not open the file — the variant to match on to decide whether to
    /// prompt again. Everything else an open can fail with is
    /// [`Error::Open`](crate::Error::Open).
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
            session: Mutex::new(Diagnostics::default()),
        })
    }

    /// How many pages the document has.
    ///
    /// A `u32` and not a [`PageIndex`]: a count is not an index. The valid
    /// indices of a three-page document are 0, 1 and 2, and giving the count
    /// and the index one type would let each be passed where the other is
    /// meant.
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
    pub fn page(&self, index: impl Into<PageIndex>) -> Result<Page<'_>> {
        Page::load(self, index.into())
    }

    /// Every page in order, interpreted lazily as the iterator advances.
    ///
    /// A page that will not load is skipped rather than ending the walk,
    /// which is what a viewer does: one broken page does not hide the rest of
    /// the document. Use [`Document::page`] when you need to know.
    pub fn pages(&self) -> impl Iterator<Item = Page<'_>> + '_ {
        (0..self.page_count()).filter_map(|index| self.page(index).ok())
    }

    /// The label a page carries in `/PageLabels` — the "iv" or "A-3" a reader
    /// shows instead of the page's ordinal (ISO 32000-1 §12.4.2).
    ///
    /// `None` when the document numbers its pages plainly, which most do.
    #[must_use]
    pub fn page_label(&self, index: impl Into<PageIndex>) -> Option<String> {
        let catalog = self.catalog();
        let mut diags = Diagnostics::default();
        let label = pdfrum_doc::page_label(
            &catalog,
            index.into(),
            self.page_count(),
            &self.inner,
            &self.limits,
            &mut diags,
        );
        self.note(&diags);
        label
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
        let xmp = pdfrum_doc::xmp(&self.catalog(), &self.inner, &self.limits, &mut diags);
        self.note(&diags);
        xmp
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
        let attachments = (0..count)
            .filter_map(|index| {
                let (name, value) =
                    tree.lookup_by_index(index, &self.inner, &self.limits, &mut diags)?;
                Some(Attachment {
                    name,
                    spec: pdfrum_doc::FileSpec::new(value),
                    doc: self,
                })
            })
            .collect();
        self.note(&diags);
        attachments
    }

    /// Everything the reader repaired, worked around, or refused while
    /// opening the file.
    ///
    /// Damage tolerance is a channel of its own, not an error: a document
    /// with a rebuilt cross-reference table opens successfully and says so
    /// here. [`Document::all_diagnostics`] is the running total over every
    /// lazy read since.
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

    /// Everything this document has needed repaired **in total** — at load,
    /// and in every lazy read since.
    ///
    /// [`Document::diagnostics`] is the load-time snapshot alone. Reading is
    /// lazy: an object is parsed when something first asks for it, a content
    /// stream is decoded when its page is rendered, and a font is substituted
    /// when a glyph from it is drawn, so a bad `/Length` on page 400 is a
    /// repair that happens hundreds of calls after `open` returned.
    ///
    /// It is a **running total** — read it *after* the work. A document opened
    /// but not yet rendered has nothing to say about its content streams.
    #[must_use]
    pub fn all_diagnostics(&self) -> Diagnostics {
        let mut all = self.inner.diags.clone();
        all.extend(&self.inner.lazy_diagnostics());
        if let Ok(guard) = self.session.lock() {
            all.extend(&guard);
        }
        all
    }

    /// Fold what a call recovered into the document's running record.
    ///
    /// Every facade method that reads through the stack creates a sink to
    /// satisfy the signatures below it; this is where that sink goes instead
    /// of into the floor. A poisoned lock drops the entries rather than
    /// panicking — losing a diagnostic is not worth failing a render for.
    pub(crate) fn note(&self, diags: &Diagnostics) {
        if diags.is_empty() {
            return;
        }
        if let Ok(mut guard) = self.session.lock() {
            guard.extend(diags);
        }
    }

    /// The PDF version the header declares: [`PdfVersion::PDF_1_7`] for
    /// `%PDF-1.7`, and `None` for a file that declares none.
    ///
    /// Never validated — a header claiming 9.9 reports 9.9.
    #[must_use]
    pub fn version(&self) -> Option<PdfVersion> {
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

    /// What the document permits, for the password that opened it
    /// (ISO 32000-1 §7.6.4).
    ///
    /// An unencrypted document grants everything. The owner's own
    /// unrestricted view is [`Document::owner_permissions`].
    ///
    /// ```
    /// let doc = pdfrum::Document::open("tests/fixtures/hello_world.pdf")?;
    /// assert!(doc.permissions().print);
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    #[must_use]
    pub fn permissions(&self) -> Permissions {
        self.inner.permissions()
    }

    /// What the document permits under the owner's view.
    ///
    /// Every permission, for a document the owner password opened; otherwise
    /// the same answer as [`Document::permissions`].
    #[must_use]
    pub fn owner_permissions(&self) -> Permissions {
        self.inner.owner_permissions()
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

    /// **Escape hatch — requires `pdfrum-parser`.** The parser document
    /// underneath, for a caller who needs the object model this crate
    /// deliberately does not re-export.
    ///
    /// It is meant to be one: everything below the facade is a stable public
    /// API of its own, so reaching for a `/Dict` key nothing here surfaces
    /// does not require forking anything. The return type stays namespaced so
    /// the collision with this crate's own [`Document`] is visible at the call
    /// site rather than a surprise at the type checker.
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
        let decoded =
            pdfrum_filters::decode_chain(&stream, 0, &self.doc.inner, &self.doc.limits, &mut diags);
        self.doc.note(&diags);
        Some(decoded.data)
    }

    /// The `/Desc` text of the file specification: empty when absent or not
    /// a string.
    #[must_use]
    pub fn description(&self) -> String {
        self.spec
            .object
            .as_dict()
            .and_then(|dict| dict.text(&Name::from("Desc"), &self.doc.inner))
            .unwrap_or_default()
    }

    /// The embedded file's `/Subtype` — its MIME type, such as `text/plain`:
    /// `None` without an embedded file, empty when the stream names none.
    #[must_use]
    pub fn subtype(&self) -> Option<String> {
        let stream = self.spec.file_stream(&self.doc.inner)?;
        let name = stream
            .dict
            .get(&Name::from("Subtype"), &self.doc.inner)
            .and_then(|value| {
                value
                    .get()
                    .as_name()
                    .map(|n| String::from_utf8_lossy(n.as_bytes()).into_owned())
            });
        Some(name.unwrap_or_default())
    }

    /// Whether the embedded file's `/Params` carry `key`.
    #[must_use]
    pub fn has_param(&self, key: &str) -> bool {
        self.spec
            .params(&self.doc.inner)
            .is_some_and(|params| params.contains_key(&Name::from(key)))
    }

    /// A `/Params` entry as text — `CreationDate`, `ModDate`, `CheckSum`:
    /// `None` when absent; a string or a name gives its text, anything else
    /// is empty.
    #[must_use]
    pub fn param(&self, key: &str) -> Option<String> {
        let params = self.spec.params(&self.doc.inner)?;
        let key = Name::from(key);
        if !params.contains_key(&key) {
            return None;
        }
        let object = params.get(&key, &self.doc.inner)?;
        Some(match object.get() {
            // A checksum written as a hex string is shown as that hex string,
            // brackets and all, rather than as the sixteen bytes it decodes to.
            pdfrum_object::Object::Str(string) if key.as_bytes() == b"CheckSum" && string.hex => {
                use std::fmt::Write as _;
                let mut hex = String::with_capacity(string.bytes.len() * 2 + 2);
                hex.push('<');
                for byte in &string.bytes {
                    let _ = write!(hex, "{byte:02X}");
                }
                hex.push('>');
                hex
            }
            pdfrum_object::Object::Str(string) => string.as_text().into_owned(),
            pdfrum_object::Object::Name(name) => {
                String::from_utf8_lossy(name.as_bytes()).into_owned()
            }
            _ => String::new(),
        })
    }
}
