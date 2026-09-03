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
/// Kept as the raw word rather than a set, and **deliberately without the
/// `contains` / `union` algebra its two sibling flag types have**: the meaning
/// of a bit depends on the field type, so the same bit 26 is "file select" on
/// a text field and "sort" on a choice field, and `FieldFlags::COMBO |
/// FieldFlags::MULTILINE` would be a lie. The predicates below — the ones
/// whose reading is type-independent or whose type is implied by the name —
/// are the API. [`FieldFlags::bits`] and [`FieldFlags::from_bits`] exist for
/// round-tripping the word itself, unknown bits included.
///
/// ```
/// use pdfrum_doc::form::FieldFlags;
///
/// // Bit 1 is `ReadOnly` on every field type.
/// assert!(FieldFlags::from_bits(1).is_read_only());
/// // A reserved bit survives the trip.
/// assert_eq!(FieldFlags::from_bits(1 << 40).bits(), 1 << 40);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct FieldFlags(i64);

impl FieldFlags {
    /// The raw `/Ff` word, including every bit no predicate here reads.
    #[must_use]
    pub const fn bits(self) -> i64 {
        self.0
    }

    /// The word as written in the file. **Unknown bits are retained**: a bit
    /// whose meaning belongs to a `/FT` this type knows nothing about is
    /// kept, not dropped.
    #[must_use]
    pub const fn from_bits(bits: i64) -> Self {
        Self(bits)
    }

    /// Bit 1: the field may not be changed.
    #[must_use]
    pub const fn is_read_only(self) -> bool {
        self.0 & (1 << 0) != 0
    }

    /// Bit 2: the field must have a value when the form is submitted.
    #[must_use]
    pub const fn is_required(self) -> bool {
        self.0 & (1 << 1) != 0
    }

    /// Bit 16, on a `/Btn`: the field is a radio button rather than a check
    /// box.
    #[must_use]
    pub const fn is_radio(self) -> bool {
        self.0 & (1 << 15) != 0
    }

    /// Bit 17, on a `/Btn`: the field is a push button and holds no value.
    #[must_use]
    pub const fn is_push_button(self) -> bool {
        self.0 & (1 << 16) != 0
    }

    /// Bit 18, on a `/Ch`: the field is a drop-down rather than a list box.
    #[must_use]
    pub const fn is_combo(self) -> bool {
        self.0 & (1 << 17) != 0
    }

    /// Bit 19, on a `/Ch`: the combo box includes an editable text box.
    #[must_use]
    pub const fn is_editable_combo(self) -> bool {
        self.0 & (1 << 18) != 0
    }

    /// Bit 22, on a `/Ch`: more than one option may be selected at once.
    #[must_use]
    pub const fn is_multi_select(self) -> bool {
        self.0 & (1 << 21) != 0
    }

    /// Bit 13, on a `/Tx`: the field accepts more than one line.
    #[must_use]
    pub const fn is_multiline(self) -> bool {
        self.0 & (1 << 12) != 0
    }

    /// Bit 14, on a `/Tx`: the field's contents are obscured as they are
    /// typed.
    #[must_use]
    pub const fn is_password(self) -> bool {
        self.0 & (1 << 13) != 0
    }

    /// Bit 25, on a `/Tx`: the text is laid out in equally spaced cells.
    #[must_use]
    #[doc(alias = "Comb")]
    pub const fn is_comb(self) -> bool {
        self.0 & (1 << 24) != 0
    }

    /// Bit 24, on a `/Tx`: whether the field scrolls to fit more text than
    /// its rectangle holds.
    ///
    /// The positive reading of the spec's `DoNotScroll` bit: a field scrolls
    /// *unless* the bit is set.
    #[must_use]
    #[doc(alias = "DoNotScroll")]
    pub const fn scrolls(self) -> bool {
        self.0 & (1 << 23) == 0
    }

    /// Bit 23, on a `/Tx` or `/Ch`: whether the value is spell-checked.
    ///
    /// The positive reading of the spec's `DoNotSpellCheck` bit.
    #[must_use]
    #[doc(alias = "DoNotSpellCheck")]
    pub const fn spell_checks(self) -> bool {
        self.0 & (1 << 22) == 0
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

    /// The order a recalculation visits fields in, read from `/AcroForm /CO`.
    ///
    /// Indices into [`Form::fields`], in the order the array lists them.
    ///
    /// # An absent `/CO` is the answer, not a fallback
    ///
    /// A document with no `/CO` array recalculates **nothing**, however many
    /// of its fields carry an `/AA /C` script. `CountFieldsInCalculationOrder`
    /// returns 0 and `GetFieldInCalculationOrder` returns null the moment
    /// `GetArrayFor("CO")` finds nothing
    /// (`core/fpdfdoc/cpdf_interactiveform.cpp:739-761`), and the sweep that
    /// drives calculation walks exactly that list. So an empty answer here is
    /// "no calculation runs", and a reader tempted to fall back to "every
    /// field, in `/Fields` order" would recalculate documents the oracle
    /// leaves alone — visibly, on any file with a calculation script and no
    /// `/CO`.
    ///
    /// Entries that resolve to nothing, to a non-dictionary, or to a
    /// dictionary that is not one of this form's terminal fields are dropped,
    /// which is `GetFieldByDict` answering null. Duplicates are kept: the
    /// array is the order, and the oracle indexes it positionally.
    #[must_use]
    pub fn calculation_order<R: Resolve>(&self, catalog: &Dict, r: &R) -> Vec<usize> {
        let Some(acro) = catalog.dict(names::ACRO_FORM, r) else {
            return Vec::new();
        };
        let Some(order) = acro.array(names::CALCULATION_ORDER, r) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for index in 0..order.len() {
            // The reference identifies the field where there is one, which is
            // the ordinary shape — `/CO` holds indirect references to the same
            // field dictionaries `/Fields` does. A directly-written entry is
            // matched on the dictionary itself, which is what `GetFieldByDict`
            // compares.
            let reference = order.reference_at(index);
            let dict = order.dict_at(index, r);
            let found = self
                .fields
                .iter()
                .position(|field| match (reference, &dict) {
                    (Some(reference), _) if field.reference == Some(reference) => true,
                    (_, Some(dict)) => field.reference.is_none() && &field.dict == dict,
                    _ => false,
                });
            if let Some(found) = found {
                out.push(found);
            }
        }
        out
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
    let flags = FieldFlags::from_bits(
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
        // A field's fully-qualified name is its **identity**, not a label:
        // the merge below keys on it, `Form::field` is the only public lookup,
        // and `pdfrum-form` allocates one `FieldId` per distinct name. So an
        // empty name is not merely an unaddressable field — it is a field that
        // every *other* unnamed field in the document would be merged into.
        // ISO 32000-1 §12.7.3.2 makes the fully qualified name the thing an
        // action, an export or a JavaScript reference names a field by, and a
        // node with no `/T` anywhere in its ancestry has none, so there is
        // nothing a caller could do with the entry. Upstream drops it too
        // (`cpdf_interactiveform.cpp:914-917`, `AddTerminalField`).
        if name.is_empty() {
            diags.record(
                pdfrum_common::Severity::Suspicious,
                pdfrum_common::DiagKind::FieldSkippedNoName,
                None,
            );
            return;
        }
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
        // No `/FT` on this dictionary or its `/Parent`, and no `/Kids` to
        // inherit one down: upstream's `AddTerminalField` returns here
        // (`cpdf_interactiveform.cpp:905-912`, "Key \"FT\" is required for
        // terminal fields") and the dictionary contributes no field at all.
        if field_type.is_empty() {
            diags.record(
                pdfrum_common::Severity::Suspicious,
                pdfrum_common::DiagKind::FieldSkippedNoType,
                None,
            );
        }
        return;
    };
    for index in 0..kids.len() {
        let kid_ref = kids.reference_at(index);
        // [oracle-bug] A `/Kids` entry that is not a dictionary costs *that
        // entry* and nothing else. `CPDF_InteractiveForm::LoadField` reads
        // `kids->GetDictAt(0)` and returns outright when it is null
        // (`cpdf_interactiveform.cpp:871-874`), so one unresolvable first kid
        // silently discards every sibling under the node — a whole page of
        // fields lost to one broken reference. Nothing recovers them: the
        // walk has already returned, and `FixPageFields` only re-enters
        // through `/Annots`.
        //
        // That `GetDictAt(0)` is a **probe**, not a guard: the two lines after
        // it (`:876-880`) ask whether the first kid has `/T` or `/Kids` to
        // decide whether this node is the terminal field or a branch. The
        // early return is what happens when the probe cannot be taken, and it
        // throws away the siblings as a side effect rather than as a
        // decision — a non-dict first kid says nothing about whether the
        // *array* is a field tree. Our own probe (`kids_are_fields` above)
        // scans every kid rather than only the first, so a null at index 0
        // does not blind it and there is nothing to recover from.
        //
        // pdf.js is the tiebreaker and skips the entry: `#collectFieldObjects`
        // (`src/core/document.js`) recurses per kid and its
        // `if (!(fieldRef instanceof Ref) || visitedRefs.has(fieldRef))`
        // guard returns from *that* kid alone, leaving the loop to continue
        // with the siblings. ISO 32000-1 §12.7.3.1 says `/Kids` holds the
        // field's children and gives no rule making the array's validity
        // depend on its first element.
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

/// Which rows of a choice field an **interaction** treats as selected.
///
/// # Why this is not the appearance's answer
///
/// A choice field records its selection twice — `/I` as indices, `/V` as the
/// selected options' export values — and upstream reads the pair *differently
/// depending on who is asking*. The two readers are not reconcilable and
/// pretending they are is how a corpus row moves in the wrong direction:
///
/// - **Interaction** — "is row `n` selected?", the question a click, an arrow
///   key or an embedder's query asks — is `CPDF_FormField::IsItemSelected`
///   (`core/fpdfdoc/cpdf_formfield.cpp:546-554`). It consults **`/I` first**,
///   as integer indices, and falls back to `/V` only when `/I` is not usable.
///   That is this function.
/// - **Appearance** — what the generated `/AP` draws a band behind — is
///   `CPDFSDK_AppStream::SetAsListBox` by way of
///   `CPDF_FormField::GetSelectedIndex` (`:GetSelectedIndex`), which reads
///   `GetValueOrSelectedIndicesObject` — **`/V` first**, `/I` only when there
///   is no `/V` — and then matches each entry's *text* against the option
///   values, so an integer index matches nothing. That is
///   `ap::field_body::selected_indices`, and it is deliberately the
///   other way round.
///
/// So `listbox_form.pdf`'s `Listbox_MultiSelectMultipleIndices` — `/I [1 3]`
/// and no `/V` — draws **no** selection band while an embedder asking about
/// its rows is told 1 and 3 are selected. Both are correct; they are answers
/// to different questions.
///
/// # What "usable" means
///
/// `indices_are_usable` is the test, from `UseSelectedIndicesObject`
/// (`cpdf_formfield.cpp:863-950`), and it is strict because its job is to
/// catch a stale `/I` left behind by an editor that rewrote `/V`. `/I` is
/// usable when either
///
/// - there is **no `/V` at all** — nothing can contradict it; or
/// - `/I` and `/V` **agree exactly**: the same number of entries, every index
///   in range, and the multiset of options those indices name equal to the
///   multiset `/V` lists. A duplicate on one side must be matched by a
///   duplicate on the other, which is why occurrences are counted rather than
///   membership tested.
///
/// One disagreement anywhere discards `/I` entirely — it is not repaired
/// entry by entry — and `/V` then decides alone, matched as text exactly as
/// the appearance reader does.
///
/// `options` is the field's `/Opt` in order, as the **values** a selection is
/// compared against: an `[export, label]` pair contributes its export, never
/// its label.
#[must_use]
pub fn selected_indices_for_interaction<R: Resolve>(
    dict: &Dict,
    options: &[String],
    r: &R,
) -> Vec<usize> {
    // `/V` and `/I` are inheritable field attributes, and a damaged
    // inheritance chain is not this function's to report on: it answers with
    // what it could reach.
    let (limits, mut diags) = (Limits::default(), Diagnostics::default());
    let value = field_attr(dict, names::V, r, &limits, &mut diags);
    let indices = field_attr(dict, names::I, r, &limits, &mut diags);
    if let Some(indices) = indices.as_ref()
        && indices_are_usable(indices, value.as_ref(), options, r)
    {
        return listed_indices(indices, r)
            .into_iter()
            .filter_map(|index| usize::try_from(index).ok())
            .filter(|index| *index < options.len())
            .collect();
    }
    let Some(value) = value else {
        return Vec::new();
    };
    let wanted: Vec<String> = match value.as_array() {
        Some(array) => (0..array.len())
            .map(|slot| {
                array
                    .get(slot, r)
                    .as_deref()
                    .map(Object::to_text)
                    .unwrap_or_default()
            })
            .collect(),
        None => vec![value.to_text()],
    };
    wanted
        .into_iter()
        .filter_map(|text| options.iter().position(|option| *option == text))
        .collect()
}

/// `/I`'s entries as raw integers, or nothing when any entry is not a number.
///
/// A bare number stands for a one-entry array, which is the shape
/// `UseSelectedIndicesObject` admits alongside the array.
fn listed_indices<R: Resolve>(indices: &Object, r: &R) -> Vec<i64> {
    match indices.as_array() {
        Some(array) => (0..array.len())
            .map(|slot| array.get(slot, r).as_deref().and_then(Object::as_int))
            .collect::<Option<Vec<i64>>>()
            .unwrap_or_default(),
        None => indices.as_int().into_iter().collect(),
    }
}

/// Whether `/I` may be believed in preference to `/V`.
///
/// See [`selected_indices_for_interaction`] for the rule and why it is all or
/// nothing.
fn indices_are_usable<R: Resolve>(
    indices: &Object,
    value: Option<&Object>,
    options: &[String],
    r: &R,
) -> bool {
    // No `/V` to contradict it.
    let Some(value) = value else {
        return true;
    };
    // A non-number entry anywhere fails outright: `/I` is trusted whole or
    // not at all, and an empty answer here would be indistinguishable from a
    // genuinely empty `/I`.
    let listed = listed_indices(indices, r);
    let declared = match indices.as_array() {
        Some(array) => array.len(),
        None => usize::from(indices.as_int().is_some()),
    };
    if listed.len() != declared || declared == 0 {
        return false;
    }

    // `/V`'s texts, as counts, so a repeated value needs a repeated index.
    let mut wanted: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    if let Some(array) = value.as_array() {
        if array.len() != listed.len() {
            return false;
        }
        for slot in 0..array.len() {
            // Only strings are counted — upstream ignores any other type
            // here, which then leaves a count `/I` cannot satisfy.
            if let Some(object) = array.get(slot, r)
                && object.as_string().is_some()
            {
                *wanted.entry(object.to_text()).or_default() += 1;
            }
        }
    } else {
        // A lone string is the one-selection spelling, so it can only ever
        // account for one index.
        if listed.len() != 1 {
            return false;
        }
        if value.as_string().is_some() {
            *wanted.entry(value.to_text()).or_default() += 1;
        }
    }

    for index in listed {
        let Ok(index) = usize::try_from(index) else {
            return false;
        };
        let Some(option) = options.get(index) else {
            return false;
        };
        let Some(count) = wanted.get_mut(option) else {
            return false;
        };
        *count -= 1;
        if *count == 0 {
            wanted.remove(option);
        }
    }
    wanted.is_empty()
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

    /// The four `listbox_form.pdf` shapes, as the interaction reader sees
    /// them. Contrast `ap::field_body::selected_indices`, which answers the
    /// appearance's question and disagrees on the first of these on purpose.
    fn opts() -> Vec<String> {
        ["Albania", "Belgium", "Croatia", "Denmark", "Estonia"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect()
    }

    fn selected(pairs: &[(&str, Object)]) -> Vec<usize> {
        selected_indices_for_interaction(&dict(pairs), &opts(), &NoResolve)
    }

    fn strings(values: &[&str]) -> Object {
        Object::Array(Array::of(
            values.iter().map(|v| text(v)).collect::<Vec<_>>(),
        ))
    }

    #[test]
    fn indices_alone_are_believed_because_nothing_contradicts_them() {
        // `Listbox_MultiSelectMultipleIndices`: `/I [1 3]`, no `/V`.
        assert_eq!(
            selected(&[(
                "I",
                Object::Array(Array::of([Object::Int(1), Object::Int(3)]))
            )]),
            vec![1, 3]
        );
        // A bare number is the one-entry spelling.
        assert_eq!(selected(&[("I", Object::Int(2))]), vec![2]);
    }

    #[test]
    fn a_value_alone_selects_every_option_it_names() {
        // `Listbox_MultiSelectMultipleValues`, restated over these options.
        assert_eq!(
            selected(&[("V", strings(&["Belgium", "Denmark"]))]),
            vec![1, 3]
        );
        // And a lone string is the single-selection spelling.
        assert_eq!(selected(&[("V", text("Croatia"))]), vec![2]);
        // A value naming no option selects nothing rather than guessing.
        assert_eq!(selected(&[("V", text("Zambia"))]), Vec::<usize>::new());
    }

    #[test]
    fn consistent_indices_win_over_the_values_they_agree_with() {
        // Same count, in range, naming exactly what `/V` lists.
        assert_eq!(
            selected(&[
                ("V", strings(&["Belgium", "Denmark"])),
                (
                    "I",
                    Object::Array(Array::of([Object::Int(1), Object::Int(3)]))
                ),
            ]),
            vec![1, 3]
        );
        // Occurrences are counted, not sequences compared, so the two may be
        // listed in different orders — and `/I`'s order is what comes back.
        assert_eq!(
            selected(&[
                ("V", strings(&["Denmark", "Belgium"])),
                (
                    "I",
                    Object::Array(Array::of([Object::Int(3), Object::Int(1)]))
                ),
            ]),
            vec![3, 1]
        );
    }

    #[test]
    fn inconsistent_indices_are_discarded_whole_and_the_values_decide() {
        // `Listbox_MultiSelectMultipleMismatch`'s shape: three indices
        // against two values, so the counts differ and `/I` is rejected
        // before any index is looked up.
        assert_eq!(
            selected(&[
                ("V", strings(&["Albania", "Croatia"])),
                (
                    "I",
                    Object::Array(Array::of([Object::Int(1), Object::Int(3), Object::Int(4),])),
                ),
            ]),
            vec![0, 2]
        );
        // Equal counts, but an index naming an option `/V` does not list.
        assert_eq!(
            selected(&[
                ("V", strings(&["Albania"])),
                ("I", Object::Array(Array::of([Object::Int(1)]))),
            ]),
            vec![0]
        );
        // An index out of range poisons the whole array rather than being
        // dropped on its own.
        assert_eq!(
            selected(&[
                ("V", strings(&["Albania", "Croatia"])),
                (
                    "I",
                    Object::Array(Array::of([Object::Int(0), Object::Int(9)]))
                ),
            ]),
            vec![0, 2]
        );
        // Two indices naming one option cannot satisfy two distinct values.
        assert_eq!(
            selected(&[
                ("V", strings(&["Albania", "Belgium"])),
                (
                    "I",
                    Object::Array(Array::of([Object::Int(0), Object::Int(0)]))
                ),
            ]),
            vec![0, 1]
        );
        // A non-number entry fails the whole array too.
        assert_eq!(
            selected(&[
                ("V", strings(&["Albania"])),
                ("I", Object::Array(Array::of([text("0")]))),
            ]),
            vec![0]
        );
    }

    #[test]
    fn a_field_declaring_neither_selects_nothing() {
        assert_eq!(selected(&[]), Vec::<usize>::new());
    }

    fn load(catalog: &Dict) -> Option<Form> {
        load_with_diags(catalog).0
    }

    /// `load`, plus the diagnostics the walk recorded.
    fn load_with_diags(catalog: &Dict) -> (Option<Form>, Diagnostics) {
        let (limits, mut diags) = (Limits::default(), Diagnostics::default());
        let form = Form::load(catalog, &NoResolve, &limits, &mut diags);
        (form, diags)
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
            FieldKind::classify(b"Btn", FieldFlags::from_bits(0)),
            Some(FieldKind::Check)
        );
        // Bit 16: a radio button.
        assert_eq!(
            FieldKind::classify(b"Btn", FieldFlags::from_bits(1 << 15)),
            Some(FieldKind::Radio)
        );
        // Bit 17 wins over bit 16: a push button.
        assert_eq!(
            FieldKind::classify(b"Btn", FieldFlags::from_bits((1 << 16) | (1 << 15))),
            Some(FieldKind::Button)
        );
    }

    #[test]
    fn a_choice_field_splits_on_the_combo_bit() {
        assert_eq!(
            FieldKind::classify(b"Ch", FieldFlags::from_bits(0)),
            Some(FieldKind::List)
        );
        assert_eq!(
            FieldKind::classify(b"Ch", FieldFlags::from_bits(1 << 17)),
            Some(FieldKind::Combo)
        );
    }

    #[test]
    fn a_node_with_no_field_type_is_not_a_field() {
        assert_eq!(FieldKind::classify(b"", FieldFlags::from_bits(0)), None);
        assert_eq!(
            FieldKind::classify(b"Nonsense", FieldFlags::from_bits(0)),
            None
        );
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
    fn a_field_with_no_name_anywhere_in_its_ancestry_is_dropped() {
        // ISO 32000-1 §12.7.3.2: a field is addressed by its fully qualified
        // name, and this one has none — no `/T` on itself and no `/Parent`
        // carrying one — so no action, export or script could ever name it.
        // `AddTerminalField` drops it (`cpdf_interactiveform.cpp:914-917`).
        let catalog = catalog_with(vec![Object::Dict(dict(&[
            ("FT", name("Tx")),
            ("V", text("unreachable")),
        ]))]);
        let (form, diags) = load_with_diags(&catalog);
        let form = form.expect("an AcroForm is still a form");
        assert!(form.is_empty(), "an unnamed terminal field is not a field");
        assert!(diags.contains(&pdfrum_common::DiagKind::FieldSkippedNoName));
    }

    #[test]
    fn unnamed_fields_do_not_collapse_into_one() {
        // The reason the drop is the *correct* answer and not merely the
        // oracle's: `name` is the identity the merge below keys on, so
        // keeping the empty name would fold every unnamed field in the
        // document into a single field carrying all their widgets — a field
        // that is not in the file. Two unnamed entries plus a real one must
        // leave exactly the real one.
        let unnamed = || Object::Dict(dict(&[("FT", name("Tx")), ("V", text("a"))]));
        let catalog = catalog_with(vec![
            unnamed(),
            unnamed(),
            Object::Dict(dict(&[
                ("FT", name("Tx")),
                ("T", text("real")),
                ("V", text("b")),
            ])),
        ]);
        let form = load(&catalog).expect("a form");
        assert_eq!(only_field(&form).name, "real");
    }

    #[test]
    fn a_field_named_only_by_an_ancestor_survives() {
        // The drop is about the *fully qualified* name, not about `/T` on the
        // node itself: a kid with no `/T` inherits its parent's name and is
        // addressable as it, so it must be kept.
        let kid = Object::Dict(dict(&[
            ("Subtype", name("Widget")),
            (
                "Parent",
                Object::Dict(dict(&[("FT", name("Tx")), ("T", text("parent"))])),
            ),
        ]));
        let catalog = catalog_with(vec![Object::Dict(dict(&[
            ("FT", name("Tx")),
            ("T", text("parent")),
            ("Kids", Object::Array(Array::of([kid]))),
        ]))]);
        let form = load(&catalog).expect("a form");
        assert_eq!(only_field(&form).name, "parent");
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
        let f = FieldFlags::from_bits;
        assert!(f(1).is_read_only());
        assert!(f(2).is_required());
        assert!(f(1 << 12).is_multiline());
        assert!(f(1 << 13).is_password());
        assert!(f(1 << 18).is_editable_combo());
        assert!(f(1 << 21).is_multi_select());
        assert!(f(1 << 24).is_comb());
        assert!(!f(0).is_read_only());
        assert!(!f(0).is_editable_combo());
        assert!(!f(0).is_multi_select());
        assert!(!f(0).is_comb());
        // Neighbouring bits must not alias: combo (bit 18) is not editable
        // combo (bit 19), and do-not-spell-check (bit 23) is not do-not-scroll
        // (bit 24).
        assert!(!f(1 << 17).is_editable_combo());
    }

    /// The two negative spec bits, read positively. `DoNotScroll` and
    /// `DoNotSpellCheck` are adjacent, so the alias test is also an
    /// anti-aliasing test.
    #[test]
    fn the_negative_spec_bits_read_positively() {
        let f = FieldFlags::from_bits;
        assert!(f(0).scrolls());
        assert!(!f(1 << 23).scrolls());
        assert!(f(1 << 22).scrolls());
        assert!(f(0).spell_checks());
        assert!(!f(1 << 22).spell_checks());
        assert!(f(1 << 23).spell_checks());
    }

    /// `/Ff` has no set algebra by design (§6): the round trip is the whole
    /// contract, and a bit belonging to a `/FT` nothing here reads survives.
    #[test]
    fn the_flag_word_round_trips_unknown_bits() {
        let raw = (1 << 40) | (1 << 25) | 1;
        let f = FieldFlags::from_bits(raw);
        assert_eq!(f.bits(), raw);
        assert!(f.is_read_only());
        assert_eq!(FieldFlags::default().bits(), 0);
    }

    // ---- `/CO`, the calculation order ----

    /// A map-backed resolver, because `/CO` is a list of *references* and
    /// `NoResolve` cannot follow one.
    struct Store(std::collections::HashMap<u32, std::sync::Arc<Object>>);

    impl Store {
        fn of(pairs: impl IntoIterator<Item = (u32, Object)>) -> Store {
            Store(
                pairs
                    .into_iter()
                    .map(|(num, obj)| (num, std::sync::Arc::new(obj)))
                    .collect(),
            )
        }
    }

    impl Resolve for Store {
        fn fetch(&self, r: ObjRef) -> Result<std::sync::Arc<Object>, pdfrum_object::Error> {
            self.0
                .get(&r.num)
                .map(std::sync::Arc::clone)
                .ok_or(pdfrum_object::Error::UnresolvedRef(r))
        }
    }

    fn reference(num: u32) -> Object {
        Object::Ref(ObjRef { num, generation: 0 })
    }

    #[test]
    fn a_junk_first_kid_costs_that_kid_and_not_its_siblings() {
        // [oracle-bug] `LoadField` returns when `kids->GetDictAt(0)` is null
        // (`cpdf_interactiveform.cpp:871-874`), losing `real` along with the
        // broken entry. pdf.js skips the entry and keeps walking
        // (`#collectFieldObjects`, `src/core/document.js`), and so do we —
        // one unresolvable reference must not cost a page of fields.
        //
        // Object 9 is not in the store, so `/Kids[0]` resolves to nothing;
        // object 2 is a real text field.
        let store = Store::of([(
            2,
            Object::Dict(dict(&[
                ("FT", name("Tx")),
                ("T", text("real")),
                ("V", text("kept")),
            ])),
        )]);
        let catalog = catalog_with(vec![Object::Dict(dict(&[(
            "Kids",
            Object::Array(Array::of([reference(9), reference(2)])),
        )]))]);

        let (limits, mut diags) = (Limits::default(), Diagnostics::default());
        let form = Form::load(&catalog, &store, &limits, &mut diags).expect("a form");

        let field = only_field(&form);
        assert_eq!(field.name, "real");
        assert_eq!(field.stored_value(&store), "kept");
    }

    #[test]
    fn a_junk_first_kid_does_not_blind_the_terminal_probe() {
        // The oracle's `GetDictAt(0)` is a probe as well as a guard: the
        // lines after it (`:876-880`) ask the *first* kid whether it carries
        // `/T` or `/Kids` to decide branch-versus-terminal. Ours scans every
        // kid instead (`kids_are_fields`), so a null at index 0 cannot make a
        // branch node look terminal. Here `/Kids[1]` is a named field, so the
        // parent is naming structure and the kid is the field — even though
        // `/Kids[0]` says nothing.
        // The kid carries a `/Parent` back to the branch, which is how a
        // real file writes it: that is what its `/FT` and the first half of
        // its qualified name are inherited through.
        let parent = dict(&[("FT", name("Tx")), ("T", text("parent"))]);
        let store = Store::of([(
            2,
            Object::Dict(dict(&[
                ("T", text("kid")),
                ("V", text("v")),
                ("Parent", Object::Dict(parent.clone())),
            ])),
        )]);
        let catalog = catalog_with(vec![Object::Dict(dict(&[
            ("FT", name("Tx")),
            ("T", text("parent")),
            (
                "Kids",
                Object::Array(Array::of([reference(9), reference(2)])),
            ),
        ]))]);

        let (limits, mut diags) = (Limits::default(), Diagnostics::default());
        let form = Form::load(&catalog, &store, &limits, &mut diags).expect("a form");

        // The parent is a branch, so the field is the kid, qualified by it.
        assert_eq!(only_field(&form).name, "parent.kid");
    }

    /// Three text fields as objects 1, 2 and 3, and the catalog that lists
    /// them — `co` becomes the `/CO` array when it is `Some`.
    fn three_fields(co: Option<Object>) -> (Dict, Store) {
        let field_of = |title: &str| {
            Object::Dict(dict(&[
                ("FT", name("Tx")),
                ("T", text(title)),
                ("V", text("")),
            ]))
        };
        let store = Store::of([(1, field_of("a")), (2, field_of("b")), (3, field_of("c"))]);
        let mut acro = vec![(
            "Fields",
            Object::Array(Array::of([reference(1), reference(2), reference(3)])),
        )];
        if let Some(co) = co {
            acro.push(("CO", co));
        }
        let catalog = dict(&[("AcroForm", Object::Dict(dict(&acro)))]);
        (catalog, store)
    }

    fn order_of(co: Option<Object>) -> Vec<usize> {
        let (catalog, store) = three_fields(co);
        let (limits, mut diags) = (Limits::default(), Diagnostics::default());
        let form = Form::load(&catalog, &store, &limits, &mut diags).expect("form");
        assert_eq!(form.len(), 3, "the fixture has three fields");
        form.calculation_order(&catalog, &store)
    }

    /// The rule the whole feature turns on: no `/CO`, no calculation. Falling
    /// back to "every field" here would recalculate documents the oracle
    /// leaves alone (`cpdf_interactiveform.cpp:739-745`).
    #[test]
    fn a_document_with_no_calculation_order_calculates_nothing() {
        assert_eq!(order_of(None), Vec::<usize>::new());
        // An empty array is the same answer arrived at the other way.
        assert_eq!(order_of(Some(Object::Array(Array::of([])))), Vec::new());
    }

    /// The array's order is the answer, and it need not be `/Fields`' order.
    #[test]
    fn the_array_is_the_order() {
        assert_eq!(
            order_of(Some(Object::Array(Array::of([
                reference(3),
                reference(1),
                reference(2),
            ])))),
            vec![2, 0, 1]
        );
        // A subset is legal: only the fields listed are calculated.
        assert_eq!(
            order_of(Some(Object::Array(Array::of([reference(2)])))),
            vec![1]
        );
    }

    /// `GetFieldByDict` answers null for anything it cannot map, and the
    /// sweep skips it rather than stopping.
    #[test]
    fn entries_that_resolve_to_nothing_are_dropped() {
        assert_eq!(
            order_of(Some(Object::Array(Array::of([
                reference(9),   // no such object
                Object::Int(7), // not a dictionary at all
                reference(2),
            ])))),
            vec![1]
        );
        // A `/CO` that is not an array is not an order.
        assert_eq!(order_of(Some(Object::Int(1))), Vec::<usize>::new());
    }

    /// The oracle indexes the array positionally, so a repeated field is
    /// calculated twice rather than de-duplicated.
    #[test]
    fn duplicates_are_kept_because_the_array_is_indexed_positionally() {
        assert_eq!(
            order_of(Some(Object::Array(Array::of(
                [reference(1), reference(1),]
            )))),
            vec![0, 0]
        );
    }
}
