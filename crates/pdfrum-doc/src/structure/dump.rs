//! The `--show-structure` output format.
//!
//! This lives beside the tree rather than in `pdfrum-tool` because the
//! ordering rules *are* behavior: which fields print, in which order, at what
//! indent, and — the one that surprises — with **sorted** attribute keys. A
//! dictionary keeps insertion order everywhere else in this workspace, but
//! the API this dump goes through hands back the *i*-th key of a `std::map`,
//! so the golden files are alphabetical. Sorting happens here and at the one
//! other exposed site, never in storage.
//!
//! Indentation is `2·depth + 1` spaces: the format string writes `depth·2`
//! spaces through a width specifier and then one literal space of its own.

use std::fmt::Write as _;

use pdfrum_common::PageIndex;
use pdfrum_object::{Dict, Object, Resolve};

use crate::names;
use crate::structure::{StructTree, element};

/// Renders one page's structure tree.
///
/// A document with no tree contributes **nothing at all** — not even a
/// header — which is why most of the corpus's structure goldens are empty
/// files. A tree that loaded but reached no elements still prints its header
/// and the two trailing blank lines.
#[must_use]
pub fn render<R: Resolve>(
    tree: Option<&StructTree>,
    page_index: impl Into<PageIndex>,
    r: &R,
) -> String {
    let page_index = page_index.into();
    let Some(tree) = tree else {
        return String::new();
    };
    let mut out = format!("Structure Tree for Page {page_index}\n");
    for index in tree.top.iter().flatten() {
        dump_element(&mut out, tree, *index, 0, r);
    }
    out.push_str("\n\n");
    out
}

/// One element and its subtree.
fn dump_element<R: Resolve>(
    out: &mut String,
    tree: &StructTree,
    index: usize,
    depth: usize,
    r: &R,
) {
    let Some(element) = tree.elements.get(index) else {
        return;
    };
    let pad = " ".repeat(depth * 2);

    // 1. The structure type, after the role map.
    if !element.kind.is_empty() {
        let _ = writeln!(
            out,
            "{pad} S: {}",
            wide(&String::from_utf8_lossy(&element.kind))
        );
    }

    // 2. Attribute objects, each dumped two spaces further in.
    for (attr_index, attr) in attributes(&element.dict, r).into_iter().enumerate() {
        let _ = writeln!(out, "{pad} A[{attr_index}]:");
        dump_attribute(out, &attr, depth * 2 + 2, r);
    }

    // 3-6. The text fields, each suppressed when empty.
    for (label, value) in [
        ("ActualText", element.actual_text(r)),
        ("AltText", element.alt_text(r)),
    ] {
        if !value.is_empty() {
            let _ = writeln!(out, "{pad} {label}: {}", wide(&value));
        }
    }
    // Presence, not emptiness, is what gates these two: the length the
    // emitter tests is a UTF-16 **byte** count including the terminator, so
    // a present-but-empty `/ID` measures two and prints a bare `ID: `, while
    // an absent one measures zero and prints nothing.
    if let Some(id) = element.id() {
        let _ = writeln!(out, "{pad} ID: {}", wide(&decoded(&id)));
    }
    if let Some(lang) = element.lang() {
        let _ = writeln!(out, "{pad} Lang: {}", wide(&decoded(&lang)));
    }

    // 7. Marked-content identifiers, through the *unfiltered* accessor: the
    // dump reports content on other pages too, which the page-filtered kid
    // list deliberately does not.
    // No `/K` at all means no loop.
    for index in 0..element::marked_content_id_count(&element.dict, r).unwrap_or(0) {
        if let Some(id) = element::marked_content_id_at(&element.dict, index, r) {
            let _ = writeln!(out, "{pad} MCID{index}: {id}");
        }
    }

    // 8. The parent's identifier, only when the parent has one and it
    // carries an `/ID` key at all.
    if let Some(parent) = element.parent.and_then(|p| tree.elements.get(p))
        && let Some(id) = parent.id()
    {
        let _ = writeln!(out, "{pad} Parent ID: {}", wide(&decoded(&id)));
    }

    // 9-10. The title, then `/Type` — usually the literal `StructElem`.
    let title = element.title(r);
    if !title.is_empty() {
        let _ = writeln!(out, "{pad} Title: {}", wide(&title));
    }
    let obj_type = element.obj_type(r);
    if !obj_type.is_empty() {
        let _ = writeln!(
            out,
            "{pad} Type: {}",
            wide(&String::from_utf8_lossy(&obj_type))
        );
    }

    // 11. The kids, skipping the slots the upward walk never linked.
    for kid in &element.kids {
        if let crate::structure::Kid::Element {
            linked: Some(child),
            ..
        } = kid
        {
            dump_element(out, tree, *child, depth + 1, r);
        }
    }
}

/// The element's attribute objects.
///
/// `/A` resolves, an array contributes its dictionary-valued elements at
/// their own indices, a bare dictionary contributes itself, and anything else
/// contributes none — which is what makes the enclosing `for` loop's body
/// never run.
fn attributes<R: Resolve>(dict: &Dict, r: &R) -> Vec<Dict> {
    match dict.get(names::A, r).map(|a| a.get().clone()) {
        Some(Object::Array(array)) => (0..array.len())
            .filter_map(|index| array.dict_at(index, r))
            .collect(),
        Some(Object::Dict(attr)) => vec![attr],
        _ => Vec::new(),
    }
}

/// One attribute dictionary's keys and values, **alphabetically**.
fn dump_attribute<R: Resolve>(out: &mut String, attr: &Dict, indent: usize, r: &R) {
    let mut keys: Vec<_> = attr.keys().map(pdfrum_object::Name::as_bytes).collect();
    keys.sort_unstable();
    for key in keys {
        let name = pdfrum_object::Name::new(key.to_vec());
        let value = attr.get(&name, r).map(|v| v.get().clone());
        dump_value(out, &String::from_utf8_lossy(key), value.as_ref(), indent);
    }
}

/// One attribute value, per its type.
///
/// Takes no resolver: every value reaching it has already been resolved by
/// its caller, and an array's children are deliberately read **without**
/// resolving, which is why a reference inside one reports its type tag rather
/// than what it points at.
fn dump_value(out: &mut String, name: &str, value: Option<&Object>, indent: usize) {
    let pad = " ".repeat(indent);
    match value {
        Some(Object::Bool(flag)) => {
            let _ = writeln!(out, "{pad} {name}: {}", u8::from(*flag));
        }
        // Six decimals, always: an integer 2 prints as `2.000000`.
        Some(obj @ (Object::Int(_) | Object::Real(_))) => {
            let number = f64::from(obj.number().unwrap_or(0.0));
            let _ = writeln!(out, "{pad} {name}: {number:.6}");
        }
        Some(Object::Str(text)) => {
            let _ = writeln!(out, "{pad} {name}: {}", wide(&text.as_text()));
        }
        Some(Object::Name(named)) => {
            let _ = writeln!(out, "{pad} {name}: {}", wide(&named.as_text()));
        }
        // An array prints a bare label, then every child two spaces deeper
        // **under the same name** — so a `/BBox` of four numbers shows as
        // `BBox:` followed by four `BBox:` lines.
        Some(Object::Array(array)) => {
            let _ = writeln!(out, "{pad} {name}:");
            for index in 0..array.len() {
                // The child accessor does **not** resolve, so an array of
                // references dumps each element as a reference.
                dump_value(out, name, array.raw_at(index), indent + 2);
            }
        }
        None | Some(Object::Null) => {
            let _ = writeln!(out, "{pad} {name}: FPDF_OBJECT_UNKNOWN");
        }
        // A reference, stream or dictionary reaches the fall-through arm,
        // which reports the numeric type tag rather than the value.
        Some(other) => {
            let _ = writeln!(
                out,
                "{pad} {name}: NOT_YET_IMPLEMENTED: {}",
                type_tag(other)
            );
        }
    }
}

/// The public API's numeric type tag, which the fall-through arm prints.
fn type_tag(object: &Object) -> u8 {
    match object {
        Object::Null => 1,
        Object::Bool(_) => 2,
        Object::Int(_) | Object::Real(_) => 3,
        Object::Str(_) => 4,
        Object::Name(_) => 5,
        Object::Array(_) => 6,
        Object::Dict(_) => 7,
        Object::Stream(_) => 9,
        Object::Ref(_) => 8,
    }
}

/// Decodes a string the way the dump's text pipeline does.
fn decoded(bytes: &[u8]) -> String {
    pdfrum_object::decode_text(bytes).into_owned()
}

/// Truncates a string where the oracle's wide-character output gives up.
///
/// The path is UTF-16 → `wchar_t[]` → `printf("%ls")`, which ends at the
/// first **NUL** — a real code point a PDF string may legitimately contain —
/// and glibc additionally abandons the conversion at any unit with no scalar
/// value, cutting the line short there. Our strings are already
/// scalar-valued, so a replacement character stands where a lone surrogate
/// was; both it and a NUL end the line here.
fn wide(text: &str) -> String {
    match text.find(['\u{0}', '\u{FFFD}']) {
        Some(at) => text.get(..at).unwrap_or_default().to_owned(),
        None => text.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::{attributes, dump_attribute, dump_value, render, wide};
    use pdfrum_object::{Array, Dict, Name, NoResolve, Object, PdfString};

    fn dict(pairs: &[(&str, Object)]) -> Dict {
        Dict::from_pairs(
            pairs
                .iter()
                .map(|(k, v)| (Name::from(*k), v.clone()))
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn an_absent_tree_prints_nothing_at_all() {
        assert_eq!(render(None, 0, &NoResolve), "");
    }

    #[test]
    fn an_empty_tree_still_prints_its_header_and_two_blank_lines() {
        let tree = crate::structure::StructTree::default();
        assert_eq!(
            render(Some(&tree), 3, &NoResolve),
            "Structure Tree for Page 3\n\n\n"
        );
    }

    #[test]
    fn attribute_keys_print_alphabetically_whatever_the_file_says() {
        // Written Scope-then-O; the golden files are alphabetical.
        let attr = dict(&[
            ("Scope", Object::Name(Name::from("Row"))),
            ("O", Object::Name(Name::from("Table"))),
        ]);
        let mut out = String::new();
        dump_attribute(&mut out, &attr, 2, &NoResolve);
        assert_eq!(out, "   O: Table\n   Scope: Row\n");
    }

    #[test]
    fn a_number_always_prints_six_decimals() {
        let mut out = String::new();
        dump_value(&mut out, "ColSpan", Some(&Object::Int(2)), 0);
        assert_eq!(out, " ColSpan: 2.000000\n");
    }

    #[test]
    fn a_boolean_prints_as_one_or_zero() {
        let mut out = String::new();
        dump_value(&mut out, "CurUSD", Some(&Object::Bool(true)), 0);
        assert_eq!(out, " CurUSD: 1\n");
    }

    #[test]
    fn an_array_repeats_its_own_name_for_every_child() {
        let bbox = Object::Array(Array::of([Object::from(44.0_f32), Object::from(45.0_f32)]));
        let mut out = String::new();
        dump_value(&mut out, "BBox", Some(&bbox), 0);
        assert_eq!(out, " BBox:\n   BBox: 44.000000\n   BBox: 45.000000\n");
    }

    #[test]
    fn a_reference_child_reports_its_type_tag_rather_than_its_value() {
        let array = Object::Array(Array::of([Object::Ref(pdfrum_object::ObjRef::new(9, 0))]));
        let mut out = String::new();
        dump_value(&mut out, "RowSpan", Some(&array), 0);
        assert_eq!(out, " RowSpan:\n   RowSpan: NOT_YET_IMPLEMENTED: 8\n");
    }

    #[test]
    fn a_present_but_empty_string_attribute_still_prints_its_label() {
        let mut out = String::new();
        dump_value(
            &mut out,
            "Summary",
            Some(&Object::Str(PdfString::literal(b""))),
            0,
        );
        assert_eq!(out, " Summary: \n");
    }

    #[test]
    fn a_non_dict_attribute_contributes_no_indices() {
        assert!(attributes(&dict(&[("A", Object::Int(4))]), &NoResolve).is_empty());
        assert_eq!(
            attributes(&dict(&[("A", Object::Dict(Dict::new()))]), &NoResolve).len(),
            1
        );
    }

    #[test]
    fn a_string_stops_at_the_first_nul_or_unconvertible_unit() {
        assert_eq!(wide("plain"), "plain");
        assert_eq!(wide("cut\u{FFFD}here"), "cut");
        // A NUL is a real code point in a PDF string, and it ends the line.
        assert_eq!(wide("Hello!\u{0}"), "Hello!");
    }
}
