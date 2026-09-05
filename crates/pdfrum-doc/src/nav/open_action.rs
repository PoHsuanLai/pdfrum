//! The document-open action, and the one thing it changes about a page.
//!
//! # Why a reader executes anything at all
//!
//! A document's open action runs before its first page is rendered, so a
//! catalog carrying an `/OpenAction` has already acted on the reader by the
//! time any pixel or any annotation dump is produced. Of the eighteen action
//! types only one is observable in a rendered page or a dump without a user,
//! a script engine or a viewer chrome: **`/Hide`**, which sets flags on the
//! widget annotations that draw a named field.
//!
//! Everything else either needs input we never supply (a mouse, a keypress),
//! reaches a subsystem we deliberately do not have (JavaScript), or changes
//! only the view (`/GoTo` scrolls; the render is of a page, not of a
//! viewport). So this module executes exactly `/Hide` and answers, for one
//! document, which annotation dictionaries came out hidden.
//!
//! # The overlay, again
//!
//! A hide is conventionally a **mutation** — a new `/F` written into each
//! widget's dictionary, which a later reader picks up — so a widget whose
//! file says `/F` is absent still reports `Hidden` once the action has run.
//! Objects here are values, so [`hidden_by_open_action`] returns the *set* of
//! affected dictionaries instead and [`Hidden::flags`] applies the edit on
//! read.
//!
//! # What the edit is
//!
//! Not "set the hidden bit". `Invisible` and `NoView` are cleared in every
//! case, and only then is `Hidden` set or cleared per the action's `/H`:
//!
//! ```text
//! flags &= !(Invisible | NoView);
//! flags = if hide { flags | Hidden } else { flags & !Hidden };
//! ```
//!
//! `/H` **defaults to true**, so an action that names no `/H` hides. A `/Hide`
//! action naming its fields in `/T` rather than `/Fields` is the same quirk
//! [`Action::fields`](crate::nav::Action::fields) already carries, and only
//! **string** entries name a field — a `/T` holding dictionary references
//! resolves to nothing, which the test file's own comment records as
//! surprising and which is behavior.

use pdfrum_common::{Diagnostics, Limits};
use pdfrum_object::{Dict, Object, Resolve, decode_text};

use crate::annot::AnnotFlags;
use crate::form::Form;
use crate::names;
use crate::nav::{Action, ActionKind};

/// The `Invisible` and `NoView` bits, which a `/Hide` action clears whichever
/// way it is going.
const CLEARED_BY_HIDE: AnnotFlags = AnnotFlags::INVISIBLE.with(AnnotFlags::NO_VIEW);

/// Which of a document's annotations its open action left hidden or shown.
///
/// Empty for the overwhelming majority of documents — a catalog with no
/// `/OpenAction`, or one whose open action is a destination array rather than
/// an action dictionary, produces no entries and costs one dictionary lookup.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Hidden {
    /// The widget dictionaries the action touched, each with the flag word it
    /// left behind. Keyed by dictionary value rather than by reference
    /// because a widget written inline in its field's `/Kids` has no
    /// reference to key on.
    touched: Vec<(Dict, AnnotFlags)>,
}

impl Hidden {
    /// Whether the open action touched no annotation at all.
    ///
    /// ```
    /// use pdfrum_common::{Diagnostics, Limits};
    /// use pdfrum_doc::nav::hidden_by_open_action;
    /// use pdfrum_object::{Dict, NoResolve};
    ///
    /// let (limits, mut diags) = (Limits::default(), Diagnostics::default());
    /// // No `/OpenAction`: nothing is touched, at the cost of one lookup.
    /// let hidden = hidden_by_open_action(&Dict::default(), &NoResolve, &limits, &mut diags);
    /// assert!(hidden.is_empty());
    /// ```
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.touched.is_empty()
    }

    /// One annotation's flag word, as the open action left it.
    ///
    /// The dictionary's own `/F` when the action did not name it, which is
    /// every annotation in a document with no `/OpenAction`.
    ///
    /// ```
    /// use pdfrum_common::{Diagnostics, Limits};
    /// use pdfrum_doc::AnnotFlags;
    /// use pdfrum_doc::nav::hidden_by_open_action;
    /// use pdfrum_object::{Dict, Name, NoResolve, Object};
    ///
    /// let (limits, mut diags) = (Limits::default(), Diagnostics::default());
    /// let hidden = hidden_by_open_action(&Dict::default(), &NoResolve, &limits, &mut diags);
    ///
    /// // Untouched by any action: the dictionary's own `/F`.
    /// let widget = Dict::from_pairs([(Name::from("F"), Object::Int(4))]);
    /// assert_eq!(hidden.flags(&widget, &NoResolve), AnnotFlags::PRINT);
    /// ```
    #[must_use]
    pub fn flags<R: Resolve>(&self, dict: &Dict, r: &R) -> AnnotFlags {
        self.touched
            .iter()
            .find(|(touched, _)| touched == dict)
            .map_or_else(
                || AnnotFlags::from_bits(dict.int(names::F, r).unwrap_or(0)),
                |(_, f)| *f,
            )
    }
}

/// Runs a document's open action far enough to know what it hid.
///
/// The `/Next` chain is walked, because a hide can sit behind a `/GoTo` that
/// we otherwise ignore; every action in the chain that reads as `/Hide`
/// contributes, in order, so a later one showing what an earlier one hid wins.
///
/// ```
/// use pdfrum_common::{Diagnostics, Limits};
/// use pdfrum_doc::nav::hidden_by_open_action;
/// use pdfrum_object::{Dict, NoResolve};
///
/// let (limits, mut diags) = (Limits::default(), Diagnostics::default());
/// let hidden = hidden_by_open_action(&Dict::default(), &NoResolve, &limits, &mut diags);
/// assert!(hidden.is_empty());
/// ```
#[must_use]
pub fn hidden_by_open_action<R: Resolve>(
    catalog: &Dict,
    r: &R,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> Hidden {
    let mut hidden = Hidden::default();
    // A destination *array* is the other legal `/OpenAction`, and it is not an
    // action: `ProcOpenAction` returns having done nothing
    // (`cpdfsdk_formfillenvironment.cpp:704-729`).
    let Some(dict) = catalog.dict(names::OPEN_ACTION, r) else {
        return hidden;
    };
    let root = Action::new(dict);
    // Loading the form is the expensive half, so it waits until an action in
    // the chain actually reads as a hide.
    let mut form: Option<Option<Form>> = None;
    for action in std::iter::once(root.clone()).chain(root.chain(r, limits, diags)) {
        if action.kind() != ActionKind::Hide {
            continue;
        }
        let form = form.get_or_insert_with(|| Form::load(catalog, r, limits, diags));
        let Some(form) = form.as_ref() else {
            continue;
        };
        apply_hide(&action, form, &mut hidden, r);
    }
    hidden
}

/// One `/Hide` action's effect on the widgets its fields draw.
fn apply_hide<R: Resolve>(action: &Action, form: &Form, hidden: &mut Hidden, r: &R) {
    let hide = action.hide_status();
    for named in action.fields(r) {
        // Only a **string** names a field. `GetFieldFromObjects` skips every
        // other type outright, which is why the test file's `/T` may not hold
        // the dictionary references it looks like it could.
        let Object::Str(text) = named else {
            continue;
        };
        let name = decode_text(&text.bytes).into_owned();
        let Some(field) = form.field(&name) else {
            continue;
        };
        for widget in &field.widgets {
            let flags = hidden.flags(&widget.dict, r).without(CLEARED_BY_HIDE);
            let flags = if hide {
                flags.with(AnnotFlags::HIDDEN)
            } else {
                flags.without(AnnotFlags::HIDDEN)
            };
            set(hidden, &widget.dict, flags);
        }
    }
}

/// Records one widget's new flag word, replacing an earlier action's.
fn set(hidden: &mut Hidden, dict: &Dict, flags: AnnotFlags) {
    match hidden
        .touched
        .iter_mut()
        .find(|(touched, _)| touched == dict)
    {
        Some((_, slot)) => *slot = flags,
        None => hidden.touched.push((dict.clone(), flags)),
    }
}

#[cfg(test)]
mod tests {
    use super::{Hidden, hidden_by_open_action};
    use crate::annot::AnnotFlags;
    use pdfrum_common::{Diagnostics, Limits};
    use pdfrum_object::{Array, Dict, Name, NoResolve, Object, PdfString};

    fn dict(pairs: &[(&str, Object)]) -> Dict {
        Dict::from_pairs(
            pairs
                .iter()
                .map(|(k, v)| (Name::from(*k), v.clone()))
                .collect::<Vec<_>>(),
        )
    }

    /// A catalog with one text field named `f`, whose widget is the field
    /// itself, and the given open action.
    fn catalog(widget: Dict, open_action: Option<Object>) -> (Dict, Dict) {
        let mut pairs = vec![(
            "AcroForm",
            Object::Dict(dict(&[(
                "Fields",
                Object::Array(Array::of([Object::Dict(widget.clone())])),
            )])),
        )];
        if let Some(action) = open_action {
            pairs.push(("OpenAction", action));
        }
        (dict(&pairs), widget)
    }

    fn field_widget(flags: i64) -> Dict {
        dict(&[
            ("Type", Object::Name(Name::from("Annot"))),
            ("Subtype", Object::Name(Name::from("Widget"))),
            ("FT", Object::Name(Name::from("Tx"))),
            ("T", Object::Str(PdfString::literal(b"f"))),
            ("F", Object::Int(flags)),
        ])
    }

    fn hide_action(names: &[&str], h: Option<bool>) -> Object {
        let mut pairs = vec![
            ("Type", Object::Name(Name::from("Action"))),
            ("S", Object::Name(Name::from("Hide"))),
            (
                "T",
                Object::Array(Array::of(
                    names
                        .iter()
                        .map(|n| Object::Str(PdfString::literal(n.as_bytes()))),
                )),
            ),
        ];
        if let Some(h) = h {
            pairs.push(("H", Object::Bool(h)));
        }
        Object::Dict(dict(&pairs))
    }

    fn run(catalog: &Dict) -> Hidden {
        let (limits, mut diags) = (Limits::default(), Diagnostics::default());
        hidden_by_open_action(catalog, &NoResolve, &limits, &mut diags)
    }

    #[test]
    fn a_document_with_no_open_action_hides_nothing() {
        let (cat, widget) = catalog(field_widget(4), None);
        let hidden = run(&cat);
        assert!(hidden.is_empty());
        // And an untouched annotation still reads its own `/F`.
        assert_eq!(hidden.flags(&widget, &NoResolve), AnnotFlags::from_bits(4));
    }

    #[test]
    fn a_hide_action_with_no_h_key_hides() {
        // `/H` defaults to **true**, which is what `checkbox_radiobutton_hide`
        // relies on: it names no `/H` at all.
        let (cat, widget) = catalog(field_widget(4), Some(hide_action(&["f"], None)));
        let hidden = run(&cat);
        assert_eq!(
            hidden.flags(&widget, &NoResolve),
            AnnotFlags::from_bits(4 | 2)
        );
    }

    #[test]
    fn showing_clears_the_hidden_bit_and_the_two_others() {
        // `Invisible | NoView | Hidden` going in; `/H false` clears all three,
        // because the two are cleared whichever way the action goes.
        let (cat, widget) = catalog(
            field_widget(1 | 2 | 32),
            Some(hide_action(&["f"], Some(false))),
        );
        let hidden = run(&cat);
        assert_eq!(hidden.flags(&widget, &NoResolve), AnnotFlags::from_bits(0));
    }

    #[test]
    fn hiding_also_clears_invisible_and_no_view() {
        let (cat, widget) = catalog(
            field_widget(1 | 32 | 4),
            Some(hide_action(&["f"], Some(true))),
        );
        let hidden = run(&cat);
        assert_eq!(
            hidden.flags(&widget, &NoResolve),
            AnnotFlags::from_bits(4 | 2)
        );
    }

    #[test]
    fn a_name_no_field_carries_touches_nothing() {
        let (cat, _) = catalog(field_widget(4), Some(hide_action(&["other"], None)));
        assert!(run(&cat).is_empty());
    }

    #[test]
    fn only_a_string_names_a_field() {
        // A `/T` holding the field's own dictionary reference resolves to
        // nothing, which the C++ skips outright and the test file's comment
        // records as surprising.
        let widget = field_widget(4);
        let action = Object::Dict(dict(&[
            ("Type", Object::Name(Name::from("Action"))),
            ("S", Object::Name(Name::from("Hide"))),
            (
                "T",
                Object::Array(Array::of([Object::Dict(widget.clone())])),
            ),
        ]));
        let (cat, _) = catalog(widget, Some(action));
        assert!(run(&cat).is_empty());
    }

    #[test]
    fn an_open_action_that_is_not_a_hide_leaves_every_flag_alone() {
        let goto = Object::Dict(dict(&[
            ("Type", Object::Name(Name::from("Action"))),
            ("S", Object::Name(Name::from("GoTo"))),
        ]));
        let (cat, _) = catalog(field_widget(4), Some(goto));
        assert!(run(&cat).is_empty());
    }

    #[test]
    fn a_hide_behind_a_next_chain_still_runs() {
        let goto = Object::Dict(dict(&[
            ("Type", Object::Name(Name::from("Action"))),
            ("S", Object::Name(Name::from("GoTo"))),
            ("Next", hide_action(&["f"], None)),
        ]));
        let (cat, widget) = catalog(field_widget(0), Some(goto));
        assert_eq!(
            run(&cat).flags(&widget, &NoResolve),
            AnnotFlags::from_bits(2)
        );
    }

    #[test]
    fn a_destination_array_open_action_is_not_an_action() {
        let (cat, _) = catalog(
            field_widget(4),
            Some(Object::Array(Array::of([Object::Int(0)]))),
        );
        assert!(run(&cat).is_empty());
    }
}
