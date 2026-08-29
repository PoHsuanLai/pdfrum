//! Building a `/ToUnicode` CMap (ISO 32000-1 §9.10.3).
//!
//! Text extraction reads this map, so a subsetted font whose codes were
//! renumbered must ship a rebuilt one or its text stops being extractable —
//! which is the strongest guarantee that re-keying preserved meaning.
//!
//! # Three buckets, because the CMap syntax has three shapes
//!
//! - a **singleton** — one code, one string — goes in `beginbfchar`;
//! - a run of **consecutive codes with consecutive scalars** goes in
//!   `beginbfrange` naming only the first destination, which is by far the
//!   most compact form;
//! - a run of **consecutive codes with unrelated scalars** goes in
//!   `beginbfrange` with an explicit `[…]` of destinations.
//!
//! # A range may not cross a 256-code boundary
//!
//! §9.10.3 permits only the **low byte** of a code to vary within a range, so
//! a run is cut at every multiple of 256. A run that would start exactly on
//! such a boundary is degraded to two singletons rather than emitted as a
//! one-element range.
//!
//! Blocks are chunked at 100 entries, which is the limit the specification
//! sets on a single `bfchar`/`bfrange` section.

use std::collections::BTreeMap;
use std::fmt::Write as _;

/// The most entries one `begin…`/`end…` block may hold (§9.10.3).
const MAX_BLOCK: usize = 100;

/// The fixed prologue every `/ToUnicode` stream opens with.
const PROLOGUE: &str = "/CIDInit /ProcSet findresource begin\n\
12 dict begin\n\
begincmap\n\
/CIDSystemInfo\n\
<< /Registry (Adobe)\n\
/Ordering (Identity)\n\
/Supplement 0 >> def\n\
/CMapName /Adobe-Identity-H def\n\
/CMapType 2 def\n\
1 begincodespacerange\n\
<0000> <FFFF>\n\
endcodespacerange\n";

/// The fixed epilogue.
const EPILOGUE: &str = "endcmap\n\
CMapName currentdict /CMap defineresource pop\n\
end\n\
end\n";

/// Build a `/ToUnicode` CMap covering exactly `map`.
///
/// Keys are the codes the content stream uses — for a subsetted Identity-H
/// font, the **new** glyph IDs.
#[must_use]
pub fn to_unicode_cmap(map: &BTreeMap<u32, Vec<u32>>) -> Vec<u8> {
    let mut out = String::from(PROLOGUE);

    let entries: Vec<(u32, &Vec<u32>)> = map.iter().map(|(c, u)| (*c, u)).collect();
    let (singles, ranges) = classify(&entries);

    for block in singles.chunks(MAX_BLOCK) {
        let _ = writeln!(out, "{} beginbfchar", block.len());
        for (code, scalars) in block {
            let _ = writeln!(out, "<{code:04X}> <{}>", hex_scalars(scalars));
        }
        out.push_str("endbfchar\n");
    }

    for block in ranges.chunks(MAX_BLOCK) {
        let _ = writeln!(out, "{} beginbfrange", block.len());
        for range in block {
            match range {
                Range::Consecutive { start, end, first } => {
                    let _ = writeln!(
                        out,
                        "<{start:04X}> <{end:04X}> <{}>",
                        hex_scalars(std::slice::from_ref(first))
                    );
                }
                Range::Explicit { start, end, values } => {
                    let _ = write!(out, "<{start:04X}> <{end:04X}> [");
                    for (i, scalars) in values.iter().enumerate() {
                        if i > 0 {
                            out.push(' ');
                        }
                        let _ = write!(out, "<{}>", hex_scalars(scalars));
                    }
                    out.push_str("]\n");
                }
            }
        }
        out.push_str("endbfrange\n");
    }

    out.push_str(EPILOGUE);
    out.into_bytes()
}

/// One `bfrange` entry.
#[derive(Debug, PartialEq, Eq)]
enum Range {
    /// Consecutive codes mapping to consecutive scalars: name the first.
    Consecutive { start: u32, end: u32, first: u32 },
    /// Consecutive codes with unrelated destinations: list them.
    Explicit {
        start: u32,
        end: u32,
        values: Vec<Vec<u32>>,
    },
}

/// Split the entries into singletons and ranges.
fn classify<'a>(entries: &[(u32, &'a Vec<u32>)]) -> (Vec<(u32, &'a Vec<u32>)>, Vec<Range>) {
    let mut singles = Vec::new();
    let mut ranges = Vec::new();

    let mut i = 0usize;
    while i < entries.len() {
        let Some(&(start, scalars)) = entries.get(i) else {
            break;
        };
        // A range may only vary the low byte, so it can extend at most to the
        // end of this 256-code block.
        let room = 255u32.saturating_sub(start % 256);

        // A run of consecutive codes whose single scalars also run
        // consecutively is the most compact form.
        let consecutive = scalars.len() == 1;
        let mut run = i;
        while let Some(&(code, next)) = entries.get(run.saturating_add(1)) {
            let offset = step(run, i).saturating_add(1);
            if offset > room || code != start.saturating_add(offset) {
                break;
            }
            if consecutive {
                // Both sides must step by one.
                let Some(first) = scalars.first() else { break };
                if next.len() != 1 || next.first() != Some(&first.saturating_add(offset)) {
                    break;
                }
            }
            run = run.saturating_add(1);
        }

        if consecutive && run > i {
            let Some(first) = scalars.first().copied() else {
                i = i.saturating_add(1);
                continue;
            };
            ranges.push(Range::Consecutive {
                start,
                end: start.saturating_add(step(run, i)),
                first,
            });
            i = run.saturating_add(1);
            continue;
        }

        // Otherwise gather consecutive codes into an explicit range.
        let mut values = vec![scalars.clone()];
        let mut j = i.saturating_add(1);
        while let Some(&(code, next)) = entries.get(j) {
            let offset = step(j, i);
            if offset > room || code != start.saturating_add(offset) {
                break;
            }
            values.push(next.clone());
            j = j.saturating_add(1);
        }

        if values.len() == 1 {
            // A run starting on a 256-boundary with nothing to join it
            // degrades to a singleton rather than a one-element range.
            singles.push((start, scalars));
            i = i.saturating_add(1);
        } else {
            ranges.push(Range::Explicit {
                start,
                end: start.saturating_add(step(values.len(), 1)),
                values,
            });
            i = j;
        }
    }
    (singles, ranges)
}

/// Scalars as UTF-16BE hex digits.
///
/// A scalar outside the basic plane needs a surrogate pair; one that is
/// itself a surrogate half is not a character and is written as `0000`.
fn hex_scalars(scalars: &[u32]) -> String {
    let mut out = String::new();
    for scalar in scalars {
        match scalar {
            // A lone surrogate is not a scalar value; the CMap says nothing.
            0xD800..=0xDFFF => out.push_str("0000"),
            0..=0xFFFF => {
                let _ = write!(out, "{scalar:04X}");
            }
            _ => {
                let shifted = scalar.saturating_sub(0x1_0000);
                let high = 0xD800 + (shifted >> 10);
                let low = 0xDC00 + (shifted & 0x3FF);
                let _ = write!(out, "{high:04X}{low:04X}");
            }
        }
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
    use super::{hex_scalars, to_unicode_cmap};
    use std::collections::BTreeMap;

    fn cmap(pairs: &[(u32, &[u32])]) -> String {
        let map: BTreeMap<u32, Vec<u32>> = pairs.iter().map(|(c, u)| (*c, u.to_vec())).collect();
        String::from_utf8_lossy(&to_unicode_cmap(&map)).into_owned()
    }

    // TrueType (:398-428) asserts these three markers are present.
    #[test]
    fn the_prologue_and_epilogue_are_the_fixed_text() {
        let out = cmap(&[(1, &[0x41])]);
        assert!(out.contains("/CIDInit /ProcSet findresource begin"));
        assert!(out.contains("begincmap"));
        assert!(out.contains("endcmap"));
        assert!(out.contains("/CMapType 2 def"));
        assert!(out.contains("/CMapName /Adobe-Identity-H def"));
        assert!(out.contains("<0000> <FFFF>"));
    }

    // A lone code is a bfchar.
    #[test]
    fn a_singleton_is_a_bfchar() {
        let out = cmap(&[(1, &[0x41])]);
        assert!(
            out.contains("1 beginbfchar\n<0001> <0041>\nendbfchar"),
            "got {out}"
        );
    }

    // Consecutive codes with consecutive scalars name only the first.
    #[test]
    fn a_consecutive_run_names_only_its_first_destination() {
        let out = cmap(&[(1, &[0x41]), (2, &[0x42]), (3, &[0x43])]);
        assert!(
            out.contains("1 beginbfrange\n<0001> <0003> <0041>\nendbfrange"),
            "got {out}"
        );
    }

    // Consecutive codes with unrelated scalars list them.
    #[test]
    fn an_unrelated_run_lists_its_destinations() {
        let out = cmap(&[(1, &[0x41]), (2, &[0x5A]), (3, &[0x30])]);
        assert!(
            out.contains("<0001> <0003> [<0041> <005A> <0030>]"),
            "got {out}"
        );
    }

    // A gap splits the runs.
    #[test]
    fn a_gap_splits_the_run() {
        let out = cmap(&[(1, &[0x41]), (2, &[0x42]), (100, &[0x43])]);
        assert!(out.contains("<0001> <0002> <0041>"), "got {out}");
        assert!(out.contains("<0064> <0043>"), "got {out}");
    }

    // A range may only vary the low byte, so it is cut at 256.
    #[test]
    fn a_run_is_cut_at_a_256_boundary() {
        let pairs: Vec<(u32, Vec<u32>)> = (0x00FE..=0x0101)
            .map(|c| (c, vec![0x41 + c - 0x00FE]))
            .collect();
        let map: BTreeMap<u32, Vec<u32>> = pairs.into_iter().collect();
        let out = String::from_utf8_lossy(&to_unicode_cmap(&map)).into_owned();
        // The first range stops at 0x00FF; nothing spans the boundary.
        assert!(out.contains("<00FE> <00FF>"), "got {out}");
        assert!(!out.contains("<00FE> <0101>"), "got {out}");
    }

    // A run starting exactly on a boundary with only one member degrades to
    // a singleton.
    #[test]
    fn a_boundary_run_of_one_degrades_to_a_singleton() {
        let out = cmap(&[(0x00FF, &[0x41]), (0x0100, &[0x42])]);
        assert!(out.contains("beginbfchar"), "got {out}");
        assert!(!out.contains("<00FF> <0100>"), "got {out}");
    }

    // Blocks are chunked at 100 entries.
    #[test]
    fn blocks_are_chunked_at_one_hundred() {
        // 150 singletons: every other code, so nothing forms a range.
        let map: BTreeMap<u32, Vec<u32>> =
            (0..150u32).map(|i| (i * 2, vec![0x41 + i * 7])).collect();
        let out = String::from_utf8_lossy(&to_unicode_cmap(&map)).into_owned();
        assert!(out.contains("100 beginbfchar"), "got a full block");
        assert!(out.contains("50 beginbfchar"), "and the remainder");
    }

    // Scalars above the basic plane become surrogate pairs.
    #[test]
    fn an_astral_scalar_becomes_a_surrogate_pair() {
        assert_eq!(hex_scalars(&[0x1_F600]), "D83DDE00");
    }

    // A lone surrogate is not a character; it is written as zero.
    #[test]
    fn a_lone_surrogate_is_written_as_zero() {
        assert_eq!(hex_scalars(&[0xD800]), "0000");
        assert_eq!(hex_scalars(&[0xDFFF]), "0000");
    }

    // A code mapping to several scalars concatenates them.
    #[test]
    fn a_multi_scalar_destination_concatenates() {
        assert_eq!(hex_scalars(&[0x66, 0x69]), "00660069");
    }

    #[test]
    fn an_empty_map_still_produces_a_valid_cmap() {
        let out = cmap(&[]);
        assert!(out.contains("begincmap"));
        assert!(out.contains("endcmap"));
        assert!(!out.contains("beginbfchar"));
        assert!(!out.contains("beginbfrange"));
    }
}
