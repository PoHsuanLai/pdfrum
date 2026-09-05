//! The form handle: reading fields, filling them, saving the result.

use core::ffi::c_char;
use std::sync::Arc;

use crate::document::{pdfrum_document, pdfrum_save_options};
use crate::error::{Failure, Result, pdfrum_error, with_handle, with_handle_mut};
use crate::str::{Buffer, borrow_str, pdfrum_buffer};

/// A document's interactive form, and the values written into it so far.
///
/// Holds a reference to its document, so the document may be closed while the
/// form is still open.
///
/// **Writes are buffered.** `pdfrum_form_set` and `pdfrum_form_set_checked`
/// change nothing in the document; they record a value that
/// [`pdfrum_form_save`] writes out. That is what lets one immutable document
/// back several independent forms at once, and it is why filling a form never
/// fails for want of a lock.
///
/// **Threads.** A form is single-threaded, and the setters need it
/// exclusively.
#[derive(Debug)]
pub struct pdfrum_form {
    document: Arc<pdfrum::Document>,
    /// The buffered writes, in the order they were made.
    ///
    /// Kept as a list and replayed onto a fresh [`pdfrum::Form`] per call
    /// rather than held as a live `Form`: `Form<'a>` borrows its document, and
    /// a handle C keeps across calls cannot carry that lifetime. Replaying is
    /// what the facade's own buffering already does — `Form::set` records into
    /// a `FieldValues` and nothing reaches the file until a save — so this is
    /// the same work in the same order, not a second mechanism.
    writes: Vec<(String, FieldWrite)>,
    /// The strings the last `pdfrum_form_field` lent out.
    ///
    /// A field's name and value are borrowed from the handle, and the header
    /// says so: the next call replaces them. Keeping exactly one call's worth
    /// — rather than every string a loop over the fields ever asked for — is
    /// what stops a caller that walks a thousand fields from holding a
    /// thousand strings it never asked to own.
    scratch: Scratch,
}

/// The strings one call lends to its caller.
///
/// Freed when the next call replaces them, and when the handle is dropped.
#[derive(Debug, Default)]
struct Scratch(Vec<*mut c_char>);

impl Scratch {
    /// Drops what the previous call lent and interns `text` for this one.
    ///
    /// The clear happens on the *first* intern of a call rather than at the
    /// call's start, so a failed call leaves the previous call's strings alone
    /// — a caller reading them after a failure sees what it saw before.
    fn intern(&mut self, text: &str) -> *const c_char {
        let ptr = crate::str::CString::new(text).into_raw();
        self.0.push(ptr);
        ptr.cast_const()
    }

    /// Frees everything lent so far.
    fn clear(&mut self) {
        for ptr in self.0.drain(..) {
            // SAFETY: every pointer came from `CString::into_raw` in `intern`,
            // and nothing else frees them: they are lent to C as borrows, not
            // handed over as owned pointers.
            unsafe { crate::str::pdfrum_free(ptr.cast()) };
        }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        self.clear();
    }
}

/// One buffered write.
#[derive(Debug, Clone)]
enum FieldWrite {
    /// A text, choice or state value, verbatim.
    Value(String),
    /// A check box or radio button, set or cleared. The facade turns this into
    /// the field's own first non-`Off` state, which this layer does not know.
    Checked(bool),
}

impl pdfrum_form {
    /// Opens a document's form as a handle for C.
    fn open(document: &Arc<pdfrum::Document>) -> Option<*mut pdfrum_form> {
        // The document has a form only if the facade says so; asking now means
        // `pdfrum_form_open` answers null for a document with no form, rather
        // than handing back a handle whose every call fails.
        document.form()?;
        Some(Box::into_raw(Box::new(pdfrum_form {
            document: Arc::clone(document),
            writes: Vec::new(),
            scratch: Scratch::default(),
        })))
    }

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

/// One field of a form.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct pdfrum_field {
    /// The field's fully-qualified name, owned by the form handle and invalid
    /// once the form is freed.
    pub name: *const c_char,
    /// Its value, seen through every `pdfrum_form_set` made so far. Owned by
    /// the form handle.
    pub value: *const c_char,
    /// What kind of control the field is.
    pub kind: pdfrum_field_kind,
    /// Whether a check box or radio button is currently selected. Always
    /// `false` for a field of another kind.
    pub is_checked: bool,
    /// Whether the document marks this field as not editable.
    pub is_read_only: bool,
    /// Whether the document marks this field as one that must be filled.
    pub is_required: bool,
}

/// What kind of control a form field is.
///
/// The number in `pdfrum_field::kind`. A total match over
/// [`pdfrum::FieldKind`], which is not `#[non_exhaustive]`, so a kind added
/// upstream is a compile error here rather than a silent renumbering — which
/// is what a C caller switching on this number needs.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum pdfrum_field_kind {
    /// A push button: it carries no value.
    Button = 0,
    /// A check box.
    Checkbox = 1,
    /// One button of a radio group.
    Radio = 2,
    /// A single- or multi-line text field.
    Text = 3,
    /// A drop-down list.
    Combo = 4,
    /// A scrolling list.
    List = 5,
    /// A signature field.
    Signature = 6,
}

impl pdfrum_field_kind {
    /// The C kind for a facade one.
    fn from_facade(kind: pdfrum::FieldKind) -> pdfrum_field_kind {
        match kind {
            pdfrum::FieldKind::Button => pdfrum_field_kind::Button,
            pdfrum::FieldKind::Check => pdfrum_field_kind::Checkbox,
            pdfrum::FieldKind::Radio => pdfrum_field_kind::Radio,
            pdfrum::FieldKind::Text => pdfrum_field_kind::Text,
            pdfrum::FieldKind::Combo => pdfrum_field_kind::Combo,
            pdfrum::FieldKind::List => pdfrum_field_kind::List,
            pdfrum::FieldKind::Signature => pdfrum_field_kind::Signature,
        }
    }
}

/// Opens a document's interactive form.
///
/// Returns null when the document has no form — which is not a failure and
/// fills no error — or for a null document. The caller frees the handle with
/// [`pdfrum_form_free`]; the document may be closed first.
///
/// # Safety
///
/// `document` is null or a live, unclosed document handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pdfrum_form_open(document: *const pdfrum_document) -> *mut pdfrum_form {
    // SAFETY: the caller's contract says `document` is null or live.
    unsafe { document.as_ref() }
        .and_then(|document| pdfrum_form::open(document.inner()))
        .unwrap_or(core::ptr::null_mut())
}

/// How many fields the form has.
///
/// Returns 0 for a null handle.
///
/// # Safety
///
/// `form` is null or a live, unfreed form handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pdfrum_form_field_count(form: *const pdfrum_form) -> usize {
    // SAFETY: the caller's contract says `form` is null or live.
    unsafe { form.as_ref() }.map_or(0, |form| {
        form.replayed().map_or(0, |form| form.field_count())
    })
}

/// Copies one field into `out`.
///
/// `index` is zero-based, in the order the field tree reaches the fields.
/// Returns `false` — leaving `out` untouched — for a null handle, a null
/// `out`, or an index past the end.
///
/// **`name` and `value` are borrowed from the form handle** and are invalidated
/// by the next call on it, including the next `pdfrum_form_field`. A caller
/// that keeps either past that copies it.
///
/// # Safety
///
/// `form` is null or a live, unfreed form handle, and the caller has it
/// exclusively for the duration of the call. `out` is null or points to a
/// writable `pdfrum_field`. `error` is null or points to a writable
/// `pdfrum_error`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pdfrum_form_field(
    form: *mut pdfrum_form,
    index: usize,
    out: *mut pdfrum_field,
    error: *mut pdfrum_error,
) -> bool {
    // SAFETY: forwarded from this function's own contract.
    unsafe {
        with_handle_mut(form, error, |form| {
            if out.is_null() {
                return Err(Failure::Argument("null out"));
            }
            let replayed = form.replayed()?;
            let fields = replayed.fields();
            let field = fields
                .get(index)
                .ok_or(Failure::Argument("index out of range"))?;
            let name = field.name().to_owned();
            let value = field.value();
            let kind = pdfrum_field_kind::from_facade(field.kind());
            let is_checked = field.is_checked();
            let is_read_only = field.is_read_only();
            let is_required = field.is_required();
            drop(replayed);
            // The two strings live in the handle's own scratch, which this
            // call replaces — the borrow the doc comment promises.
            form.scratch.clear();
            let name = form.scratch.intern(&name);
            let value = form.scratch.intern(&value);
            // SAFETY: `out` is non-null and the caller's contract says it is
            // a writable `pdfrum_field`.
            out.write(pdfrum_field {
                name,
                value,
                kind,
                is_checked,
                is_read_only,
                is_required,
            });
            Ok(())
        })
    }
    .is_some()
}

/// Writes a value into a field.
///
/// The write is buffered: nothing in the document changes until
/// [`pdfrum_form_save`]. For a check box or radio button `value` is the state
/// name the widget's appearance dictionary lists, or `Off` to clear it;
/// [`pdfrum_form_set_checked`] handles the common two-state case.
///
/// Returns `false` on failure with `error` filled — in particular
/// `PDFRUM_CODE_ARGUMENT` when no field has this name.
///
/// # Safety
///
/// `form` is null or a live, unfreed form handle held exclusively. `name` and
/// `value` are non-null NUL-terminated UTF-8 strings. `error` is null or
/// points to a writable `pdfrum_error`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pdfrum_form_set(
    form: *mut pdfrum_form,
    name: *const c_char,
    value: *const c_char,
    error: *mut pdfrum_error,
) -> bool {
    // SAFETY: forwarded from this function's own contract.
    unsafe {
        with_handle_mut(form, error, |form| {
            let name = borrow_str(name, "null field name")?;
            let value = borrow_str(value, "null value")?;
            form.write(name, FieldWrite::Value(value.to_owned()))
        })
    }
    .is_some()
}

/// Checks or clears a check box or radio button.
///
/// `true` selects the field's first non-`Off` state, which for a check box is
/// the only one it has. Buffered like [`pdfrum_form_set`].
///
/// # Safety
///
/// As [`pdfrum_form_set`], without a `value`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pdfrum_form_set_checked(
    form: *mut pdfrum_form,
    name: *const c_char,
    checked: bool,
    error: *mut pdfrum_error,
) -> bool {
    // SAFETY: forwarded from this function's own contract.
    unsafe {
        with_handle_mut(form, error, |form| {
            let name = borrow_str(name, "null field name")?;
            form.write(name, FieldWrite::Checked(checked))
        })
    }
    .is_some()
}

/// Writes the document out with the form's buffered values applied.
///
/// `options` may be null, which is the same as a zeroed
/// `pdfrum_save_options`. On success `out` is filled and the caller frees
/// `out->data` with `pdfrum_free`.
///
/// The form handle is unchanged: its buffered writes stay buffered, so a
/// caller may save, write more, and save again.
///
/// # Safety
///
/// `form` is null or a live, unfreed form handle. `options` is null or points
/// to a readable `pdfrum_save_options`. `out` is non-null and points to a
/// writable `pdfrum_buffer`. `error` is null or points to a writable
/// `pdfrum_error`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pdfrum_form_save(
    form: *const pdfrum_form,
    options: *const pdfrum_save_options,
    out: *mut pdfrum_buffer,
    error: *mut pdfrum_error,
) -> bool {
    // SAFETY: forwarded from this function's own contract.
    unsafe {
        with_handle(form, error, |form| {
            if out.is_null() {
                return Err(Failure::Argument("null out"));
            }
            // SAFETY: the caller's contract says `options` is null or readable.
            let facade = options
                .as_ref()
                .map_or_else(pdfrum::SaveOptions::default, pdfrum_save_options::to_facade);
            let replayed = form.replayed()?;
            let mut bytes = Vec::new();
            form.document
                .write_form_to(&mut bytes, &replayed, &facade)?;
            // SAFETY: `out` is non-null and the caller's contract says it is
            // a writable `pdfrum_buffer`.
            Buffer::give(&bytes, out)
        })
    }
    .is_some()
}

/// Frees a form handle and every string it lent out.
///
/// The document it came from stays open. A null pointer is a no-op.
///
/// # Safety
///
/// `form` is null or a live form handle that has not already been freed, and
/// no other thread is using it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pdfrum_form_free(form: *mut pdfrum_form) {
    if form.is_null() {
        return;
    }
    // SAFETY: the caller's contract says `form` is a live, unfreed handle that
    // `pdfrum_form::open` made with `Box::into_raw`.
    drop(unsafe { Box::from_raw(form) });
}
