//! Writing documents back out.

use std::borrow::Cow;
use std::io::Write;
use std::path::Path;
use std::time::SystemTime;

use pdfrum_common::{Diagnostics, PageIndex, PdfVersion};
use pdfrum_edit::{EditDoc, Encryption, IdSource, PageBox, SaveMode};
use pdfrum_object::names;
#[cfg(feature = "forms")]
use pdfrum_object::{Dict, Object};

#[cfg(feature = "forms")]
use crate::Form;
use crate::{
    Document, EmbeddedFont, EmbeddedImage, FontEncoding, Metadata, PageEdit, PixelFormat, Result,
    StandardFont,
};

/// How a document is written back out.
///
/// A config struct with [`Default`]. `#[non_exhaustive]` so a field added
/// later is not a major break; fill one in with [`SaveOptions::builder`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
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
    ///
    /// It governs identifiers only. An [`SaveOptions::encrypt`] save draws
    /// its file key and AES vectors from the operating system whatever this
    /// is set to, so encrypted output is never byte-reproducible.
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

/// Builds a [`SaveOptions`] a setting at a time.
///
/// The way to change one field from outside this crate: the type is
/// `#[non_exhaustive]`, so struct-update syntax is a same-crate spelling.
/// Every method consumes and returns the builder; [`build`](Self::build)
/// hands back the options.
///
/// ```
/// use pdfrum::{SaveOptions, Update};
///
/// let options = SaveOptions::builder()
///     .update(Update::Incremental)
///     .subset_new_fonts(true)
///     .build();
///
/// assert_eq!(options.update, Update::Incremental);
/// assert!(options.subset_new_fonts);
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[must_use]
pub struct SaveOptionsBuilder(SaveOptions);

impl SaveOptionsBuilder {
    /// Rewrite the file or append to it — [`SaveOptions::update`].
    ///
    /// ```
    /// use pdfrum::{SaveOptions, Update};
    ///
    /// let options = SaveOptions::builder().update(Update::Incremental).build();
    /// ```
    pub fn update(mut self, update: Update) -> Self {
        self.0.update = update;
        self
    }

    /// Append after the original bytes — [`Update::Incremental`].
    ///
    /// ```
    /// let options = pdfrum::SaveOptions::builder().incremental().build();
    /// assert_eq!(options.update, pdfrum::Update::Incremental);
    /// ```
    pub fn incremental(self) -> Self {
        self.update(Update::Incremental)
    }

    /// The version to declare in the header — [`SaveOptions::version`].
    ///
    /// ```
    /// use pdfrum::{PdfVersion, SaveOptions};
    ///
    /// let options = SaveOptions::builder().version(PdfVersion::PDF_1_7).build();
    /// assert_eq!(options.version, Some(PdfVersion::PDF_1_7));
    /// ```
    pub fn version(mut self, version: PdfVersion) -> Self {
        self.0.version = Some(version);
        self
    }

    /// Write an encrypted document out in the clear —
    /// [`SaveOptions::remove_security`].
    ///
    /// ```
    /// let options = pdfrum::SaveOptions::builder().remove_security(true).build();
    /// assert!(options.remove_security);
    /// ```
    pub fn remove_security(mut self, remove: bool) -> Self {
        self.0.remove_security = remove;
        self
    }

    /// Subset the fonts this save writes as new —
    /// [`SaveOptions::subset_new_fonts`].
    ///
    /// ```
    /// let options = pdfrum::SaveOptions::builder().subset_new_fonts(true).build();
    /// assert!(options.subset_new_fonts);
    /// ```
    pub fn subset_new_fonts(mut self, subset: bool) -> Self {
        self.0.subset_new_fonts = subset;
        self
    }

    /// Where the trailer's fresh `/ID` bytes come from —
    /// [`SaveOptions::id_source`].
    ///
    /// ```
    /// use pdfrum::{IdSource, SaveOptions};
    ///
    /// // A fixed seed makes the same input save to the same bytes.
    /// let options = SaveOptions::builder().id_source(IdSource::Fixed([0; 16])).build();
    /// ```
    pub fn id_source(mut self, id_source: IdSource) -> Self {
        self.0.id_source = id_source;
        self
    }

    /// Encrypt an unencrypted document on the way out —
    /// [`SaveOptions::encrypt`].
    pub fn encrypt(mut self, encryption: Encryption) -> Self {
        self.0.encrypt = Some(encryption);
        self
    }

    /// The options as built.
    ///
    /// ```
    /// let options = pdfrum::SaveOptions::builder().build();
    /// assert_eq!(options, pdfrum::SaveOptions::default());
    /// ```
    #[must_use]
    pub fn build(self) -> SaveOptions {
        self.0
    }
}

impl SaveOptions {
    /// A builder starting from the defaults.
    ///
    /// ```
    /// let options = pdfrum::SaveOptions::builder().incremental().build();
    /// ```
    pub fn builder() -> SaveOptionsBuilder {
        SaveOptionsBuilder::default()
    }
}

/// The error [`Update`]'s [`FromStr`](std::str::FromStr) returns.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not an update mode: {0}")]
pub struct UnknownUpdate(String);

impl std::fmt::Display for Update {
    /// `rewrite` or `incremental`, which round-trip through
    /// [`FromStr`](std::str::FromStr).
    ///
    /// ```
    /// assert_eq!(pdfrum::Update::Incremental.to_string(), "incremental");
    /// ```
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Update::Rewrite => "rewrite",
            Update::Incremental => "incremental",
        })
    }
}

impl std::str::FromStr for Update {
    type Err = UnknownUpdate;

    /// The inverse of [`Display`](std::fmt::Display) — what a `--update`
    /// flag parses.
    ///
    /// # Errors
    ///
    /// [`UnknownUpdate`] for anything but `rewrite` and `incremental`.
    fn from_str(s: &str) -> core::result::Result<Update, UnknownUpdate> {
        match s {
            "rewrite" => Ok(Update::Rewrite),
            "incremental" => Ok(Update::Incremental),
            other => Err(UnknownUpdate(other.to_owned())),
        }
    }
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
            #[cfg(feature = "svg-text")]
            svg_fonts: crate::SvgFonts::new(),
        }
    }
}

/// Document-level edits — pages, fonts, images, attachments, stamps — and the
/// save that writes them.
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
    /// The faces an ingested SVG's `<text>` is set in. Empty until
    /// [`DocEdit::set_svg_fonts`], and an empty set is what makes every
    /// `<text>` a reported gap rather than a drawn one.
    #[cfg(feature = "svg-text")]
    pub(crate) svg_fonts: crate::SvgFonts,
}

impl<'a> DocEdit<'a> {
    /// Stamp `text` on every page, positioned by `options`.
    ///
    /// # Errors
    ///
    /// [`Error::Save`](crate::Error::Save) when the font cannot be added, and
    /// [`Error::Read`](crate::Error::Read) when a page cannot be opened.
    pub fn stamp_text(
        &mut self,
        text: &str,
        options: &pdfrum_edit::StampOptions,
    ) -> crate::Result<()> {
        Ok(self.inner.stamp_text(text, options, &self.doc.limits)?)
    }

    /// Stamp `image` on every page at `width` points wide, positioned by
    /// `options`. The height follows the image's aspect ratio.
    ///
    /// # Errors
    ///
    /// [`Error::Save`](crate::Error::Save) when `width` is not positive, and
    /// [`Error::Read`](crate::Error::Read) when a page cannot be opened.
    pub fn stamp_image(
        &mut self,
        image: &pdfrum_edit::EmbeddedImage,
        width: f64,
        options: &pdfrum_edit::StampOptions,
    ) -> crate::Result<()> {
        Ok(self
            .inner
            .stamp_image(image, width, options, &self.doc.limits)?)
    }

    /// Compile an SVG into a Form `XObject`, placed with
    /// [`Canvas::place_svg`](crate::Canvas::place_svg).
    ///
    /// One object however many pages place it, where
    /// [`Canvas::draw_svg`](crate::Canvas::draw_svg) writes the operators
    /// again per page.
    ///
    /// # Errors
    ///
    /// [`Error::Svg`](crate::Error::Svg) when the SVG will not resolve.
    #[cfg(feature = "svg-import")]
    pub fn compile_svg(
        &mut self,
        svg: &str,
    ) -> crate::Result<(pdfrum_edit::SvgForm, pdfrum_edit::SvgIngestReport)> {
        self.compile_svg_from(svg, None)
    }

    /// Compile an SVG whose relative `<image href>` links resolve against
    /// `resources_dir`.
    ///
    /// # Errors
    ///
    /// As [`DocEdit::compile_svg`].
    #[cfg(feature = "svg-import")]
    pub fn compile_svg_from(
        &mut self,
        svg: &str,
        resources_dir: Option<&std::path::Path>,
    ) -> crate::Result<(pdfrum_edit::SvgForm, pdfrum_edit::SvgIngestReport)> {
        #[cfg(feature = "svg-text")]
        let out = self.inner.compile_svg_from_with_fonts(
            svg,
            resources_dir,
            &self.doc.limits,
            &self.svg_fonts,
        );
        #[cfg(not(feature = "svg-text"))]
        let out = self
            .inner
            .compile_svg_from(svg, resources_dir, &self.doc.limits);
        Ok(out?)
    }

    /// Draw on page `index` with a [`Canvas`](crate::Canvas).
    ///
    /// The canvas writes one more content stream and merges what it names
    /// into the page's `/Resources`; nothing already on the page is touched.
    ///
    /// # Errors
    ///
    /// Whatever the closure refused to draw, and
    /// [`Error::Save`](crate::Error::Save) for a page written inline in its
    /// parent's `/Kids`.
    pub fn draw_page(
        &mut self,
        index: impl Into<pdfrum_common::PageIndex>,
        body: impl FnOnce(&mut pdfrum_edit::Canvas<'_, '_>),
    ) -> crate::Result<()> {
        #[cfg(feature = "svg-text")]
        let out = self
            .inner
            .draw_page_with_fonts(index, &self.doc.limits, &self.svg_fonts, body);
        #[cfg(not(feature = "svg-text"))]
        let out = self.inner.draw_page(index, &self.doc.limits, body);
        Ok(out?)
    }

    /// Draw on every page, one canvas each.
    ///
    /// A page written inline in its parent's `/Kids` is skipped rather than
    /// refused.
    ///
    /// # Errors
    ///
    /// As [`DocEdit::draw_page`], for the first page whose drawing failed.
    pub fn draw_pages(
        &mut self,
        body: impl FnMut(&mut pdfrum_edit::Canvas<'_, '_>),
    ) -> crate::Result<()> {
        #[cfg(feature = "svg-text")]
        let out = self
            .inner
            .draw_pages_with_fonts(&self.doc.limits, &self.svg_fonts, body);
        #[cfg(not(feature = "svg-text"))]
        let out = self.inner.draw_pages(&self.doc.limits, body);
        Ok(out?)
    }

    /// Adds `bytes` as an embedded file named `name`, and returns its index.
    ///
    /// # Errors
    ///
    /// When the document has no catalog to attach to.
    pub fn add_attachment(
        &mut self,
        name: &str,
        bytes: &[u8],
        options: &pdfrum_edit::AttachmentOptions,
    ) -> crate::Result<usize> {
        Ok(pdfrum_edit::add_attachment(
            &mut self.inner,
            &self.doc.limits,
            name,
            bytes,
            options,
        )?)
    }

    /// Removes the attachment at `index`; `false` when out of range.
    ///
    /// # Errors
    ///
    /// When the document has no catalog.
    pub fn delete_attachment(&mut self, index: usize) -> crate::Result<bool> {
        Ok(pdfrum_edit::delete_attachment(
            &mut self.inner,
            &self.doc.limits,
            index,
        )?)
    }

    /// Removes every attachment named `name`; `false` when none matched.
    ///
    /// # Errors
    ///
    /// When the document has no catalog.
    pub fn remove_attachment(&mut self, name: &str) -> crate::Result<bool> {
        Ok(pdfrum_edit::remove_attachment(
            &mut self.inner,
            &self.doc.limits,
            name,
        )?)
    }

    /// Replaces the bytes of the attachment at `index`; `false` when out of
    /// range.
    ///
    /// # Errors
    ///
    /// When the document has no catalog.
    pub fn set_attachment_file(&mut self, index: usize, bytes: &[u8]) -> crate::Result<bool> {
        Ok(pdfrum_edit::set_attachment_file(
            &mut self.inner,
            &self.doc.limits,
            index,
            bytes,
        )?)
    }

    /// Sets one `/Params` entry on the attachment at `index`; `false` when
    /// out of range.
    ///
    /// # Errors
    ///
    /// When the document has no catalog.
    pub fn set_attachment_param(
        &mut self,
        index: usize,
        key: &str,
        text: &str,
    ) -> crate::Result<bool> {
        Ok(pdfrum_edit::set_attachment_param(
            &mut self.inner,
            &self.doc.limits,
            index,
            key,
            text,
        )?)
    }

    /// Sets the `/Desc` of the attachment at `index`; `false` when out of
    /// range.
    ///
    /// # Errors
    ///
    /// When the document has no catalog.
    pub fn set_attachment_description(&mut self, index: usize, text: &str) -> crate::Result<bool> {
        Ok(pdfrum_edit::set_attachment_description(
            &mut self.inner,
            &self.doc.limits,
            index,
            text,
        )?)
    }

    /// Bakes annotation appearances into the page's content stream.
    ///
    /// `mode` selects which annotations qualify. Appearance generation is
    /// recorded in the document's diagnostics.
    ///
    /// # Errors
    ///
    /// When `page` is out of range or the document has no catalog.
    pub fn flatten(
        &mut self,
        page: impl Into<PageIndex>,
        mode: pdfrum_edit::FlattenMode,
    ) -> crate::Result<pdfrum_edit::Flattened> {
        let mut diags = Diagnostics::default();
        let out = pdfrum_edit::flatten(&mut self.inner, &self.doc.limits, page, mode, &mut diags);
        self.doc.note(&diags);
        Ok(out?)
    }

    /// Create an annotation on `page` and append it to the page's `/Annots`.
    ///
    /// Covers the six subtypes Rotero writes today — `Highlight`, `Text` (note),
    /// `Square` (area), `Underline`, `Ink`, and `FreeText` — via [`crate::AnnotSpec`].
    /// Appearance streams are not generated.
    ///
    /// Returns the new annotation's object reference.
    ///
    /// ```
    /// use pdfrum::{AnnotSpec, Color, Document, Rect, SaveOptions};
    ///
    /// let doc = Document::open("tests/fixtures/hello_world.pdf")?;
    /// let mut edit = doc.edit();
    /// let rect = Rect::new(50.0, 50.0, 150.0, 70.0);
    /// edit.add_annotation(
    ///     0,
    ///     AnnotSpec::text(rect, Color::from_rgb8(255, 200, 0)).with_contents("hello"),
    /// )?;
    /// let mut bytes = Vec::new();
    /// edit.write_to(&mut bytes, &SaveOptions::default())?;
    /// let saved = Document::from_bytes(bytes)?;
    /// let annot = saved.page(0)?.annotations().next().expect("one annot");
    /// assert_eq!(annot.subtype(), pdfrum::Subtype::Text);
    /// assert_eq!(annot.contents().as_deref(), Some("hello"));
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    ///
    /// # Errors
    ///
    /// When `page` is out of range, the page is inline, or a highlight /
    /// underline has no quadrilaterals.
    pub fn add_annotation(
        &mut self,
        page: impl Into<PageIndex>,
        write: impl Into<pdfrum_edit::AnnotWrite>,
    ) -> crate::Result<pdfrum_object::ObjRef> {
        Ok(pdfrum_edit::add_annotation(&mut self.inner, page, write)?)
    }

    /// Replace an existing annotation in place, regenerating `/AP` when needed.
    ///
    /// `annot` must already appear in `page`'s `/Annots`. The object number is
    /// preserved.
    ///
    /// # Errors
    ///
    /// Propagates [`pdfrum_edit::Error`] from the edit crate (bad page, annot
    /// not on page, empty quads, …).
    pub fn update_annotation(
        &mut self,
        page: impl Into<PageIndex>,
        annot: pdfrum_object::ObjRef,
        write: impl Into<pdfrum_edit::AnnotWrite>,
    ) -> crate::Result<()> {
        Ok(pdfrum_edit::update_annotation(
            &mut self.inner,
            page,
            annot,
            write,
        )?)
    }

    /// Remove `annot` from `page`'s `/Annots` and drop the annotation object.
    ///
    /// Returns `Ok(true)` when it was present and removed.
    ///
    /// # Errors
    ///
    /// When `page` is out of range or inline.
    pub fn delete_annotation(
        &mut self,
        page: impl Into<PageIndex>,
        annot: pdfrum_object::ObjRef,
    ) -> crate::Result<bool> {
        Ok(pdfrum_edit::delete_annotation(
            &mut self.inner,
            page,
            annot,
        )?)
    }

    /// Update the annotation at `index` in `page`'s `/Annots` (0-based).
    ///
    /// Resolves the index to an object reference, then calls
    /// [`Self::update_annotation`].
    ///
    /// # Errors
    ///
    /// When the index is out of range, or when update itself fails.
    pub fn update_annotation_at(
        &mut self,
        page: impl Into<PageIndex>,
        index: usize,
        write: impl Into<pdfrum_edit::AnnotWrite>,
    ) -> crate::Result<()> {
        Ok(pdfrum_edit::update_annotation_at(
            &mut self.inner,
            page,
            index,
            write,
        )?)
    }

    /// Delete the annotation at `index` in `page`'s `/Annots` (0-based).
    ///
    /// # Errors
    ///
    /// When the index is out of range, or when delete itself fails.
    pub fn delete_annotation_at(
        &mut self,
        page: impl Into<PageIndex>,
        index: usize,
    ) -> crate::Result<bool> {
        Ok(pdfrum_edit::delete_annotation_at(
            &mut self.inner,
            page,
            index,
        )?)
    }

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
    /// The faces an ingested SVG's `<text>` is set in, for the rest of this
    /// session.
    ///
    /// Set once, and every [`Canvas::draw_svg`](crate::Canvas::draw_svg) and
    /// [`DocEdit::compile_svg`] after it lays its text out in these faces and
    /// draws it as **outlines**. Without this the set is empty, every
    /// `<text>` is reported as
    /// [`Unsupported::Text`](crate::Unsupported::Text) and nothing of it is
    /// drawn — which is what this crate did before the `svg-text` feature
    /// existed, and still does with the feature off.
    ///
    /// A session field rather than an argument on the four ingestion entry
    /// points: a caller's font set does not change between one logo and the
    /// next, and threading it through every signature would grow four
    /// surfaces to say one thing.
    ///
    /// ```no_run
    /// use pdfrum::{Document, SvgFonts};
    ///
    /// let mut fonts = SvgFonts::new();
    /// fonts.register(std::fs::read("Inter.ttf")?);
    ///
    /// let doc = Document::open("in.pdf")?;
    /// let mut edit = doc.edit();
    /// edit.set_svg_fonts(fonts);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[cfg(feature = "svg-text")]
    pub fn set_svg_fonts(&mut self, fonts: crate::SvgFonts) {
        self.svg_fonts = fonts;
    }

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
    /// let saved = Document::from_bytes(bytes)?;
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
    /// (width × height in points), inserting the sheets at the front of this
    /// document — N-up imposition, the way `FPDF_ImportNPagesToOne` does it.
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
        sheet: kurbo::Size,
    ) -> Result<()> {
        pdfrum_edit::n_page_to_one(
            &mut self.inner,
            &source.inner,
            &pdfrum_edit::PageRange::of(pages),
            &pdfrum_edit::NUpOptions {
                sheet: (sheet.width as f32, sheet.height as f32),
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
            self.inner.apply_page(page, &shared)?;
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

/// This crate's options as the writer's own.
///
/// Public for the same reason [`RenderOptions`](crate::RenderOptions)'s is:
/// `pdfrum-edit` takes its own type, and the two do not line up field for
/// field -- this crate's `update` is the engine's `mode`, and the engine's
/// `keep_original` is not exposed here at all.
///
/// ```
/// let engine = pdfrum_edit::SaveOptions::from(&pdfrum::SaveOptions::default());
/// assert!(!engine.subset_new_fonts);
/// ```
impl From<&SaveOptions> for pdfrum_edit::SaveOptions {
    fn from(options: &SaveOptions) -> Self {
        Self {
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
            ..Self::default()
        }
    }
}

/// The one place a [`SaveOptions`] becomes the writer's own options.
fn write_edit(edit: &EditDoc<'_>, options: &SaveOptions, out: &mut impl Write) -> Result<()> {
    pdfrum_edit::save(edit, &pdfrum_edit::SaveOptions::from(options), out)?;
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
