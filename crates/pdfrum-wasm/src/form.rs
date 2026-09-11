//! The form handle: reading fields, filling them, saving the result.

use std::sync::Arc;

use wasm_bindgen::prelude::wasm_bindgen;

use crate::document::Document;
use crate::error::{Failure, Result};
use crate::options::SaveOptions;
use crate::value::{Field, FieldKind};

/// One buffered write.
///
/// An enum rather than a `(String, Option<bool>)`, because the two kinds are
/// different operations on the facade and a caller of `set` never means
/// `setChecked`: `Form::set` writes a value verbatim and `Form::set_checked`
/// asks the facade to pick the field's own first non-`Off` state, which this
/// layer does not know and must not guess.
#[derive(Debug, Clone)]
enum FieldWrite {
    /// A text, choice or state value, verbatim.
    Value(String),
    /// A check box or radio button, set or cleared.
    Checked(bool),
}

/// A document's interactive form, and the values written into it so far.
///
/// Holds a reference to its document, so the document may be freed while the
/// form is still in use. Free the form with `free()` when done.
///
/// **Writes are buffered.** `set` and `setChecked` change nothing in the
/// document; they record a value that `save` writes out. That is what lets one
/// immutable document back several independent forms at once, and it is why
/// filling a form never fails for want of a lock.
#[wasm_bindgen]
#[derive(Debug)]
pub struct Form {
    document: Arc<pdfrum::Document>,
    /// The buffered writes, in the order they were made.
    ///
    /// Kept as a list and replayed onto a fresh `pdfrum::Form` per call rather
    /// than held as a live one: `Form<'a>` borrows its document, and a handle
    /// JavaScript keeps across calls cannot carry that lifetime — the same
    /// reason the C binding replays. Replaying is what the facade's own
    /// buffering already does, so this is the same work in the same order, not
    /// a second mechanism.
    writes: Vec<(String, FieldWrite)>,
}

impl Form {
    /// The document's form with every buffered write replayed onto it.
    fn replayed(&self) -> Result<pdfrum::Form<'_>> {
        let mut form = self
            .document
            .form()
            .ok_or(Failure::Argument("the document has no form"))?;
        for (name, write) in &self.writes {
            let wrote = match write {
                FieldWrite::Value(value) => form.set(name, value.clone()),
                FieldWrite::Checked(checked) => form.set_checked(name, *checked),
            };
            // Unreachable in practice: `write` checks the name against this
            // same document before it buffers anything, and a `Document` is
            // immutable, so a name that existed then exists now. Reported
            // rather than ignored because "cannot happen" is a claim about
            // today's facade, not a property the type system holds.
            wrote.map_err(|_| Failure::Argument("a buffered field is no longer in the form"))?;
        }
        Ok(form)
    }

    /// Buffers a write, after checking the field exists.
    fn write(&mut self, name: &str, write: FieldWrite) -> Result<()> {
        let form = self.replayed()?;
        if form.field(name).is_none() {
            return Err(Failure::Argument("no field has this name"));
        }
        drop(form);
        self.writes.push((name.to_owned(), write));
        Ok(())
    }
}

#[wasm_bindgen]
impl Form {
    /// Opens a document's interactive form.
    ///
    /// # Errors
    ///
    /// Throws with `.code === 100` for a document that has no form, rather
    /// than handing back a handle whose every call would fail.
    pub fn open(document: &Document) -> Result<Form> {
        let document = document.inner();
        // Ask now, so the failure is at the open rather than at the first use.
        if document.form().is_none() {
            return Err(Failure::Argument("the document has no form"));
        }
        Ok(Form {
            document: Arc::clone(document),
            writes: Vec::new(),
        })
    }

    /// Every field of the form, as it stands after each `set` made so far.
    ///
    /// # Errors
    ///
    /// Throws with `.code === 100` when the document's form has gone away
    /// under the handle, which a `Document` being immutable makes unreachable.
    pub fn fields(&self) -> Result<Vec<Field>> {
        Ok(self
            .replayed()?
            .fields()
            .map(|field| Field {
                name: field.name().to_owned(),
                value: field.value(),
                kind: FieldKind::from_facade(field.kind()),
                is_checked: field.is_checked(),
                is_read_only: field.is_read_only(),
                is_required: field.is_required(),
            })
            .collect())
    }

    /// Buffers a value for one field, by its fully-qualified name.
    ///
    /// For a check box or radio button, `value` is the state's own name —
    /// [`Form::set_checked`] is the call that does not need to know it.
    ///
    /// # Errors
    ///
    /// Throws with `.code === 100` when no field has this name.
    pub fn set(&mut self, name: &str, value: String) -> Result<()> {
        self.write(name, FieldWrite::Value(value))
    }

    /// Buffers a check box or radio button as selected or cleared.
    ///
    /// The facade turns `true` into the field's own first non-`Off` state, so
    /// a caller does not have to read the document to find out what it is
    /// called.
    ///
    /// # Errors
    ///
    /// As [`Form::set`].
    #[wasm_bindgen(js_name = setChecked)]
    pub fn set_checked(&mut self, name: &str, on: bool) -> Result<()> {
        self.write(name, FieldWrite::Checked(on))
    }

    /// Writes the document out with every buffered value in place.
    ///
    /// The form stays usable afterwards: a caller may save, write more, and
    /// save again.
    ///
    /// # Errors
    ///
    /// Throws with `.code === 7` when the document cannot be written out.
    pub fn save(&self, options: Option<SaveOptions>) -> Result<Vec<u8>> {
        let form = self.replayed()?;
        let options = options.unwrap_or_default().to_facade();
        let mut bytes = Vec::new();
        self.document.write_form_to(&mut bytes, &form, &options)?;
        Ok(bytes)
    }
}
