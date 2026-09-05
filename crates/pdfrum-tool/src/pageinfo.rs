//! `--show-pageinfo`: the five page boundary boxes (ISO 32000-1 §14.11.2).
//!
//! Five lines per page, in a fixed order:
//!
//! ```text
//! Page 0: MediaBox: 0.00 0.00 612.00 792.00
//! Page 0: No CropBox.
//! ```
//!
//! The surprise here is how *little* the oracle does. It reads the page
//! dictionary's own box entry and stops:
//!
//! - **No inheritance.** A `/MediaBox` on the `/Pages` node is invisible to
//!   this dump even though every renderer honours it, so `No MediaBox.` is
//!   common — 278 lines of it across the corpus.
//! - **No normalization.** The four numbers are printed in file order, so a
//!   box written `[0 0 200 -50]` prints its negative top verbatim.
//! - **No length check.** A short array still answers, with zeros standing in
//!   for the elements it lacks; only a missing key or a non-array declines.
//! - **No substitution.** The letter-sized default a page falls back to for
//!   rendering never appears here.

use pdfrum_object::{Dict, Name, Resolve, names};

/// The five boxes, in the order the dump prints them.
pub const BOXES: [&Name; 5] = [
    names::MEDIA_BOX,
    names::CROP_BOX,
    names::BLEED_BOX,
    names::TRIM_BOX,
    names::ART_BOX,
];

/// A box as the dump reads it: left, bottom, right, top, unnormalized.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoxRect {
    /// The array's first element, in PDF user space units.
    pub left: f32,
    /// The array's second element, in PDF user space units.
    pub bottom: f32,
    /// The array's third element; not guaranteed to exceed `left`.
    pub right: f32,
    /// The array's fourth element; not guaranteed to exceed `bottom`.
    pub top: f32,
}

/// Reads one box off a page dictionary, without inheriting or normalizing.
///
/// `None` is the `No <box>.` line: the key is absent, or its value is not an
/// array once one level of indirection is followed.
pub fn read(page: &Dict, key: &Name, r: &impl Resolve) -> Option<BoxRect> {
    let array = page.array(key, r)?;
    // Each element is resolved one level too, and anything that is not a
    // number reads as zero rather than declining the box.
    let at = |index: usize| {
        array
            .get(index, r)
            .and_then(|value| value.as_direct().and_then(pdfrum_object::Object::number))
            .unwrap_or(0.0)
    };
    Some(BoxRect {
        left: at(0),
        bottom: at(1),
        right: at(2),
        top: at(3),
    })
}

/// The five lines for one page.
pub fn render(page: &Dict, index: u32, r: &impl Resolve) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    for key in BOXES {
        let Some(name) = key.as_str() else { continue };
        // Writing into a `String` cannot fail, and there is nothing sensible
        // to do about it if the formatter ever said otherwise.
        let _ = match read(page, key, r) {
            None => writeln!(out, "Page {index}: No {name}."),
            Some(rect) => writeln!(
                out,
                "Page {index}: {name}: {} {} {} {}",
                two_places(rect.left),
                two_places(rect.bottom),
                two_places(rect.right),
                two_places(rect.top),
            ),
        };
    }
    out
}

/// A number as `printf("%0.2f")` writes it.
///
/// The value reaches the C library as a `float` promoted to `double`, so the
/// rounding is over the exact `f32` value; Rust's own `{:.2}` rounds the same
/// way once the widening is explicit. Infinities and NaN cannot occur — the
/// only source is a parsed PDF number — but they are given the C spellings
/// rather than Rust's so a malformed file could never print `inf` where the
/// oracle prints something else.
fn two_places(value: f32) -> String {
    let widened = f64::from(value);
    if widened.is_nan() {
        return if widened.is_sign_negative() {
            "-nan".to_owned()
        } else {
            "nan".to_owned()
        };
    }
    if widened.is_infinite() {
        return if widened < 0.0 { "-inf" } else { "inf" }.to_owned();
    }
    format!("{widened:.2}")
}

#[cfg(test)]
// Exact float equality is deliberate: these assertions pin values that are
// exact by construction — a box element that is not a number reads as
// literally 0.0, not as approximately zero.
#[allow(clippy::float_cmp, reason = "asserting exactly-representable values")]
mod tests {
    use super::*;
    use pdfrum_object::{Array, NoResolve, Object};

    fn page_with(key: &str, value: Object) -> Dict {
        Dict::from_pairs([(Name::from(key), value)])
    }

    fn boxed(values: &[f32]) -> Object {
        Object::Array(Array::of(
            values.iter().copied().map(Object::Real).collect::<Vec<_>>(),
        ))
    }

    #[test]
    fn a_full_box_prints_its_four_numbers_in_file_order() {
        let page = page_with("MediaBox", boxed(&[0.0, 0.0, 612.0, 792.0]));
        assert_eq!(
            render(&page, 0, &NoResolve).lines().next(),
            Some("Page 0: MediaBox: 0.00 0.00 612.00 792.00")
        );
    }

    #[test]
    fn a_missing_box_prints_the_no_line() {
        let page = Dict::new();
        assert_eq!(
            render(&page, 3, &NoResolve),
            "Page 3: No MediaBox.\n\
             Page 3: No CropBox.\n\
             Page 3: No BleedBox.\n\
             Page 3: No TrimBox.\n\
             Page 3: No ArtBox.\n"
        );
    }

    #[test]
    fn an_inherited_box_is_invisible_to_this_dump() {
        // The renderer walks /Parent for /MediaBox; this getter does not, and
        // the 278 `No MediaBox.` lines in the corpus are the proof.
        let page = page_with("Parent", Object::Ref(pdfrum_object::ObjRef::new(1, 0)));
        assert!(read(&page, names::MEDIA_BOX, &NoResolve).is_none());
    }

    #[test]
    fn a_short_array_answers_with_zeros_rather_than_declining() {
        let page = page_with("MediaBox", boxed(&[10.0, 20.0]));
        assert_eq!(
            read(&page, names::MEDIA_BOX, &NoResolve),
            Some(BoxRect {
                left: 10.0,
                bottom: 20.0,
                right: 0.0,
                top: 0.0
            })
        );
    }

    #[test]
    fn an_empty_array_is_still_a_box() {
        let page = page_with("MediaBox", Object::Array(Array::new()));
        assert_eq!(
            render(&page, 0, &NoResolve).lines().next(),
            Some("Page 0: MediaBox: 0.00 0.00 0.00 0.00")
        );
    }

    #[test]
    fn a_non_array_value_declines_the_box() {
        for value in [
            Object::Int(5),
            Object::Dict(Dict::new()),
            Object::Name(Name::from("x")),
            Object::Null,
        ] {
            let page = page_with("CropBox", value);
            assert!(read(&page, names::CROP_BOX, &NoResolve).is_none());
        }
    }

    #[test]
    fn a_non_numeric_element_reads_as_zero() {
        let page = page_with(
            "MediaBox",
            Object::Array(Array::of([
                Object::Int(0),
                Object::Int(0),
                Object::Name(Name::from("Foo")),
                Object::Real(792.0),
            ])),
        );
        assert_eq!(
            read(&page, names::MEDIA_BOX, &NoResolve).unwrap().right,
            0.0
        );
    }

    #[test]
    fn negative_and_inverted_boxes_are_printed_as_written() {
        // corpus/.../bug_*.pdf carry these; normalizing them would silently
        // change six MediaBox lines in the golden store.
        let page = page_with("MediaBox", boxed(&[-30.0, -30.0, 225.0, 225.0]));
        assert!(render(&page, 0, &NoResolve).contains("-30.00 -30.00 225.00 225.00"));
        let inverted = page_with("MediaBox", boxed(&[0.0, 0.0, 200.0, -50.0]));
        assert!(render(&inverted, 0, &NoResolve).contains("0.00 0.00 200.00 -50.00"));
    }

    #[test]
    fn the_boxes_are_printed_in_the_oracles_order() {
        let names: Vec<&str> = BOXES.iter().filter_map(|b| b.as_str()).collect();
        assert_eq!(
            names,
            ["MediaBox", "CropBox", "BleedBox", "TrimBox", "ArtBox"]
        );
    }

    #[test]
    fn fractional_values_round_to_two_places() {
        assert_eq!(two_places(1105.514), "1105.51");
        assert_eq!(two_places(28.3465), "28.35");
        assert_eq!(two_places(841.89), "841.89");
        assert_eq!(two_places(-0.0), "-0.00");
    }
}
