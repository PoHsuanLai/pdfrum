//! Writing documents back out.

use std::borrow::Cow;
use std::io::Write;
use std::path::Path;
use std::time::SystemTime;

#[cfg(feature = "forms")]
use pdfrum_common::Diagnostics;
use pdfrum_common::{PageIndex, PdfVersion};
use pdfrum_edit::{EditDoc, Encryption, IdSource, PageBox, SaveMode};
use pdfrum_object::{Dict, ObjRef, Object, Resolve, names};

#[cfg(feature = "forms")]
use crate::Form;
use crate::{
    Document, EmbeddedFont, EmbeddedImage, FontEncoding, Metadata, PageEdit, PixelFormat, Result,
    StandardFont,
};

/// How a document is written back out.
///
/// A config struct with [`Default`], filled in with struct-update syntax.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SaveOptions {
    /// Whether to rewrite the file or append to it.
    pub update: Update,
    /// The PDF version to declare in the header. 1.0 through 1.7 are
    /// honoured; anything else, and `None`, keep the document's own.
    pub version: Option<PdfVersion>,
    /// Write an encrypted document out in the clear, dropping `/Encrypt`.
    ///
    /// Off by default: an encrypted document saves encrypted under the handler
    /// its password opened, and the output opens with that same password.
    /// Setting this is the explicit way to decrypt one on the way out, and it
    /// forces a full rewrite — an incremental append cannot decrypt the bytes
    /// already in the file.
    pub remove_security: bool,
    /// Subset fonts this save writes as new, dropping glyphs no page shows.
    ///
    /// Off by default. When set, a `/Type0` `CIDFontType2` with `/FontFile2`
    /// that a show operator draws with is replaced by a subset named
    /// `ABCDEF+Original`. See [`pdfrum_edit::SaveOptions::subset_new_fonts`].
    pub subset_new_fonts: bool,
    /// Where the trailer's fresh `/ID` bytes come from: random per save by
    /// default, or [`IdSource::Fixed`] for a reproducible file — the same
    /// input then saves to the same bytes, subset-font tags included.
    pub id_source: IdSource,
    /// Encrypt an unencrypted document on the way out — AES-256, revision
    /// 6 — under these passwords and permissions. A document that is
    /// already encrypted cannot be re-keyed in one save: decrypt it with
    /// [`SaveOptions::remove_security`], then encrypt the result.
    pub encrypt: Option<Encryption>,
}

/// Whether a save rewrites the whole file or appends to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Update {
    /// Rewrite the whole file, dropping every object nothing points at.
    ///
    /// The smaller output, and the one to use when the result is a new file.
    #[default]
    Rewrite,
    /// Append the changes after the original bytes, leaving them untouched.
    ///
    /// The original file stays byte-identical at the front, which preserves
    /// any digital signature over it and makes the change auditable. Falls
    /// back to [`Update::Rewrite`] when the document's cross-reference table
    /// had to be rebuilt — there would be no previous section to chain from.
    Incremental,
}

impl Document {
    /// Writes the document to `path`.
    ///
    /// # Errors
    ///
    /// [`Error::Io`](crate::Error::Io) when the file cannot be written, and
    /// [`Error::Save`](crate::Error::Save) when the document cannot be
    /// serialized.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        self.save_with(path, &SaveOptions::default())
    }

    /// Writes the document to `path` with explicit options.
    ///
    /// # Errors
    ///
    /// As [`Document::save`].
    pub fn save_with(&self, path: impl AsRef<Path>, options: &SaveOptions) -> Result<()> {
        self.edit().save(path, options)
    }

    /// Appends the document's changes after its original bytes.
    ///
    /// Shorthand for [`Document::save_with`] and [`Update::Incremental`].
    ///
    /// # Errors
    ///
    /// As [`Document::save`].
    pub fn save_incremental(&self, path: impl AsRef<Path>) -> Result<()> {
        self.save_with(
            path,
            &SaveOptions {
                update: Update::Incremental,
                ..SaveOptions::default()
            },
        )
    }

    /// Writes the document to any [`Write`] sink.
    ///
    /// # Errors
    ///
    /// [`Error::Save`](crate::Error::Save) when the sink refuses the bytes or
    /// the document cannot be serialized.
    pub fn write_to(&self, out: &mut impl Write, options: &SaveOptions) -> Result<()> {
        self.edit().write_to(out, options)
    }

    #[cfg(feature = "forms")]
    /// Writes the document with a form's filled-in values applied.
    ///
    /// The values written through [`Form::set`] are turned into replacement
    /// objects here — each edited field's `/V`, each toggle widget's `/AS`,
    /// and a regenerated appearance stream for every widget whose own `/AP`
    /// cannot show the new value. Nothing was mutated before this call, which
    /// is why filling a form does not need a `&mut Document`.
    ///
    /// # Errors
    ///
    /// [`Error::Io`](crate::Error::Io) when the file cannot be written, and
    /// [`Error::Save`](crate::Error::Save) when the document cannot be
    /// serialized.
    ///
    /// ```
    /// # let dir = std::env::temp_dir().join("pdfrum-form-save");
    /// # std::fs::create_dir_all(&dir)?;
    /// # let out = dir.join("filled.pdf");
    /// let doc = pdfrum::Document::open("tests/fixtures/text_form.pdf")?;
    /// let mut form = doc.form().expect("form");
    /// form.set("Text Box", "Hello").expect("field exists");
    /// doc.save_form(&out, &form, &pdfrum::SaveOptions::default())?;
    ///
    /// // Reopening shows the value the file now holds.
    /// let filled = pdfrum::Document::open(&out)?;
    /// let field = filled.form().expect("form");
    /// assert_eq!(field.field("Text Box").map(|f| f.value()), Some("Hello".into()));
    /// # std::fs::remove_dir_all(&dir).ok();
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    pub fn save_form(
        &self,
        path: impl AsRef<Path>,
        form: &Form<'_>,
        options: &SaveOptions,
    ) -> Result<()> {
        let mut bytes = Vec::new();
        self.write_form_to(&mut bytes, form, options)?;
        std::fs::write(path.as_ref(), &bytes)?;
        Ok(())
    }

    #[cfg(feature = "forms")]
    /// Writes the document with a form's values applied, to any [`Write`]
    /// sink.
    ///
    /// # Errors
    ///
    /// As [`Document::save_form`].
    pub fn write_form_to(
        &self,
        out: &mut impl Write,
        form: &Form<'_>,
        options: &SaveOptions,
    ) -> Result<()> {
        let mut edit = EditDoc::new(&self.inner);
        let mut diags = Diagnostics::default();
        let edits = pdfrum_doc::form::apply(form.inner(), form.values(), &self.inner, &mut diags);
        self.note(&diags);
        for field in edits {
            edit.replace(field.reference, Object::Dict(field.dict));
            for (reference, dict, generated) in field.widgets {
                let dict = if generated.stream.is_empty() {
                    dict
                } else {
                    // The generated appearance becomes a form XObject the
                    // widget's `/AP /N` names, which is the only shape a
                    // reader looks for.
                    let stream = edit.add(Object::Stream(Box::new(pdfrum_object::Stream::new(
                        pdfrum_doc::ap::stream_dict(&generated),
                        generated.stream.clone().into(),
                    ))));
                    let normal = pdfrum_object::Dict::from_pairs([(
                        pdfrum_object::Name::from("N"),
                        Object::Ref(stream),
                    )]);
                    set_key(&dict, "AP", Object::Dict(normal))
                };
                edit.replace(reference, Object::Dict(dict));
            }
        }
        let _ = form.document();
        write_edit(&edit, options, out)
    }

    /// Writes the document with edited pages' content regenerated.
    ///
    /// Each [`PageEdit`] whose objects were changed has its content streams
    /// written again from its object graph; a page that was opened and not
    /// changed, and every page not listed at all, comes through untouched.
    /// See [`PageEdit`] for what regeneration loses.
    ///
    /// The no-new-objects convenience: it opens a [`DocEdit`] with nothing
    /// added and delegates to [`DocEdit::save_pages`]. Adding a font, or any
    /// other new object, needs [`Document::edit`] directly.
    ///
    /// # Errors
    ///
    /// [`Error::Io`](crate::Error::Io) when the file cannot be written, and
    /// [`Error::Save`](crate::Error::Save) when the document cannot be
    /// serialized.
    pub fn save_pages(
        &self,
        path: impl AsRef<Path>,
        pages: &[PageEdit],
        options: &SaveOptions,
    ) -> Result<()> {
        self.edit().save_pages(path, pages, options)
    }

    /// Writes the document with edited pages applied, to any [`Write`] sink.
    ///
    /// # Errors
    ///
    /// As [`Document::save_pages`].
    pub fn write_pages_to(
        &self,
        out: &mut impl Write,
        pages: &[PageEdit],
        options: &SaveOptions,
    ) -> Result<()> {
        self.edit().write_pages_to(out, pages, options)
    }

    /// Copies pages from another document into this one, writing the result
    /// to `path`.
    ///
    /// `pages` names zero-based indices of `source`; `at` is where they land
    /// in this document's page list, with the pages there and after shifted
    /// up. Passing this document's own page count appends.
    ///
    /// # Errors
    ///
    /// [`Error::Io`](crate::Error::Io) when the file cannot be written, and
    /// [`Error::Save`](crate::Error::Save) when a named page is outside the
    /// source or this document has no catalog to import into. Nothing is
    /// written unless the whole import succeeds.
    ///
    /// ```
    /// # let dir = std::env::temp_dir().join("pdfrum-import");
    /// # std::fs::create_dir_all(&dir)?;
    /// # let out = dir.join("merged.pdf");
    /// let doc = pdfrum::Document::open("tests/fixtures/hello_world.pdf")?;
    /// let extra = pdfrum::Document::open("tests/fixtures/bookmarks.pdf")?;
    ///
    /// // Append both of the other document's pages.
    /// doc.import_pages(&out, &extra, [0, 1], doc.page_count())?;
    ///
    /// let merged = pdfrum::Document::open(&out)?;
    /// assert_eq!(merged.page_count(), 3);
    /// # std::fs::remove_dir_all(&dir).ok();
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    pub fn import_pages(
        &self,
        path: impl AsRef<Path>,
        source: &Document,
        pages: impl IntoIterator<Item = impl Into<PageIndex>>,
        at: impl Into<PageIndex>,
    ) -> Result<()> {
        let mut edit = EditDoc::new(&self.inner);
        pdfrum_edit::import_pages(
            &mut edit,
            &source.inner,
            &pdfrum_edit::PageRange::of(pages),
            &pdfrum_edit::ImportOptions {
                at: at.into(),
                viewer_preferences: false,
            },
        )?;
        let mut bytes = Vec::new();
        write_edit(&edit, &SaveOptions::default(), &mut bytes)?;
        std::fs::write(path.as_ref(), &bytes)?;
        Ok(())
    }

    /// A session for adding objects this document does not yet have — fonts
    /// first — and saving them together with page edits.
    ///
    /// [`Document`] itself is never mutated. New objects live on this handle
    /// until [`DocEdit::save_pages`] / [`DocEdit::write_pages_to`] writes them.
    #[must_use]
    pub fn edit(&self) -> DocEdit<'_> {
        DocEdit {
            doc: self,
            inner: EditDoc::new(&self.inner),
            stamp_mod_date: false,
        }
    }
}

/// Document-level edits: embedded fonts, then a save that writes them.
///
/// Page content is still edited through [`PageEdit`]; this handle is the
/// place new *objects* are allocated so a [`crate::TextBuilder`] can name
/// them.
#[derive(Debug)]
pub struct DocEdit<'a> {
    pub(crate) doc: &'a Document,
    pub(crate) inner: EditDoc<'a>,
    /// Whether [`DocEdit::set_metadata`] was called, so a save that is not
    /// reproducible stamps `/ModDate` with its own time.
    stamp_mod_date: bool,
}

impl<'a> DocEdit<'a> {
    /// The objects as the save writes them: the session's, with `/ModDate`
    /// stamped when a metadata edit asked for the save's own time and
    /// `options` do not ask for a reproducible file.
    ///
    /// A stamped save works on a clone so the session itself never carries a
    /// date it did not set; the clone shares its objects and costs one map.
    fn for_save(&self, options: &SaveOptions) -> Cow<'_, EditDoc<'a>> {
        if !self.stamp_mod_date || matches!(options.id_source, IdSource::Fixed(_)) {
            return Cow::Borrowed(&self.inner);
        }
        let mut inner = self.inner.clone();
        pdfrum_edit::set_info_entry(
            &mut inner,
            names::MOD_DATE,
            Some(&pdfrum_edit::pdf_date(SystemTime::now())),
        );
        Cow::Owned(inner)
    }
}

impl DocEdit<'_> {
    /// Replace the document's `/Info` metadata (ISO 32000-1 §14.3.3).
    ///
    /// Every field of `metadata` is written as a PDF text string —
    /// `PDFDocEncoding` when it fits, UTF-16BE behind a byte-order mark when
    /// it does not — and a `None` or empty field removes its key, so the saved
    /// `/Info` holds exactly what the value holds. Start from
    /// [`Document::metadata`] to change one key and keep the rest. A document
    /// without an `/Info` gains one.
    ///
    /// `/ModDate` is the one key the save may overwrite: a save whose
    /// [`SaveOptions::id_source`] is [`IdSource::Random`] — the default —
    /// stamps the time of the save, while a reproducible save
    /// ([`IdSource::Fixed`]) writes `modification_date` as given, so the same
    /// input saves to the same bytes.
    ///
    /// The catalog's XMP `/Metadata` stream is not touched. A document that
    /// carries both will have the two disagree after this; rewriting the
    /// packet is not something this crate does.
    ///
    /// ```
    /// use pdfrum::{Document, SaveOptions};
    ///
    /// let doc = Document::open("tests/fixtures/hello_world.pdf")?;
    /// let mut metadata = doc.metadata();
    /// metadata.title = Some("Hello".into());
    /// metadata.author = Some("Ann".into());
    ///
    /// let mut edit = doc.edit();
    /// edit.set_metadata(&metadata);
    /// let mut bytes = Vec::new();
    /// edit.write_to(&mut bytes, &SaveOptions::default())?;
    ///
    /// let saved = Document::from_bytes(bytes.into())?;
    /// assert_eq!(saved.metadata().title.as_deref(), Some("Hello"));
    /// assert!(saved.metadata().modification_date.is_some(), "stamped by the save");
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    pub fn set_metadata(&mut self, metadata: &Metadata) {
        let entries = [
            (names::TITLE, &metadata.title),
            (names::AUTHOR, &metadata.author),
            (names::SUBJECT, &metadata.subject),
            (names::KEYWORDS, &metadata.keywords),
            (names::CREATOR, &metadata.creator),
            (names::PRODUCER, &metadata.producer),
            (names::CREATION_DATE, &metadata.creation_date),
            (names::MOD_DATE, &metadata.modification_date),
        ];
        for (key, value) in entries {
            pdfrum_edit::set_info_entry(&mut self.inner, key, value.as_deref());
        }
        self.stamp_mod_date = true;
    }

    /// A page's dictionary as the session's edits leave it, with the
    /// reference the writer replaces and the resources the page reaches.
    ///
    /// Read through the overlay, not the base, so an earlier edit of the same
    /// page — a rotation, a stamp — is what a later content rewrite builds on.
    /// `None` for a page written inline in its parent's `/Kids`, which has no
    /// object to replace.
    pub(crate) fn page_state(&self, index: PageIndex) -> Result<Option<(ObjRef, Dict, Dict)>> {
        let page = self.doc.inner.page(index)?;
        let Some(reference) = page.reference else {
            return Ok(None);
        };
        let dict = self
            .inner
            .fetch(reference)
            .ok()
            .as_deref()
            .and_then(Object::as_dict)
            .cloned()
            .unwrap_or_else(|| page.dict.clone());
        let resources = dict
            .dict(names::RESOURCES, &self.inner)
            .or_else(|| {
                page.inherited(names::RESOURCES, &self.inner)?
                    .resolve(&self.inner)
                    .ok()?
                    .as_dict()
                    .cloned()
            })
            .unwrap_or_default();
        Ok(Some((reference, dict, resources)))
    }

    /// Turn one page edit into replacement objects on the session.
    ///
    /// `shared` is [`pdfrum_edit::shared_objects`] over the session, computed
    /// once by the caller for however many pages it applies.
    pub(crate) fn apply_page(
        &mut self,
        page: &PageEdit,
        shared: &pdfrum_edit::ShareCounts,
    ) -> Result<()> {
        let Some((reference, dict, resources)) = self.page_state(page.index())? else {
            // Rather than half-apply the change, leave the page as it was.
            return Ok(());
        };
        let Some(rewrite) = pdfrum_edit::regenerate(page.graph(), &resources, &self.inner) else {
            return Ok(());
        };
        pdfrum_edit::apply_rewrite(&mut self.inner, reference, &dict, &rewrite, shared);
        Ok(())
    }
    /// Import `pages` of `source` as a contiguous run at `at` (past the end
    /// appends), in the order given, duplicates included. Objects two
    /// imported pages share are copied once.
    ///
    /// # Errors
    ///
    /// [`Error::Save`](crate::Error::Save) when `source` has no such page or
    /// this document has no catalog to hang a page tree on.
    pub fn import_pages(
        &mut self,
        source: &Document,
        pages: impl IntoIterator<Item = impl Into<PageIndex>>,
        at: impl Into<PageIndex>,
    ) -> Result<()> {
        pdfrum_edit::import_pages(
            &mut self.inner,
            &source.inner,
            &pdfrum_edit::PageRange::of(pages),
            &pdfrum_edit::ImportOptions {
                at: at.into(),
                viewer_preferences: false,
            },
        )?;
        Ok(())
    }

    /// Delete `pages`, by this document's numbering as opened. A duplicate
    /// index deletes once; the page objects go with the next full save's
    /// garbage collection.
    ///
    /// # Errors
    ///
    /// [`Error::Save`](crate::Error::Save) when there is no such page; nothing
    /// is deleted then.
    pub fn delete_pages(
        &mut self,
        pages: impl IntoIterator<Item = impl Into<PageIndex>>,
    ) -> Result<()> {
        pdfrum_edit::delete_pages(&mut self.inner, &pdfrum_edit::PageRange::of(pages))?;
        Ok(())
    }

    /// Add an empty page of `width` by `height` points at `at` (past the end
    /// appends). It has no contents until something is drawn on it.
    ///
    /// # Errors
    ///
    /// [`Error::Save`](crate::Error::Save) when the document has no catalog.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a page size in points fits f32, which is what the file stores"
    )]
    pub fn add_page(&mut self, width: f64, height: f64, at: impl Into<PageIndex>) -> Result<()> {
        pdfrum_edit::add_blank_page(
            &mut self.inner,
            width as f32,
            height as f32,
            u32::from(at.into()),
        )?;
        Ok(())
    }

    /// Set a page's `/Rotate`; `degrees` rounds to the nearest quarter turn.
    ///
    /// # Errors
    ///
    /// [`Error::Save`](crate::Error::Save) when there is no such page.
    pub fn set_rotation(&mut self, page: impl Into<PageIndex>, degrees: i32) -> Result<()> {
        pdfrum_edit::set_page_rotation(&mut self.inner, page.into(), degrees)?;
        Ok(())
    }

    /// Set one of a page's boxes (ISO 32000-1 §14.11.2).
    ///
    /// # Errors
    ///
    /// [`Error::Save`](crate::Error::Save) when there is no such page.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a box in points fits f32, which is what the file stores"
    )]
    pub fn set_page_box(
        &mut self,
        page: impl Into<PageIndex>,
        which: PageBox,
        rect: kurbo::Rect,
    ) -> Result<()> {
        pdfrum_edit::set_page_box(
            &mut self.inner,
            page.into(),
            which,
            [
                rect.x0 as f32,
                rect.y0 as f32,
                rect.x1 as f32,
                rect.y1 as f32,
            ],
        )?;
        Ok(())
    }

    /// Lay `pages` of `source` out `columns` by `rows` per sheet of `sheet`
    /// points, inserting the sheets at the front of this document — N-up
    /// imposition, the way `FPDF_ImportNPagesToOne` does it.
    ///
    /// # Errors
    ///
    /// [`Error::Save`](crate::Error::Save) when a page does not exist, the
    /// grid or sheet is empty, or this document has no catalog.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a sheet size in points fits f32, which is what the file stores"
    )]
    pub fn n_up(
        &mut self,
        source: &Document,
        pages: impl IntoIterator<Item = impl Into<PageIndex>>,
        columns: u32,
        rows: u32,
        sheet: (f64, f64),
    ) -> Result<()> {
        pdfrum_edit::n_page_to_one(
            &mut self.inner,
            &source.inner,
            &pdfrum_edit::PageRange::of(pages),
            &pdfrum_edit::NUpOptions {
                sheet: (sheet.0 as f32, sheet.1 as f32),
                grid: (columns, rows),
            },
        )?;
        Ok(())
    }

    /// Embed `program` as a new `/Font`.
    ///
    /// ```
    /// use pdfrum::{Document, FontEncoding, SaveOptions, TextBuilder};
    ///
    /// let doc = Document::open("tests/fixtures/hello_world.pdf")?;
    /// let mut edit = doc.edit();
    /// let font = edit.embed_font(
    ///     include_bytes!("../../pdfrum-edit/tests/files/tiny.ttf"),
    ///     FontEncoding::Composite,
    /// )?;
    /// let mut page = doc.page(0)?.edit();
    /// page.push(
    ///     TextBuilder {
    ///         position: pdfrum::Point::new(20.0, 80.0),
    ///         ..TextBuilder::new(font.encode("Hi"), font.object(), 24.0)
    ///     }
    ///     .build(),
    /// );
    /// let mut bytes = Vec::new();
    /// edit.write_pages_to(&mut bytes, &[page], &SaveOptions::default())?;
    /// assert!(bytes.starts_with(b"%PDF-"));
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    ///
    /// # Errors
    ///
    /// [`Error::Save`](crate::Error::Save) when the bytes are not a font
    /// program, or the face has no glyphs.
    pub fn embed_font(&mut self, program: &[u8], encoding: FontEncoding) -> Result<EmbeddedFont> {
        Ok(self.inner.embed_font(program, encoding)?)
    }

    /// Embed `program` as a composite font whose `/ToUnicode` CMap and
    /// `/CIDToGIDMap` are the caller's, not generated from the program.
    ///
    /// The counterpart of `FPDFText_LoadCidType2Font`. `cid_to_gid` is one
    /// big-endian `u16` glyph index per CID, indexed by CID; `/W` is computed
    /// per CID from it. [`EmbeddedFont::encode`] on the result writes CIDs
    /// found by inverting `to_unicode`.
    ///
    /// ```
    /// use pdfrum::{Document, SaveOptions, TextBuilder};
    ///
    /// const CMAP: &str = "\
    /// /CIDInit /ProcSet findresource begin
    /// 12 dict begin
    /// begincmap
    /// /CMapName /Adobe-Identity-H def
    /// /CMapType 2 def
    /// 1 begincodespacerange
    /// <0000> <FFFF>
    /// endcodespacerange
    /// 1 beginbfchar
    /// <0001> <0048>
    /// endbfchar
    /// endcmap
    /// CMapName currentdict /CMap defineresource pop
    /// end
    /// end
    /// ";
    ///
    /// let doc = Document::open("tests/fixtures/hello_world.pdf")?;
    /// let mut edit = doc.edit();
    /// // CID 0 → GID 0, CID 1 → GID 1.
    /// let font = edit.embed_cid_font(
    ///     include_bytes!("../../pdfrum-edit/tests/files/tiny.ttf"),
    ///     CMAP,
    ///     &[0, 0, 0, 1],
    /// )?;
    /// assert_eq!(font.encode("H"), vec![0, 1]);
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    ///
    /// # Errors
    ///
    /// [`Error::Save`](crate::Error::Save) when the bytes are not a font
    /// program, the face has no glyphs, `to_unicode` is empty, or
    /// `cid_to_gid` is empty or not a whole number of two-byte entries.
    pub fn embed_cid_font(
        &mut self,
        program: &[u8],
        to_unicode: &str,
        cid_to_gid: &[u8],
    ) -> Result<EmbeddedFont> {
        Ok(self.inner.embed_cid_font(program, to_unicode, cid_to_gid)?)
    }

    /// Add a non-embedded standard-14 Type 1 font.
    ///
    /// # Errors
    ///
    /// [`Error::Save`](crate::Error::Save) if the editor cannot allocate the
    /// dictionary.
    pub fn standard_font(&mut self, which: StandardFont) -> Result<EmbeddedFont> {
        Ok(self.inner.standard_font(which)?)
    }

    /// Embed a JPEG or JPEG 2000 codestream as a new image `/XObject`.
    ///
    /// The bytes are stored verbatim under `/DCTDecode` or `/JPXDecode`;
    /// nothing is decoded or re-encoded. The dimensions and colour space come
    /// from the codestream's own header, which is why they are read back off
    /// the result rather than passed in.
    ///
    /// ```
    /// use pdfrum::{Document, ImageBuilder, Rect, SaveOptions};
    ///
    /// let doc = Document::open("tests/fixtures/hello_world.pdf")?;
    /// let mut edit = doc.edit();
    /// let image = edit.embed_jpeg(include_bytes!("../tests/fixtures/mona_lisa.jpg"))?;
    /// assert_eq!((image.width(), image.height()), (120, 120));
    ///
    /// let mut page = doc.page(0)?.edit();
    /// page.push(ImageBuilder::at(image.object(), Rect::new(20.0, 20.0, 140.0, 140.0)).build());
    /// let mut bytes = Vec::new();
    /// edit.write_pages_to(&mut bytes, &[page], &SaveOptions::default())?;
    /// assert!(bytes.starts_with(b"%PDF-"));
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    ///
    /// # Errors
    ///
    /// [`Error::Save`](crate::Error::Save) when the bytes are neither a JPEG
    /// nor a JPEG 2000 codestream, or their header cannot be read.
    pub fn embed_jpeg(&mut self, bytes: &[u8]) -> Result<EmbeddedImage> {
        Ok(self.inner.embed_jpeg(bytes)?)
    }

    /// Embed raw interleaved samples as a new image `/XObject`.
    ///
    /// The samples are flate-compressed by the save. A
    /// [`PixelFormat::Rgba8`] alpha channel becomes a separate `/SMask`
    /// image, and [`PixelFormat::Mask1`] writes an `/ImageMask` whose set
    /// bits paint.
    ///
    /// # Errors
    ///
    /// [`Error::Save`](crate::Error::Save) when either dimension is zero, or
    /// `pixels` is not exactly `width * height * components` bytes — one
    /// eighth of that, rounded up per row, for [`PixelFormat::Mask1`].
    pub fn embed_image(
        &mut self,
        pixels: &[u8],
        width: u32,
        height: u32,
        format: PixelFormat,
    ) -> Result<EmbeddedImage> {
        Ok(self.inner.embed_image(pixels, width, height, format)?)
    }

    /// Write the document with this session's new objects and `pages` applied.
    ///
    /// # Errors
    ///
    /// As [`Document::save_pages`].
    pub fn save_pages(
        &mut self,
        path: impl AsRef<std::path::Path>,
        pages: &[PageEdit],
        options: &SaveOptions,
    ) -> Result<()> {
        let mut bytes = Vec::new();
        self.write_pages_to(&mut bytes, pages, options)?;
        std::fs::write(path.as_ref(), &bytes)?;
        Ok(())
    }

    /// Write the document with this session's new objects and `pages` applied,
    /// to any [`Write`] sink.
    ///
    /// # Errors
    ///
    /// As [`Document::save_pages`].
    pub fn write_pages_to(
        &mut self,
        out: &mut impl Write,
        pages: &[PageEdit],
        options: &SaveOptions,
    ) -> Result<()> {
        let shared = pdfrum_edit::shared_objects(&self.inner);
        for page in pages {
            self.apply_page(page, &shared)?;
        }
        write_edit(&self.for_save(options), options, out)
    }

    /// Write the document with this session's new objects and no page edits.
    ///
    /// # Errors
    ///
    /// As [`Document::save`].
    pub fn save(&self, path: impl AsRef<std::path::Path>, options: &SaveOptions) -> Result<()> {
        let mut bytes = Vec::new();
        self.write_to(&mut bytes, options)?;
        std::fs::write(path.as_ref(), &bytes)?;
        Ok(())
    }

    /// Write the document with this session's new objects, to any [`Write`]
    /// sink.
    ///
    /// # Errors
    ///
    /// As [`Document::save`].
    pub fn write_to(&self, out: &mut impl Write, options: &SaveOptions) -> Result<()> {
        write_edit(&self.for_save(options), options, out)
    }
}

/// The one place a [`SaveOptions`] becomes the writer's own options.
fn write_edit(edit: &EditDoc<'_>, options: &SaveOptions, out: &mut impl Write) -> Result<()> {
    let opts = pdfrum_edit::SaveOptions {
        mode: match options.update {
            Update::Rewrite => SaveMode::Full,
            Update::Incremental => SaveMode::Incremental,
        },
        version: options.version,
        // An encrypted document saves **encrypted**, under the handler its
        // password opened, so the output opens with that same password. The
        // regenerated content streams of an edited page go through the same
        // cipher as everything else, because they are written the same way.
        remove_security: options.remove_security,
        subset_new_fonts: options.subset_new_fonts,
        id_source: options.id_source,
        encrypt: options.encrypt.clone(),
        ..pdfrum_edit::SaveOptions::default()
    };
    pdfrum_edit::save(edit, &opts, out)?;
    Ok(())
}

#[cfg(feature = "forms")]
/// A copy of `dict` with `key` set, keeping every other entry in place.
fn set_key(dict: &Dict, key: &str, value: Object) -> Dict {
    let key = pdfrum_object::Name::from(key);
    let mut out = Dict::new();
    let mut replaced = false;
    for (existing, held) in dict.iter() {
        if *existing == key {
            if !replaced {
                out.push(existing.clone(), value.clone());
                replaced = true;
            }
        } else {
            out.push(existing.clone(), held.clone());
        }
    }
    if !replaced {
        out.push(key, value);
    }
    out
}
