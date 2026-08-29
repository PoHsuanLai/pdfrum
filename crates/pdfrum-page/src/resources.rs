//! Named-resource lookup (ISO 32000-1 §7.8.3).
//!
//! # A whole-dictionary choice, then a one-level category fallback
//!
//! Two separate rules stack here, and conflating them gets real files wrong.
//!
//! First, the *dictionary* a form uses is the **first non-null** of its own
//! `/Resources`, its parent's, and the page's — a whole-dictionary choice,
//! not a per-category merge. A form with a `/Resources` containing only
//! `/Font` therefore cannot see the page's `/XObject` through this rule.
//!
//! Second, at lookup time a *category* missing from the chosen dictionary
//! falls back to the page's — but **only one level, and only to the page**. A
//! category that is present but lacks the name does **not** fall back: the
//! first dictionary holding the category wins, name or no name.

use crate::names;
use pdfrum_object::{Dict, Name, Object, Resolve};

/// The resource dictionaries a lookup consults.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Resources {
    /// The dictionary chosen by the whole-dictionary rule.
    pub chosen: Option<Dict>,
    /// The page's, for the one-level category fallback.
    pub page: Option<Dict>,
}

impl Resources {
    /// Choose the dictionary a form will use.
    ///
    /// The **first non-null** of the three, which is why a form with any
    /// `/Resources` at all stops seeing its parent's.
    #[must_use]
    pub fn choose(own: Option<Dict>, parent: Option<Dict>, page: Option<Dict>) -> Self {
        let chosen = own.or(parent).or_else(|| page.clone());
        Self { chosen, page }
    }

    /// A page's own resources, where the chosen dictionary and the page's are
    /// the same.
    #[must_use]
    pub fn for_page(page: Option<Dict>) -> Self {
        Self {
            chosen: page.clone(),
            page,
        }
    }

    /// The dictionary holding `category`, applying the one-level fallback.
    #[must_use]
    pub fn holder<R: Resolve>(&self, category: &Name, r: &R) -> Option<Dict> {
        if let Some(chosen) = &self.chosen
            && let Some(found) = chosen.dict(category, r)
        {
            // The first dictionary holding the category wins outright, even
            // when it lacks the name the caller wants.
            return Some(found);
        }
        // Only when the category is *absent* does the page get a turn — and
        // only when it is a different dictionary.
        let page = self.page.as_ref()?;
        if self.chosen.as_ref() == Some(page) {
            return None;
        }
        page.dict(category, r)
    }

    /// The resolved object a named resource denotes.
    #[must_use]
    pub fn find<R: Resolve>(&self, category: &Name, name: &Name, r: &R) -> Option<Object> {
        let holder = self.holder(category, r)?;
        Some(holder.get(name, r)?.into_owned())
    }

    /// The `/ColorSpace` dictionary, which colorspace loading needs by
    /// itself rather than by name.
    #[must_use]
    pub fn color_spaces<R: Resolve>(&self, r: &R) -> Option<Dict> {
        self.holder(names::COLOR_SPACE, r)
    }
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

    use super::Resources;
    use crate::names;
    use pdfrum_object::{Dict, Name, NoResolve, Object};

    fn category(key: &str, entry: &str) -> Dict {
        Dict::from_pairs([(
            Name::from(key),
            Object::Dict(Dict::from_pairs([(
                Name::from(entry),
                Object::Name(Name::from(entry)),
            )])),
        )])
    }

    #[test]
    fn the_first_non_null_dictionary_wins_whole() {
        let own = category("Font", "F1");
        let page = category("XObject", "X1");
        let res = Resources::choose(Some(own), None, Some(page));
        // The form's own `/Font` is visible.
        assert!(
            res.find(names::FONT, &Name::from("F1"), &NoResolve)
                .is_some()
        );
        // The page's `/XObject` is too, because the category is *absent*
        // from the chosen dictionary.
        assert!(
            res.find(names::XOBJECT, &Name::from("X1"), &NoResolve)
                .is_some()
        );
    }

    #[test]
    fn a_present_category_lacking_the_name_does_not_fall_back() {
        let mut own = category("Font", "F1");
        // The form has an `/XObject` category, but not the name wanted.
        own.push(
            Name::from("XObject"),
            Object::Dict(Dict::from_pairs([(
                Name::from("Other"),
                Object::Name(Name::from("Other")),
            )])),
        );
        let page = category("XObject", "X1");
        let res = Resources::choose(Some(own), None, Some(page));
        assert!(
            res.find(names::XOBJECT, &Name::from("X1"), &NoResolve)
                .is_none(),
            "a present-but-incomplete category must not fall back"
        );
    }

    #[test]
    fn the_parent_is_consulted_before_the_page() {
        let parent = category("Font", "PARENT");
        let page = category("Font", "PAGE");
        let res = Resources::choose(None, Some(parent), Some(page));
        assert!(
            res.find(names::FONT, &Name::from("PARENT"), &NoResolve)
                .is_some()
        );
        assert!(
            res.find(names::FONT, &Name::from("PAGE"), &NoResolve)
                .is_none()
        );
    }

    #[test]
    fn a_page_looks_up_only_in_itself() {
        let page = category("Font", "F1");
        let res = Resources::for_page(Some(page));
        assert!(
            res.find(names::FONT, &Name::from("F1"), &NoResolve)
                .is_some()
        );
        assert!(
            res.find(names::XOBJECT, &Name::from("X1"), &NoResolve)
                .is_none()
        );
    }

    #[test]
    fn no_resources_at_all_finds_nothing() {
        let res = Resources::default();
        assert!(
            res.find(names::FONT, &Name::from("F1"), &NoResolve)
                .is_none()
        );
        assert!(res.color_spaces(&NoResolve).is_none());
    }
}
