//! Destinations (ISO 32000-1 §12.3.2): where a link, bookmark or action
//! takes the reader.
//!
//! A destination is one array: a page at index 0, a view-mode name at index
//! 1, and up to four parameters after that. Eight modes are defined and the
//! array is read leniently at every position — a missing parameter is zero, a
//! mistyped one is zero, and an index past the end is zero rather than an
//! error.
//!
//! The strict reader is [`Dest::xyz`], which is the only accessor that
//! type-filters the mode name and the only one where a zero has meaning: a
//! zoom of exactly zero means "keep the current zoom", while an x or y of
//! zero is a real coordinate.

use pdfrum_common::{DiagKind, Diagnostics, Limits, PageIndex, Severity};
use pdfrum_object::{Array, Dict, Object, Resolve};

use crate::nav::name_tree;

/// A destination's view mode.
///
/// The discriminants are API — they are the numbering the public interface
/// exposes — and index 0 is reserved for "no recognized mode", which is why a
/// literal `/Unknown` in a file matches nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ZoomMode {
    /// The mode name matched nothing.
    #[default]
    Unknown,
    /// Position and zoom named explicitly.
    Xyz,
    /// Fit the whole page.
    Fit,
    /// Fit the width, at a given top edge.
    FitH,
    /// Fit the height, at a given left edge.
    FitV,
    /// Fit a named rectangle.
    FitR,
    /// Fit the bounding box of the page's contents.
    FitB,
    /// Fit the bounding box's width.
    FitBH,
    /// Fit the bounding box's height.
    FitBV,
}

/// The mode table: spelling, and how many parameters the mode consumes.
const MODES: [(ZoomMode, &[u8], usize); 8] = [
    (ZoomMode::Xyz, b"XYZ", 3),
    (ZoomMode::Fit, b"Fit", 0),
    (ZoomMode::FitH, b"FitH", 1),
    (ZoomMode::FitV, b"FitV", 1),
    (ZoomMode::FitR, b"FitR", 4),
    (ZoomMode::FitB, b"FitB", 0),
    (ZoomMode::FitBH, b"FitBH", 1),
    (ZoomMode::FitBV, b"FitBV", 1),
];

/// The three coordinates an `XYZ` destination names, each independently
/// present or absent.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Xyz {
    /// The left edge, when named.
    pub x: Option<f32>,
    /// The top edge, when named.
    pub y: Option<f32>,
    /// The zoom factor. A written zero means "unchanged" and reads as absent
    /// here; a negative factor is kept.
    pub zoom: Option<f32>,
}

/// A destination.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Dest {
    /// The underlying array, empty when the destination did not resolve.
    pub array: Option<Array>,
}

impl Dest {
    /// Reads a `/Dest` or `/D` value of any type.
    ///
    /// A string **or a name** is looked up through the named-destination
    /// ladder against the *current* document — including for a remote
    /// `/GoToR` destination, whose named target is never resolved in the file
    /// it actually names. That is the behavior, not an oversight to fix.
    #[must_use]
    pub fn create<R: Resolve>(
        catalog: &Dict,
        object: Option<&Object>,
        r: &R,
        limits: &Limits,
        diags: &mut Diagnostics,
    ) -> Dest {
        let array = match object {
            Some(Object::Array(array)) => Some(array.clone()),
            Some(named @ (Object::Str(_) | Object::Name(_))) => {
                let key = named.to_byte_string();
                name_tree::lookup_named_dest(catalog, &key, r, limits, diags)
            }
            _ => None,
        };
        Dest { array }
    }

    /// The view mode.
    ///
    /// The name at index 1 is read **coercively**, so a string `(XYZ)`
    /// matches here even though [`Dest::xyz`] would reject it.
    #[must_use]
    pub fn zoom_mode<R: Resolve>(&self, r: &R) -> ZoomMode {
        let Some(array) = &self.array else {
            return ZoomMode::Unknown;
        };
        let Some(spelling) = array.get(1, r).map(|v| v.get().to_byte_string()) else {
            return ZoomMode::Unknown;
        };
        MODES
            .iter()
            .find(|(_, name, _)| *name == spelling.as_slice())
            .map_or(ZoomMode::Unknown, |(mode, _, _)| *mode)
    }

    /// How many parameters this destination actually carries: the mode's
    /// maximum, capped by what the array holds.
    #[must_use]
    pub fn num_params<R: Resolve>(&self, r: &R) -> usize {
        let Some(array) = &self.array else {
            return 0;
        };
        if array.len() < 2 {
            return 0;
        }
        let mode = self.zoom_mode(r);
        let max = MODES
            .iter()
            .find(|(m, _, _)| *m == mode)
            .map_or(0, |(_, _, max)| *max);
        max.min(array.len() - 2)
    }

    /// Parameter `index`, counting from the first one after the mode name.
    ///
    /// Unbounded: an index past the end reads as zero.
    #[must_use]
    pub fn param(&self, index: usize) -> f32 {
        self.array
            .as_ref()
            .map_or(0.0, |array| array.number_at_or_zero(index + 2))
    }

    /// Every parameter, ignoring the mode entirely.
    #[must_use]
    pub fn params_all(&self) -> Vec<f32> {
        let Some(array) = &self.array else {
            return Vec::new();
        };
        (2..array.len())
            .map(|index| array.number_at_or_zero(index))
            .collect()
    }

    /// The three `XYZ` coordinates, when this is a well-formed `XYZ`
    /// destination.
    ///
    /// Four gates, in order: at least five elements; index 1 is a **name**
    /// (not a string) spelling `XYZ`; each of indices 2–4 is a number or it
    /// is absent; and a zoom of exactly zero counts as absent while an x or y
    /// of zero does not.
    #[must_use]
    pub fn xyz<R: Resolve>(&self, r: &R) -> Option<Xyz> {
        let array = self.array.as_ref()?;
        if array.len() < 5 {
            return None;
        }
        let name = array.get(1, r)?.get().as_name()?.as_bytes().to_vec();
        if name != b"XYZ" {
            return None;
        }
        let number = |index: usize| {
            array
                .get(index, r)
                .and_then(|v| v.get().as_number().and_then(Object::number))
        };
        Some(Xyz {
            x: number(2),
            y: number(3),
            zoom: number(4).filter(|z| *z != 0.0),
        })
    }

    /// The page index this destination names, or `None` when it names none.
    ///
    /// A **number** at index 0 is returned verbatim, with no bounds check
    /// against the document's page count — a file naming page 11 of a
    /// three-page document reports 11. A **dictionary** needs its object
    /// number looked up in the page tree, so an inline page dictionary, which
    /// has no number, cannot be found.
    ///
    /// An out-of-range page index is a `Some`; only a genuine failure — four
    /// of them, all answering alike — is `None`.
    ///
    /// `page_index_of` maps an **object number** — the indirect reference a
    /// dictionary entry carries — to the page it is, so its `u32` argument is
    /// an object number and not an index. Only the answer is a [`PageIndex`].
    #[must_use]
    pub fn page_index<R: Resolve>(
        &self,
        r: &R,
        page_index_of: impl Fn(u32) -> Option<PageIndex>,
        diags: &mut Diagnostics,
    ) -> Option<PageIndex> {
        let entry = self.array.as_ref()?.get(0, r)?;
        if let Some(number) = entry.get().as_number() {
            return u32::try_from(number.as_int()?).ok().map(PageIndex::from);
        }
        if entry.get().as_dict().is_none() {
            diags.record(Severity::Suspicious, DiagKind::DestPageUnresolved, None);
            return None;
        }
        let index = self
            .array
            .as_ref()
            .and_then(|array| array.reference_at(0))
            .and_then(|r| page_index_of(r.num));
        if index.is_none() {
            diags.record(Severity::Suspicious, DiagKind::DestPageUnresolved, None);
        }
        index
    }
}

#[cfg(test)]
mod tests {
    use super::{Dest, ZoomMode};
    use pdfrum_common::{Diagnostics, PageIndex};
    use pdfrum_object::{Array, Name, NoResolve, Object, PdfString};

    fn dest(values: Vec<Object>) -> Dest {
        Dest {
            array: Some(Array::of(values)),
        }
    }

    fn xyz_dest(x: Object, y: Object, z: Object) -> Dest {
        dest(vec![
            Object::Int(0),
            Object::Name(Name::from("XYZ")),
            x,
            y,
            z,
        ])
    }

    #[test]
    fn xyz_needs_five_elements() {
        let short = dest(vec![
            Object::Int(0),
            Object::Name(Name::from("XYZ")),
            Object::Int(4),
        ]);
        assert_eq!(short.xyz(&NoResolve), None);
        let full = xyz_dest(Object::Int(4), Object::Int(5), Object::Int(6));
        let got = full.xyz(&NoResolve).expect("five elements");
        assert_eq!((got.x, got.y, got.zoom), (Some(4.0), Some(5.0), Some(6.0)));
    }

    #[test]
    fn a_zero_zoom_reads_as_absent_but_a_zero_coordinate_does_not() {
        let got = xyz_dest(Object::Int(0), Object::Int(0), Object::Int(0))
            .xyz(&NoResolve)
            .expect("well formed");
        assert_eq!((got.x, got.y, got.zoom), (Some(0.0), Some(0.0), None));
    }

    #[test]
    fn a_negative_zoom_is_kept() {
        let got = xyz_dest(Object::Int(0), Object::Int(0), Object::from(-200.0_f32))
            .xyz(&NoResolve)
            .expect("well formed");
        assert_eq!(got.zoom, Some(-200.0));
    }

    #[test]
    fn a_null_coordinate_is_absent_while_the_destination_still_reads() {
        let got = xyz_dest(Object::Null, Object::Null, Object::Null)
            .xyz(&NoResolve)
            .expect("still well formed");
        assert_eq!((got.x, got.y, got.zoom), (None, None, None));
    }

    #[test]
    fn xyz_type_filters_the_mode_name_where_zoom_mode_does_not() {
        let stringly = dest(vec![
            Object::Int(0),
            Object::Str(PdfString::literal(b"XYZ")),
            Object::Int(1),
            Object::Int(2),
            Object::Int(3),
        ]);
        assert_eq!(stringly.zoom_mode(&NoResolve), ZoomMode::Xyz);
        assert_eq!(stringly.xyz(&NoResolve), None);
    }

    #[test]
    fn a_literal_unknown_mode_name_matches_nothing() {
        let d = dest(vec![Object::Int(0), Object::Name(Name::from("Unknown"))]);
        assert_eq!(d.zoom_mode(&NoResolve), ZoomMode::Unknown);
        assert_eq!(d.num_params(&NoResolve), 0);
    }

    #[test]
    fn the_parameter_count_is_the_modes_maximum_capped_by_the_array() {
        let full = dest(vec![
            Object::Int(0),
            Object::Name(Name::from("FitR")),
            Object::Int(1),
            Object::Int(2),
        ]);
        assert_eq!(full.num_params(&NoResolve), 2);
        assert!((full.param(0) - 1.0).abs() < f32::EPSILON);
        // Past the end reads as zero rather than panicking.
        assert!(full.param(99).abs() < f32::EPSILON);
    }

    #[test]
    fn a_page_number_is_returned_verbatim_with_no_bounds_check() {
        let d = dest(vec![Object::Int(11), Object::Name(Name::from("Fit"))]);
        let mut diags = Diagnostics::default();
        // Page 11 of a document that may have three pages: the number is
        // returned as it stands. `Some` now says "the file named a page",
        // which is a different question from "that page exists" -- the two
        // used to share one `-1` channel and could not be told apart.
        assert_eq!(
            d.page_index(&NoResolve, |_| Some(PageIndex::FIRST), &mut diags),
            Some(PageIndex::new(11))
        );
    }

    #[test]
    fn a_null_destination_answers_none() {
        let mut diags = Diagnostics::default();
        assert_eq!(
            Dest::default().page_index(&NoResolve, |_| Some(PageIndex::new(3)), &mut diags),
            None
        );
    }
}
