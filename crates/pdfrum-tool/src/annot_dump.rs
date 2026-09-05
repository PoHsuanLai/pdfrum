//! The `--annot` output format.
//!
//! This walks the page's `/Annots` array **directly**, not the rendering
//! annotation list, so it reports pop-ups that are in the file and none of
//! the ones a viewer would synthesize.
//!
//! What it reports has already been changed by appearance generation, and
//! that is not an accident of ordering — a viewer builds its annotation list
//! before anything reads a dictionary, so a sticky note's `/Rect` is already
//! the 20×20 box the generator wrote and an ink annotation's is already
//! inflated. Callers pass the [`AnnotOverlay`] that generation produced;
//! passing `None` reports the file's pre-generation state, which is a
//! different document than the one the format describes.

// The alpha byte reproduces a C float-to-integer conversion; its truncation
// is the behaviour being matched.
#![allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]

use std::fmt::Write as _;

use pdfrum_common::{DiagKind, Diagnostics, Severity};
use pdfrum_object::{Array, Dict, Resolve, decode_text, names as obj_names};

use pdfrum_doc::annot::{self, Subtype};
use pdfrum_doc::ap::AnnotOverlay;
use pdfrum_doc::color::Color;
use pdfrum_doc::geom;
use pdfrum_doc::nav::Hidden;

/// The two `/F`-adjacent keys this format reads that `pdfrum_object::names`
/// does not declare. `pdfrum-doc`'s own table is private, and the emitter no
/// longer lives there.
mod names {
    pub(super) use pdfrum_object::names::{C, CA};

    pdfrum_object::names! {
        /// An annotation's interior colour (`/IC`).
        IC = "IC";
        /// A text-markup annotation's quadrilaterals (`/QuadPoints`).
        QUAD_POINTS = "QuadPoints";
    }
}

/// What kind of page object an appearance stream drew, as the dump names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectKind {
    /// Glyphs.
    Text,
    /// A filled or stroked path.
    Path,
    /// A sampled image.
    Image,
    /// A shading.
    Shading,
    /// A nested form `XObject`.
    Form,
}

impl ObjectKind {
    /// The spelling the dump prints.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            ObjectKind::Text => "Text",
            ObjectKind::Path => "Path",
            ObjectKind::Image => "Image",
            ObjectKind::Shading => "Shading",
            ObjectKind::Form => "Form",
        }
    }
}

/// Renders a page's annotation dump.
///
/// `objects_in_ap` is asked, per `/Annots` index, what the annotation's normal
/// appearance stream draws. It is a callback rather than a parameter because
/// answering means parsing a content stream — which this crate does not do,
/// and which the caller will want to answer against its own caches.
///
/// `hidden` is what the document's open action did to the flag words, for the
/// same reason the overlay is threaded: a reader runs that action before it
/// reads any dictionary, so `Flags set:` reports the *post*-action state.
/// `&Hidden::default()` is the honest answer for a caller that has not run one.
#[must_use]
pub fn render<R: Resolve>(
    page: &Dict,
    overlay: Option<&AnnotOverlay>,
    hidden: &Hidden,
    mut objects_in_ap: impl FnMut(usize, &Dict) -> Vec<ObjectKind>,
    r: &R,
    diags: &mut Diagnostics,
) -> String {
    let annots = page.array(obj_names::ANNOTS, r);
    let count = annots.as_ref().map_or(0, Array::len);
    let mut out = format!("Number of annotations: {count}\n\n");

    for index in 0..count {
        let _ = writeln!(out, "Annotation #{}:", index + 1);
        // An entry that does not resolve to a dictionary is a null
        // annotation: it still counts, and it still gets its own block.
        let Some(dict) = annots.as_ref().and_then(|a| a.dict_at(index, r)) else {
            out.push_str("Failed to retrieve annotation!\n\n");
            continue;
        };
        write_annotation(
            &mut out,
            &dict,
            index,
            overlay,
            hidden,
            &mut objects_in_ap,
            r,
            diags,
        );
    }
    out
}

/// One annotation's block.
#[allow(clippy::too_many_arguments)]
fn write_annotation<R: Resolve>(
    out: &mut String,
    dict: &Dict,
    index: usize,
    overlay: Option<&AnnotOverlay>,
    hidden: &Hidden,
    objects_in_ap: &mut impl FnMut(usize, &Dict) -> Vec<ObjectKind>,
    r: &R,
    diags: &mut Diagnostics,
) {
    let subtype = Subtype::from_bytes(&dict.byte_string(obj_names::SUBTYPE, r).unwrap_or_default());
    if subtype == Subtype::Unknown {
        // The oracle's own table has no arm for an unknown subtype and none
        // for a redaction either; reaching one aborts it, which is why no
        // golden file contains either spelling. We name it and carry on.
        diags.record(Severity::Recovered, DiagKind::AnnotSubtypeUnknown, None);
        let _ = writeln!(out, "Subtype: Unknown");
    } else {
        let _ = writeln!(
            out,
            "Subtype: {}",
            String::from_utf8_lossy(subtype.as_bytes())
        );
    }

    // Post-open-action, not as written: a `/Hide` in the document's
    // `/OpenAction` has already rewritten `/F` by the time anything reads it.
    let flags = hidden.flags(dict, r);
    let _ = writeln!(out, "Flags set: {}", flags.names().join(", "));

    let objects = objects_in_ap(index, dict);
    let _ = writeln!(out, "Number of objects: {}", objects.len());
    if !objects.is_empty() {
        out.push_str("Object types: ");
        for kind in &objects {
            let _ = write!(out, "{}  ", kind.as_str());
        }
        out.push('\n');
    }

    let has_appearance = overlay.is_some_and(|overlay| overlay.get(index).is_some())
        || annot::annot_ap(dict, annot::ApMode::Normal, true, r).is_some();
    write_color(out, dict, has_appearance, false, r);
    write_color(out, dict, has_appearance, true, r);

    for (label, key) in [("Content", obj_names::CONTENTS), ("Author", obj_names::T)] {
        let text = dict
            .byte_string(key, r)
            .map(|bytes| decode_text(&bytes).into_owned())
            .unwrap_or_default();
        let _ = writeln!(out, "{label}: {}", wide(&text));
    }

    if subtype.has_attachment_points() {
        let quads = dict.array(names::QUAD_POINTS, r);
        let count = annot::quad_point_count(quads.as_ref());
        let _ = writeln!(out, "Number of quadpoints sets: {count}");
        if let Some(quads) = &quads {
            if quads.len() % 8 != 0 {
                diags.record(Severity::Suspicious, DiagKind::QuadPointsTruncated, None);
            }
            for set in 0..count {
                let at = |offset: usize| three_places(quads.number_at_or_zero(set * 8 + offset));
                let _ = writeln!(
                    out,
                    "Quadpoints set #{}: ({}, {}), ({}, {}), ({}, {}), ({}, {})",
                    set + 1,
                    at(0),
                    at(1),
                    at(2),
                    at(3),
                    at(4),
                    at(5),
                    at(6),
                    at(7)
                );
            }
        }
    }

    // The rectangle is the raw `/Rect`, unnormalized — except where
    // appearance generation rewrote it, which the overlay records.
    let raw = dict.rect(obj_names::RECT, r);
    let rect = overlay.map_or(raw, |overlay| overlay.rect(index, raw));
    let _ = writeln!(
        out,
        "Rectangle: l - {}, b - {}, r - {}, t - {}\n",
        three_places(geom::left(rect)),
        three_places(geom::bottom(rect)),
        three_places(geom::right(rect)),
        three_places(geom::top(rect))
    );
}

/// One of the two colour lines.
///
/// Both fail — printing a message rather than numbers — whenever the
/// annotation resolves to a normal appearance **stream**, because the
/// stream's own colour operators outrank anything the dictionary says. That
/// includes a stream this run just generated, which is why `has_appearance`
/// is passed in rather than looked up: by the time the format describes the
/// document, generation has already given most annotations one.
///
/// When there is no colour array at all the line still succeeds, using the
/// default the matching appearance generator would have used: yellow for a
/// highlight, black for everything else.
fn write_color<R: Resolve>(
    out: &mut String,
    dict: &Dict,
    has_appearance: bool,
    interior: bool,
    r: &R,
) {
    let label = if interior { "Interior color" } else { "Color" };
    if has_appearance {
        let _ = writeln!(out, "Failed to retrieve {}.", label.to_lowercase());
        return;
    }

    // Alpha comes from `/CA` when the key is present, whatever its type.
    let alpha = if dict.contains_key(names::CA) {
        dict.number(names::CA, r).unwrap_or(0.0)
    } else {
        1.0
    };
    let alpha = alpha_byte(alpha);

    let key = if interior { names::IC } else { names::C };
    let Some(array) = dict.array(key, r) else {
        // The highlight default is chosen on a **name-typed** `/Subtype`, so
        // a string-valued one falls to black even though it names a highlight
        // everywhere else in this crate.
        let named_highlight = dict
            .name(obj_names::SUBTYPE)
            .map(pdfrum_object::Name::as_bytes)
            == Some(b"Highlight");
        let _ = if named_highlight {
            writeln!(out, "{label} in RGBA: 255 255 0 {alpha}")
        } else {
            writeln!(out, "{label} in RGBA: 0 0 0 {alpha}")
        };
        return;
    };
    let (red, green, blue) = Color::from_array(&array).annot_rgb_bytes();
    let _ = writeln!(out, "{label} in RGBA: {red} {green} {blue} {alpha}");
}

/// The alpha byte, truncated toward zero and saturating rather than wrapping.
fn alpha_byte(alpha: f32) -> u32 {
    let scaled = alpha * 255.0;
    if scaled <= 0.0 {
        0
    } else if scaled >= 4_294_967_296.0 {
        u32::MAX
    } else {
        scaled as u32
    }
}

/// A number with three decimals, rounding halves to even.
///
/// The oracle promotes a `float` to `double` and hands it to the C library,
/// which rounds a tie to the nearest **even** last digit. Rust's own
/// formatting rounds a tie away from zero, and while a `float` almost never
/// lands exactly on a half-milli boundary, "almost never" is not a property
/// a byte-exact contract can rest on.
#[must_use]
fn three_places(value: f32) -> String {
    round_half_even(f64::from(value), 3)
}

/// A number with six decimals, rounding halves to even.
///
/// The format's other fixed-point width. No line in the current dump uses it,
/// but it and [`three_places`] are one rule with two precisions, and the
/// tie-breaking test covers both.
#[cfg(test)]
fn six_places(value: f32) -> String {
    round_half_even(f64::from(value), 6)
}

/// Fixed-point formatting with the C library's tie-breaking rule.
fn round_half_even(value: f64, places: usize) -> String {
    if value.is_nan() {
        return if value.is_sign_negative() {
            "-nan"
        } else {
            "nan"
        }
        .to_owned();
    }
    if value.is_infinite() {
        return if value < 0.0 { "-inf" } else { "inf" }.to_owned();
    }
    // Ask for one more digit than needed. If that digit is a lone `5` with
    // nothing after it, the value sits exactly on the boundary and the rule
    // differs from Rust's; otherwise Rust's rounding already agrees.
    let extended = format!("{:.*}", places + 1, value);
    let Some(last) = extended.as_bytes().last().copied() else {
        return extended;
    };
    if last != b'5' || f64::from_str_exact(&extended) != Some(value) {
        return format!("{value:.places$}");
    }
    // A true tie: keep the shorter form whose final digit is even.
    let truncated = extended.get(..extended.len() - 1).unwrap_or_default();
    let keep = truncated
        .as_bytes()
        .last()
        .copied()
        .is_some_and(|digit| digit.is_ascii_digit() && (digit - b'0').is_multiple_of(2));
    if keep {
        truncated.to_owned()
    } else {
        format!("{value:.places$}")
    }
}

/// Whether a decimal spelling names the value exactly.
trait ExactParse {
    /// The value the spelling names, or `None` when it is itself a rounding.
    fn from_str_exact(text: &str) -> Option<f64>;
}

impl ExactParse for f64 {
    fn from_str_exact(text: &str) -> Option<f64> {
        let parsed: f64 = text.parse().ok()?;
        // A round-trip through the same number of digits confirms the
        // spelling is not itself a rounding of something longer.
        let places = text.split_once('.').map_or(0, |(_, frac)| frac.len());
        (format!("{parsed:.places$}") == text).then_some(parsed)
    }
}

/// Truncates a string where the oracle's wide-character output gives up: at
/// the first NUL, or at the first unit with no scalar value.
fn wide(text: &str) -> String {
    match text.find(['\u{0}', '\u{FFFD}']) {
        Some(at) => text.get(..at).unwrap_or_default().to_owned(),
        None => text.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::{Hidden, ObjectKind, render, six_places, three_places};
    use pdfrum_common::Diagnostics;
    use pdfrum_object::{Array, ByteSpan, Dict, Name, NoResolve, Object, PdfString, Stream};

    fn dict(pairs: &[(&str, Object)]) -> Dict {
        Dict::from_pairs(
            pairs
                .iter()
                .map(|(k, v)| (Name::from(*k), v.clone()))
                .collect::<Vec<_>>(),
        )
    }

    fn page(annots: &[Object]) -> Dict {
        dict(&[("Annots", Object::Array(Array::of(annots.to_vec())))])
    }

    fn dump(page: &Dict) -> String {
        let mut diags = Diagnostics::default();
        render(
            page,
            None,
            &Hidden::default(),
            |_, _| Vec::new(),
            &NoResolve,
            &mut diags,
        )
    }

    #[test]
    fn a_page_with_no_annotations_is_two_lines() {
        assert_eq!(dump(&Dict::new()), "Number of annotations: 0\n\n");
        assert_eq!(dump(&page(&[])), "Number of annotations: 0\n\n");
    }

    #[test]
    fn an_unresolvable_entry_still_counts_and_gets_its_own_block() {
        let got = dump(&page(&[Object::Int(4)]));
        assert_eq!(
            got,
            "Number of annotations: 1\n\nAnnotation #1:\nFailed to retrieve annotation!\n\n"
        );
    }

    #[test]
    fn a_link_prints_a_quadpoint_count_even_with_no_quadpoints() {
        let link = Object::Dict(dict(&[
            ("Subtype", Object::Name(Name::from("Link"))),
            (
                "Rect",
                Object::Array(Array::of([0.0_f32, 1.0, 2.0, 3.0].map(Object::from))),
            ),
        ]));
        let got = dump(&page(&[link]));
        assert!(got.contains("Number of quadpoints sets: 0\n"), "{got}");
        assert!(
            got.ends_with("Rectangle: l - 0.000, b - 1.000, r - 2.000, t - 3.000\n\n"),
            "{got}"
        );
    }

    #[test]
    fn a_square_prints_no_quadpoint_line_at_all() {
        let square = Object::Dict(dict(&[("Subtype", Object::Name(Name::from("Square")))]));
        assert!(!dump(&page(&[square])).contains("quadpoints"));
    }

    #[test]
    fn an_existing_appearance_stream_outranks_the_colour_keys() {
        let with_ap = Object::Dict(dict(&[
            ("Subtype", Object::Name(Name::from("Square"))),
            (
                "AP",
                Object::Dict(dict(&[(
                    "N",
                    Object::Stream(Box::new(Stream::new(
                        Dict::new(),
                        ByteSpan::from(b"x".to_vec()),
                    ))),
                )])),
            ),
            (
                "C",
                Object::Array(Array::of([1.0_f32, 0.0, 0.0].map(Object::from))),
            ),
        ]));
        let got = dump(&page(&[with_ap]));
        assert!(got.contains("Failed to retrieve color.\n"), "{got}");
        assert!(
            got.contains("Failed to retrieve interior color.\n"),
            "{got}"
        );
    }

    #[test]
    fn a_highlight_with_no_colour_array_defaults_to_yellow() {
        let highlight = Object::Dict(dict(&[("Subtype", Object::Name(Name::from("Highlight")))]));
        assert!(dump(&page(&[highlight])).contains("Color in RGBA: 255 255 0 255\n"));

        // The default is chosen on a *name*-typed subtype, so a string one
        // falls through to black.
        let stringly = Object::Dict(dict(&[(
            "Subtype",
            Object::Str(PdfString::literal(b"Highlight")),
        )]));
        assert!(dump(&page(&[stringly])).contains("Color in RGBA: 0 0 0 255\n"));
    }

    #[test]
    fn alpha_comes_from_the_opacity_key_when_it_is_present() {
        let half = Object::Dict(dict(&[
            ("Subtype", Object::Name(Name::from("Square"))),
            ("CA", Object::from(0.3_f32)),
        ]));
        // 0.3 * 255 = 76.5, truncated.
        assert!(dump(&page(&[half])).contains("Color in RGBA: 0 0 0 76\n"));
    }

    #[test]
    fn object_types_are_followed_by_two_spaces_each() {
        let square = Object::Dict(dict(&[("Subtype", Object::Name(Name::from("Square")))]));
        let mut diags = Diagnostics::default();
        let got = render(
            &page(&[square]),
            None,
            &Hidden::default(),
            |_, _| vec![ObjectKind::Text, ObjectKind::Path],
            &NoResolve,
            &mut diags,
        );
        assert!(
            got.contains("Number of objects: 2\nObject types: Text  Path  \n"),
            "{got}"
        );
    }

    #[test]
    fn flags_join_with_a_comma_and_a_space_in_bit_order() {
        let flagged = Object::Dict(dict(&[
            ("Subtype", Object::Name(Name::from("Text"))),
            ("F", Object::Int(4 | 8 | 16)),
        ]));
        assert!(dump(&page(&[flagged])).contains("Flags set: Print, NoZoom, NoRotate\n"));
    }

    #[test]
    fn fixed_point_formatting_breaks_ties_toward_an_even_digit() {
        // 0.0625 sits exactly on the boundary at three places.
        assert_eq!(three_places(0.0625), "0.062");
        assert_eq!(three_places(0.0635), "0.064");
        // Ordinary values are unaffected.
        assert_eq!(three_places(1.5), "1.500");
        assert_eq!(three_places(-0.0), "-0.000");
        assert_eq!(three_places(234.372), "234.372");
        assert_eq!(six_places(2.0), "2.000000");
    }
}
