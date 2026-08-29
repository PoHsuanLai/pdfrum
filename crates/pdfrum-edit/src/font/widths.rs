//! The `/W` array of a `CIDFont` (ISO 32000-1 §9.7.4.3).
//!
//! `/W` has two forms and a good encoder uses both:
//!
//! - `c_first c_last w` — a run of consecutive CIDs that all share one width;
//! - `c [w1 w2 …]` — a run of consecutive CIDs with differing widths.
//!
//! The choice is per run: two or more consecutive CIDs sharing a width take
//! the first form, everything else gathers into the second. A single CID with
//! a unique width is a one-element array rather than a `c c w` triple,
//! because the array form costs fewer bytes at length one.
//!
//! # The keys are new glyph IDs, not char codes
//!
//! PDFium builds this map keyed by *char code*, which works for it because
//! `RETAIN_GIDS` leaves char code = CID = GID under Identity-H. Our subsetter
//! renumbers, so the CID a subsetted font uses is the **new** glyph ID and
//! the array must be keyed by that.

use std::collections::BTreeMap;

use pdfrum_object::{Array, Object};

/// Build a `/W` array from an ordered CID-to-width map.
///
/// Widths are in glyph space (1/1000 em), the same units `/W` always uses.
#[must_use]
#[expect(
    clippy::float_cmp,
    reason = "two glyphs share a width only when the width is the same \
              number; a tolerance would merge two widths into one and move \
              every glyph after them"
)]
pub fn widths_array(widths: &BTreeMap<u32, f32>) -> Array {
    let mut out = Array::new();
    let entries: Vec<(u32, f32)> = widths.iter().map(|(c, w)| (*c, *w)).collect();

    let mut i = 0usize;
    while i < entries.len() {
        let Some(&(start, width)) = entries.get(i) else {
            break;
        };

        // How far a run of consecutive CIDs sharing this width goes.
        let mut same = i;
        while let Some(&(code, w)) = entries.get(same.saturating_add(1)) {
            if code != start.saturating_add(step(same, i).saturating_add(1)) || w != width {
                break;
            }
            same = same.saturating_add(1);
        }

        if same > i {
            // Two or more sharing a width: the compact form.
            out.push(Object::Int(i64::from(start)));
            out.push(Object::Int(i64::from(start.saturating_add(step(same, i)))));
            out.push(Object::Real(width));
            i = same.saturating_add(1);
            continue;
        }

        // Otherwise gather consecutive CIDs into one array, stopping where a
        // gap appears or where a shared-width run would start.
        let mut run = vec![width];
        let mut j = i.saturating_add(1);
        while let Some(&(code, w)) = entries.get(j) {
            if code != start.saturating_add(step(j, i)) {
                break;
            }
            // A pair sharing a width ahead is better spelled as a triple, so
            // stop the array before it.
            if entries
                .get(j.saturating_add(1))
                .is_some_and(|&(next_code, next_w)| {
                    next_w == w && next_code == code.saturating_add(1)
                })
            {
                break;
            }
            run.push(w);
            j = j.saturating_add(1);
        }

        out.push(Object::Int(i64::from(start)));
        out.push(Object::Array(Array::of(run.into_iter().map(Object::Real))));
        i = j;
    }
    out
}

/// The distance between two indices, as the `u32` a code offset needs.
///
/// A run cannot be longer than the map that produced it, and a `/W` array or
/// a CMap holding more than 2^32 entries is not a thing a file contains — but
/// saturating is still the right answer, because a wrapped offset would name
/// the wrong glyph rather than an implausible one.
fn step(to: usize, from: usize) -> u32 {
    u32::try_from(to.saturating_sub(from)).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::widths_array;
    use pdfrum_object::{Array, Object};
    use std::collections::BTreeMap;

    fn build(pairs: &[(u32, f32)]) -> Vec<String> {
        spell(&widths_array(
            &pairs.iter().copied().collect::<BTreeMap<_, _>>(),
        ))
    }

    /// A readable spelling of the array, so the tests read like the spec's
    /// own examples.
    fn spell(a: &Array) -> Vec<String> {
        a.iter()
            .map(|o| match o {
                Object::Int(i) => i.to_string(),
                Object::Real(r) => pdfrum_object::fmt_number(*r),
                Object::Array(inner) => format!("[{}]", spell(inner).join(" ")),
                other => format!("{other:?}"),
            })
            .collect()
    }

    // Three consecutive CIDs sharing a width take the `c_first c_last w`
    // form.
    #[test]
    fn a_shared_width_run_becomes_a_triple() {
        assert_eq!(
            build(&[(1, 500.0), (2, 500.0), (3, 500.0)]),
            vec!["1", "3", "500"]
        );
    }

    // Differing widths gather into one array.
    #[test]
    fn differing_widths_gather_into_an_array() {
        assert_eq!(
            build(&[(1, 100.0), (2, 200.0), (3, 300.0)]),
            vec!["1", "[100 200 300]"]
        );
    }

    // A gap in the CIDs starts a new entry.
    #[test]
    fn a_gap_starts_a_new_entry() {
        assert_eq!(
            build(&[(1, 100.0), (2, 200.0), (10, 300.0)]),
            vec!["1", "[100 200]", "10", "[300]"]
        );
    }

    // A single CID is a one-element array, not a `c c w` triple: shorter.
    #[test]
    fn a_lone_cid_is_a_one_element_array() {
        assert_eq!(build(&[(7, 250.0)]), vec!["7", "[250]"]);
    }

    // The two forms interleave: an array stops where a shared run begins.
    #[test]
    fn the_two_forms_interleave() {
        assert_eq!(
            build(&[(1, 100.0), (2, 200.0), (3, 500.0), (4, 500.0), (5, 500.0),]),
            vec!["1", "[100 200]", "3", "5", "500"]
        );
    }

    // Exactly two sharing a width is still a triple.
    #[test]
    fn two_sharing_a_width_is_a_triple() {
        assert_eq!(build(&[(1, 500.0), (2, 500.0)]), vec!["1", "2", "500"]);
    }

    #[test]
    fn an_empty_map_is_an_empty_array() {
        assert!(build(&[]).is_empty());
    }

    // Fractional widths survive: `/W` is not integers-only.
    #[test]
    fn fractional_widths_are_kept() {
        assert_eq!(build(&[(1, 277.5)]), vec!["1", "[277.5]"]);
    }

    // Descending or duplicate keys cannot occur — the map orders them.
    #[test]
    fn the_map_orders_the_output_by_cid() {
        assert_eq!(
            build(&[(3, 300.0), (1, 100.0), (2, 200.0)]),
            vec!["1", "[100 200 300]"]
        );
    }
}
