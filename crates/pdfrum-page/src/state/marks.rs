//! Marked content: `BMC`, `BDC`, `EMC` (ISO 32000-1 §14.6).
//!
//! The stack carries a **sentinel** at the bottom that `EMC` can never pop,
//! so a content stream with more `EMC`s than `BDC`s is harmless rather than
//! corrupting.
//!
//! `BDC` is fussier than it looks: a null tag pushes **nothing at all**, and
//! a named property list that does not resolve through `/Properties` pushes
//! nothing either — so an unresolvable `BDC` leaves the mark stack untouched
//! and its matching `EMC` then pops a mark it did not push.

use crate::ops::MarkProperties;
use pdfrum_object::{Dict, Name};
use std::sync::Arc;

/// One marked-content entry.
#[derive(Debug, Clone, PartialEq)]
pub struct Mark {
    /// The tag naming the role, e.g. `/Span` or `/OC`.
    pub tag: Name,
    /// The property list, when the operator carried one that resolved.
    pub properties: Option<Arc<Dict>>,
    /// Whether the properties came from the `/Properties` resource rather
    /// than being written inline — which is what optional-content visibility
    /// requires.
    pub from_resources: bool,
}

impl Mark {
    /// The `/MCID` this mark's properties declare.
    #[must_use]
    pub fn content_id(&self) -> Option<i64> {
        self.properties
            .as_ref()
            .and_then(|d| d.direct_int(&Name::from("MCID")))
    }
}

/// The mark stack a page object is stamped with.
///
/// Cheap to clone: `Arc` inside, and page objects snapshot it at creation.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ContentMarks {
    marks: Vec<Mark>,
}

impl ContentMarks {
    /// An empty stack.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The marks, outermost first.
    #[must_use]
    pub fn marks(&self) -> &[Mark] {
        &self.marks
    }

    /// How many marks are open.
    #[must_use]
    pub fn len(&self) -> usize {
        self.marks.len()
    }

    /// Whether nothing is marked.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.marks.is_empty()
    }

    /// `BMC`: push a tag with no properties.
    pub fn push(&mut self, tag: Name) {
        self.marks.push(Mark {
            tag,
            properties: None,
            from_resources: false,
        });
    }

    /// `BDC`: push a tag with a property list.
    ///
    /// `resolve` is what turns a named property list into a dictionary; when
    /// it yields nothing, **no mark is pushed at all**.
    pub fn push_with_properties(
        &mut self,
        tag: Name,
        properties: &MarkProperties,
        resolve: impl FnOnce(&Name) -> Option<Dict>,
    ) -> bool {
        let (dict, from_resources) = match properties {
            MarkProperties::Named(name) => match resolve(name) {
                Some(d) => (d, true),
                // An unresolvable name pushes nothing.
                None => return false,
            },
            // An inline dictionary is stored as a clone.
            MarkProperties::Inline(d) => ((**d).clone(), false),
        };
        self.marks.push(Mark {
            tag,
            properties: Some(Arc::new(dict)),
            from_resources,
        });
        true
    }

    /// `EMC`: pop one mark, never past the sentinel.
    ///
    /// Answers whether anything was popped — a question, not a failed
    /// mutation — so a caller can record the imbalance.
    pub fn pop(&mut self) -> bool {
        self.marks.pop().is_some()
    }

    /// The `/MCID` of the **first** mark that declares one, or `None`.
    #[must_use]
    pub fn content_id(&self) -> Option<i64> {
        self.marks.iter().find_map(Mark::content_id)
    }

    /// The optional-content group this content belongs to, if any.
    ///
    /// Only a mark tagged exactly `OC` **whose properties came from the
    /// `/Properties` resource** counts — an inline `BDC /OC << … >>` is
    /// ignored entirely and its content stays visible.
    #[must_use]
    pub fn optional_content(&self) -> Option<&Dict> {
        self.optional_content_all().into_iter().next()
    }

    /// **Every** optional-content dictionary enclosing this content, outermost
    /// first.
    ///
    /// A visibility test scans the whole mark stack and any one entry can
    /// veto, so nested `BDC /OC` sequences each get a say — which the
    /// innermost alone does not capture. The same
    /// two conditions apply to each: the tag is exactly `OC`, and the
    /// properties came from the `/Properties` resource rather than being
    /// written inline.
    #[must_use]
    pub fn optional_content_all(&self) -> Vec<&Dict> {
        self.marks
            .iter()
            .filter(|m| m.tag.as_bytes() == b"OC" && m.from_resources)
            .filter_map(|m| m.properties.as_deref())
            .collect()
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

    use super::ContentMarks;
    use crate::ops::MarkProperties;
    use pdfrum_object::{Dict, Name, Object};

    fn mcid(n: i64) -> Dict {
        Dict::from_pairs([(Name::from("MCID"), Object::Int(n))])
    }

    #[test]
    fn emc_never_pops_past_the_bottom() {
        let mut marks = ContentMarks::new();
        // Every pop on an empty stack simply reports nothing happened.
        assert!(!marks.pop());
        assert!(!marks.pop());
        assert!(marks.is_empty());
        marks.push(Name::from("Span"));
        assert!(marks.pop());
        assert!(!marks.pop());
    }

    #[test]
    fn an_unresolvable_named_property_list_pushes_nothing() {
        let mut marks = ContentMarks::new();
        let pushed = marks.push_with_properties(
            Name::from("OC"),
            &MarkProperties::Named(Name::from("MC0")),
            |_| None,
        );
        assert!(!pushed);
        assert!(marks.is_empty());
    }

    #[test]
    fn the_first_mark_with_an_mcid_wins() {
        let mut marks = ContentMarks::new();
        marks.push(Name::from("Span"));
        marks.push_with_properties(
            Name::from("P"),
            &MarkProperties::Inline(Box::new(mcid(7))),
            |_| None,
        );
        marks.push_with_properties(
            Name::from("Span"),
            &MarkProperties::Inline(Box::new(mcid(9))),
            |_| None,
        );
        assert_eq!(marks.content_id(), Some(7));
    }

    #[test]
    fn an_inline_oc_dictionary_is_ignored_for_visibility() {
        let mut marks = ContentMarks::new();
        // Inline: not from `/Properties`, so it does not count.
        marks.push_with_properties(
            Name::from("OC"),
            &MarkProperties::Inline(Box::new(Dict::new())),
            |_| None,
        );
        assert!(marks.optional_content().is_none());

        // The same tag resolved through `/Properties` does count.
        let mut marks = ContentMarks::new();
        marks.push_with_properties(
            Name::from("OC"),
            &MarkProperties::Named(Name::from("MC0")),
            |_| Some(Dict::new()),
        );
        assert!(marks.optional_content().is_some());
    }

    #[test]
    fn only_the_exact_oc_tag_counts() {
        let mut marks = ContentMarks::new();
        marks.push_with_properties(
            Name::from("OCX"),
            &MarkProperties::Named(Name::from("MC0")),
            |_| Some(Dict::new()),
        );
        assert!(marks.optional_content().is_none());
    }
}
