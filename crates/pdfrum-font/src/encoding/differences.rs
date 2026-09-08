//! The `/Encoding` `/Differences` array: a sparse code→glyph-name overlay.
//!
//! The array interleaves integers (a new starting code) with names (glyphs at
//! consecutive codes from there). Its damage behavior is specific and
//! load-bearing.

use crate::ids::GlyphName;
use pdfrum_object::{Array, Object, Resolve};

/// Read a `/Differences` array into a 256-entry name table.
///
/// Three behaviors reproduce PDFium exactly, and all three are reachable from
/// real files:
///
/// - **A null or dangling element is skipped without advancing the cursor.**
///   `[65 /A null /B]` names `A` at 65 and `B` at **66**, not 67.
/// - **Any non-name element resets the cursor** to its integer value, and a
///   real, a string or a dictionary reads as integer **0** — so a stray
///   `(foo)` mid-array silently restarts numbering from code 0.
/// - **The cursor keeps advancing past 255.** Names beyond it are dropped, but
///   a later integer can bring the cursor back into range.
pub fn load_differences(
    diffs: &Array,
    r: &impl Resolve,
    out: &mut [Option<GlyphName>; 256],
) -> bool {
    let mut cur_code: u32 = 0;
    let mut wrote = false;
    for i in 0..diffs.len() {
        // A dangling reference resolves to nothing and is skipped *without*
        // advancing — `Resolved::as_direct` is `None` for a ref-to-ref too,
        // which is the same absence PDFium sees.
        let Some(resolved) = diffs.get(i, r) else {
            continue;
        };
        let Some(element) = resolved.as_direct() else {
            continue;
        };
        match element {
            Object::Null => {}
            Object::Name(name) => {
                if let Some(slot) = usize::try_from(cur_code).ok().and_then(|c| out.get_mut(c)) {
                    *slot = Some(GlyphName::new(name.as_bytes().to_vec()));
                    wrote = true;
                }
                // Advances even past 255, and saturates rather than wrapping:
                // PDFium's `uint32_t` would wrap, but only after 2^32 names,
                // which no array can hold.
                cur_code = cur_code.saturating_add(1);
            }
            // Everything else resets the cursor. `as_int` is the C-integer
            // view, which yields 0 for a string, a dict or an array — exactly
            // what `GetInteger` does.
            other => {
                let v = other.as_int().unwrap_or(0);
                // A negative integer becomes a huge `size_t` in the C++ and
                // every later name is dropped by the `< 256` guard while the
                // cursor keeps climbing. `u32::MAX` reproduces that, since
                // saturating addition then keeps it out of range forever.
                cur_code = u32::try_from(v).unwrap_or(u32::MAX);
            }
        }
    }
    wrote
}

#[cfg(test)]
mod tests {
    // Test fixtures are fixed-size arrays with known contents.
    #![allow(clippy::indexing_slicing)]
    use super::*;
    use pdfrum_object::{Name, NoResolve, PdfString};

    fn table() -> [Option<GlyphName>; 256] {
        [const { None }; 256]
    }

    fn names_at(out: &[Option<GlyphName>; 256], code: usize) -> Option<&[u8]> {
        out[code].as_ref().map(GlyphName::as_bytes)
    }

    #[test]
    fn integers_set_the_cursor_and_names_advance_it() {
        let a = Array::of([
            Object::Int(65),
            Object::Name(Name::from("A")),
            Object::Name(Name::from("B")),
            Object::Int(200),
            Object::Name(Name::from("zed")),
        ]);
        let mut out = table();
        assert!(load_differences(&a, &NoResolve, &mut out));
        assert_eq!(names_at(&out, 65), Some(&b"A"[..]));
        assert_eq!(names_at(&out, 66), Some(&b"B"[..]));
        assert_eq!(names_at(&out, 200), Some(&b"zed"[..]));
        assert_eq!(names_at(&out, 67), None);
    }

    #[test]
    fn a_null_element_is_skipped_without_advancing() {
        let a = Array::of([
            Object::Int(65),
            Object::Name(Name::from("A")),
            Object::Null,
            Object::Name(Name::from("B")),
        ]);
        let mut out = table();
        load_differences(&a, &NoResolve, &mut out);
        // `B` lands at 66, not 67 — the null consumed no code.
        assert_eq!(names_at(&out, 66), Some(&b"B"[..]));
        assert_eq!(names_at(&out, 67), None);
    }

    #[test]
    fn a_dangling_reference_is_skipped_without_advancing() {
        let a = Array::of([
            Object::Int(65),
            Object::Name(Name::from("A")),
            Object::Ref(pdfrum_object::ObjRef::new(99, 0)),
            Object::Name(Name::from("B")),
        ]);
        let mut out = table();
        load_differences(&a, &NoResolve, &mut out);
        assert_eq!(names_at(&out, 66), Some(&b"B"[..]));
    }

    #[test]
    fn a_string_resets_the_cursor_to_zero() {
        let a = Array::of([
            Object::Int(65),
            Object::Name(Name::from("A")),
            Object::Str(PdfString::literal(b"bar")),
            Object::Name(Name::from("atzero")),
        ]);
        let mut out = table();
        load_differences(&a, &NoResolve, &mut out);
        assert_eq!(names_at(&out, 65), Some(&b"A"[..]));
        // The string read as integer 0, restarting numbering there.
        assert_eq!(names_at(&out, 0), Some(&b"atzero"[..]));
    }

    #[test]
    fn a_real_also_resets_the_cursor_to_its_truncated_value() {
        let a = Array::of([Object::Real(70.9), Object::Name(Name::from("F"))]);
        let mut out = table();
        load_differences(&a, &NoResolve, &mut out);
        assert_eq!(names_at(&out, 70), Some(&b"F"[..]));
    }

    #[test]
    fn names_past_255_are_dropped_while_the_cursor_advances() {
        let a = Array::of([
            Object::Int(254),
            Object::Name(Name::from("a")),
            Object::Name(Name::from("b")),
            Object::Name(Name::from("dropped")),
            Object::Int(1),
            Object::Name(Name::from("back")),
        ]);
        let mut out = table();
        load_differences(&a, &NoResolve, &mut out);
        assert_eq!(names_at(&out, 254), Some(&b"a"[..]));
        assert_eq!(names_at(&out, 255), Some(&b"b"[..]));
        // "dropped" would be code 256, which has no slot.
        assert_eq!(names_at(&out, 1), Some(&b"back"[..]));
    }

    #[test]
    fn a_negative_integer_drops_everything_after_it() {
        let a = Array::of([
            Object::Int(-5),
            Object::Name(Name::from("gone")),
            Object::Name(Name::from("also_gone")),
        ]);
        let mut out = table();
        assert!(!load_differences(&a, &NoResolve, &mut out));
        assert!(out.iter().all(Option::is_none));
    }

    #[test]
    fn an_empty_array_writes_nothing() {
        let mut out = table();
        assert!(!load_differences(&Array::new(), &NoResolve, &mut out));
    }
}
