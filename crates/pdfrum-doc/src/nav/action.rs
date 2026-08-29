//! Actions (ISO 32000-1 §12.6): what a link, bookmark or field trigger does.
//!
//! Eighteen action types, matched **exactly and case-sensitively** — a
//! `/S /Javascript` is not a JavaScript action — behind a validity gate that
//! also rejects a dictionary whose `/Type` is present and is anything other
//! than the name `Action`.
//!
//! The per-type accessors are each restricted to the types they mean
//! something for, with one exception worth knowing: [`Action::fields`] reads
//! `/S` **coercively**, so a string-valued `/S (Hide)` reaches the hide
//! branch there while [`Action::kind`] would have called the whole action
//! unknown.

use std::collections::HashSet;

use pdfrum_common::{DiagKind, Diagnostics, Limits, Severity};
use pdfrum_object::{Dict, Object, Resolve, decode_text};

use crate::names;
use crate::nav::dest::Dest;
use crate::nav::filespec::FileSpec;

/// An action's `/S` type. The discriminant order is the public numbering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ActionKind {
    /// `/S` matched no known spelling, or the dictionary failed its type
    /// check.
    #[default]
    Unknown,
    /// Go to a destination in this document.
    GoTo,
    /// Go to a destination in another document.
    GoToR,
    /// Go to a destination in an embedded document.
    GoToE,
    /// Launch an application.
    Launch,
    /// Jump to an article thread.
    Thread,
    /// Resolve a uniform resource identifier.
    Uri,
    /// Play a sound.
    Sound,
    /// Play a movie.
    Movie,
    /// Hide or show annotations.
    Hide,
    /// A named viewer action.
    Named,
    /// Submit form data.
    SubmitForm,
    /// Reset form fields.
    ResetForm,
    /// Import form data.
    ImportData,
    /// Run a script.
    JavaScript,
    /// Change optional-content group states.
    SetOcgState,
    /// Play a rendition.
    Rendition,
    /// Perform a page transition.
    Trans,
    /// Set a three-dimensional view.
    GoTo3DView,
}

/// The spelling table, in declaration order.
const KINDS: [(ActionKind, &[u8]); 18] = [
    (ActionKind::GoTo, b"GoTo"),
    (ActionKind::GoToR, b"GoToR"),
    (ActionKind::GoToE, b"GoToE"),
    (ActionKind::Launch, b"Launch"),
    (ActionKind::Thread, b"Thread"),
    (ActionKind::Uri, b"URI"),
    (ActionKind::Sound, b"Sound"),
    (ActionKind::Movie, b"Movie"),
    (ActionKind::Hide, b"Hide"),
    (ActionKind::Named, b"Named"),
    (ActionKind::SubmitForm, b"SubmitForm"),
    (ActionKind::ResetForm, b"ResetForm"),
    (ActionKind::ImportData, b"ImportData"),
    (ActionKind::JavaScript, b"JavaScript"),
    (ActionKind::SetOcgState, b"SetOCGState"),
    (ActionKind::Rendition, b"Rendition"),
    (ActionKind::Trans, b"Trans"),
    (ActionKind::GoTo3DView, b"GoTo3DView"),
];

/// A trigger in an additional-actions dictionary.
///
/// The key table has a **genuine collision**: `C` names both
/// [`AActionType::ClosePage`] and [`AActionType::Calculate`]. The enum
/// discriminant is the interface, not the string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AActionType {
    /// The cursor enters the annotation's area.
    CursorEnter,
    /// The cursor leaves it.
    CursorExit,
    /// A mouse button goes down inside it.
    ButtonDown,
    /// A mouse button comes up inside it.
    ButtonUp,
    /// The annotation gains focus.
    GetFocus,
    /// It loses focus.
    LoseFocus,
    /// Its page opens.
    PageOpen,
    /// Its page closes.
    PageClose,
    /// Its page becomes visible.
    PageVisible,
    /// Its page becomes invisible.
    PageInvisible,
    /// The document opens.
    OpenPage,
    /// The page closes.
    ClosePage,
    /// A keystroke reaches a form field.
    KeyStroke,
    /// A field's value is formatted for display.
    Format,
    /// A field's value is validated.
    Validate,
    /// A field's value is recalculated.
    Calculate,
    /// Before the document closes.
    CloseDocument,
    /// Before the document is saved.
    SaveDocument,
    /// After the document is saved.
    DocumentSaved,
    /// Before the document is printed.
    PrintDocument,
    /// After the document is printed.
    DocumentPrinted,
}

/// The additional-action key table, in enum order, collision included.
const AACTION_KEYS: [(AActionType, &[u8]); 21] = [
    (AActionType::CursorEnter, b"E"),
    (AActionType::CursorExit, b"X"),
    (AActionType::ButtonDown, b"D"),
    (AActionType::ButtonUp, b"U"),
    (AActionType::GetFocus, b"Fo"),
    (AActionType::LoseFocus, b"Bl"),
    (AActionType::PageOpen, b"PO"),
    (AActionType::PageClose, b"PC"),
    (AActionType::PageVisible, b"PV"),
    (AActionType::PageInvisible, b"PI"),
    (AActionType::OpenPage, b"O"),
    (AActionType::ClosePage, b"C"),
    (AActionType::KeyStroke, b"K"),
    (AActionType::Format, b"F"),
    (AActionType::Validate, b"V"),
    (AActionType::Calculate, b"C"),
    (AActionType::CloseDocument, b"WC"),
    (AActionType::SaveDocument, b"WS"),
    (AActionType::DocumentSaved, b"DS"),
    (AActionType::PrintDocument, b"WP"),
    (AActionType::DocumentPrinted, b"DP"),
];

impl AActionType {
    /// The dictionary key this trigger lives under.
    #[must_use]
    pub fn key(self) -> &'static [u8] {
        AACTION_KEYS
            .iter()
            .find(|(kind, _)| *kind == self)
            .map_or(&b""[..], |(_, key)| key)
    }

    /// Whether the trigger is a direct user input rather than a document or
    /// page event.
    #[must_use]
    pub fn is_user_input(self) -> bool {
        matches!(
            self,
            AActionType::ButtonUp | AActionType::ButtonDown | AActionType::KeyStroke
        )
    }
}

/// An action dictionary.
#[derive(Debug, Clone, PartialEq)]
pub struct Action {
    /// The dictionary itself.
    pub dict: Dict,
}

impl Action {
    /// Wraps a dictionary as an action.
    #[must_use]
    pub fn new(dict: Dict) -> Action {
        Action { dict }
    }

    /// The action's type.
    ///
    /// Two gates: `/Type`, if present, must be the **name** `Action`; and
    /// `/S` is read **name-typed and without resolving**, so a string `/S`
    /// yields nothing and the action is unknown.
    #[must_use]
    pub fn kind(&self) -> ActionKind {
        if let Some(kind) = self.dict.name(names::TYPE)
            && kind != names::ANNOT_ACTION
        {
            return ActionKind::Unknown;
        }
        let Some(spelling) = self.dict.name(names::S) else {
            return ActionKind::Unknown;
        };
        KINDS
            .iter()
            .find(|(_, name)| *name == spelling.as_bytes())
            .map_or(ActionKind::Unknown, |(kind, _)| *kind)
    }

    /// The action's destination, for the three kinds that have one.
    #[must_use]
    pub fn dest<R: Resolve>(
        &self,
        catalog: &Dict,
        r: &R,
        limits: &Limits,
        diags: &mut Diagnostics,
    ) -> Dest {
        if !matches!(
            self.kind(),
            ActionKind::GoTo | ActionKind::GoToR | ActionKind::GoToE
        ) {
            return Dest::default();
        }
        let target = self.dict.get(names::D, r).map(|d| d.get().clone());
        Dest::create(catalog, target.as_ref(), r, limits, diags)
    }

    /// The file this action names, for the five kinds that name one.
    ///
    /// A `Launch` action with no `/F` falls back to `/Win /F`, decoded as
    /// **Latin-1** rather than as PDF text — the only such decoding in the
    /// navigation path, and it changes what high bytes mean.
    #[must_use]
    pub fn file_path<R: Resolve>(&self, r: &R) -> String {
        let kind = self.kind();
        if !matches!(
            kind,
            ActionKind::GoToR
                | ActionKind::GoToE
                | ActionKind::Launch
                | ActionKind::SubmitForm
                | ActionKind::ImportData
        ) {
            return String::new();
        }
        if let Some(spec) = self.dict.get(names::F, r) {
            return FileSpec::new(spec.get().clone()).file_name(r);
        }
        if kind != ActionKind::Launch {
            return String::new();
        }
        self.dict
            .dict(names::WIN, r)
            .and_then(|win| win.byte_string(names::F, r))
            .map(|bytes| bytes.iter().map(|b| char::from(*b)).collect())
            .unwrap_or_default()
    }

    /// The URI this action resolves, as **raw bytes**.
    ///
    /// Never re-encoded and never percent-decoded: a high byte written into
    /// the file comes back out unchanged. A relative URI — one with no colon
    /// past its first character — is prefixed with the catalog's `/URI /Base`
    /// when there is one. Note a URI *starting* with a colon counts as
    /// relative.
    #[must_use]
    pub fn uri<R: Resolve>(&self, catalog: &Dict, r: &R) -> Vec<u8> {
        if self.kind() != ActionKind::Uri {
            return Vec::new();
        }
        let uri = self.dict.byte_string(names::URI, r).unwrap_or_default();
        let is_absolute = uri.iter().position(|b| *b == b':').is_some_and(|at| at > 0);
        if is_absolute {
            return uri;
        }
        let base = catalog
            .dict(names::URI, r)
            .and_then(|dict| dict.get(names::BASE, r).map(|b| b.get().clone()))
            // A stream satisfies the type test but spells as nothing, so it
            // contributes an empty prefix rather than being rejected.
            .filter(|value| matches!(value, Object::Str(_) | Object::Stream(_)))
            .map(|value| value.to_byte_string())
            .unwrap_or_default();
        let mut joined = base;
        joined.extend_from_slice(&uri);
        joined
    }

    /// Whether a hide action hides (true) or shows (false).
    ///
    /// The key is **Boolean-typed**, so an integer `/H 0` is not false — it
    /// is absent, and the default is to hide.
    #[must_use]
    pub fn hide_status(&self) -> bool {
        self.dict.bool(names::H).unwrap_or(true)
    }

    /// A named action's name.
    #[must_use]
    pub fn named_action<R: Resolve>(&self, r: &R) -> Vec<u8> {
        self.dict.byte_string(names::N, r).unwrap_or_default()
    }

    /// A submit- or reset-form action's flag word.
    #[must_use]
    pub fn flags<R: Resolve>(&self, r: &R) -> i64 {
        self.dict.int(names::FLAGS, r).unwrap_or(0)
    }

    /// The fields this action applies to.
    ///
    /// `/S` is read **coercively** here, unlike in [`Action::kind`], so a
    /// string-valued `/S (Hide)` reaches the `/T` branch even though the
    /// action's type reads as unknown.
    #[must_use]
    pub fn fields<R: Resolve>(&self, r: &R) -> Vec<Object> {
        let is_hide = self.dict.byte_string(names::S, r).as_deref() == Some(b"Hide");
        let key = if is_hide { names::T } else { names::FIELDS };
        let Some(value) = self.dict.get(key, r).map(|v| v.get().clone()) else {
            return Vec::new();
        };
        match value {
            single @ (Object::Dict(_) | Object::Str(_)) => vec![single],
            Object::Array(array) => (0..array.len())
                .filter_map(|index| array.get(index, r).map(|v| v.get().clone()))
                .filter(|value| !value.is_null())
                .collect(),
            _ => Vec::new(),
        }
    }

    /// The script this action runs.
    ///
    /// The `Option` distinguishes "no `/JS` at all" from "a `/JS` that
    /// decodes to nothing", which callers rely on. A stream's script decodes
    /// its **raw**, still-encoded bytes.
    #[must_use]
    pub fn javascript<R: Resolve>(&self, r: &R) -> Option<String> {
        let value = self.dict.get(names::JS, r).map(|v| v.get().clone())?;
        match value {
            Object::Str(text) => Some(decode_text(&text.bytes).into_owned()),
            Object::Stream(stream) => Some(decode_text(stream.data.as_bytes()).into_owned()),
            _ => None,
        }
    }

    /// How many actions follow this one.
    ///
    /// The key's *presence* is tested before its value, so a `/Next` holding
    /// an unresolvable reference has the key but no direct object and counts
    /// zero.
    #[must_use]
    pub fn next_count<R: Resolve>(&self, r: &R) -> usize {
        if !self.dict.contains_key(names::NEXT) {
            return 0;
        }
        match self.dict.get(names::NEXT, r).map(|n| n.get().clone()) {
            Some(Object::Dict(_)) => 1,
            Some(Object::Array(array)) => array.len(),
            _ => 0,
        }
    }

    /// The `index`-th following action.
    #[must_use]
    pub fn next<R: Resolve>(&self, index: usize, r: &R) -> Option<Action> {
        if !self.dict.contains_key(names::NEXT) {
            return None;
        }
        match self.dict.get(names::NEXT, r).map(|n| n.get().clone())? {
            // An array element that is not a dictionary yields an *empty*
            // action at that index while the count still includes it.
            Object::Array(array) => Some(Action::new(array.dict_at(index, r).unwrap_or_default())),
            Object::Dict(dict) if index == 0 => Some(Action::new(dict)),
            _ => None,
        }
    }

    /// Walks the `/Next` chain depth-first, cutting any cycle.
    ///
    /// Upstream has no guard here at all — an action whose `/Next` points at
    /// itself loops forever — so this adds a visited set keyed on object
    /// number and a depth cap.
    #[must_use]
    pub fn chain<R: Resolve>(
        &self,
        r: &R,
        limits: &Limits,
        diags: &mut Diagnostics,
    ) -> Vec<Action> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        collect_chain(self, 0, &mut seen, &mut out, r, limits, diags);
        out
    }
}

fn collect_chain<R: Resolve>(
    action: &Action,
    depth: u32,
    seen: &mut HashSet<u32>,
    out: &mut Vec<Action>,
    r: &R,
    limits: &Limits,
    diags: &mut Diagnostics,
) {
    if depth > limits.max_name_tree_depth {
        diags.record(Severity::Suspicious, DiagKind::TreeDepthExceeded, None);
        return;
    }
    for index in 0..action.next_count(r) {
        let Some(next) = action.next(index, r) else {
            continue;
        };
        if let Some(reference) = action.dict.reference(names::NEXT)
            && !seen.insert(reference.num)
        {
            diags.record(Severity::Recovered, DiagKind::NavigationCycle, None);
            return;
        }
        out.push(next.clone());
        collect_chain(&next, depth + 1, seen, out, r, limits, diags);
    }
}

/// Reads one trigger out of an additional-actions dictionary.
#[must_use]
pub fn additional_action<R: Resolve>(
    aactions: &Dict,
    trigger: AActionType,
    r: &R,
) -> Option<Action> {
    let key = pdfrum_object::Name::new(trigger.key().to_vec());
    aactions.dict(&key, r).map(Action::new)
}

#[cfg(test)]
mod tests {
    use super::{AActionType, Action, ActionKind};
    use pdfrum_object::{Array, Dict, Name, NoResolve, Object, PdfString};

    fn action(pairs: &[(&str, Object)]) -> Action {
        Action::new(Dict::from_pairs(
            pairs
                .iter()
                .map(|(k, v)| (Name::from(*k), v.clone()))
                .collect::<Vec<_>>(),
        ))
    }

    const SPELLINGS: [&str; 18] = [
        "GoTo",
        "GoToR",
        "GoToE",
        "Launch",
        "Thread",
        "URI",
        "Sound",
        "Movie",
        "Hide",
        "Named",
        "SubmitForm",
        "ResetForm",
        "ImportData",
        "JavaScript",
        "SetOCGState",
        "Rendition",
        "Trans",
        "GoTo3DView",
    ];

    #[test]
    fn every_spelling_resolves_with_and_without_an_explicit_type() {
        for spelling in SPELLINGS {
            let bare = action(&[("S", Object::Name(Name::from(spelling)))]);
            assert_ne!(bare.kind(), ActionKind::Unknown, "{spelling}");
            let typed = action(&[
                ("Type", Object::Name(Name::from("Action"))),
                ("S", Object::Name(Name::from(spelling))),
            ]);
            assert_eq!(typed.kind(), bare.kind(), "{spelling}");
        }
    }

    #[test]
    fn a_wrong_type_or_a_string_subtype_makes_every_action_unknown() {
        for spelling in SPELLINGS {
            let wrong_type = action(&[
                ("Type", Object::Name(Name::from("Lights"))),
                ("S", Object::Name(Name::from(spelling))),
            ]);
            assert_eq!(wrong_type.kind(), ActionKind::Unknown, "{spelling}");

            let stringly = action(&[("S", Object::Str(PdfString::literal(spelling.as_bytes())))]);
            assert_eq!(stringly.kind(), ActionKind::Unknown, "{spelling}");
        }
    }

    #[test]
    fn matching_is_case_sensitive() {
        for spelling in ["Camera", "Javascript", "Unknown", "goto"] {
            let a = action(&[("S", Object::Name(Name::from(spelling)))]);
            assert_eq!(a.kind(), ActionKind::Unknown, "{spelling}");
        }
    }

    #[test]
    fn the_hide_flag_is_boolean_typed_so_an_integer_reads_as_absent() {
        assert!(action(&[]).hide_status());
        assert!(!action(&[("H", Object::Bool(false))]).hide_status());
        // `/H 0` is an integer, so the Boolean reader sees nothing.
        assert!(action(&[("H", Object::Int(0))]).hide_status());
    }

    #[test]
    fn a_uri_travels_through_as_raw_bytes() {
        let a = action(&[
            ("S", Object::Name(Name::from("URI"))),
            (
                "URI",
                Object::Str(PdfString::literal(b"https://example.com/\xA5x\xC7y")),
            ),
        ]);
        assert_eq!(
            a.uri(&Dict::new(), &NoResolve),
            b"https://example.com/\xA5x\xC7y".to_vec()
        );
    }

    #[test]
    fn a_relative_uri_picks_up_the_catalogs_base() {
        let catalog = Dict::from_pairs([(
            Name::from("URI"),
            Object::Dict(Dict::from_pairs([(
                Name::from("Base"),
                Object::Str(PdfString::literal(b"https://example.com/")),
            )])),
        )]);
        let relative = action(&[
            ("S", Object::Name(Name::from("URI"))),
            ("URI", Object::Str(PdfString::literal(b"page.html"))),
        ]);
        assert_eq!(
            relative.uri(&catalog, &NoResolve),
            b"https://example.com/page.html".to_vec()
        );

        // A colon at index zero still counts as relative.
        let leading_colon = action(&[
            ("S", Object::Name(Name::from("URI"))),
            ("URI", Object::Str(PdfString::literal(b":odd"))),
        ]);
        assert_eq!(
            leading_colon.uri(&catalog, &NoResolve),
            b"https://example.com/:odd".to_vec()
        );

        // An absolute one is left alone.
        let absolute = action(&[
            ("S", Object::Name(Name::from("URI"))),
            ("URI", Object::Str(PdfString::literal(b"ftp://host/x"))),
        ]);
        assert_eq!(absolute.uri(&catalog, &NoResolve), b"ftp://host/x".to_vec());
    }

    #[test]
    fn the_field_list_reads_its_subtype_coercively() {
        // A *string* `/S (Hide)` reaches the `/T` branch even though the
        // action's own type reads as unknown.
        let hide = action(&[
            ("S", Object::Str(PdfString::literal(b"Hide"))),
            ("T", Object::Str(PdfString::literal(b"field"))),
        ]);
        assert_eq!(hide.kind(), ActionKind::Unknown);
        assert_eq!(hide.fields(&NoResolve).len(), 1);
    }

    #[test]
    fn a_next_key_holding_nothing_usable_counts_zero() {
        assert_eq!(action(&[]).next_count(&NoResolve), 0);
        assert_eq!(action(&[("Next", Object::Null)]).next_count(&NoResolve), 0);
        assert_eq!(
            action(&[("Next", Object::Dict(Dict::new()))]).next_count(&NoResolve),
            1
        );
        let two = action(&[(
            "Next",
            Object::Array(Array::of([Object::Dict(Dict::new()), Object::Int(4)])),
        )]);
        assert_eq!(two.next_count(&NoResolve), 2);
        // The non-dictionary element still yields an empty action.
        assert_eq!(
            two.next(1, &NoResolve).map(|a| a.dict.is_empty()),
            Some(true)
        );
    }

    #[test]
    fn the_additional_action_table_keeps_its_key_collision() {
        assert_eq!(AActionType::ClosePage.key(), b"C");
        assert_eq!(AActionType::Calculate.key(), b"C");
        assert!(AActionType::ButtonUp.is_user_input());
        assert!(!AActionType::Format.is_user_input());
    }
}
