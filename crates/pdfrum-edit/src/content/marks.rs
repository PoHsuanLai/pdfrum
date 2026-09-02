//! Marked-content operators (ISO 32000-1 §14.6).
//!
//! Marks nest, and consecutive objects usually share a prefix of the nesting.
//! So rather than closing and reopening everything per object, the emitter
//! diffs against the previous object's marks: find the first level where they
//! differ, close everything from there down with `EMC`, and open the new ones.
//!
//! A mark's parameters take one of two spellings, and which one is not a
//! choice: a property list written inline in the original stream is written
//! inline again, while one that came from the `/Properties` resource is
//! written as `/Name BDC`. The two are not interchangeable — optional-content
//! visibility reads the *resource* entry, so inlining a named property list
//! would silently detach the mark from the layer controlling it.
//!
//! The `/Properties` resource itself is neither created nor preserved by the
//! resource sweep, an inconsistency inherited from the C++ and recorded here
//! rather than quietly fixed: emitting a resource the oracle's output does
//! not have would fail the comparison the sweep exists to pass.

use pdfrum_object::Name;
use pdfrum_page::state::{ContentMarks, Mark};

use crate::write::object::{write_dict, write_name};

/// How a mark's property list is named when it came from `/Properties`.
///
/// The parser resolved the name into a dictionary and did not keep the name,
/// so realizing the resource again is the caller's job — it is the same
/// caller that owns the resource dictionary. Returning `None` falls back to
/// writing the properties inline, which renders correctly but detaches the
/// mark from any optional-content group.
pub(crate) type PropertyNamer<'a> = &'a dyn Fn(&Mark) -> Option<Name>;

/// Emit the operators that turn `previous`'s marks into `current`'s.
///
/// Returns how many marks are open afterwards, which [`finish_marks`] needs
/// at the end of a stream.
pub(crate) fn emit_mark_diff(
    out: &mut String,
    previous: &ContentMarks,
    current: &ContentMarks,
    namer: PropertyNamer<'_>,
) -> usize {
    let shared = common_prefix(previous, current);

    // Close what the previous object opened below the shared prefix, deepest
    // first — `EMC` closes the innermost open mark.
    for _ in shared..previous.len() {
        out.push_str("EMC\n");
    }

    for mark in current.marks().get(shared..).unwrap_or_default() {
        emit_open(out, mark, namer);
    }
    current.len()
}

/// Close every mark still open at the end of a stream.
pub(crate) fn finish_marks(out: &mut String, open: usize) {
    for _ in 0..open {
        out.push_str("EMC\n");
    }
}

/// How many leading marks two lists share.
fn common_prefix(a: &ContentMarks, b: &ContentMarks) -> usize {
    a.marks()
        .iter()
        .zip(b.marks())
        .take_while(|(x, y)| same(x, y))
        .count()
}

/// Two marks are the same when their tag and their parameters agree.
fn same(a: &Mark, b: &Mark) -> bool {
    a.tag == b.tag
        && a.from_resources == b.from_resources
        && match (&a.properties, &b.properties) {
            (None, None) => true,
            (Some(x), Some(y)) => x == y,
            _ => false,
        }
}

/// One mark's opening operator.
fn emit_open(out: &mut String, mark: &Mark, namer: PropertyNamer<'_>) {
    let mut bytes = Vec::new();
    write_name(&mut bytes, &mark.tag);
    out.push_str(&String::from_utf8_lossy(&bytes));
    out.push(' ');

    let Some(dict) = mark.properties.as_ref() else {
        // No parameters at all: the plain form.
        out.push_str("BMC\n");
        return;
    };

    // A property list that lived in `/Properties` is named, not inlined —
    // optional-content visibility reads the resource entry.
    if mark.from_resources
        && let Some(name) = namer(mark)
    {
        let mut bytes = Vec::new();
        write_name(&mut bytes, &name);
        out.push_str(&String::from_utf8_lossy(&bytes));
        out.push_str(" BDC\n");
        return;
    }

    let mut bytes = Vec::new();
    write_dict(&mut bytes, dict, None);
    out.push_str(&String::from_utf8_lossy(&bytes));
    out.push_str(" BDC\n");
}

#[cfg(test)]
mod tests {
    use super::{emit_mark_diff, finish_marks};
    use pdfrum_object::{Dict, Name, Object};
    use pdfrum_page::MarkProperties;
    use pdfrum_page::state::ContentMarks;

    fn marks(specs: &[(&str, Option<Dict>)]) -> ContentMarks {
        let mut out = ContentMarks::new();
        for (tag, props) in specs {
            match props {
                None => out.push(Name::from(*tag)),
                Some(d) => {
                    out.push_with_properties(
                        Name::from(*tag),
                        &MarkProperties::Inline(Box::new(d.clone())),
                        |_| None,
                    );
                }
            }
        }
        out
    }

    /// Marks whose property lists came from `/Properties` rather than being
    /// written inline.
    fn named_marks(specs: &[(&str, &str, Dict)]) -> ContentMarks {
        let mut out = ContentMarks::new();
        for (tag, resource, dict) in specs {
            let dict = dict.clone();
            out.push_with_properties(
                Name::from(*tag),
                &MarkProperties::Named(Name::from(*resource)),
                move |_| Some(dict),
            );
        }
        out
    }

    fn diff(previous: &ContentMarks, current: &ContentMarks) -> String {
        let mut out = String::new();
        emit_mark_diff(&mut out, previous, current, &|_| None);
        out
    }

    #[test]
    fn a_mark_with_no_parameters_opens_with_bmc() {
        assert_eq!(
            diff(&ContentMarks::new(), &marks(&[("Span", None)])),
            "/Span BMC\n"
        );
    }

    // ProcessContentMarksWithProperties (:450-474): the tag is name-encoded,
    // so a space becomes `#20`.
    #[test]
    fn a_parameter_dictionary_opens_with_bdc_and_encodes_its_tag() {
        let props = Dict::from_pairs([(
            Name::from("Name"),
            Object::Name(Name::from("Property Name With Space")),
        )]);
        let out = diff(&ContentMarks::new(), &marks(&[("M1", Some(props))]));
        assert!(
            out.contains("/M1 <</Name/Property#20Name#20With#20Space>> BDC"),
            "got {out}"
        );
    }

    // The diff is what keeps nested marks from being torn down and rebuilt
    // between every pair of objects.
    #[test]
    fn a_shared_prefix_is_left_open() {
        let a = marks(&[("Outer", None), ("Inner", None)]);
        let b = marks(&[("Outer", None), ("Other", None)]);
        // Only the inner one closes and reopens.
        assert_eq!(diff(&a, &b), "EMC\n/Other BMC\n");
    }

    #[test]
    fn identical_marks_emit_nothing() {
        let a = marks(&[("Outer", None), ("Inner", None)]);
        assert_eq!(diff(&a, &a), "");
    }

    #[test]
    fn closing_everything_writes_one_emc_per_open_mark() {
        let a = marks(&[("A", None), ("B", None), ("C", None)]);
        assert_eq!(diff(&a, &ContentMarks::new()), "EMC\nEMC\nEMC\n");
    }

    #[test]
    fn opening_from_nothing_writes_no_emc() {
        let b = marks(&[("A", None), ("B", None)]);
        assert_eq!(diff(&ContentMarks::new(), &b), "/A BMC\n/B BMC\n");
    }

    // Two marks with the same tag but different parameters are different
    // marks, so the shared prefix stops before them.
    #[test]
    fn the_same_tag_with_different_parameters_is_a_different_mark() {
        let one = Dict::from_pairs([(Name::from("K"), Object::Int(1))]);
        let two = Dict::from_pairs([(Name::from("K"), Object::Int(2))]);
        let a = marks(&[("Span", Some(one))]);
        let b = marks(&[("Span", Some(two))]);
        assert!(diff(&a, &b).starts_with("EMC\n"));
    }

    #[test]
    fn finishing_closes_exactly_what_is_open() {
        let mut out = String::new();
        finish_marks(&mut out, 2);
        assert_eq!(out, "EMC\nEMC\n");
        let mut none = String::new();
        finish_marks(&mut none, 0);
        assert_eq!(none, "");
    }

    #[test]
    fn the_diff_reports_how_many_stay_open() {
        let mut out = String::new();
        let open = emit_mark_diff(
            &mut out,
            &ContentMarks::new(),
            &marks(&[("A", None), ("B", None)]),
            &|_| None,
        );
        assert_eq!(open, 2);
    }

    // A property list that came from `/Properties` is written as a *name*.
    // Inlining it would render the same and silently detach the mark from the
    // optional-content group the resource entry belongs to.
    #[test]
    fn a_resource_property_list_is_written_as_a_name() {
        let dict = Dict::from_pairs([(Name::from("MCID"), Object::Int(3))]);
        let current = named_marks(&[("OC", "MC0", dict)]);
        let mut out = String::new();
        emit_mark_diff(&mut out, &ContentMarks::new(), &current, &|_| {
            Some(Name::from("MC0"))
        });
        assert_eq!(out, "/OC /MC0 BDC\n");
    }

    // Without a name to realize it under, the properties go inline: it
    // renders, which is better than dropping the mark.
    #[test]
    fn an_unrealizable_resource_property_list_falls_back_to_inline() {
        let dict = Dict::from_pairs([(Name::from("MCID"), Object::Int(3))]);
        let current = named_marks(&[("OC", "MC0", dict)]);
        let mut out = String::new();
        emit_mark_diff(&mut out, &ContentMarks::new(), &current, &|_| None);
        assert_eq!(out, "/OC <</MCID 3>> BDC\n");
    }
}
