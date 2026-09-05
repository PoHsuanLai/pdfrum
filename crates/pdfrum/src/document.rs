//! Opening a file and everything that belongs to the document as a whole.

use std::path::Path;
use std::sync::{Arc, Mutex};

use pdfrum_common::{Diagnostics, Limits, PageIndex, PdfVersion};
use pdfrum_crypt::Permissions;
use pdfrum_object::{Dict, Name, ObjRef, Object, Resolve, names};

#[cfg(feature = "forms")]
use crate::form::Form;
use crate::{Outline, Page, RenderSession, Result};

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
    /// The depth and size caps recovery enforces while reading, and the two
    /// ceilings a host adds: [`Limits::max_render_pixels`] for every render
    /// of this document, [`Limits::deadline`] for everything done with it.
    /// There is no per-render copy — [`RenderOptions`](crate::RenderOptions)
    /// says how a page is drawn, this says how much a document may cost.
    pub limits: Limits,
}

/// Builds an [`OpenOptions`] a setting at a time.
///
/// Sugar over the struct-update syntax, which still works. It earns its place
/// on a two-field struct because of the password: `.password("secret")` takes
/// anything byte-shaped, where the field itself is the `Option<Vec<u8>>` the
/// format actually calls for.
///
/// ```
/// use pdfrum::{Limits, OpenOptions};
///
/// let options = OpenOptions::builder()
///     .password("secret")
///     .limits(Limits::default())
///     .build();
///
/// assert_eq!(options.password.as_deref(), Some(&b"secret"[..]));
/// ```
#[derive(Debug, Clone, Default)]
#[must_use]
pub struct OpenOptionsBuilder(OpenOptions);

impl OpenOptionsBuilder {
    /// The password to try — [`OpenOptions::password`].
    ///
    /// Takes a `&str`, a `String`, a `&[u8]` or a `Vec<u8>`: PDF passwords
    /// are byte strings and need not be UTF-8, and this accepts both
    /// spellings without the caller converting.
    ///
    /// ```
    /// let options = pdfrum::OpenOptions::builder().password(b"\xff\xfe".as_slice()).build();
    /// assert_eq!(options.password.as_deref(), Some(&b"\xff\xfe"[..]));
    /// ```
    pub fn password(mut self, password: impl AsRef<[u8]>) -> Self {
        self.0.password = Some(password.as_ref().to_vec());
        self
    }

    /// The caps recovery and rendering enforce — [`OpenOptions::limits`].
    ///
    /// ```
    /// let options = pdfrum::OpenOptions::builder()
    ///     .limits(pdfrum::Limits::default())
    ///     .build();
    /// ```
    pub fn limits(mut self, limits: Limits) -> Self {
        self.0.limits = limits;
        self
    }

    /// The options as built.
    #[must_use]
    pub fn build(self) -> OpenOptions {
        self.0
    }
}

impl OpenOptions {
    /// A builder starting from the defaults.
    ///
    /// ```
    /// let options = pdfrum::OpenOptions::builder().password("secret").build();
    /// ```
    pub fn builder() -> OpenOptionsBuilder {
        OpenOptionsBuilder::default()
    }
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
    /// The loaded fonts every session over this document shares.
    ///
    /// One document, one cache: a font named by an `ObjRef` is parsed once —
    /// its `/ToUnicode`, its CID tables, its glyph cache — and every session
    /// [`Document::render_session`] hands out reads that one instance. Before
    /// this, each session reloaded every font it met, which is what made a
    /// parallel render's per-thread work grow with the thread count.
    ///
    /// Dropped with the document, so nothing outlives the file it came from.
    fonts: Arc<pdfrum_font::FontCache>,
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
            limits: options.limits.clone(),
        };
        Ok(Document {
            inner: pdfrum_parser::load(bytes, &load)?,
            limits: options.limits.clone(),
            session: Mutex::new(Diagnostics::default()),
            fonts: Arc::default(),
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

    /// A [`RenderSession`] wired to this document's shared font cache.
    ///
    /// Prefer this to [`RenderSession::new`] whenever the session is for a
    /// document you hold: the fonts a session meets are then loaded once for
    /// the document rather than once per session, which is what makes a
    /// parallel render scale. Everything else about the session — its colour
    /// space, function and image caches, its glyph outlines — is still the
    /// session's own, because those are the caches a `&mut` run owns.
    ///
    /// ```
    /// use pdfrum::{Document, RenderOptions, VelloCpuBackend};
    ///
    /// let doc = Document::open("tests/fixtures/bookmarks.pdf")?;
    /// let backend = VelloCpuBackend::new();
    ///
    /// // Two sessions, one set of loaded fonts.
    /// let mut first = doc.render_session();
    /// let mut second = doc.render_session();
    /// let a = doc.page(0)?.render_on(&backend, &RenderOptions::default(), &mut first)?;
    /// let b = doc.page(0)?.render_on(&backend, &RenderOptions::default(), &mut second)?;
    /// assert_eq!(a.width(), b.width());
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    #[must_use]
    pub fn render_session(&self) -> RenderSession {
        let mut session = RenderSession::new();
        session.build.fonts = self.fonts();
        session
    }

    /// This document's shared loaded-font cache, for a context built here.
    pub(crate) fn fonts(&self) -> Arc<pdfrum_font::FontCache> {
        Arc::clone(&self.fonts)
    }

    /// Every page in order, interpreted lazily as the iterator advances.
    ///
    /// A page that will not load is skipped rather than ending the walk,
    /// which is what a viewer does: one broken page does not hide the rest of
    /// the document. Use [`Document::page`] when you need to know — a
    /// [`Limits::deadline`] that passes mid-walk ends it here, and only
    /// [`Document::page`] says so.
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

    #[cfg(feature = "forms")]
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

    /// A new document with one empty page of `width` by `height` points.
    ///
    /// The starting point for a file built from nothing — a page of images,
    /// a merge of other documents — through [`Document::edit`]. The page has
    /// no contents until something is drawn on it or it is deleted.
    ///
    /// # Errors
    ///
    /// Never in practice: the bytes are written here and open by
    /// construction; the `Result` is the one every open returns.
    pub fn blank(width: f64, height: f64) -> Result<Document> {
        use std::fmt::Write;
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /Count 1 /Kids [3 0 R] >>".to_owned(),
            format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {width} {height}] >>"),
        ];
        let mut out = String::from("%PDF-1.7\n%\u{e2}\u{e3}\u{cf}\u{d3}\n");
        let mut offsets = Vec::with_capacity(objects.len());
        for (i, body) in objects.iter().enumerate() {
            offsets.push(out.len());
            let _ = writeln!(out, "{} 0 obj\n{body}\nendobj", i + 1);
        }
        let xref = out.len();
        let _ = writeln!(out, "xref\n0 {}\n0000000000 65535 f ", objects.len() + 1);
        for offset in offsets {
            let _ = writeln!(out, "{offset:010} 00000 n ");
        }
        let _ = writeln!(
            out,
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF",
            objects.len() + 1
        );
        Document::from_bytes(Arc::from(out.into_bytes()))
    }

    /// A stream's data with its filters applied — what the stream carries,
    /// not how it was stored.
    ///
    /// # Errors
    ///
    /// [`Error::Read`](crate::Error::Read) carrying
    /// [`Unresolved`](pdfrum_parser::Error::Unresolved) when `reference`
    /// names no object, or names one that is not a stream. A filter that
    /// fails is a notice, not an error: the data comes back as far as it
    /// decoded, the way every reader in this crate takes it.
    pub fn stream_data(&self, reference: ObjRef) -> Result<Vec<u8>> {
        let unresolved = || crate::Error::Read(pdfrum_parser::Error::Unresolved(reference));
        let object = self.inner.fetch(reference).map_err(|_| unresolved())?;
        let Some(stream) = object.as_stream() else {
            return Err(unresolved());
        };
        let mut diags = Diagnostics::default();
        let decoded =
            pdfrum_filters::decode_chain(stream, 0, &self.inner, &self.limits, &mut diags);
        self.note(&diags);
        Ok(decoded.data.clone())
    }

    /// Every font program embedded in the document, in object order: the
    /// `/FontFile`, `/FontFile2` and `/FontFile3` streams of its font
    /// descriptors, decoded.
    #[must_use]
    pub fn embedded_fonts(&self) -> Vec<EmbeddedFontFile> {
        let mut out = Vec::new();
        for num in self.inner.xref().object_numbers() {
            let reference = ObjRef::new(num, self.inner.xref().generation(num));
            let Ok(object) = self.inner.fetch(reference) else {
                continue;
            };
            let Some(dict) = object.as_dict() else {
                continue;
            };
            let is_descriptor = dict
                .name(names::TYPE)
                .is_some_and(|t| t.as_bytes() == b"FontDescriptor");
            let name = dict
                .name(&Name::from("FontName"))
                .map(|n| String::from_utf8_lossy(n.as_bytes()).into_owned())
                .unwrap_or_default();
            for (key, kind) in [
                ("FontFile", FontFileKind::Type1),
                ("FontFile2", FontFileKind::TrueType),
                ("FontFile3", FontFileKind::Cff),
            ] {
                let Some(Object::Ref(file)) = dict.raw(&Name::from(key)) else {
                    continue;
                };
                if !is_descriptor && kind != FontFileKind::Type1 {
                    // A stray key on a non-descriptor: `/FontFile` alone is
                    // enough of a signal; the others need the type.
                    continue;
                }
                let Ok(data) = self.stream_data(*file) else {
                    continue;
                };
                let kind = if kind == FontFileKind::Cff && data.starts_with(b"OTTO") {
                    FontFileKind::OpenType
                } else {
                    kind
                };
                out.push(EmbeddedFontFile {
                    name: name.clone(),
                    kind,
                    object: *file,
                    data,
                });
            }
        }
        out
    }

    /// The document's revisions, oldest first: one per cross-reference
    /// section the open followed, which is one per incremental update. A
    /// file whose table had to be rebuilt has none, since no chain was
    /// followed.
    #[must_use]
    pub fn revisions(&self) -> Vec<Revision> {
        let bytes = self.inner.bytes();
        self.inner
            .xref()
            .sections()
            .iter()
            .enumerate()
            .map(|(index, section)| Revision {
                index,
                xref_offset: section.offset,
                is_stream: section.is_stream,
                end: revision_end(bytes, section.offset).unwrap_or(bytes.len()),
            })
            .collect()
    }

    /// The file as it stood at revision `index` of [`Document::revisions`]:
    /// the bytes up to that revision's `%%EOF`. A later revision's changes
    /// are appended after it, so this is the earlier document exactly.
    #[must_use]
    pub fn revision_bytes(&self, index: usize) -> Option<&[u8]> {
        let revision = self.revisions().into_iter().nth(index)?;
        self.inner.bytes().get(..revision.end)
    }

    /// The trailer's `/ID` pair (ISO 32000-1 §14.4): the first element names
    /// the document as first written, the second this revision. They are equal
    /// when the file has never been re-saved. `None` when the trailer carries
    /// no well-formed pair.
    #[must_use]
    pub fn id(&self) -> Option<[Vec<u8>; 2]> {
        let array = self.inner.trailer().array(names::ID, &self.inner)?;
        let element = |index| {
            array
                .get(index, &self.inner)
                .and_then(|entry| entry.get().as_string().map(|s| s.bytes.to_vec()))
        };
        Some([element(0)?, element(1)?])
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

/// One embedded font program, from [`Document::embedded_fonts`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddedFontFile {
    /// The descriptor's `/FontName`, possibly with a subset tag.
    pub name: String,
    /// What the bytes are.
    pub kind: FontFileKind,
    /// The stream object the program came from.
    pub object: ObjRef,
    /// The program, filters applied.
    pub data: Vec<u8>,
}

/// The kind of an embedded font program, by the key that carried it and
/// the bytes' own signature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontFileKind {
    /// `/FontFile`: Type 1.
    Type1,
    /// `/FontFile2`: TrueType.
    TrueType,
    /// `/FontFile3`: a bare CFF program.
    Cff,
    /// `/FontFile3` whose bytes start `OTTO`: an OpenType wrapper.
    OpenType,
}

impl FontFileKind {
    /// The format's short name: `type1`, `truetype`, `cff` or `opentype`.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Type1 => "type1",
            Self::TrueType => "truetype",
            Self::Cff => "cff",
            Self::OpenType => "opentype",
        }
    }

    /// The file extension the program is usually given.
    #[must_use]
    pub fn extension(self) -> &'static str {
        match self {
            Self::Type1 => "pfb",
            Self::TrueType => "ttf",
            Self::Cff => "cff",
            Self::OpenType => "otf",
        }
    }
}

/// The error [`FontFileKind`]'s [`FromStr`](std::str::FromStr) returns.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not a font program kind: {0}")]
pub struct UnknownFontFileKind(String);

impl std::fmt::Display for FontFileKind {
    /// [`FontFileKind::name`], which round-trips through
    /// [`FromStr`](std::str::FromStr).
    ///
    /// ```
    /// assert_eq!(pdfrum::FontFileKind::OpenType.to_string(), "opentype");
    /// ```
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

impl std::str::FromStr for FontFileKind {
    type Err = UnknownFontFileKind;

    /// The inverse of [`Display`](std::fmt::Display), on the short name only
    /// — not on the file extension, which is not unique (`cff` is both).
    ///
    /// # Errors
    ///
    /// [`UnknownFontFileKind`] when the string names no font program kind.
    ///
    /// ```
    /// assert_eq!("truetype".parse(), Ok(pdfrum::FontFileKind::TrueType));
    /// ```
    fn from_str(s: &str) -> core::result::Result<FontFileKind, UnknownFontFileKind> {
        match s {
            "type1" => Ok(FontFileKind::Type1),
            "truetype" => Ok(FontFileKind::TrueType),
            "cff" => Ok(FontFileKind::Cff),
            "opentype" => Ok(FontFileKind::OpenType),
            other => Err(UnknownFontFileKind(other.to_owned())),
        }
    }
}

/// One revision of an incrementally updated file, from
/// [`Document::revisions`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Revision {
    /// Position in the chain, 0 the oldest.
    pub index: usize,
    /// Where its cross-reference section starts.
    pub xref_offset: u64,
    /// Whether that section is a cross-reference stream.
    pub is_stream: bool,
    /// The byte after its `%%EOF`; the file up to here is this revision.
    pub end: usize,
}

/// The byte after the `%%EOF` that closes the revision whose `startxref`
/// names `offset`.
fn revision_end(bytes: &[u8], offset: u64) -> Option<usize> {
    let needle = b"startxref";
    let mut at = 0;
    while let Some(found) = find(bytes.get(at..)?, needle) {
        let start = at + found + needle.len();
        let rest = bytes.get(start..)?;
        let digits: String = rest
            .iter()
            .skip_while(|b| b.is_ascii_whitespace())
            .take_while(|b| b.is_ascii_digit())
            .map(|&b| char::from(b))
            .collect();
        if digits.parse::<u64>().ok() == Some(offset) {
            let eof = find(rest, b"%%EOF")?;
            let mut end = start + eof + b"%%EOF".len();
            while bytes.get(end).is_some_and(|b| *b == b'\r' || *b == b'\n') {
                end += 1;
            }
            return Some(end);
        }
        at = start;
    }
    None
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
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
