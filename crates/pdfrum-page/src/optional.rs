//! Optional content: which page objects are visible
//! (ISO 32000-1 §8.11).
//!
//! The interpreter never filters — a hidden object is still built — so this
//! is a predicate that `pdfrum-render` and `pdfrum-text` share rather than
//! part of the fold.
//!
//! Several defaults here are counter-intuitive and every one is load-bearing:
//!
//! | Question | Answer |
//! |---|---|
//! | A null dictionary passed to the *content* check | **visible** |
//! | A null dictionary passed to the *group* check | **invisible** |
//! | `/P` absent on a membership dictionary | `AnyOn` |
//! | `/P` present but unrecognised, with any valid group | **invisible** |
//! | `/BaseState` absent | `ON` |
//! | Any state string other than exactly `OFF` | on |
//! | A `/VE` visibility expression | absolute precedence over `/P` |
//! | A membership naming one group as a *dictionary* | `/P` ignored entirely |

use crate::names;
use pdfrum_common::{DiagKind, Diagnostics, Severity};
use pdfrum_object::{Array, Dict, Name, Object, Resolve};
use std::collections::HashMap;

/// How deep a `/VE` visibility expression may nest.
///
/// Compared with `>` from an initial depth of zero, so **thirty-three**
/// levels are accepted.
pub const MAX_VE_DEPTH: u32 = 32;

/// Which use an optional-content configuration is being read for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum UsageType {
    /// On-screen viewing.
    #[default]
    View,
    /// Design-time display.
    Design,
    /// Printing.
    Print,
    /// Export to another format.
    Export,
}

impl UsageType {
    /// The name this usage is spelled with.
    #[must_use]
    pub fn as_bytes(self) -> &'static [u8] {
        match self {
            Self::View => b"View",
            Self::Design => b"Design",
            Self::Print => b"Print",
            Self::Export => b"Export",
        }
    }

    /// The `<usage>State` key this usage looks for.
    #[must_use]
    pub fn state_key(self) -> Name {
        let mut key = self.as_bytes().to_vec();
        key.extend_from_slice(b"State");
        Name::new(key)
    }
}

/// The visibility context: the catalog's optional-content configuration plus
/// the memoized answers.
///
/// Group answers are memoized; **membership answers are not**, matching
/// PDFium, because a membership can depend on several groups and its
/// evaluation is cheap.
#[derive(Debug)]
pub struct OcContext {
    usage: UsageType,
    /// `/OCProperties` from the catalog.
    properties: Option<Dict>,
    /// Memoized per-group answers, keyed on the group dictionary's identity
    /// as a reference.
    cache: HashMap<pdfrum_object::ObjRef, bool>,
}

impl OcContext {
    /// A context reading `usage` from the catalog's `/OCProperties`.
    #[must_use]
    pub fn new(properties: Option<Dict>, usage: UsageType) -> Self {
        Self {
            usage,
            properties,
            cache: HashMap::new(),
        }
    }

    /// A context that makes everything visible, for a document with no
    /// optional content.
    #[must_use]
    pub fn permissive() -> Self {
        Self::new(None, UsageType::View)
    }

    /// Whether a dictionary reached from content is visible.
    ///
    /// **A null dictionary is visible** here — the opposite of
    /// [`Self::group_visible`], and the asymmetry is deliberate: content with
    /// no optional-content dictionary is unconditional content.
    pub fn content_visible<R: Resolve>(
        &mut self,
        dict: Option<&Dict>,
        r: &R,
        diags: &mut Diagnostics,
    ) -> bool {
        let Some(dict) = dict else {
            return true;
        };
        // `/Type` defaults to `OCG`; anything else is treated as a
        // membership dictionary.
        if dict
            .name(names::TYPE)
            .is_none_or(|t| t.as_bytes() == b"OCG")
        {
            return self.group_visible(Some(dict), r);
        }
        self.membership_visible(dict, r, diags)
    }

    /// Whether an optional-content *group* is visible.
    ///
    /// **A null dictionary is invisible** here. The answer is memoized per
    /// group; membership answers deliberately are not.
    pub fn group_visible<R: Resolve>(&mut self, dict: Option<&Dict>, r: &R) -> bool {
        let Some(dict) = dict else {
            return false;
        };
        // Only a group named by reference has an identity to memoize on.
        let key = dict.reference(&Name::from("__self"));
        if let Some(id) = key
            && let Some(hit) = self.cache.get(&id)
        {
            return *hit;
        }
        let answer = self.load_group_state(dict, r);
        if let Some(id) = key {
            self.cache.insert(id, answer);
        }
        answer
    }

    /// How many group answers are memoized.
    #[must_use]
    pub fn memoized(&self) -> usize {
        self.cache.len()
    }

    /// Whether a membership dictionary's condition holds.
    fn membership_visible<R: Resolve>(
        &mut self,
        ocmd: &Dict,
        r: &R,
        diags: &mut Diagnostics,
    ) -> bool {
        // A visibility expression takes **absolute precedence** over `/P`.
        if let Some(ve) = ocmd.array(names::VE, r) {
            return self.eval_expression(&ve, r, 0);
        }
        let policy = ocmd
            .byte_string(names::P, r)
            .unwrap_or_else(|| b"AnyOn".to_vec());
        let Some(ocgs) = ocmd.get(names::OCGS, r) else {
            return true;
        };
        match &*ocgs {
            // A single group as a dictionary: `/P` is ignored entirely.
            Object::Dict(d) => self.group_visible(Some(d), r),
            Object::Array(array) => {
                // The seed is the vacuous truth for the "all" policies.
                let state = policy == b"AllOn" || policy == b"AllOff";
                let mut seen_valid = false;
                for element in array.iter() {
                    let Some(d) = element.resolve(r).ok().and_then(|o| o.as_dict().cloned()) else {
                        // Non-dictionary entries are skipped without
                        // counting.
                        continue;
                    };
                    seen_valid = true;
                    let visible = self.group_visible(Some(&d), r);
                    if (policy == b"AnyOn" && visible) || (policy == b"AnyOff" && !visible) {
                        return true;
                    }
                    if (policy == b"AllOn" && !visible) || (policy == b"AllOff" && visible) {
                        return false;
                    }
                }
                if !seen_valid {
                    return true;
                }
                // An unrecognised policy matches none of the four conditions,
                // so the loop never short-circuits and the seed — `false` —
                // is the answer. A membership with an unknown `/P` and at
                // least one valid group is therefore **invisible**.
                if !matches!(
                    policy.as_slice(),
                    b"AnyOn" | b"AllOn" | b"AnyOff" | b"AllOff"
                ) {
                    diags.record(
                        Severity::Suspicious,
                        DiagKind::OptionalContentPolicyUnknown,
                        None,
                    );
                }
                state
            }
            _ => true,
        }
    }

    /// Evaluate a `/VE` visibility expression.
    ///
    /// The operators are case-sensitive, and anything that is not one of the
    /// three makes the expression false — which is to say invisible.
    fn eval_expression<R: Resolve>(&mut self, expr: &Array, r: &R, depth: u32) -> bool {
        if depth > MAX_VE_DEPTH {
            return false;
        }
        let operator = expr.byte_string_at(0).unwrap_or_default();
        match operator.as_slice() {
            b"Not" => match expr.get(1, r).as_deref() {
                Some(Object::Dict(d)) => !self.group_visible(Some(d), r),
                Some(Object::Array(a)) => !self.eval_expression(a, r, depth + 1),
                _ => false,
            },
            b"Or" | b"And" => {
                let and = operator == b"And";
                let mut value = false;
                for i in 1..expr.len() {
                    let operand = expr.get(i, r);
                    // A null element is skipped **without advancing the seed
                    // logic**, so a missing first operand leaves an `And`
                    // combining against the initial `false` — which makes it
                    // false outright, while an `Or` still works.
                    let Some(operand) = operand else {
                        continue;
                    };
                    let result = match &*operand {
                        Object::Dict(d) => self.group_visible(Some(d), r),
                        Object::Array(a) => self.eval_expression(a, r, depth + 1),
                        // Anything else contributes false.
                        _ => false,
                    };
                    if i == 1 {
                        value = result;
                    } else if and {
                        value = value && result;
                    } else {
                        value = value || result;
                    }
                }
                value
            }
            _ => false,
        }
    }

    /// A group's state, memoized.
    fn load_group_state<R: Resolve>(&mut self, ocg: &Dict, r: &R) -> bool {
        // `/Intent` excluding `View` means the group is not subject to
        // view-time visibility at all, so it is always visible.
        if !has_intent(ocg, b"View", b"View", r) {
            return true;
        }
        if let Some(usage) = ocg.dict(names::USAGE, r) {
            let state_key = self.usage.state_key();
            if let Some(entry) = usage.dict(&Name::new(self.usage.as_bytes()), r)
                && entry.contains_key(&state_key)
            {
                return entry.byte_string(&state_key, r).as_deref() != Some(b"OFF");
            }
            // A non-view usage falls back to the view entry.
            if self.usage != UsageType::View
                && let Some(entry) = usage.dict(&Name::from("View"), r)
                && entry.contains_key(&Name::from("ViewState"))
            {
                return entry.byte_string(&Name::from("ViewState"), r).as_deref() != Some(b"OFF");
            }
        }
        self.state_from_config(ocg, r)
    }

    /// A group's state from the selected configuration.
    fn state_from_config<R: Resolve>(&mut self, ocg: &Dict, r: &R) -> bool {
        let Some(config) = self.select_config(ocg, r) else {
            // No configuration names this group, so it is visible.
            return true;
        };
        // `/BaseState` defaults to `ON`, and only the exact string `OFF`
        // turns it off.
        let mut on = config.byte_string(names::BASE_STATE, r).as_deref() != Some(b"OFF");
        if let Some(array) = config.array(names::ON, r)
            && contains_dict(&array, ocg, r)
        {
            on = true;
        }
        // `/OFF` wins over `/ON`.
        if let Some(array) = config.array(names::OFF, r)
            && contains_dict(&array, ocg, r)
        {
            on = false;
        }
        // Each matching `/AS` entry in array order, last one winning.
        if let Some(entries) = config.array(names::AS, r) {
            for element in entries.iter() {
                let Some(entry) = element.resolve(r).ok().and_then(|o| o.as_dict().cloned()) else {
                    continue;
                };
                // `/Event` defaults to `View`.
                let event = entry
                    .byte_string(names::EVENT, r)
                    .unwrap_or_else(|| b"View".to_vec());
                if event != self.usage.as_bytes() {
                    continue;
                }
                let Some(groups) = entry.array(names::OCGS, r) else {
                    continue;
                };
                if !contains_dict(&groups, ocg, r) {
                    continue;
                }
                let state_key = self.usage.state_key();
                if let Some(sub) = entry.dict(&Name::new(self.usage.as_bytes()), r) {
                    on = sub.byte_string(&state_key, r).as_deref() != Some(b"OFF");
                }
            }
        }
        on
    }

    /// The configuration governing `ocg`: the first `/Configs` entry whose
    /// `/Intent` names `View` or `All`, else `/D`.
    fn select_config<R: Resolve>(&self, ocg: &Dict, r: &R) -> Option<Dict> {
        let properties = self.properties.as_ref()?;
        // The catalog must list this group at all, or nothing governs it.
        let all = properties.array(names::OCGS, r)?;
        if !contains_dict(&all, ocg, r) {
            return None;
        }
        if let Some(configs) = properties.array(names::CONFIGS, r) {
            for element in configs.iter() {
                let Some(config) = element.resolve(r).ok().and_then(|o| o.as_dict().cloned())
                else {
                    continue;
                };
                // A configuration with no `/Intent` is **never** selected,
                // because the default here is the empty string.
                if has_intent(&config, b"View", b"", r) {
                    return Some(config);
                }
            }
        }
        properties.dict(names::D_CONFIG, r)
    }
}

/// Which of a page's objects optional content hides, shaped like the page.
///
/// # Why a tree and not a set of ids
///
/// A page object has no id. The graph is `Vec<PageObject>` with a form's
/// children nested inside it, so the only thing that names an object is its
/// position — and that is exactly what this mirrors: `hidden[i]` answers for
/// `objects[i]`, and a form's `children` answer for that form's own list.
/// Walking the two together costs one index per object and needs no identity
/// the page graph does not have.
///
/// An empty tree means nothing is hidden, which is what a document with no
/// `/OCProperties` produces and what [`Visibility::shows_everything`] reports,
/// so a renderer can skip the descent entirely.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Visibility {
    /// One entry per object in the list this tree describes, in order.
    nodes: Vec<Node>,
}

/// One object's answer, plus its children's when it is a form.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct Node {
    /// Whether this object is drawn at all.
    visible: bool,
    /// A form's own objects. Empty for every other kind, and also for a form
    /// that is itself hidden — there is nothing to say about children nobody
    /// will reach.
    children: Visibility,
}

impl Visibility {
    /// Nothing hidden, for a document with no optional content.
    #[must_use]
    pub fn all_visible() -> Self {
        Self::default()
    }

    /// Whether this tree hides nothing anywhere beneath it.
    ///
    /// A renderer can take this as licence to stop descending: an empty tree
    /// is the answer for a page with no optional content at all, and
    /// [`Self::visible`] already reports an absent entry as visible.
    #[must_use]
    pub fn shows_everything(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Whether the object at `index` is drawn.
    ///
    /// **An index this tree does not cover is visible.** That is not
    /// permissiveness for its own sake: it is what makes `all_visible()` an
    /// empty tree rather than a vector of `true`, and it means a caller
    /// whose object list has grown since the pre-pass ran draws the new
    /// objects rather than silently dropping them.
    #[must_use]
    pub fn visible(&self, index: usize) -> bool {
        self.nodes.get(index).is_none_or(|n| n.visible)
    }

    /// The tree describing the form at `index`'s own objects.
    #[must_use]
    pub fn children(&self, index: usize) -> Self {
        self.nodes
            .get(index)
            .map(|n| n.children.clone())
            .unwrap_or_default()
    }

    /// Drop a tree that turned out to hide nothing.
    ///
    /// Keeping it would be correct and would cost every renderer the descent
    /// [`Self::shows_everything`] exists to avoid, so the pre-pass collapses
    /// as it unwinds and a page with no optional content ends up empty at
    /// every level rather than only at the root.
    fn collapsed(self) -> Self {
        if self
            .nodes
            .iter()
            .all(|n| n.visible && n.children.shows_everything())
        {
            Self::default()
        } else {
            self
        }
    }
}

/// Resolve which of a page's objects optional content hides.
///
/// A pre-pass: it runs between building the page and rendering it and produces
/// plain data, so no resolver reaches the render walk. It answers all three
/// forms of `/OC` — the marked-content one, a form `XObject`'s own dictionary
/// and an image's — and the `XObject` ones are separate from the mark: a form
/// can be hidden by its own dictionary while the `Do` that drew it sits under
/// no `/OC` mark at all.
// The split is the point: `content_visible` needs `&mut OcContext` and a
// `Resolve`, and a rasterizer needs neither.
#[must_use]
pub fn page_visibility<R: Resolve>(
    page: &crate::Page,
    oc: &mut OcContext,
    r: &R,
    diags: &mut Diagnostics,
) -> Visibility {
    object_visibility(&page.objects, oc, r, diags)
}

/// [`page_visibility`] for one object list, which is what recursion needs.
fn object_visibility<R: Resolve>(
    objects: &[crate::PageObject],
    oc: &mut OcContext,
    r: &R,
    diags: &mut Diagnostics,
) -> Visibility {
    let nodes = objects
        .iter()
        .map(|object| {
            let visible = object_visible(object, oc, r, diags);
            let children = match object {
                crate::PageObject::Form(f) if visible => {
                    object_visibility(&f.object.objects, oc, r, diags)
                }
                _ => Visibility::default(),
            };
            Node { visible, children }
        })
        .collect();
    Visibility { nodes }.collapsed()
}

/// Whether one object is drawn, by its marks and by its own `/OC`.
fn object_visible<R: Resolve>(
    object: &crate::PageObject,
    oc: &mut OcContext,
    r: &R,
    diags: &mut Diagnostics,
) -> bool {
    // `CheckPageObjectVisible` scans **every** `/OC` mark on the object, not
    // just the innermost, so nested sequences each get a veto.
    if !object
        .marks()
        .optional_content_all()
        .into_iter()
        .all(|d| oc.content_visible(Some(d), r, diags))
    {
        return false;
    }
    // An XObject's own `/OC` is a second, independent veto through the same
    // predicate — `CheckOCGDictVisible` is what both call sites reach, and
    // an absent one is visible.
    let own = match object {
        crate::PageObject::Form(f) => f.object.oc.as_deref(),
        crate::PageObject::Image(i) => i.object.oc.as_deref(),
        crate::PageObject::Path(_) | crate::PageObject::Text(_) | crate::PageObject::Shading(_) => {
            None
        }
    };
    oc.content_visible(own, r, diags)
}

/// Whether a dictionary's `/Intent` names `element`.
///
/// An absent `/Intent` yields `element == default`, which is how the same
/// helper answers "true" for a group and "false" for a configuration.
fn has_intent(dict: &Dict, element: &[u8], default: &[u8], r: &impl Resolve) -> bool {
    let Some(intent) = dict.get(names::INTENT, r) else {
        return element == default;
    };
    match &*intent {
        Object::Array(array) => array.iter().any(|o| {
            let s = o.to_byte_string();
            s == b"All" || s == element
        }),
        other => {
            let s = other.to_byte_string();
            s == b"All" || s == element
        }
    }
}

/// Whether an array holds this exact dictionary.
fn contains_dict(array: &Array, target: &Dict, r: &impl Resolve) -> bool {
    array.iter().any(|o| {
        o.resolve(r)
            .ok()
            .and_then(|res| res.as_dict().cloned())
            .as_ref()
            == Some(target)
    })
}

#[cfg(test)]
mod tests {
    // Test fixtures quote the oracle's own vectors, compare floats exactly
    // where the behaviour being pinned is exact, and index arrays whose
    // length the fixture itself fixes.
    #![allow(
        clippy::unreadable_literal,
        clippy::float_cmp,
        clippy::indexing_slicing,
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        reason = "test fixtures quote oracle vectors verbatim and compare exactly"
    )]

    use super::{MAX_VE_DEPTH, OcContext, UsageType};
    use pdfrum_common::{DiagKind, Diagnostics};
    use pdfrum_object::{Array, Dict, Name, NoResolve, Object};

    fn ocg(name: &str) -> Dict {
        Dict::from_pairs([
            (Name::from("Type"), Object::Name(Name::from("OCG"))),
            (Name::from("Name"), Object::Name(Name::from(name))),
        ])
    }

    fn ocmd(pairs: Vec<(Name, Object)>) -> Dict {
        let mut d = Dict::from_pairs([(Name::from("Type"), Object::Name(Name::from("OCMD")))]);
        for (k, v) in pairs {
            d.push(k, v);
        }
        d
    }

    #[test]
    fn the_null_asymmetry_between_content_and_group_checks() {
        let mut ctx = OcContext::permissive();
        let mut diags = Diagnostics::default();
        // Content with no dictionary is unconditional content.
        assert!(ctx.content_visible(None, &NoResolve, &mut diags));
        // A group with no dictionary is invisible.
        assert!(!ctx.group_visible(None, &NoResolve));
    }

    #[test]
    fn a_group_with_no_configuration_is_visible() {
        let mut ctx = OcContext::permissive();
        let mut diags = Diagnostics::default();
        assert!(ctx.content_visible(Some(&ocg("Layer")), &NoResolve, &mut diags));
    }

    #[test]
    fn the_four_membership_policies() {
        let on = ocg("On");
        let mut diags = Diagnostics::default();
        for (policy, want) in [
            ("AnyOn", true),
            ("AllOn", true),
            ("AnyOff", false),
            ("AllOff", false),
        ] {
            let mut ctx = OcContext::permissive();
            let d = ocmd(vec![
                (Name::from("P"), Object::Name(Name::from(policy))),
                (
                    Name::from("OCGs"),
                    Object::Array(Array::of([Object::Dict(on.clone())])),
                ),
            ]);
            assert_eq!(
                ctx.content_visible(Some(&d), &NoResolve, &mut diags),
                want,
                "policy {policy}"
            );
        }
    }

    #[test]
    fn an_unknown_policy_with_a_valid_group_is_invisible() {
        let mut ctx = OcContext::permissive();
        let mut diags = Diagnostics::default();
        let d = ocmd(vec![
            (Name::from("P"), Object::Name(Name::from("SomeOn"))),
            (
                Name::from("OCGs"),
                Object::Array(Array::of([Object::Dict(ocg("On"))])),
            ),
        ]);
        assert!(!ctx.content_visible(Some(&d), &NoResolve, &mut diags));
        assert!(diags.contains(&DiagKind::OptionalContentPolicyUnknown));
    }

    #[test]
    fn a_membership_with_no_valid_groups_is_visible() {
        let mut ctx = OcContext::permissive();
        let mut diags = Diagnostics::default();
        let d = ocmd(vec![
            (Name::from("P"), Object::Name(Name::from("AllOn"))),
            (
                Name::from("OCGs"),
                Object::Array(Array::of([Object::Int(7), Object::Null])),
            ),
        ]);
        assert!(ctx.content_visible(Some(&d), &NoResolve, &mut diags));
    }

    #[test]
    fn a_single_group_dictionary_ignores_the_policy() {
        let mut ctx = OcContext::permissive();
        let mut diags = Diagnostics::default();
        let d = ocmd(vec![
            // A policy that would say "invisible" for an array.
            (Name::from("P"), Object::Name(Name::from("AllOff"))),
            (Name::from("OCGs"), Object::Dict(ocg("On"))),
        ]);
        assert!(ctx.content_visible(Some(&d), &NoResolve, &mut diags));
    }

    #[test]
    fn a_visibility_expression_takes_precedence_over_the_policy() {
        let mut ctx = OcContext::permissive();
        let mut diags = Diagnostics::default();
        let d = ocmd(vec![
            (Name::from("P"), Object::Name(Name::from("AnyOn"))),
            (
                Name::from("VE"),
                Object::Array(Array::of([
                    Object::Name(Name::from("Not")),
                    Object::Dict(ocg("On")),
                ])),
            ),
            (
                Name::from("OCGs"),
                Object::Array(Array::of([Object::Dict(ocg("On"))])),
            ),
        ]);
        // The group is visible, so `Not` makes the expression false — and the
        // `AnyOn` policy that would have said "visible" never runs.
        assert!(!ctx.content_visible(Some(&d), &NoResolve, &mut diags));
    }

    #[test]
    fn an_unknown_expression_operator_is_invisible() {
        let mut ctx = OcContext::permissive();
        let mut diags = Diagnostics::default();
        let d = ocmd(vec![(
            Name::from("VE"),
            Object::Array(Array::of([
                Object::Name(Name::from("Nand")),
                Object::Dict(ocg("On")),
            ])),
        )]);
        assert!(!ctx.content_visible(Some(&d), &NoResolve, &mut diags));
    }

    #[test]
    fn an_and_whose_first_operand_is_missing_is_false() {
        let mut ctx = OcContext::permissive();
        let mut diags = Diagnostics::default();
        // `And` with element 1 null: the seed never gets set, so the second
        // operand combines against `false`.
        let d = ocmd(vec![(
            Name::from("VE"),
            Object::Array(Array::of([
                Object::Name(Name::from("And")),
                Object::Null,
                Object::Dict(ocg("On")),
            ])),
        )]);
        assert!(!ctx.content_visible(Some(&d), &NoResolve, &mut diags));

        // An `Or` in the same shape still works.
        let d = ocmd(vec![(
            Name::from("VE"),
            Object::Array(Array::of([
                Object::Name(Name::from("Or")),
                Object::Null,
                Object::Dict(ocg("On")),
            ])),
        )]);
        assert!(ctx.content_visible(Some(&d), &NoResolve, &mut diags));
    }

    #[test]
    fn expressions_deeper_than_the_cap_are_invisible() {
        let mut ctx = OcContext::permissive();
        let mut diags = Diagnostics::default();
        // Build `[Not [Not [Not … group]]]` past the cap.
        let mut expr = Object::Dict(ocg("On"));
        for _ in 0..=MAX_VE_DEPTH + 1 {
            expr = Object::Array(Array::of([Object::Name(Name::from("Not")), expr]));
        }
        let d = ocmd(vec![(Name::from("VE"), expr)]);
        // Whether it comes out true or false, the point is that it
        // terminates rather than recursing forever.
        let _ = ctx.content_visible(Some(&d), &NoResolve, &mut diags);
    }

    #[test]
    fn usage_state_keys_are_built_from_the_usage_name() {
        assert_eq!(UsageType::View.state_key().as_bytes(), b"ViewState");
        assert_eq!(UsageType::Print.state_key().as_bytes(), b"PrintState");
        assert_eq!(UsageType::Export.as_bytes(), b"Export");
    }

    /// A store that hands back objects by number, so a `/VE` can point at
    /// itself and the evaluator has to survive it.
    struct Store(std::collections::HashMap<u32, std::sync::Arc<Object>>);

    impl pdfrum_object::Resolve for Store {
        fn fetch(
            &self,
            r: pdfrum_object::ObjRef,
        ) -> Result<std::sync::Arc<Object>, pdfrum_object::Error> {
            self.0
                .get(&r.num)
                .map(std::sync::Arc::clone)
                .ok_or(pdfrum_object::Error::UnresolvedRef(r))
        }
    }

    #[test]
    fn a_self_referencing_visibility_expression_terminates() {
        // `1 0 obj [/Not 1 0 R] endobj` — the expression's only operand is
        // the expression. Nothing in the *data* bounds this; only
        // `MAX_VE_DEPTH` does, and without it the evaluator would recurse
        // until the stack ran out on a file a fuzzer produces in seconds.
        let selfref = Object::Array(Array::of([
            Object::Name(Name::from("Not")),
            Object::Ref(pdfrum_object::ObjRef {
                num: 1,
                generation: 0,
            }),
        ]));
        let mut objects = std::collections::HashMap::new();
        objects.insert(1u32, std::sync::Arc::new(selfref.clone()));
        let store = Store(objects);

        let mut ctx = OcContext::permissive();
        let mut diags = Diagnostics::default();
        let d = ocmd(vec![(Name::from("VE"), selfref)]);
        // The answer itself is whatever the depth cap bottoms out at; that it
        // returns at all is the property under test.
        let _ = ctx.content_visible(Some(&d), &store, &mut diags);

        // A cycle through two objects is the same shape one step longer.
        let mut objects = std::collections::HashMap::new();
        objects.insert(
            1u32,
            std::sync::Arc::new(Object::Array(Array::of([
                Object::Name(Name::from("Not")),
                Object::Ref(pdfrum_object::ObjRef {
                    num: 2,
                    generation: 0,
                }),
            ]))),
        );
        objects.insert(
            2u32,
            std::sync::Arc::new(Object::Array(Array::of([
                Object::Name(Name::from("Not")),
                Object::Ref(pdfrum_object::ObjRef {
                    num: 1,
                    generation: 0,
                }),
            ]))),
        );
        let store = Store(objects);
        let mut ctx = OcContext::permissive();
        let d = ocmd(vec![(
            Name::from("VE"),
            Object::Ref(pdfrum_object::ObjRef {
                num: 1,
                generation: 0,
            }),
        )]);
        let _ = ctx.content_visible(Some(&d), &store, &mut diags);
    }

    // --- the pre-pass ---

    use crate::ops::MarkProperties;
    use crate::state::ContentMarks;
    use crate::{Content, PageObject, PathObject};

    fn off_group() -> Dict {
        // A group the default configuration turns off.
        Dict::from_pairs([
            (Name::from("Type"), Object::Name(Name::from("OCG"))),
            (Name::from("Name"), Object::Name(Name::from("Hidden"))),
        ])
    }

    /// A context whose default configuration switches `off` off.
    ///
    /// The catalog's own `/OCGs` has to list the group as well as the
    /// configuration's `/OFF`: `select_config` declines a group the catalog
    /// never declared, and a group nothing governs is visible.
    fn context_hiding(off: &Dict) -> OcContext {
        let properties = Dict::from_pairs([
            (
                Name::from("OCGs"),
                Object::Array(Array::of([Object::Dict(off.clone())])),
            ),
            (
                Name::from("D"),
                Object::Dict(Dict::from_pairs([(
                    Name::from("OFF"),
                    Object::Array(Array::of([Object::Dict(off.clone())])),
                )])),
            ),
        ]);
        OcContext::new(Some(properties), UsageType::View)
    }

    /// One `BDC /OC` mark, written either as a resource name or inline —
    /// which is the distinction visibility turns on.
    fn marked(oc: Option<&Dict>, from_resources: bool) -> ContentMarks {
        let mut marks = ContentMarks::new();
        if let Some(d) = oc {
            push_oc(&mut marks, d, from_resources);
        }
        marks
    }

    fn push_oc(marks: &mut ContentMarks, dict: &Dict, from_resources: bool) {
        let properties = if from_resources {
            MarkProperties::Named(Name::from("MC0"))
        } else {
            MarkProperties::Inline(Box::new(dict.clone()))
        };
        marks.push_with_properties(Name::from("OC"), &properties, |_| Some(dict.clone()));
    }

    fn path_with(marks: ContentMarks) -> PageObject {
        PageObject::Path(Box::new(Content {
            object: PathObject {
                path: kurbo::BezPath::new(),
                matrix: kurbo::Affine::IDENTITY,
                fill_rule: crate::FillRule::Winding,
                stroke: false,
            },
            state: crate::GraphicsState::default(),
            marks,
            content_stream: Some(0),
            dirty: false,
            active: true,
        }))
    }

    fn page_of(objects: Vec<PageObject>) -> crate::Page {
        crate::Page {
            objects,
            ..crate::Page::empty()
        }
    }

    #[test]
    fn a_page_with_no_optional_content_produces_an_empty_tree() {
        let page = page_of(vec![path_with(ContentMarks::new()); 3]);
        let mut ctx = OcContext::permissive();
        let mut diags = Diagnostics::default();
        let v = super::page_visibility(&page, &mut ctx, &NoResolve, &mut diags);
        assert!(
            v.shows_everything(),
            "an all-visible page collapses to nothing, so a renderer can skip \
             the descent entirely"
        );
        // And an absent entry still answers visible.
        assert!(v.visible(0));
        assert!(v.visible(99));
    }

    #[test]
    fn an_off_group_hides_the_object_its_mark_encloses() {
        let off = off_group();
        let page = page_of(vec![
            path_with(marked(None, false)),
            path_with(marked(Some(&off), true)),
            path_with(marked(None, false)),
        ]);
        let mut ctx = context_hiding(&off);
        let mut diags = Diagnostics::default();
        let v = super::page_visibility(&page, &mut ctx, &NoResolve, &mut diags);
        assert!(!v.shows_everything());
        assert!(v.visible(0));
        assert!(!v.visible(1), "the marked object is hidden");
        assert!(v.visible(2));
    }

    #[test]
    fn an_inline_property_list_never_hides_anything() {
        // `BDC /OC << … >>` written inline is ignored entirely — visibility
        // requires the properties to have come from the `/Properties`
        // resource (`kPropertiesDict`).
        let off = off_group();
        let page = page_of(vec![path_with(marked(Some(&off), false))]);
        let mut ctx = context_hiding(&off);
        let mut diags = Diagnostics::default();
        let v = super::page_visibility(&page, &mut ctx, &NoResolve, &mut diags);
        assert!(v.shows_everything());
    }

    #[test]
    fn every_enclosing_mark_gets_a_veto_not_just_the_innermost() {
        // `CheckPageObjectVisible` scans the whole mark stack, so an outer
        // sequence hides content an inner visible one is nested in.
        let off = off_group();
        let mut marks = marked(Some(&off), true);
        push_oc(&mut marks, &ocg("Shown"), true);
        let page = page_of(vec![path_with(marks)]);
        let mut ctx = context_hiding(&off);
        let mut diags = Diagnostics::default();
        let v = super::page_visibility(&page, &mut ctx, &NoResolve, &mut diags);
        assert!(!v.visible(0));
    }

    #[test]
    fn a_hidden_form_says_nothing_about_children_nobody_reaches() {
        let off = off_group();
        let form = PageObject::Form(Box::new(Content {
            object: crate::FormObject {
                objects: vec![path_with(ContentMarks::new())],
                matrix: kurbo::Affine::IDENTITY,
                bbox: None,
                transparency: crate::Transparency::default(),
                oc: Some(std::sync::Arc::new(off.clone())),
                source: None,
                live_edit: false,
            },
            state: crate::GraphicsState::default(),
            marks: ContentMarks::new(),
            content_stream: Some(0),
            dirty: false,
            active: true,
        }));
        let page = page_of(vec![form]);
        let mut ctx = context_hiding(&off);
        let mut diags = Diagnostics::default();
        let v = super::page_visibility(&page, &mut ctx, &NoResolve, &mut diags);
        assert!(!v.visible(0), "the form's own `/OC` hides it");
        assert!(
            v.children(0).shows_everything(),
            "and its children are not walked, because nothing reaches them"
        );
    }

    #[test]
    fn a_visible_forms_children_are_answered_in_their_own_frame() {
        let off = off_group();
        let form = PageObject::Form(Box::new(Content {
            object: crate::FormObject {
                objects: vec![
                    path_with(ContentMarks::new()),
                    path_with(marked(Some(&off), true)),
                ],
                matrix: kurbo::Affine::IDENTITY,
                bbox: None,
                transparency: crate::Transparency::default(),
                oc: None,
                source: None,
                live_edit: false,
            },
            state: crate::GraphicsState::default(),
            marks: ContentMarks::new(),
            content_stream: Some(0),
            dirty: false,
            active: true,
        }));
        let page = page_of(vec![form]);
        let mut ctx = context_hiding(&off);
        let mut diags = Diagnostics::default();
        let v = super::page_visibility(&page, &mut ctx, &NoResolve, &mut diags);
        assert!(v.visible(0), "the form itself is drawn");
        let inner = v.children(0);
        assert!(inner.visible(0));
        assert!(!inner.visible(1), "but its second child is not");
    }
}
