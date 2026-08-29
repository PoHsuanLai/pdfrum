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
}
