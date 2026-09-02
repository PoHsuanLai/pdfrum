//! Reading and filling the interactive form.

use pdfrum_common::Diagnostics;
use pdfrum_doc::form::{self, FieldValues};

use crate::Document;

pub use pdfrum_doc::form::{FieldFlags, FieldKind};

/// A form field name the document does not have.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("no form field named {name:?}")]
pub struct UnknownField {
    /// The name that was asked for.
    pub name: String,
}

/// A document's interactive form (ISO 32000-1 §12.7).
///
/// Obtained from [`Document::form`](crate::Document::form), which returns
/// `None` for a document that declares no `/AcroForm` at all.
///
/// # This type never runs a script, whatever the features
///
/// This is the *reading and filling* view of a form: a field's `/AA` scripts
/// are read as data, so a value that depends on a calculation script reads
/// here as whatever the file last stored. That holds with the `script`
/// feature on too, because scripts run on **events** —
/// [`FormSession`](crate::FormSession) is where they run.
///
/// ```
/// let doc = pdfrum::Document::open("tests/fixtures/text_form.pdf")?;
/// let form = doc.form().expect("this fixture has a form");
///
/// let field = &form.fields()[0];
/// assert_eq!(field.name(), "Text Box");
/// assert_eq!(field.kind(), pdfrum::FieldKind::Text);
/// assert_eq!(field.value(), "");
/// # Ok::<(), pdfrum::Error>(())
/// ```
#[derive(Debug)]
pub struct Form<'a> {
    doc: &'a Document,
    inner: form::Form,
    values: FieldValues,
}

impl<'a> Form<'a> {
    pub(crate) fn load(doc: &'a Document) -> Option<Form<'a>> {
        let mut diags = Diagnostics::default();
        let inner = form::Form::load(&doc.catalog(), &doc.inner, &doc.limits, &mut diags);
        doc.note(&diags);
        let inner = inner?;
        Some(Form {
            doc,
            inner,
            values: FieldValues::new(),
        })
    }

    /// One field, seen through the edits made so far.
    fn view(&self, index: usize, inner: &form::Field) -> Field<'a> {
        Field {
            doc: self.doc,
            inner: inner.clone(),
            index,
            values: self.values.clone(),
        }
    }

    /// The form's terminal fields, in the order the field tree reaches them.
    ///
    /// Interior nodes that exist only to prefix their children's names are
    /// not fields and do not appear; their names do, as the part of each
    /// child's fully-qualified name before the dot.
    ///
    /// Values read through these reflect any [`Form::set`] made so far.
    #[must_use]
    pub fn fields(&self) -> Vec<Field<'a>> {
        self.inner
            .fields
            .iter()
            .enumerate()
            .map(|(index, field)| self.view(index, field))
            .collect()
    }

    /// How many fields the form has.
    #[must_use]
    pub fn field_count(&self) -> usize {
        self.inner.fields.len()
    }

    /// The field with this fully-qualified name.
    ///
    /// ```
    /// let doc = pdfrum::Document::open("tests/fixtures/text_form.pdf")?;
    /// let form = doc.form().expect("form");
    /// assert!(form.field("Text Box").is_some());
    /// assert!(form.field("No Such Field").is_none());
    /// # Ok::<(), pdfrum::Error>(())
    /// ```
    #[must_use]
    pub fn field(&self, name: &str) -> Option<Field<'a>> {
        self.inner
            .fields
            .iter()
            .enumerate()
            .find(|(_, field)| field.name == name)
            .map(|(index, field)| self.view(index, field))
    }

    /// Whether the form asks a reader to regenerate every widget's appearance
    /// (`/NeedAppearances`).
    #[must_use]
    pub fn need_appearances(&self) -> bool {
        self.inner.need_appearances
    }

    /// Writes a value to the field with this fully-qualified name.
    ///
    /// The write is **buffered**: nothing in the file changes until
    /// [`Document::save_form`](crate::Document::save_form) writes it out,
    /// which is what lets a `Document` stay immutable and shareable while a
    /// form is being filled.
    ///
    /// For a check box or radio button, `value` is the **state name** — the
    /// one its widget's appearance dictionary lists, or `Off` to clear it.
    /// [`Field::states`] enumerates them; [`Form::set_checked`] handles the
    /// common two-state case.
    ///
    /// # Errors
    ///
    /// [`UnknownField`] when no field has this fully-qualified name.
    pub fn set(&mut self, name: &str, value: impl Into<String>) -> Result<(), UnknownField> {
        if self.field(name).is_none() {
            return Err(UnknownField {
                name: name.to_owned(),
            });
        }
        self.values.set(name, value);
        Ok(())
    }

    /// Checks or clears a check box or radio button.
    ///
    /// `true` selects the field's first non-`Off` state, which for a check
    /// box is the only one it has. Use [`Form::set`] with an explicit state
    /// name to pick a particular button of a radio group. `checked` itself
    /// stays a `bool` — the value *is* a bool.
    ///
    /// # Errors
    ///
    /// [`UnknownField`] when no field has this fully-qualified name.
    pub fn set_checked(&mut self, name: &str, checked: bool) -> Result<(), UnknownField> {
        let Some(field) = self.field(name) else {
            return Err(UnknownField {
                name: name.to_owned(),
            });
        };
        let state = if checked {
            field
                .states()
                .into_iter()
                .find(|state| state != "Off")
                .unwrap_or_else(|| "Yes".to_owned())
        } else {
            "Off".to_owned()
        };
        self.values.set(name, state);
        Ok(())
    }

    /// Every value written through [`Form::set`], in the order first written.
    pub fn edits(&self) -> impl Iterator<Item = (&str, &str)> {
        self.values.iter()
    }

    pub(crate) fn values(&self) -> &FieldValues {
        &self.values
    }

    pub(crate) fn inner(&self) -> &form::Form {
        &self.inner
    }

    pub(crate) fn document(&self) -> &'a Document {
        self.doc
    }
}

/// One field of an interactive form.
#[derive(Debug, Clone)]
pub struct Field<'a> {
    doc: &'a Document,
    inner: pdfrum_doc::form::Field,
    index: usize,
    /// The edits in force when this handle was taken, so a value read here
    /// is the one a [`Form::set`] wrote rather than the one the file holds.
    values: FieldValues,
}

impl Field<'_> {
    /// The field's fully-qualified name: its ancestors' names and its own,
    /// joined with dots.
    ///
    /// This is the name [`Form::set`] and [`Form::field`] take, and the one a
    /// form submission uses.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.inner.name
    }

    /// What kind of control the field is.
    #[must_use]
    pub fn kind(&self) -> FieldKind {
        self.inner.kind
    }

    /// The field's `/Ff` flag word.
    #[must_use]
    pub fn flags(&self) -> FieldFlags {
        self.inner.flags
    }

    /// The field's index in [`Form::fields`].
    #[must_use]
    pub fn index(&self) -> usize {
        self.index
    }

    /// The field's value.
    ///
    /// A value written through [`Form::set`] wins over the one the file
    /// holds, so a form reads back as it will be saved. [`Field::stored_value`]
    /// is the file's own.
    #[must_use]
    pub fn value(&self) -> String {
        self.inner.value(Some(&self.values), &self.doc.inner)
    }

    /// The value the *file* holds, ignoring any pending [`Form::set`].
    #[must_use]
    pub fn stored_value(&self) -> String {
        self.inner.stored_value(&self.doc.inner)
    }

    /// The field's default value (`/DV`) — what a form reset restores.
    #[must_use]
    pub fn default_value(&self) -> String {
        self.inner.default_value(&self.doc.inner)
    }

    /// Whether a check box or radio button is on. Always false for anything
    /// else.
    #[must_use]
    pub fn is_checked(&self) -> bool {
        self.inner.is_checked(Some(&self.values), &self.doc.inner)
    }

    /// Whether the field may not be changed (`/Ff` bit 1).
    #[must_use]
    pub fn is_read_only(&self) -> bool {
        self.inner.flags.is_read_only()
    }

    /// Whether the field must have a value when the form is submitted
    /// (`/Ff` bit 2).
    #[must_use]
    pub fn is_required(&self) -> bool {
        self.inner.flags.is_required()
    }

    /// The states a check box or radio button can take, from its widgets'
    /// appearance dictionaries. Empty for every other kind.
    #[must_use]
    pub fn states(&self) -> Vec<String> {
        self.inner.states(&self.doc.inner)
    }

    /// A choice field's selectable options (`/Opt`), as the labels a reader
    /// shows. Empty for every other kind.
    #[must_use]
    pub fn options(&self) -> Vec<String> {
        self.inner.options(&self.doc.inner)
    }

    /// The field's tooltip (`/TU`), when it has one.
    #[must_use]
    pub fn tooltip(&self) -> Option<String> {
        self.inner.tooltip(&self.doc.inner)
    }

    /// How many widget annotations draw the field.
    ///
    /// One for most fields; one per button for a radio group; more for a
    /// field that appears on several pages.
    #[must_use]
    pub fn widget_count(&self) -> usize {
        self.inner.widgets.len()
    }
}
