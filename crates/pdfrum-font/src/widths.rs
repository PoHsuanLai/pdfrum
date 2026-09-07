//! Advance widths: `/Widths` for simple fonts, `/W` and `/W2` for CID fonts.
//!
//! Two unrelated formats sharing a module because they answer the same
//! question. Both have damage behaviors that real files depend on.

use crate::names;
use pdfrum_cmap::{CharCode, Cid};
use pdfrum_common::{DiagKind, Diagnostics, Severity};
use pdfrum_object::{Array, Dict, Object, Resolve};

/// The sentinel a simple font's width table holds for "not declared".
///
/// Nonzero on purpose: the all-caps aliasing tests `width != 0` and
/// therefore propagates an *unset* width, which a zero sentinel would not.
///
/// `pub(crate)` rather than `pub`: it is read by the ladders that build the
/// table and by the all-caps aliasing, all inside this crate.
/// [`SimpleWidths::raw`] is private and [`SimpleWidths::get`] is the only
/// way in.
pub(crate) const WIDTH_UNSET: u16 = 0xffff;

/// A simple font's 256 advance widths, as declared.
///
/// `Some(w)` is a declared width; `None` means the face's own metric answers
/// instead.
#[derive(Debug, Clone, PartialEq)]
pub struct SimpleWidths {
    /// Raw values, `WIDTH_UNSET` where nothing was declared.
    pub(crate) raw: [u16; 256],
    /// Whether the PDF declared no `/Widths` at all, so face metrics govern.
    ///
    /// PDFium calls this `use_font_width_`; its negation is `HasFontWidths()`,
    /// which the glyph-spacing heuristic reads.
    pub use_face_widths: bool,
}

impl Default for SimpleWidths {
    fn default() -> Self {
        Self {
            raw: [WIDTH_UNSET; 256],
            use_face_widths: true,
        }
    }
}

impl SimpleWidths {
    /// The declared width for a code, or `None` to defer to the face.
    #[must_use]
    pub fn get(&self, code: u8) -> Option<f32> {
        match self.raw.get(usize::from(code)) {
            Some(&WIDTH_UNSET) | None => None,
            Some(&w) => Some(f32::from(w)),
        }
    }

    /// Whether the PDF declared widths, which is what the glyph-spacing
    /// heuristic of §1.15 gates on.
    #[must_use]
    pub fn has_declared_widths(&self) -> bool {
        !self.use_face_widths
    }
}

/// Read `/Widths`, `/FirstChar`, `/LastChar` and `/MissingWidth`.
///
/// Four behaviors reproduce PDFium exactly:
///
/// - **`/MissingWidth` fills the whole table**, including codes outside
///   `[FirstChar, LastChar]`, which then get the array value on top.
/// - **A `/LastChar` of 0 means "absent"** and is recomputed from the array's
///   length — so `/FirstChar 0 /LastChar 0 /Widths [500]` covers exactly code
///   0, by a different route to the same answer.
/// - **A `/LastChar` past the array is clamped**; one short of it is honoured,
///   leaving the tail of `/Widths` unread.
/// - **A `/FirstChar` outside 0..=255 drops every width silently.** PDFium
///   reads it into a `size_t`, so a negative value becomes enormous and trips
///   the same guard.
#[must_use]
pub fn load_simple(font_dict: &Dict, desc: Option<&Dict>, r: &impl Resolve) -> SimpleWidths {
    let Some(widths) = font_dict.array(names::WIDTHS, r) else {
        // No `/Widths` at all: the face's metrics answer every query.
        return SimpleWidths::default();
    };
    let mut w = SimpleWidths {
        raw: [WIDTH_UNSET; 256],
        use_face_widths: false,
    };

    if let Some(missing) = desc.and_then(|d| d.int(names::MISSING_WIDTH, r)) {
        w.raw = [missing as u16; 256];
    }

    let start = font_dict.int(names::FIRST_CHAR, r).unwrap_or(0);
    // Outside 0..=255 — including any negative value — drops everything.
    let Ok(start) = usize::try_from(start) else {
        return w;
    };
    if start > 255 {
        return w;
    }

    let declared_end = font_dict.int(names::LAST_CHAR, r).unwrap_or(0);
    let array_end = start.saturating_add(widths.len()).saturating_sub(1);
    let mut end = match usize::try_from(declared_end) {
        // Zero is "absent", and anything past the array is clamped to it.
        Ok(e) if e != 0 && e < start.saturating_add(widths.len()) => e,
        Ok(_) | Err(_) => array_end,
    };
    end = end.min(255);

    for code in start..=end {
        // A missing or non-numeric element reads as 0, not as an error.
        let value = widths.int_at(code - start).unwrap_or(0);
        if let Some(slot) = w.raw.get_mut(code) {
            *slot = value as u16;
        }
    }
    w
}

/// A CID font's horizontal widths: the `/W` records and the `/DW` default.
#[derive(Debug, Clone, PartialEq)]
pub struct CidWidths {
    /// `[low, high, width]` records in **file order**, which is the order the
    /// lookup scans — the earliest overlapping record wins.
    records: Vec<[i32; 3]>,
    /// `/DW`, defaulting to 1000.
    default: i32,
    /// Set by the GB2312 rescue path: codes below 0x80 get fixed widths.
    ansi_widths_fixed: bool,
}

impl Default for CidWidths {
    fn default() -> Self {
        Self {
            records: Vec::new(),
            default: 1000,
            ansi_widths_fixed: false,
        }
    }
}

impl CidWidths {
    /// Read `/W` and `/DW` from a descendant font dictionary.
    #[must_use]
    pub fn load(cid_dict: &Dict, r: &impl Resolve, diags: &mut Diagnostics) -> Self {
        let default = cid_dict
            .int(names::DW, r)
            .and_then(|v| i32::try_from(v).ok())
            .unwrap_or(1000);
        let records = match cid_dict.array(names::W, r) {
            Some(a) => parse_metrics(&a, 1, r, diags)
                .chunks_exact(3)
                .filter_map(|c| Some([*c.first()?, *c.get(1)?, *c.get(2)?]))
                .collect(),
            None => Vec::new(),
        };
        Self {
            records,
            default,
            ansi_widths_fixed: false,
        }
    }

    /// Mark this font as taking fixed widths for ASCII, which the GB2312
    /// rescue path of §1.10.1 sets.
    pub fn set_ansi_widths_fixed(&mut self) {
        self.ansi_widths_fixed = true;
    }

    /// The advance width for a CID, in 1000/em units.
    ///
    /// A **first-match, file-order** linear scan: `/W` records are neither
    /// sorted nor required to be disjoint, and the earliest one that covers
    /// the CID wins — not the last, and not the tightest.
    #[must_use]
    pub fn width(&self, code: CharCode, cid: Cid) -> f32 {
        if self.ansi_widths_fixed && code.0 < 0x80 {
            return if (32..127).contains(&code.0) {
                500.0
            } else {
                0.0
            };
        }
        let cid = i32::from(cid.0);
        for rec in &self.records {
            let (Some(&low), Some(&high), Some(&val)) = (rec.first(), rec.get(1), rec.get(2))
            else {
                continue;
            };
            if low <= cid && cid <= high {
                return val as f32;
            }
        }
        self.default as f32
    }

    /// `/DW`.
    #[must_use]
    #[cfg(test)]
    pub fn default_width(&self) -> f32 {
        self.default as f32
    }

    /// How many `/W` records were read.
    #[cfg(test)]
    #[must_use]
    pub fn len(&self) -> usize {
        self.records.len()
    }
}

/// A CID font's vertical metrics: `/W2` records and the `/DW2` defaults.
#[derive(Debug, Clone, PartialEq)]
pub struct VerticalMetrics {
    /// `[low, high, w1, vx, vy]` records, scanned first-match like `/W`.
    records: Vec<[i32; 5]>,
    /// `DW2[0]`: the default vertical origin's y, ISO 32000-1's 880.
    default_vy: i32,
    /// `DW2[1]`: the default vertical advance, ISO 32000-1's -1000.
    default_w1: i32,
}

impl Default for VerticalMetrics {
    fn default() -> Self {
        Self {
            records: Vec::new(),
            default_vy: 880,
            default_w1: -1000,
        }
    }
}

impl VerticalMetrics {
    /// Read `/W2` and `/DW2`.
    #[must_use]
    pub fn load(cid_dict: &Dict, r: &impl Resolve, diags: &mut Diagnostics) -> Self {
        let mut v = Self::default();
        if let Some(a) = cid_dict.array(names::W2, r) {
            v.records = parse_metrics(&a, 3, r, diags)
                .chunks_exact(5)
                .filter_map(|c| Some([*c.first()?, *c.get(1)?, *c.get(2)?, *c.get(3)?, *c.get(4)?]))
                .collect();
        }
        if let Some(dw2) = cid_dict.array(names::DW2, r) {
            if let Some(vy) = dw2.int_at(0).and_then(|v| i32::try_from(v).ok()) {
                v.default_vy = vy;
            }
            if let Some(w1) = dw2.int_at(1).and_then(|v| i32::try_from(v).ok()) {
                v.default_w1 = w1;
            }
        }
        v
    }

    /// The vertical advance for a CID.
    #[must_use]
    pub fn width(&self, cid: Cid) -> f32 {
        let cid = i32::from(cid.0);
        for rec in &self.records {
            if let (Some(&low), Some(&high), Some(&w1)) = (rec.first(), rec.get(1), rec.get(2))
                && low <= cid
                && cid <= high
            {
                return w1 as f32;
            }
        }
        self.default_w1 as f32
    }

    /// The vertical origin for a CID, in 1000/em units.
    ///
    /// Absent a `/W2` record the origin is **half the horizontal width** and
    /// `DW2[0]` — which is why this needs the horizontal table too.
    #[must_use]
    pub fn origin(&self, cid: Cid, horizontal: &CidWidths) -> (f32, f32) {
        let icid = i32::from(cid.0);
        for rec in &self.records {
            if let (Some(&low), Some(&high), Some(&vx), Some(&vy)) =
                (rec.first(), rec.get(1), rec.get(3), rec.get(4))
                && low <= icid
                && icid <= high
            {
                // The C++ narrows both to `int16_t` before returning.
                return (f32::from(vx as i16), f32::from(vy as i16));
            }
        }
        let width = horizontal.width(CharCode(u32::from(cid.0)), cid);
        ((width / 2.0).trunc(), self.default_vy as f32)
    }

    /// `DW2[0]`, the default vertical origin's y coordinate.
    #[cfg(test)]
    #[must_use]
    pub fn default_origin_y(&self) -> f32 {
        self.default_vy as f32
    }

    /// `DW2[1]`, the default vertical advance.
    #[cfg(test)]
    #[must_use]
    pub fn default_advance(&self) -> f32 {
        self.default_w1 as f32
    }

    /// How many `/W2` records were read.
    #[cfg(test)]
    #[must_use]
    pub fn len(&self) -> usize {
        self.records.len()
    }
}

/// The shared `/W` and `/W2` reader (`LoadMetricsArray`, §1.10.2).
///
/// One state machine over two interleaved forms — `c [w …]` and `c1 c2 w …` —
/// distinguished purely by whether the current element is an array. Three
/// damage behaviors are load-bearing:
///
/// - **An array arriving in the wrong state aborts the whole parse**, keeping
///   everything read so far and reading nothing further.
/// - **The overflow guard continues** rather than aborting, so the offending
///   array's contents are lost but later elements still parse.
/// - **A trailing partial group is dropped** by the final chunking, not by the
///   loop, which is why the flat buffer is chunked rather than built as
///   records.
fn parse_metrics(
    array: &Array,
    values_per_record: usize,
    r: &impl Resolve,
    diags: &mut Diagnostics,
) -> Vec<i32> {
    /// Reading `c`, expecting a first code next.
    const EXPECT_FIRST: u8 = 0;
    /// Read `c`, expecting either an array or a last code.
    const EXPECT_LAST_OR_ARRAY: u8 = 1;
    /// Read `c1 c2`, collecting values.
    const COLLECT_VALUES: u8 = 2;

    let mut out: Vec<i32> = Vec::new();
    let mut state = EXPECT_FIRST;
    let mut cur_element = 0usize;
    let mut first_code: i32 = 0;
    let mut last_code: i32 = 0;

    for i in 0..array.len() {
        let Some(resolved) = array.get(i, r) else {
            continue;
        };
        let Some(obj) = resolved.as_direct() else {
            continue;
        };
        if let Object::Array(inner) = obj {
            if state != EXPECT_LAST_OR_ARRAY {
                // Abort: keep what was parsed, read nothing more.
                diags.record(Severity::Suspicious, DiagKind::FontWidthsTruncated, None);
                return out;
            }
            let len = i32::try_from(inner.len()).unwrap_or(i32::MAX);
            if first_code > i32::MAX - len {
                // Drop this array's contents but keep parsing.
                state = EXPECT_FIRST;
                diags.record(Severity::Suspicious, DiagKind::FontWidthsTruncated, None);
                continue;
            }
            let mut j = 0;
            while j < inner.len() {
                out.push(first_code);
                out.push(first_code);
                for k in 0..values_per_record {
                    // Reading past the end yields 0 rather than failing, so a
                    // trailing partial group produces a record of zeros.
                    out.push(inner.int_at(j + k).unwrap_or(0) as i32);
                }
                first_code += 1;
                j += values_per_record;
            }
            state = EXPECT_FIRST;
        } else {
            let value = obj.as_int().unwrap_or(0) as i32;
            match state {
                EXPECT_FIRST => {
                    first_code = value;
                    state = EXPECT_LAST_OR_ARRAY;
                }
                EXPECT_LAST_OR_ARRAY => {
                    last_code = value;
                    state = COLLECT_VALUES;
                    cur_element = 0;
                }
                _ => {
                    if cur_element == 0 {
                        out.push(first_code);
                        out.push(last_code);
                    }
                    out.push(value);
                    cur_element += 1;
                    if cur_element == values_per_record {
                        state = EXPECT_FIRST;
                    }
                }
            }
        }
    }
    // A record left mid-collection is truncated whether or not a value had
    // been pushed yet: `c1 c2` with the array ending is the common shape, and
    // the final chunking drops the partial record either way.
    if state != EXPECT_FIRST {
        diags.record(Severity::Suspicious, DiagKind::FontWidthsTruncated, None);
    }
    out
}

#[cfg(test)]
mod tests {
    // Test expectations are exact values by design.
    #![allow(clippy::float_cmp)]
    use super::*;
    use pdfrum_object::{Name, NoResolve};

    fn nums(v: &[i64]) -> Array {
        Array::of(v.iter().copied().map(Object::Int))
    }

    fn font_dict(pairs: Vec<(&Name, Object)>) -> Dict {
        Dict::from_pairs(pairs.into_iter().map(|(k, v)| (k.clone(), v)))
    }

    fn quiet() -> Diagnostics {
        Diagnostics::default()
    }

    // ---- /Widths ----------------------------------------------------------

    #[test]
    fn no_widths_array_means_the_face_answers() {
        let w = load_simple(&Dict::new(), None, &NoResolve);
        assert!(w.use_face_widths);
        assert!(!w.has_declared_widths());
        assert_eq!(w.get(65), None);
    }

    #[test]
    fn widths_are_indexed_from_first_char() {
        let d = font_dict(vec![
            (names::FIRST_CHAR, Object::Int(65)),
            (names::LAST_CHAR, Object::Int(67)),
            (names::WIDTHS, Object::Array(nums(&[500, 600, 700]))),
        ]);
        let w = load_simple(&d, None, &NoResolve);
        assert_eq!(w.get(65), Some(500.0));
        assert_eq!(w.get(66), Some(600.0));
        assert_eq!(w.get(67), Some(700.0));
        assert_eq!(w.get(68), None);
        assert!(w.has_declared_widths());
    }

    #[test]
    fn a_last_char_of_zero_is_recomputed_from_the_array() {
        let d = font_dict(vec![
            (names::FIRST_CHAR, Object::Int(0)),
            (names::LAST_CHAR, Object::Int(0)),
            (names::WIDTHS, Object::Array(nums(&[500]))),
        ]);
        let w = load_simple(&d, None, &NoResolve);
        assert_eq!(w.get(0), Some(500.0));
        assert_eq!(w.get(1), None);

        // With a longer array the recompute really does extend the range.
        let d = font_dict(vec![
            (names::FIRST_CHAR, Object::Int(10)),
            (names::LAST_CHAR, Object::Int(0)),
            (names::WIDTHS, Object::Array(nums(&[1, 2, 3]))),
        ]);
        let w = load_simple(&d, None, &NoResolve);
        assert_eq!(w.get(10), Some(1.0));
        assert_eq!(w.get(12), Some(3.0));
        assert_eq!(w.get(13), None);
    }

    #[test]
    fn a_last_char_past_the_array_is_clamped_to_it() {
        let d = font_dict(vec![
            (names::FIRST_CHAR, Object::Int(65)),
            (names::LAST_CHAR, Object::Int(200)),
            (names::WIDTHS, Object::Array(nums(&[500, 600]))),
        ]);
        let w = load_simple(&d, None, &NoResolve);
        assert_eq!(w.get(66), Some(600.0));
        assert_eq!(w.get(67), None);
    }

    #[test]
    fn a_last_char_short_of_the_array_leaves_the_tail_unread() {
        let d = font_dict(vec![
            (names::FIRST_CHAR, Object::Int(65)),
            (names::LAST_CHAR, Object::Int(65)),
            (names::WIDTHS, Object::Array(nums(&[500, 600, 700]))),
        ]);
        let w = load_simple(&d, None, &NoResolve);
        assert_eq!(w.get(65), Some(500.0));
        assert_eq!(w.get(66), None);
    }

    #[test]
    fn a_first_char_above_255_drops_every_width() {
        for first in [256, 1000, -1, -100] {
            let d = font_dict(vec![
                (names::FIRST_CHAR, Object::Int(first)),
                (names::WIDTHS, Object::Array(nums(&[500]))),
            ]);
            let w = load_simple(&d, None, &NoResolve);
            assert!(w.raw.iter().all(|&x| x == WIDTH_UNSET), "first {first}");
            // But `use_face_widths` is still false: a /Widths key was present.
            assert!(w.has_declared_widths(), "first {first}");
        }
    }

    #[test]
    fn missing_width_fills_the_whole_table_including_outside_the_range() {
        let d = font_dict(vec![
            (names::FIRST_CHAR, Object::Int(65)),
            (names::LAST_CHAR, Object::Int(66)),
            (names::WIDTHS, Object::Array(nums(&[500, 600]))),
        ]);
        let desc = font_dict(vec![(names::MISSING_WIDTH, Object::Int(250))]);
        let w = load_simple(&d, Some(&desc), &NoResolve);
        assert_eq!(w.get(65), Some(500.0));
        assert_eq!(w.get(66), Some(600.0));
        // Outside the declared range, the default fills in.
        assert_eq!(w.get(0), Some(250.0));
        assert_eq!(w.get(255), Some(250.0));
    }

    #[test]
    fn missing_width_alone_without_widths_does_nothing() {
        // `LoadCharWidths` returns before reading `/MissingWidth` when there
        // is no `/Widths`, which is what makes one branch of `LoadCharMetrics`
        // unreachable.
        let desc = font_dict(vec![(names::MISSING_WIDTH, Object::Int(250))]);
        let w = load_simple(&Dict::new(), Some(&desc), &NoResolve);
        assert_eq!(w.get(65), None);
        assert!(w.use_face_widths);
    }

    #[test]
    fn a_non_numeric_widths_element_reads_as_zero() {
        let d = font_dict(vec![
            (names::FIRST_CHAR, Object::Int(65)),
            (
                names::WIDTHS,
                Object::Array(Array::of([
                    Object::Int(500),
                    Object::Name(Name::from("oops")),
                    Object::Int(700),
                ])),
            ),
        ]);
        let w = load_simple(&d, None, &NoResolve);
        assert_eq!(w.get(66), Some(0.0));
        assert_eq!(w.get(67), Some(700.0));
    }

    // ---- /W ---------------------------------------------------------------

    #[test]
    fn the_c_first_c_last_w_form_parses() {
        let cid = font_dict(vec![(names::W, Object::Array(nums(&[1, 10, 500])))]);
        let w = CidWidths::load(&cid, &NoResolve, &mut quiet());
        assert_eq!(w.width(CharCode(0), Cid(1)), 500.0);
        assert_eq!(w.width(CharCode(0), Cid(10)), 500.0);
        assert_eq!(w.width(CharCode(0), Cid(11)), 1000.0);
    }

    #[test]
    fn the_c_array_form_parses() {
        let cid = font_dict(vec![(
            names::W,
            Object::Array(Array::of([
                Object::Int(5),
                Object::Array(nums(&[100, 200, 300])),
            ])),
        )]);
        let w = CidWidths::load(&cid, &NoResolve, &mut quiet());
        assert_eq!(w.width(CharCode(0), Cid(5)), 100.0);
        assert_eq!(w.width(CharCode(0), Cid(6)), 200.0);
        assert_eq!(w.width(CharCode(0), Cid(7)), 300.0);
        assert_eq!(w.width(CharCode(0), Cid(8)), 1000.0);
    }

    #[test]
    fn both_forms_interleave() {
        let cid = font_dict(vec![(
            names::W,
            Object::Array(Array::of([
                Object::Int(1),
                Object::Int(3),
                Object::Int(400),
                Object::Int(10),
                Object::Array(nums(&[700, 800])),
            ])),
        )]);
        let w = CidWidths::load(&cid, &NoResolve, &mut quiet());
        assert_eq!(w.width(CharCode(0), Cid(2)), 400.0);
        assert_eq!(w.width(CharCode(0), Cid(10)), 700.0);
        assert_eq!(w.width(CharCode(0), Cid(11)), 800.0);
    }

    #[test]
    fn the_first_overlapping_record_wins_regardless_of_order() {
        // Two overlapping records; the earlier one answers even though the
        // later is tighter.
        let cid = font_dict(vec![(
            names::W,
            Object::Array(nums(&[1, 100, 500, 50, 60, 999])),
        )]);
        let w = CidWidths::load(&cid, &NoResolve, &mut quiet());
        assert_eq!(w.width(CharCode(0), Cid(55)), 500.0);
    }

    #[test]
    fn an_array_in_the_wrong_state_aborts_the_parse() {
        // The array arrives where a *first code* was expected, which stops
        // everything — but keeps the record already read.
        let cid = font_dict(vec![(
            names::W,
            Object::Array(Array::of([
                Object::Int(1),
                Object::Int(3),
                Object::Int(400),
                Object::Array(nums(&[1, 2])),
                Object::Int(20),
                Object::Int(30),
                Object::Int(600),
            ])),
        )]);
        let mut diags = quiet();
        let w = CidWidths::load(&cid, &NoResolve, &mut diags);
        assert_eq!(w.width(CharCode(0), Cid(2)), 400.0);
        // Everything after the misplaced array is gone.
        assert_eq!(w.width(CharCode(0), Cid(25)), 1000.0);
        assert!(diags.contains(&DiagKind::FontWidthsTruncated));
    }

    #[test]
    fn a_trailing_partial_group_is_dropped() {
        // `20 30` with no width: the chunking discards the incomplete record.
        let cid = font_dict(vec![(names::W, Object::Array(nums(&[1, 3, 400, 20, 30])))]);
        let mut diags = quiet();
        let w = CidWidths::load(&cid, &NoResolve, &mut diags);
        assert_eq!(w.len(), 1);
        assert_eq!(w.width(CharCode(0), Cid(25)), 1000.0);
        assert!(diags.contains(&DiagKind::FontWidthsTruncated));
    }

    #[test]
    fn a_partial_group_inside_an_array_reads_as_zero() {
        // W2 takes three values per record; an array of four supplies one full
        // group and one that reads past the end.
        let cid = font_dict(vec![(
            names::W2,
            Object::Array(Array::of([
                Object::Int(0),
                Object::Array(nums(&[1, 2, 3, 4])),
            ])),
        )]);
        let v = VerticalMetrics::load(&cid, &NoResolve, &mut quiet());
        assert_eq!(v.len(), 2);
        assert_eq!(v.width(Cid(0)), 1.0);
        // The second record read `4, 0, 0`.
        assert_eq!(v.width(Cid(1)), 4.0);
    }

    #[test]
    fn dw_defaults_to_1000_and_is_honoured_when_present() {
        let w = CidWidths::load(&Dict::new(), &NoResolve, &mut quiet());
        assert_eq!(w.default_width(), 1000.0);
        let cid = font_dict(vec![(names::DW, Object::Int(742))]);
        let w = CidWidths::load(&cid, &NoResolve, &mut quiet());
        assert_eq!(w.width(CharCode(0), Cid(9)), 742.0);
    }

    #[test]
    fn the_gb2312_rescue_fixes_ascii_widths() {
        let mut w = CidWidths::load(&Dict::new(), &NoResolve, &mut quiet());
        w.set_ansi_widths_fixed();
        assert_eq!(w.width(CharCode(31), Cid(0)), 0.0);
        assert_eq!(w.width(CharCode(32), Cid(0)), 500.0);
        assert_eq!(w.width(CharCode(126), Cid(0)), 500.0);
        assert_eq!(w.width(CharCode(127), Cid(0)), 0.0);
        // At and above 0x80 the ordinary lookup resumes.
        assert_eq!(w.width(CharCode(0x80), Cid(0)), 1000.0);
    }

    // ---- /W2 and /DW2 -----------------------------------------------------

    #[test]
    fn dw2_defaults_to_the_iso_values() {
        let v = VerticalMetrics::default();
        assert_eq!(v.default_origin_y(), 880.0);
        assert_eq!(v.default_advance(), -1000.0);
    }

    #[test]
    fn w2_records_hold_five_values() {
        let cid = font_dict(vec![(
            names::W2,
            Object::Array(nums(&[10, 20, -900, 450, 800])),
        )]);
        let v = VerticalMetrics::load(&cid, &NoResolve, &mut quiet());
        assert_eq!(v.width(Cid(15)), -900.0);
        assert_eq!(v.origin(Cid(15), &CidWidths::default()), (450.0, 800.0));
    }

    #[test]
    fn a_cid_with_no_w2_record_takes_half_the_horizontal_width() {
        let cid = font_dict(vec![(names::W, Object::Array(nums(&[1, 10, 741])))]);
        let h = CidWidths::load(&cid, &NoResolve, &mut quiet());
        let v = VerticalMetrics::default();
        // Half of 741, truncated, and DW2[0].
        assert_eq!(v.origin(Cid(5), &h), (370.0, 880.0));
    }

    #[test]
    fn dw2_overrides_the_defaults() {
        let cid = font_dict(vec![(names::DW2, Object::Array(nums(&[700, -800])))]);
        let v = VerticalMetrics::load(&cid, &NoResolve, &mut quiet());
        assert_eq!(v.default_origin_y(), 700.0);
        assert_eq!(v.default_advance(), -800.0);
    }

    #[test]
    fn vertical_origins_narrow_through_i16() {
        // 40000 does not fit an `i16`; the C++ narrows and so do we.
        let cid = font_dict(vec![(
            names::W2,
            Object::Array(nums(&[0, 0, -1000, 40000, 0])),
        )]);
        let v = VerticalMetrics::load(&cid, &NoResolve, &mut quiet());
        assert_eq!(
            v.origin(Cid(0), &CidWidths::default()).0,
            f32::from(40000i32 as i16)
        );
    }

    #[test]
    fn malformed_w_arrays_never_panic() {
        for a in [
            nums(&[]),
            nums(&[1]),
            // `Object::Int`'s documented reachable range, not `i64`'s.
            nums(&[4_294_967_295, -2_147_483_648, 5]),
            Array::of([Object::Array(nums(&[1, 2]))]),
            Array::of([Object::Null, Object::Int(1), Object::Int(2), Object::Int(3)]),
            Array::of([
                Object::Int(i32::MAX.into()),
                Object::Array(nums(&[1, 2, 3])),
            ]),
        ] {
            let cid = font_dict(vec![(names::W, Object::Array(a))]);
            let w = CidWidths::load(&cid, &NoResolve, &mut quiet());
            let _ = w.width(CharCode(0), Cid(0));
            let _ = w.width(CharCode(0), Cid(u16::MAX));
        }
    }
}
