//! Writing documents back out.

use std::io::Write;
use std::path::Path;

use pdfrum_common::Diagnostics;
use pdfrum_edit::{EditDoc, SaveMode};
use pdfrum_object::Object;

use crate::{Document, Form, PageEdit, Result};

/// How a document is written back out.
///
/// A plain config struct with [`Default`], filled in with struct-update
/// syntax (STYLE.md §4).
///
/// ```
/// use pdfrum::{SaveOptions, Update};
///
/// let opts = SaveOptions { update: Update::Incremental, ..SaveOptions::default() };
/// assert_eq!(opts.update, Update::Incremental);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SaveOptions {
    /// Whether to rewrite the file or append to it.
    pub update: Update,
    /// The PDF version to declare in the header, as major × 10 + minor
    /// (`17` for 1.7). `None` keeps the document's own.
    pub version: Option<u8>,
    /// Write an encrypted document out in the clear, dropping `/Encrypt`.
    ///
    /// Off by default: an encrypted document saves encrypted under the handler
    /// its password opened, and the output opens with that same password.
    /// Setting this is the explicit way to decrypt one on the way out, and it
    /// forces a full rewrite — an incremental append cannot decrypt the bytes
    /// already in the file.
    pub remove_security: bool,
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
    ///
    /// ```
    /// # let dir = std::env::temp_dir().join("pdfrum-doc-save");
    /// # std::fs::create_dir_all(&dir)?;
    /// # let out = dir.join("copy.pdf");
    /// let doc = pdfrum::Document::open("tests/fixtures/hello_world.pdf")?;
    /// doc.save(&out)?;
    ///
    /// // What we wrote opens again, with the same page.
    /// let reopened = pdfrum::Document::open(&out)?;
    /// assert_eq!(reopened.page_count(), 1);
    /// # std::fs::remove_dir_all(&dir).ok();
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        self.save_with(path, &SaveOptions::default())
    }

    /// Writes the document to `path` with explicit options.
    ///
    /// # Errors
    ///
    /// As [`Document::save`].
    pub fn save_with(&self, path: impl AsRef<Path>, options: &SaveOptions) -> Result<()> {
        let mut bytes = Vec::new();
        self.write_to(&mut bytes, options)?;
        std::fs::write(path.as_ref(), &bytes)?;
        Ok(())
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
    ///
    /// ```
    /// let doc = pdfrum::Document::open("tests/fixtures/hello_world.pdf")?;
    /// let mut bytes = Vec::new();
    /// doc.write_to(&mut bytes, &pdfrum::SaveOptions::default())?;
    /// assert!(bytes.starts_with(b"%PDF-"));
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    pub fn write_to(&self, out: &mut impl Write, options: &SaveOptions) -> Result<()> {
        let edit = EditDoc::new(&self.inner);
        write_edit(&edit, *options, out)
    }

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
    /// form.set("Text Box", "Hello");
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
                    let stream = edit.add(Object::Stream(pdfrum_object::Stream::new(
                        pdfrum_doc::ap::stream_dict(&generated),
                        generated.stream.clone().into(),
                    )));
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
        write_edit(&edit, *options, out)
    }

    /// Writes the document with edited pages' content regenerated.
    ///
    /// Each [`PageEdit`] whose objects were changed has its content streams
    /// written again from its object graph; a page that was opened and not
    /// changed, and every page not listed at all, comes through untouched.
    /// See [`edit`](crate::edit) for what regeneration loses.
    ///
    /// # Errors
    ///
    /// [`Error::Io`](crate::Error::Io) when the file cannot be written, and
    /// [`Error::Save`](crate::Error::Save) when the document cannot be
    /// serialized.
    ///
    /// ```
    /// # let dir = std::env::temp_dir().join("pdfrum-page-edit");
    /// # std::fs::create_dir_all(&dir)?;
    /// # let out = dir.join("edited.pdf");
    /// use pdfrum::SaveOptions;
    ///
    /// let doc = pdfrum::Document::open("tests/fixtures/hello_world.pdf")?;
    /// let mut page = doc.page(0)?.edit();
    /// assert_eq!(page.len(), 2);
    /// page.remove(1);
    /// doc.save_pages(&out, &[page], &SaveOptions::default())?;
    ///
    /// // Reopening finds one object where there were two.
    /// let saved = pdfrum::Document::open(&out)?;
    /// assert_eq!(saved.page(0)?.edit().len(), 1);
    /// # std::fs::remove_dir_all(&dir).ok();
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    pub fn save_pages(
        &self,
        path: impl AsRef<Path>,
        pages: &[PageEdit],
        options: &SaveOptions,
    ) -> Result<()> {
        let mut bytes = Vec::new();
        self.write_pages_to(&mut bytes, pages, options)?;
        std::fs::write(path.as_ref(), &bytes)?;
        Ok(())
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
        let mut edit = EditDoc::new(&self.inner);
        let shared = pdfrum_edit::shared_objects(&edit);
        for page in pages {
            let Some(rewrite) =
                pdfrum_edit::regenerate(page.graph(), &page.resources(self), &self.inner)
            else {
                continue;
            };
            let Some(reference) = self.inner.page(page.index())?.reference else {
                // A page written inline in its parent's `/Kids` has no object
                // to replace, so its content cannot be rewritten. Rather than
                // half-apply the change, leave the page as it was.
                continue;
            };
            let dict = self.inner.page(page.index())?.dict;
            pdfrum_edit::apply_rewrite(&mut edit, reference, &dict, &rewrite, &shared);
        }
        write_edit(&edit, *options, out)
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
    /// doc.import_pages(&out, &extra, &[0, 1], doc.page_count())?;
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
        pages: &[u32],
        at: u32,
    ) -> Result<()> {
        let mut edit = EditDoc::new(&self.inner);
        pdfrum_edit::import_pages(
            &mut edit,
            &source.inner,
            &pdfrum_edit::PageRange::of(pages.iter().copied()),
            &pdfrum_edit::ImportOptions {
                at,
                viewer_preferences: false,
            },
        )?;
        let mut bytes = Vec::new();
        write_edit(&edit, SaveOptions::default(), &mut bytes)?;
        std::fs::write(path.as_ref(), &bytes)?;
        Ok(())
    }
}

/// The one place a [`SaveOptions`] becomes the writer's own options.
fn write_edit(edit: &EditDoc<'_>, options: SaveOptions, out: &mut impl Write) -> Result<()> {
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
        ..pdfrum_edit::SaveOptions::default()
    };
    pdfrum_edit::save(edit, &opts, out)?;
    Ok(())
}

/// A copy of `dict` with `key` set, keeping every other entry in place.
fn set_key(dict: &pdfrum_object::Dict, key: &str, value: Object) -> pdfrum_object::Dict {
    let key = pdfrum_object::Name::from(key);
    let mut out = pdfrum_object::Dict::new();
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
