//! Page labels (ISO 32000-1 §12.4.2): the page numbers a reader sees, which
//! need not be `1, 2, 3`.
//!
//! `/PageLabels` is a number tree from a *starting* page index to a labelling
//! rule, and every page from there to the next entry follows that rule. So a
//! label is always a lower-bound lookup plus arithmetic, never a direct hit.
//!
//! Two "no label" answers exist and they differ. A page index outside the
//! document has **no label at all**. A page inside the document with no
//! matching tree entry gets its **one-based index as a decimal**, which is
//! also what happens when the entry exists but is not a dictionary.

use pdfrum_common::{DiagKind, Diagnostics, Limits, Severity};
use pdfrum_object::{Dict, Object, Resolve};

use crate::names;
use crate::nav::number_tree;

/// The label for one page, or nothing when the index is out of range.
#[must_use]
pub fn page_label<R: Resolve>(
    catalog: &Dict,
    page_index: i64,
    page_count: u32,
    r: &R,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> Option<String> {
    if page_index < 0 || page_index >= i64::from(page_count) {
        return None;
    }
    let labels = catalog.dict(names::PAGE_LABELS, r)?;
    let (key, value) = number_tree::lower_bound(&labels, page_index, r, limits, diags)?;

    let Some(rule) = value.as_ref().and_then(Object::as_dict) else {
        // The lower bound landed, but its value is absent or is not a
        // dictionary: the key is discarded and the decimal fallback applies.
        return Some((page_index + 1).to_string());
    };

    // A present-but-empty `/P` contributes an empty prefix, which is not the
    // same as an absent one only in that the key's presence is what is
    // tested — both end up empty here.
    let mut label = if rule.contains_key(names::P) {
        rule.text(names::P, r).unwrap_or_default()
    } else {
        String::new()
    };
    // `/S` is read coercively, so a *string* `(R)` selects upper-case roman.
    let style = rule.byte_string(names::S, r).unwrap_or_default();
    let number = page_index - key + rule.int(names::ST, r).unwrap_or(1);
    label.push_str(&num_portion(number, &style, diags));
    Some(label)
}

/// The numeric part of a label, per its style.
///
/// An unrecognized style — including an absent one — contributes **nothing**,
/// so the label is its prefix alone.
fn num_portion(number: i64, style: &[u8], diags: &mut Diagnostics) -> String {
    match style {
        b"D" => decimal(number),
        b"R" => roman(number).to_uppercase(),
        b"r" => roman(number),
        b"A" => letters(number).to_uppercase(),
        b"a" => letters(number),
        _ => {
            if !style.is_empty() {
                diags.record(Severity::Recovered, DiagKind::PageLabelStyleUnknown, None);
            }
            String::new()
        }
    }
}

/// A decimal number as a C `int` would print it.
fn decimal(number: i64) -> String {
    pdfrum_object::fmt_int(number)
}

/// Lower-case roman numerals by greedy subtraction.
///
/// The value is first reduced modulo a million — applied to the **signed**
/// value, so a large negative stays negative — and a number at or below zero
/// produces the empty string because the loop never runs.
fn roman(number: i64) -> String {
    const TABLE: [(i64, &str); 13] = [
        (1000, "m"),
        (900, "cm"),
        (500, "d"),
        (400, "cd"),
        (100, "c"),
        (90, "xc"),
        (50, "l"),
        (40, "xl"),
        (10, "x"),
        (9, "ix"),
        (5, "v"),
        (4, "iv"),
        (1, "i"),
    ];
    let mut remaining = number % 1_000_000;
    let mut out = String::new();
    for (value, glyph) in TABLE {
        if remaining <= 0 {
            break;
        }
        while remaining >= value {
            out.push_str(glyph);
            remaining -= value;
        }
    }
    out
}

/// Lower-case letter labels: `a`, …, `z`, `aa`, …, `zz`, `aaa`.
///
/// Two cliffs worth knowing. Zero and anything below produce the empty
/// string. And the repeat count is taken modulo a thousand, so a number that
/// would need exactly 1000 repeats — 26 000 — produces **nothing at all**.
fn letters(number: i64) -> String {
    if number <= 0 {
        return String::new();
    }
    let index = number - 1;
    let count = (index / 26 + 1) % 1000;
    if count <= 0 {
        return String::new();
    }
    let letter = char::from(b'a' + u8::try_from(index % 26).unwrap_or(0));
    std::iter::repeat_n(letter, usize::try_from(count).unwrap_or(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::{letters, page_label, roman};
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

    /// A catalog whose page labels are the given `(start, rule)` pairs.
    fn catalog(entries: &[(i64, Dict)]) -> Dict {
        let mut nums = Array::new();
        for (start, rule) in entries {
            nums.push(Object::Int(*start));
            nums.push(Object::Dict(rule.clone()));
        }
        dict(&[(
            "PageLabels",
            Object::Dict(dict(&[("Nums", Object::Array(nums))])),
        )])
    }

    fn label(catalog: &Dict, index: i64, count: u32) -> Option<String> {
        let (l, mut d) = (Limits::default(), Diagnostics::default());
        page_label(catalog, index, count, &NoResolve, &l, &mut d)
    }

    #[test]
    fn roman_numerals_cover_the_greedy_table_and_its_cliffs() {
        assert_eq!(roman(0), "");
        assert_eq!(roman(1), "i");
        assert_eq!(roman(4), "iv");
        assert_eq!(roman(38), "xxxviii");
        assert_eq!(roman(1900), "mcm");
        assert_eq!(roman(-5), "");
        // The modulus is applied first, so a million produces nothing.
        assert_eq!(roman(1_000_000), "");
        assert_eq!(roman(1_000_001), "i");
    }

    #[test]
    fn letter_labels_repeat_and_then_fall_off_a_cliff() {
        assert_eq!(letters(0), "");
        assert_eq!(letters(1), "a");
        assert_eq!(letters(26), "z");
        assert_eq!(letters(27), "aa");
        assert_eq!(letters(52), "zz");
        assert_eq!(letters(53), "aaa");
        assert_eq!(letters(-1), "");
        // The repeat count is taken modulo a thousand.
        assert_eq!(letters(26_000), "");
    }

    #[test]
    fn an_index_outside_the_document_has_no_label_while_one_inside_falls_back() {
        let empty = dict(&[("PageLabels", Object::Dict(Dict::new()))]);
        assert_eq!(label(&empty, -1, 7), None);
        assert_eq!(label(&empty, 7, 7), None);
        // In range with no tree entry: the one-based index, as a decimal.
        assert_eq!(label(&empty, 3, 7), None);

        let no_rule = catalog(&[(0, Dict::new())]);
        assert_eq!(label(&no_rule, 3, 7), Some(String::new()));
    }

    #[test]
    fn a_run_continues_until_the_next_entry() {
        let cat = catalog(&[
            (0, dict(&[("S", Object::Name(Name::from("r")))])),
            (2, dict(&[("S", Object::Name(Name::from("D")))])),
        ]);
        assert_eq!(label(&cat, 0, 7).as_deref(), Some("i"));
        assert_eq!(label(&cat, 1, 7).as_deref(), Some("ii"));
        assert_eq!(label(&cat, 2, 7).as_deref(), Some("1"));
        assert_eq!(label(&cat, 3, 7).as_deref(), Some("2"));
    }

    #[test]
    fn a_prefix_and_a_start_number_combine() {
        let cat = catalog(&[(
            0,
            dict(&[
                ("P", Object::Str(PdfString::literal(b"zz"))),
                ("S", Object::Name(Name::from("A"))),
                ("St", Object::Int(1)),
            ]),
        )]);
        assert_eq!(label(&cat, 0, 7).as_deref(), Some("zzA"));
        assert_eq!(label(&cat, 1, 7).as_deref(), Some("zzB"));
    }

    #[test]
    fn an_unrecognized_style_leaves_the_prefix_alone() {
        let cat = catalog(&[(0, dict(&[("P", Object::Str(PdfString::literal(b"x")))]))]);
        assert_eq!(label(&cat, 0, 7).as_deref(), Some("x"));
        let odd = catalog(&[(
            0,
            dict(&[
                ("P", Object::Str(PdfString::literal(b"x"))),
                ("S", Object::Name(Name::from("Q"))),
            ]),
        )]);
        assert_eq!(label(&odd, 0, 7).as_deref(), Some("x"));
    }

    #[test]
    fn a_string_valued_style_still_selects_a_numbering() {
        let cat = catalog(&[(0, dict(&[("S", Object::Str(PdfString::literal(b"R")))]))]);
        assert_eq!(label(&cat, 0, 7).as_deref(), Some("I"));
    }

    #[test]
    fn a_present_but_empty_label_is_not_the_same_as_none() {
        let cat = catalog(&[(0, dict(&[("P", Object::Str(PdfString::literal(b"")))]))]);
        assert_eq!(label(&cat, 0, 7), Some(String::new()));
        assert_eq!(label(&cat, 9, 7), None);
    }
}
