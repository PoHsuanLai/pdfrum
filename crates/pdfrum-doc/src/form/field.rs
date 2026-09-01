//! The AcroForm field model: what a field *is*, what it holds, and what
//! changing it implies.
//!
//! A field is a dictionary reachable from the catalog's `/AcroForm /Fields`,
//! and the tree it sits in is not the annotation tree: an interior node may
//! carry no widget at all and exist only to prefix its children's names, and
//! a leaf may be merged with its own widget annotation into one dictionary.
//! Both shapes are common and neither is an error, so the walk here collects
//! **terminal** fields — the nodes that carry an `/FT`, inherited or not —
//! and treats everything above them as naming structure.
//!
//! # Values are not stored here
//!
//! [`Field`] is a record of where a field lives, not a copy of what it holds:
//! its value is read back out of the dictionary on demand. That is what makes
//! [`FieldValues`] — the edit buffer — the only mutable thing in this module,
//! and it is why reading a field never has to be told whether someone has
//! written to it.

use pdfrum_common::{Diagnostics, Limits};
use pdfrum_object::{Dict, Name, ObjRef, Object, Resolve};

use crate::ap::{self, GeneratedAp};
use crate::form::attr::{field_attr, full_name};
use crate::names;

/// What kind of control a form field is (ISO 32000-1 §12.7.4).
///
/// The variants are the `/FT` values crossed with the two `/Ff` bits that
/// split them: a `/Btn` is a push button, a radio button or a check box
/// depending on bits 17 and 16, and a `/Ch` is a combo box or a list box
/// depending on bit 18.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FieldKind {
    /// A text field (`/Tx`).
    Text,
    /// A check box (`/Btn` with neither the push-button nor the radio bit).
    Check,
    /// A radio button (`/Btn` with bit 16 set).
    Radio,
    /// A push button (`/Btn` with bit 17 set) — it holds no value.
    Button,
    /// A drop-down (`/Ch` with bit 18 set).
    Combo,
    /// A list box (`/Ch` without bit 18).
    List,
    /// A signature field (`/Sig`).
    Signature,
}

impl FieldKind {
    /// Classifies a field from its `/FT` and `/Ff`.
    ///
    /// Returns `None` for a node with no field type at all, which is how the
    /// walk tells a naming-only interior node from a terminal field.
    #[must_use]
    pub fn classify(field_type: &[u8], flags: FieldFlags) -> Option<FieldKind> {
        match field_type {
            b"Tx" => Some(FieldKind::Text),
            b"Sig" => Some(FieldKind::Signature),
            b"Btn" => Some(if flags.is_push_button() {
                FieldKind::Button
            } else if flags.is_radio() {
                FieldKind::Radio
            } else {
                FieldKind::Check
            }),
            b"Ch" => Some(if flags.is_combo() {
                FieldKind::Combo
            } else {
                FieldKind::List
            }),
            _ => None,
        }
    }

    /// Whether the field holds a value a caller can write.
    ///
    /// False only for [`FieldKind::Button`], which fires an action rather
    /// than storing anything, and [`FieldKind::Signature`], whose value is a
    /// signature dictionary this crate does not synthesize.
    #[must_use]
    pub fn is_writable(self) -> bool {
        !matches!(self, FieldKind::Button | FieldKind::Signature)
    }

    /// Whether the field is one of the two on/off controls, whose value is a
    /// state name rather than free text.
    #[must_use]
    pub fn is_toggle(self) -> bool {
        matches!(self, FieldKind::Check | FieldKind::Radio)
    }
}

/// A field's `/Ff` flag word (ISO 32000-1 tables 227–230).
///
/// Kept as the raw word rather than a bitflags set: the meaning of a bit
/// depends on the field type, so the same bit 26 is "file select" on a text
/// field and "sort" on a choice field. The accessors below are the ones whose
/// reading is type-independent or whose type is implied by the name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct FieldFlags(pub i64);

impl FieldFlags {
    /// Bit 1: the field may not be changed.
    #[must_use]
    pub fn is_read_only(self) -> bool {
        self.0 & (1 << 0) != 0
    }

    /// Bit 2: the field must have a value when the form is submitted.
    #[must_use]
    pub fn is_required(self) -> bool {
        self.0 & (1 << 1) != 0
    }

    /// Bit 16, on a `/Btn`: the field is a radio button rather than a check
    /// box.
    #[must_use]
    pub fn is_radio(self) -> bool {
        self.0 & (1 << 15) != 0
    }

    /// Bit 17, on a `/Btn`: the field is a push button and holds no value.
    #[must_use]
    pub fn is_push_button(self) -> bool {
        self.0 & (1 << 16) != 0
    }

    /// Bit 18, on a `/Ch`: the field is a drop-down rather than a list box.
    #[must_use]
    pub fn is_combo(self) -> bool {
        self.0 & (1 << 17) != 0
    }

    /// Bit 19, on a `/Ch`: the combo box includes an editable text box.
    #[must_use]
    pub fn is_editable_combo(self) -> bool {
        self.0 & (1 << 18) != 0
    }

    /// Bit 22, on a `/Ch`: more than one option may be selected at once.
    #[must_use]
    pub fn is_multi_select(self) -> bool {
        self.0 & (1 << 21) != 0
    }

    /// Bit 13, on a `/Tx`: the field accepts more than one line.
    #[must_use]
    pub fn is_multiline(self) -> bool {
        self.0 & (1 << 12) != 0
    }

    /// Bit 14, on a `/Tx`: the field's contents are obscured as they are
    /// typed.
    #[must_use]
    pub fn is_password(self) -> bool {
        self.0 & (1 << 13) != 0
    }

    /// Bit 24, on a `/Tx`: the field does not scroll to fit more text than
    /// its rectangle holds.
    #[must_use]
    pub fn do_not_scroll(self) -> bool {
        self.0 & (1 << 23) != 0
    }
}

/// One terminal form field.
///
/// A record of *where* the field is — the dictionary, the reference that names
/// it, its widgets — plus the classification derived once at load. What it
/// currently holds is read back through [`Field::value`], because a
/// [`FieldValues`] edit may have superseded the file's own `/V`.
#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    /// The field's own dictionary.
    pub dict: Dict,
    /// The reference that names it, when it has one. A field written inline
    /// in its parent's `/Kids` has none, and cannot be written back.
    pub reference: Option<ObjRef>,
    /// The fully-qualified name: the ancestors' `/T` values and its own,
    /// joined with dots.
    pub name: String,
    /// What kind of control it is.
    pub kind: FieldKind,
    /// The `/Ff` flag word, read through the inheritance chain.
    pub flags: FieldFlags,
    /// The widget annotations that draw it.
    ///
    /// Usually one. A radio group has one per button, and a field whose
    /// dictionary *is* its widget has one that is the field itself.
    pub widgets: Vec<Widget>,
}

/// One widget annotation drawing a field.
#[derive(Debug, Clone, PartialEq)]
pub struct Widget {
    /// The widget's dictionary. Equal to the field's when the two are merged.
    pub dict: Dict,
    /// The reference naming it, when it has one.
    pub reference: Option<ObjRef>,
}

impl Field {
    /// The field's current value, as text.
    ///
    /// `values` is consulted first, so a field written through
    /// [`FieldValues::set`] reads back as what was written rather than what
    /// the file holds. Pass `None` to read the file's own `/V`.
    ///
    /// For a check box or radio button this is the *state name* — `Off` for
    /// clear, and whatever the widget's `/AP /N` calls its on-state
    /// otherwise. Use [`Field::is_checked`] for the boolean.
    #[must_use]
    pub fn value<R: Resolve>(&self, values: Option<&FieldValues>, r: &R) -> String {
        if let Some(edited) = values.and_then(|values| values.get(&self.name)) {
            return edited.to_owned();
        }
        self.stored_value(r)
    }

    /// The value the *file* holds, ignoring any edit.
    #[must_use]
    pub fn stored_value<R: Resolve>(&self, r: &R) -> String {
        let (limits, mut diags) = (Limits::default(), Diagnostics::default());
        field_attr(&self.dict, names::V, r, &limits, &mut diags)
            .map(|value| value.to_text())
            .unwrap_or_default()
    }

    /// The field's default value (`/DV`) — what a form reset restores.
    #[must_use]
    pub fn default_value<R: Resolve>(&self, r: &R) -> String {
        let (limits, mut diags) = (Limits::default(), Diagnostics::default());
        field_attr(&self.dict, names::DV, r, &limits, &mut diags)
            .map(|value| value.to_text())
            .unwrap_or_default()
    }

    /// Whether a check box or radio button is on.
    ///
    /// Always false for a field that is not a toggle. A state of `Off`, and an
    /// absent state, both read as clear — every other name is on, which is the
    /// spec's own rule.
    #[must_use]
    pub fn is_checked<R: Resolve>(&self, values: Option<&FieldValues>, r: &R) -> bool {
        if !self.kind.is_toggle() {
            return false;
        }
        let value = self.value(values, r);
        !value.is_empty() && value != "Off"
    }

    /// The states a check box or radio button can take, from its widgets'
    /// `/AP /N` sub-dictionaries.
    ///
    /// `Off` is included when a widget offers it. The order is the widgets'
    /// order, then each widget's own appearance-dictionary order.
    #[must_use]
    pub fn states<R: Resolve>(&self, r: &R) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for widget in &self.widgets {
            let Some(ap) = widget.dict.dict(names::AP, r) else {
                continue;
            };
            let Some(normal) = ap.dict(names::N, r) else {
                continue;
            };
            for key in normal.keys() {
                let state = String::from_utf8_lossy(key.as_bytes()).into_owned();
                if !out.contains(&state) {
                    out.push(state);
                }
            }
        }
        out
    }

    /// A choice field's selectable options (`/Opt`).
    ///
    /// An entry written as a two-element array is an export-value/label pair;
    /// the label is what a reader shows, and that is what comes back here.
    #[must_use]
    pub fn options<R: Resolve>(&self, r: &R) -> Vec<String> {
        let (limits, mut diags) = (Limits::default(), Diagnostics::default());
        let Some(opt) = field_attr(&self.dict, names::OPT, r, &limits, &mut diags) else {
            return Vec::new();
        };
        let Some(array) = opt.as_array() else {
            return Vec::new();
        };
        (0..array.len())
            .map(|index| {
                let Some(entry) = array.get(index, r) else {
                    return String::new();
                };
                // A pair is [export, label]; a bare string is both.
                match entry.get() {
                    Object::Array(pair) => pair
                        .raw_at(1)
                        .or_else(|| pair.raw_at(0))
                        .map(Object::to_text)
                        .unwrap_or_default(),
                    object => object.to_text(),
                }
            })
            .collect()
    }

    /// The field's user-facing tooltip (`/TU`), when it has one.
    #[must_use]
    pub fn tooltip<R: Resolve>(&self, r: &R) -> Option<String> {
        let (limits, mut diags) = (Limits::default(), Diagnostics::default());
        field_attr(&self.dict, names::TU, r, &limits, &mut diags).map(|value| value.to_text())
    }
}

/// Every terminal field of a document's interactive form.
///
/// Loaded once from the catalog; the walk is the expensive part and nothing
/// below repeats it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Form {
    /// The terminal fields, in the order the `/Fields` tree reaches them.
    pub fields: Vec<Field>,
    /// Whether the form asks a reader to regenerate every widget's appearance
    /// (`/NeedAppearances`).
    pub need_appearances: bool,
}

/// How deep the field tree may nest before the walk gives up.
///
/// The same cap the name-tree walks use, for the same reason: a `/Kids` cycle
/// is stopped by the visited set, but a legitimately deep tree still has to
/// end somewhere.
const MAX_FIELD_DEPTH: u32 = 32;

impl Form {
    /// Loads the document's form, or nothing when the catalog declares none.
    ///
    /// A catalog with an `/AcroForm` whose `/Fields` is absent or empty still
    /// yields a `Form` — an empty form is a different thing from no form, and
    /// only the second means "this document is not interactive".
    #[must_use]
    pub fn load<R: Resolve>(
        catalog: &Dict,
        r: &R,
        limits: &Limits,
        diags: &mut Diagnostics,
    ) -> Option<Form> {
        let acro = catalog.dict(names::ACRO_FORM, r)?;
        let need_appearances = acro
            .get(names::NEED_APPEARANCES, r)
            .and_then(|value| value.as_direct().and_then(Object::as_bool))
            .unwrap_or(false);
        let mut form = Form {
            fields: Vec::new(),
            need_appearances,
        };
        let Some(fields) = acro.array(names::FIELDS, r) else {
            return Some(form);
        };
        let mut seen: Vec<ObjRef> = Vec::new();
        for index in 0..fields.len() {
            let reference = fields.reference_at(index);
            let Some(dict) = fields.dict_at(index, r) else {
                continue;
            };
            visit(
                &dict,
                reference,
                0,
                &mut seen,
                &mut form.fields,
                r,
                limits,
                diags,
            );
        }
        Some(form)
    }

    /// How many terminal fields the form has.
    #[must_use]
    pub fn len(&self) -> usize {
        self.fields.len()
    }

    /// Whether the form has no fields at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    /// The field with this fully-qualified name.
    #[must_use]
    pub fn field(&self, name: &str) -> Option<&Field> {
        self.fields.iter().find(|field| field.name == name)
    }
}

/// Walks one node of the field tree, collecting the terminal fields under it.
#[allow(clippy::too_many_arguments)]
fn visit<R: Resolve>(
    dict: &Dict,
    reference: Option<ObjRef>,
    depth: u32,
    seen: &mut Vec<ObjRef>,
    out: &mut Vec<Field>,
    r: &R,
    limits: &Limits,
    diags: &mut Diagnostics,
) {
    if depth > MAX_FIELD_DEPTH {
        diags.record(
            pdfrum_common::Severity::Suspicious,
            pdfrum_common::DiagKind::TreeDepthExceeded,
            None,
        );
        return;
    }
    // A `/Kids` cycle would otherwise spin forever. Only referenced nodes can
    // close one; an inline dictionary is a fresh value every time.
    if let Some(reference) = reference {
        if seen.contains(&reference) {
            diags.record(
                pdfrum_common::Severity::Recovered,
                pdfrum_common::DiagKind::NavigationCycle,
                None,
            );
            return;
        }
        seen.push(reference);
    }

    let kids = dict.array(names::KIDS, r);
    let field_type = field_attr(dict, names::FT, r, limits, diags)
        .map(|value| value.to_byte_string())
        .unwrap_or_default();
    let flags = FieldFlags(
        field_attr(dict, names::FF, r, limits, diags)
            .and_then(|value| value.as_int())
            .unwrap_or(0),
    );

    // A node is terminal when it has a field type and its kids — if any — are
    // widgets rather than further fields. A kid carrying its own `/T` is a
    // field in its own right, and makes this node naming structure even
    // though it has an `/FT` to inherit down.
    let kids_are_fields = kids.as_ref().is_some_and(|kids| {
        (0..kids.len()).any(|index| {
            kids.dict_at(index, r)
                .is_some_and(|kid| kid.contains_key(names::T))
        })
    });

    if let Some(kind) = FieldKind::classify(&field_type, flags)
        && !kids_are_fields
    {
        let name = full_name(dict, r);
        let widgets = widgets_of(dict, reference, kids.as_ref(), r);
        // A name already in the tree gets these widgets **added as further
        // controls** rather than a second field of its own. Upstream's
        // `AddTerminalField` looks the name up first and only builds a
        // `CPDF_FormField` when it is new, so two `/Annots` entries sharing a
        // `/T` are one field with two controls — and the value every one of
        // them shows is the *field's*, which is the first dictionary's.
        // `bug_733528` is exactly that: two widgets named `SharedField`, the
        // first holding `/V (Hello, world)` and the second `/V ()`, and the
        // golden reports the second drawing the first's text.
        if let Some(existing) = out.iter_mut().find(|field| field.name == name) {
            existing.widgets.extend(widgets);
            return;
        }
        out.push(Field {
            name,
            kind,
            flags,
            widgets,
            dict: dict.clone(),
            reference,
        });
        return;
    }

    let Some(kids) = kids else {
        return;
    };
    for index in 0..kids.len() {
        let kid_ref = kids.reference_at(index);
        let Some(kid) = kids.dict_at(index, r) else {
            continue;
        };
        visit(&kid, kid_ref, depth + 1, seen, out, r, limits, diags);
    }
}

/// The widgets drawing a terminal field.
///
/// Either the field's kids — a radio group's buttons, or a field split across
/// pages — or the field's own dictionary when the two are merged, which is
/// the common single-widget shape.
fn widgets_of<R: Resolve>(
    dict: &Dict,
    reference: Option<ObjRef>,
    kids: Option<&pdfrum_object::Array>,
    r: &R,
) -> Vec<Widget> {
    if let Some(kids) = kids
        && !kids.is_empty()
    {
        let found: Vec<Widget> = (0..kids.len())
            .filter_map(|index| {
                let kid = kids.dict_at(index, r)?;
                Some(Widget {
                    reference: kids.reference_at(index),
                    dict: kid,
                })
            })
            .collect();
        if !found.is_empty() {
            return found;
        }
    }
    // Merged field-and-widget: the field dictionary is the annotation.
    if dict.byte_string(names::SUBTYPE, r).as_deref() == Some(b"Widget") {
        return vec![Widget {
            dict: dict.clone(),
            reference,
        }];
    }
    Vec::new()
}

/// Values written to a form's fields, keyed by fully-qualified name.
///
/// The edit buffer, and the reason this crate can fill a form without
/// mutating anything: the parser's object store is immutable and its objects
/// are values, so a write is recorded here and every reader consults it. It
/// is the same shape as the appearance
/// [`AnnotOverlay`](crate::AnnotOverlay), for the same reason.
///
/// Turning the buffer into a file is [`apply`]'s job.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FieldValues {
    entries: Vec<(String, String)>,
}

impl FieldValues {
    /// An empty buffer.
    #[must_use]
    pub fn new() -> FieldValues {
        FieldValues::default()
    }

    /// Records a value for the field with this fully-qualified name,
    /// replacing any earlier one.
    pub fn set(&mut self, name: impl Into<String>, value: impl Into<String>) {
        let name = name.into();
        let value = value.into();
        match self.entries.iter_mut().find(|(key, _)| *key == name) {
            Some(entry) => entry.1 = value,
            None => self.entries.push((name, value)),
        }
    }

    /// What was written for this field, if anything.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    /// Every recorded write, in the order it was first made.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.entries
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
    }

    /// How many fields have been written to.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether nothing has been written.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// One field's edited dictionary, and the widget appearances that follow from
/// it.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldEdit {
    /// The reference to replace.
    pub reference: ObjRef,
    /// The field dictionary with its `/V` — and, for a toggle, its `/AS` —
    /// rewritten.
    pub dict: Dict,
    /// Regenerated appearances for this field's widgets, each with the
    /// reference to replace. Empty when the widgets' own appearances already
    /// cover the new value, which is the case for a toggle whose `/AP /N`
    /// lists the state it was switched to.
    pub widgets: Vec<(ObjRef, Dict, GeneratedAp)>,
}

/// Turns an edit buffer into the object replacements that write it to a file.
///
/// This is the whole "fill a form and save it" step: it rewrites each edited
/// field's `/V`, sets a toggle's widget `/AS` to the state chosen, and
/// regenerates the appearance of any widget whose own `/AP` cannot show the
/// new value.
///
/// A field the buffer names but the form does not have is skipped, as is one
/// whose dictionary is inline and therefore has no reference to replace.
#[must_use]
pub fn apply<R: Resolve>(
    form: &Form,
    values: &FieldValues,
    r: &R,
    diags: &mut Diagnostics,
) -> Vec<FieldEdit> {
    let mut out = Vec::new();
    for (name, value) in values.iter() {
        let Some(field) = form.field(name) else {
            continue;
        };
        let Some(reference) = field.reference else {
            continue;
        };
        if !field.kind.is_writable() {
            continue;
        }

        let dict = rewrite(&field.dict, names::V, value_object(field.kind, value));
        let mut widgets = Vec::new();
        for widget in &field.widgets {
            let Some(widget_ref) = widget.reference else {
                continue;
            };
            // A toggle's widget selects its appearance with `/AS`; a text or
            // choice field's has to have one drawn.
            let widget_dict = if field.kind.is_toggle() {
                rewrite(
                    &widget.dict,
                    names::AS,
                    Object::Name(Name::from(value.as_bytes())),
                )
            } else {
                widget.dict.clone()
            };
            // Merged field-and-widget: the value edit and the widget edit are
            // the same object, so fold them together rather than emitting two
            // replacements that would clobber each other.
            let widget_dict = if widget_ref == reference {
                merge(&dict, &widget_dict)
            } else {
                widget_dict
            };
            if let Some(generated) = ap::widget::generate(&widget_dict, r) {
                widgets.push((widget_ref, widget_dict, generated));
            } else if widget_ref != reference && field.kind.is_toggle() {
                // No appearance needed to be drawn, but `/AS` still changed.
                widgets.push((
                    widget_ref,
                    widget_dict,
                    GeneratedAp {
                        stream: Vec::new(),
                        bbox: kurbo::Rect::ZERO,
                        matrix: kurbo::Affine::IDENTITY,
                        resources: Dict::new(),
                        rect_override: None,
                        as_override: None,
                    },
                ));
            }
        }
        let _ = diags;
        out.push(FieldEdit {
            reference,
            dict,
            widgets,
        });
    }
    out
}

/// The object a value is stored as: a name for a toggle's state, a string for
/// everything else.
fn value_object(kind: FieldKind, value: &str) -> Object {
    if kind.is_toggle() {
        Object::Name(Name::from(value.as_bytes()))
    } else {
        Object::Str(pdfrum_object::PdfString::literal(value.as_bytes()))
    }
}

/// A copy of `dict` with `key` set to `value`, keeping every other entry in
/// its original position.
fn rewrite(dict: &Dict, key: &Name, value: Object) -> Dict {
    let mut out = Dict::new();
    let mut replaced = false;
    for (existing, held) in dict.iter() {
        if existing == key {
            if !replaced {
                out.push(existing.clone(), value.clone());
                replaced = true;
            }
        } else {
            out.push(existing.clone(), held.clone());
        }
    }
    if !replaced {
        out.push(key.clone(), value);
    }
    out
}

/// Folds `overlay`'s entries onto `base`, for the merged field-and-widget
/// case where two edits target one object.
fn merge(base: &Dict, overlay: &Dict) -> Dict {
    let mut out = base.clone();
    for (key, value) in overlay.iter() {
        if base.raw(key) != Some(value) {
            out = rewrite(&out, key, value.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdfrum_object::{Array, NoResolve, PdfString};

    fn dict(pairs: &[(&str, Object)]) -> Dict {
        Dict::from_pairs(
            pairs
                .iter()
                .map(|(k, v)| (Name::from(*k), v.clone()))
                .collect::<Vec<_>>(),
        )
    }

    fn text(value: &str) -> Object {
        Object::Str(PdfString::literal(value.as_bytes()))
    }

    fn name(value: &str) -> Object {
        Object::Name(Name::from(value))
    }

    fn load(catalog: &Dict) -> Option<Form> {
        let (limits, mut diags) = (Limits::default(), Diagnostics::default());
        Form::load(catalog, &NoResolve, &limits, &mut diags)
    }

    /// The single field a one-field fixture is expected to have.
    fn only_field(form: &Form) -> &Field {
        assert_eq!(form.len(), 1, "the fixture has exactly one field");
        form.fields.first().expect("one field")
    }

    fn catalog_with(fields: Vec<Object>) -> Dict {
        let acro = dict(&[("Fields", Object::Array(Array::of(fields)))]);
        dict(&[("AcroForm", Object::Dict(acro))])
    }

    #[test]
    fn a_catalog_without_an_acroform_has_no_form() {
        assert_eq!(load(&Dict::new()), None);
    }

    #[test]
    fn an_acroform_without_fields_is_an_empty_form_not_an_absent_one() {
        let catalog = dict(&[("AcroForm", Object::Dict(Dict::new()))]);
        let form = load(&catalog).expect("an AcroForm is a form");
        assert!(form.is_empty());
    }

    #[test]
    fn a_terminal_field_is_classified_from_its_type_and_flags() {
        let catalog = catalog_with(vec![Object::Dict(dict(&[
            ("FT", name("Tx")),
            ("T", text("greeting")),
            ("V", text("hello")),
        ]))]);
        let form = load(&catalog).expect("form");
        assert_eq!(form.len(), 1);
        let field = &only_field(&form);
        assert_eq!(field.name, "greeting");
        assert_eq!(field.kind, FieldKind::Text);
        assert_eq!(field.stored_value(&NoResolve), "hello");
    }

    #[test]
    fn the_button_flags_split_the_three_button_kinds() {
        // No bits: a check box.
        assert_eq!(
            FieldKind::classify(b"Btn", FieldFlags(0)),
            Some(FieldKind::Check)
        );
        // Bit 16: a radio button.
        assert_eq!(
            FieldKind::classify(b"Btn", FieldFlags(1 << 15)),
            Some(FieldKind::Radio)
        );
        // Bit 17 wins over bit 16: a push button.
        assert_eq!(
            FieldKind::classify(b"Btn", FieldFlags((1 << 16) | (1 << 15))),
            Some(FieldKind::Button)
        );
    }

    #[test]
    fn a_choice_field_splits_on_the_combo_bit() {
        assert_eq!(
            FieldKind::classify(b"Ch", FieldFlags(0)),
            Some(FieldKind::List)
        );
        assert_eq!(
            FieldKind::classify(b"Ch", FieldFlags(1 << 17)),
            Some(FieldKind::Combo)
        );
    }

    #[test]
    fn a_node_with_no_field_type_is_not_a_field() {
        assert_eq!(FieldKind::classify(b"", FieldFlags(0)), None);
        assert_eq!(FieldKind::classify(b"Nonsense", FieldFlags(0)), None);
    }

    #[test]
    fn a_naming_node_contributes_its_children_and_its_name_prefix() {
        // An interior node with a `/T` but no `/FT`, whose kids are fields.
        // The kid carries the `/Parent` back-pointer a real file writes,
        // because that is the edge `full_name` walks — the tree is navigated
        // downwards to find fields and upwards to name them.
        let parent_dict = dict(&[("T", text("address"))]);
        let kid = dict(&[
            ("FT", name("Tx")),
            ("T", text("street")),
            ("Parent", Object::Dict(parent_dict)),
        ]);
        let parent = dict(&[
            ("T", text("address")),
            ("Kids", Object::Array(Array::of([Object::Dict(kid)]))),
        ]);
        let catalog = catalog_with(vec![Object::Dict(parent)]);
        let form = load(&catalog).expect("form");
        assert_eq!(form.len(), 1);
        // The name is qualified through the parent, which is the whole point
        // of the interior node.
        assert_eq!(only_field(&form).name, "address.street");
    }

    #[test]
    fn a_field_whose_kids_are_widgets_stays_one_field() {
        // A radio group: the `/FT` is on the parent, the kids are widgets
        // with no `/T` of their own.
        let on = dict(&[("Subtype", name("Widget")), ("AS", name("A"))]);
        let off = dict(&[("Subtype", name("Widget")), ("AS", name("Off"))]);
        let group = dict(&[
            ("FT", name("Btn")),
            ("Ff", Object::Int(1 << 15)),
            ("T", text("choice")),
            ("V", name("A")),
            (
                "Kids",
                Object::Array(Array::of([Object::Dict(on), Object::Dict(off)])),
            ),
        ]);
        let catalog = catalog_with(vec![Object::Dict(group)]);
        let form = load(&catalog).expect("form");
        assert_eq!(form.len(), 1, "a radio group is one field, not two");
        let field = &only_field(&form);
        assert_eq!(field.kind, FieldKind::Radio);
        assert_eq!(field.widgets.len(), 2);
        assert!(field.is_checked(None, &NoResolve));
    }

    #[test]
    fn a_merged_field_and_widget_reports_itself_as_its_widget() {
        let merged = dict(&[
            ("FT", name("Tx")),
            ("T", text("box")),
            ("Subtype", name("Widget")),
        ]);
        let catalog = catalog_with(vec![Object::Dict(merged)]);
        let form = load(&catalog).expect("form");
        assert_eq!(only_field(&form).widgets.len(), 1);
        assert_eq!(
            only_field(&form).widgets.first().expect("one widget").dict,
            only_field(&form).dict
        );
    }

    #[test]
    fn a_toggle_reads_off_and_absent_as_clear_and_everything_else_as_set() {
        let make = |value: Option<Object>| {
            let mut pairs = vec![("FT", name("Btn")), ("T", text("t"))];
            if value.is_some() {
                pairs.push(("V", value.clone().unwrap_or(Object::Null)));
            }
            let catalog = catalog_with(vec![Object::Dict(dict(&pairs))]);
            let form = load(&catalog).expect("form");
            only_field(&form).is_checked(None, &NoResolve)
        };
        assert!(!make(None), "absent is clear");
        assert!(!make(Some(name("Off"))), "Off is clear");
        assert!(make(Some(name("Yes"))), "any other state is set");
    }

    #[test]
    fn a_push_button_and_a_signature_hold_no_writable_value() {
        assert!(!FieldKind::Button.is_writable());
        assert!(!FieldKind::Signature.is_writable());
        assert!(FieldKind::Text.is_writable());
        assert!(FieldKind::Check.is_writable());
    }

    #[test]
    fn writing_a_value_records_it_and_reading_sees_it() {
        let catalog = catalog_with(vec![Object::Dict(dict(&[
            ("FT", name("Tx")),
            ("T", text("greeting")),
            ("V", text("hello")),
        ]))]);
        let form = load(&catalog).expect("form");
        let mut values = FieldValues::new();
        values.set("greeting", "goodbye");
        // The edit wins over the file.
        assert_eq!(
            only_field(&form).value(Some(&values), &NoResolve),
            "goodbye"
        );
        // And the file is unchanged.
        assert_eq!(only_field(&form).stored_value(&NoResolve), "hello");
    }

    #[test]
    fn setting_the_same_field_twice_keeps_the_last_write_and_one_entry() {
        let mut values = FieldValues::new();
        values.set("a", "one");
        values.set("a", "two");
        assert_eq!(values.len(), 1);
        assert_eq!(values.get("a"), Some("two"));
    }

    #[test]
    fn an_option_pair_reports_its_label_rather_than_its_export_value() {
        let pair = Object::Array(Array::of([text("export"), text("Label")]));
        let catalog = catalog_with(vec![Object::Dict(dict(&[
            ("FT", name("Ch")),
            ("T", text("pick")),
            ("Opt", Object::Array(Array::of([text("Plain"), pair]))),
        ]))]);
        let form = load(&catalog).expect("form");
        assert_eq!(only_field(&form).options(&NoResolve), ["Plain", "Label"]);
    }

    #[test]
    fn the_states_of_a_toggle_come_from_its_widgets_appearances() {
        let normal = dict(&[("Off", Object::Null), ("Yes", Object::Null)]);
        let ap = dict(&[("N", Object::Dict(normal))]);
        let widget = dict(&[
            ("FT", name("Btn")),
            ("T", text("t")),
            ("Subtype", name("Widget")),
            ("AP", Object::Dict(ap)),
        ]);
        let catalog = catalog_with(vec![Object::Dict(widget)]);
        let form = load(&catalog).expect("form");
        assert_eq!(only_field(&form).states(&NoResolve), ["Off", "Yes"]);
    }

    #[test]
    fn a_field_with_no_reference_cannot_be_written_back() {
        // The field is written inline in `/Fields`, so nothing names it.
        let catalog = catalog_with(vec![Object::Dict(dict(&[
            ("FT", name("Tx")),
            ("T", text("inline")),
        ]))]);
        let form = load(&catalog).expect("form");
        assert_eq!(only_field(&form).reference, None);
        let mut values = FieldValues::new();
        values.set("inline", "x");
        let mut diags = Diagnostics::default();
        assert!(
            apply(&form, &values, &NoResolve, &mut diags).is_empty(),
            "an unnamed field produces no replacement"
        );
    }

    #[test]
    fn rewriting_a_key_keeps_the_dictionary_order() {
        let source = dict(&[
            ("A", Object::Int(1)),
            ("V", text("old")),
            ("B", Object::Int(2)),
        ]);
        let out = rewrite(&source, &Name::from("V"), text("new"));
        let keys: Vec<&[u8]> = out.keys().map(pdfrum_object::Name::as_bytes).collect();
        assert_eq!(keys, [b"A".as_slice(), b"V".as_slice(), b"B".as_slice()]);
        assert_eq!(
            out.text(&Name::from("V"), &NoResolve).as_deref(),
            Some("new")
        );
    }

    #[test]
    fn rewriting_an_absent_key_appends_it() {
        let out = rewrite(&Dict::new(), &Name::from("V"), text("v"));
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn two_fields_entries_sharing_a_name_are_one_field_with_two_widgets() {
        // `AddTerminalField` looks the fully-qualified name up before it
        // builds anything, so the second entry becomes another *control* of
        // the first's field rather than a field of its own. `bug_733528` is
        // that shape, and its golden has the second widget drawing the
        // first's value.
        let widget = |value: &str| {
            Object::Dict(dict(&[
                ("Type", name("Annot")),
                ("Subtype", name("Widget")),
                ("FT", name("Tx")),
                ("T", text("SharedField")),
                ("V", text(value)),
            ]))
        };
        let catalog = dict(&[(
            "AcroForm",
            Object::Dict(dict(&[(
                "Fields",
                Object::Array(Array::of([widget("Hello, world"), widget("")])),
            )])),
        )]);
        let form = load(&catalog).expect("a form");
        let field = only_field(&form);
        assert_eq!(field.name, "SharedField");
        assert_eq!(field.widgets.len(), 2);
        // The field's value is the **first** entry's, which is what both
        // controls show.
        assert_eq!(field.value(None, &NoResolve), "Hello, world");
    }

    #[test]
    fn two_fields_entries_with_different_names_stay_two_fields() {
        let widget = |field_name: &str| {
            Object::Dict(dict(&[
                ("Type", name("Annot")),
                ("Subtype", name("Widget")),
                ("FT", name("Tx")),
                ("T", text(field_name)),
            ]))
        };
        let catalog = dict(&[(
            "AcroForm",
            Object::Dict(dict(&[(
                "Fields",
                Object::Array(Array::of([widget("one"), widget("two")])),
            )])),
        )]);
        assert_eq!(load(&catalog).expect("a form").len(), 2);
    }

    #[test]
    fn the_need_appearances_flag_is_read_off_the_acroform() {
        let acro = dict(&[("NeedAppearances", Object::Bool(true))]);
        let catalog = dict(&[("AcroForm", Object::Dict(acro))]);
        assert!(load(&catalog).expect("form").need_appearances);
        // Absent reads as false.
        let catalog = dict(&[("AcroForm", Object::Dict(Dict::new()))]);
        assert!(!load(&catalog).expect("form").need_appearances);
    }

    #[test]
    fn the_flag_word_accessors_read_the_documented_bits() {
        assert!(FieldFlags(1).is_read_only());
        assert!(FieldFlags(2).is_required());
        assert!(FieldFlags(1 << 12).is_multiline());
        assert!(FieldFlags(1 << 13).is_password());
        assert!(FieldFlags(1 << 18).is_editable_combo());
        assert!(FieldFlags(1 << 21).is_multi_select());
        assert!(FieldFlags(1 << 23).do_not_scroll());
        assert!(!FieldFlags(0).is_read_only());
        assert!(!FieldFlags(0).is_editable_combo());
        assert!(!FieldFlags(0).is_multi_select());
        assert!(!FieldFlags(0).do_not_scroll());
        // Neighbouring bits must not alias: combo (bit 18) is not editable
        // combo (bit 19), and do-not-spell-check (bit 23) is not do-not-scroll
        // (bit 24).
        assert!(!FieldFlags(1 << 17).is_editable_combo());
        assert!(!FieldFlags(1 << 22).do_not_scroll());
    }
}
