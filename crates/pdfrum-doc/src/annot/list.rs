//! A page's annotation list, which is **not** its `/Annots` array.
//!
//! Two views of the same page differ, and both are correct for their caller:
//!
//! - The `--annot` dump walks `/Annots` directly, so it sees pop-up
//!   annotations that are in the file and does not see the ones synthesized
//!   here.
//! - The rendering list drops the file's pop-ups ("the viewer provides its
//!   own") and appends a synthesized one per markup annotation that carries
//!   text — which is then never drawn, because a pop-up only draws when it is
//!   open and nothing here ever opens one.

use kurbo::Rect;
use pdfrum_object::{Dict, Name, Object, PdfString, Resolve, decode_text, names as obj_names};

use crate::annot::{Annotation, Subtype, is_popup};
use crate::geom;
use crate::names;

/// A page's annotations as the rendering path sees them.
///
/// ```
/// use pdfrum_doc::annot::AnnotList;
/// use pdfrum_object::{Array, Dict, Name, NoResolve, Object, PdfString};
///
/// let note = Dict::from_pairs([
///     (Name::from("Subtype"), Object::Name(Name::from("Text"))),
///     (Name::from("Contents"), Object::Str(PdfString::literal(b"a note"))),
///     (
///         Name::from("Rect"),
///         Object::Array(Array::of([10, 300, 30, 320].map(Object::from))),
///     ),
/// ]);
/// let page = Dict::from_pairs([(
///     Name::from("Annots"),
///     Object::Array(Array::of([Object::Dict(note)])),
/// )]);
///
/// // `page_width` is the crop-box width, which decides where a
/// // synthesized pop-up lands.
/// let list = AnnotList::load(&page, 612.0, &NoResolve);
///
/// assert_eq!(list.annots.len(), 1);
/// // The file declares no pop-up; one is synthesized for the note.
/// assert_eq!(list.popups.len(), 1);
/// ```
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AnnotList {
    /// The annotations the file declares, minus its pop-ups, in `/Annots`
    /// order.
    pub annots: Vec<Annotation>,
    /// Their indices in the original `/Annots` array, which the dump and the
    /// overlay are keyed by.
    pub source_indices: Vec<usize>,
    /// Pop-ups synthesized for the annotations above, appended after them.
    /// Each records which entry of `annots` it belongs to.
    pub popups: Vec<(usize, Annotation)>,
}

impl AnnotList {
    /// Builds the list for one page.
    ///
    /// `page_width` is the crop-box width, which decides where a synthesized
    /// pop-up lands.
    ///
    /// ```
    /// use pdfrum_doc::annot::AnnotList;
    /// use pdfrum_object::{Array, Dict, Name, NoResolve, Object, PdfString};
    ///
    /// let note = Dict::from_pairs([
    ///     (Name::from("Subtype"), Object::Name(Name::from("Text"))),
    ///     (Name::from("Contents"), Object::Str(PdfString::literal(b"a note"))),
    ///     (
    ///         Name::from("Rect"),
    ///         Object::Array(Array::of([10, 300, 30, 320].map(Object::from))),
    ///     ),
    /// ]);
    /// let page = Dict::from_pairs([(
    ///     Name::from("Annots"),
    ///     Object::Array(Array::of([Object::Dict(note)])),
    /// )]);
    ///
    /// // `page_width` is the crop-box width, which decides where a
    /// // synthesized pop-up lands.
    /// let list = AnnotList::load(&page, 612.0, &NoResolve);
    ///
    /// // The index in the original `/Annots` array survives, which is what
    /// // keeps the dump and the appearance overlay aligned.
    /// assert_eq!(list.source_indices, [0]);
    /// ```
    #[must_use]
    pub fn load<R: Resolve>(page: &Dict, page_width: f32, r: &R) -> AnnotList {
        let mut list = AnnotList::default();
        // `/Annots` is not an inherited attribute: it is read off the page's
        // own dictionary or it is not there.
        let Some(array) = page.array(obj_names::ANNOTS, r) else {
            return list;
        };
        for index in 0..array.len() {
            let Some(dict) = array.dict_at(index, r) else {
                continue;
            };
            if is_popup(&dict, r) {
                continue;
            }
            list.annots.push(Annotation::read(&dict, r));
            list.source_indices.push(index);
        }
        for (index, annot) in list.annots.iter().enumerate() {
            if let Some(popup) = create_popup(annot, page_width, r) {
                list.popups.push((index, popup));
            }
        }
        list
    }
}

/// Whether an annotation of this subtype gets a synthesized pop-up.
///
/// Free text and widgets do not, despite both carrying text.
#[must_use]
pub fn popup_appears_for(subtype: Subtype) -> bool {
    matches!(
        subtype,
        Subtype::Text
            | Subtype::Line
            | Subtype::Square
            | Subtype::Circle
            | Subtype::Polygon
            | Subtype::PolyLine
            | Subtype::Highlight
            | Subtype::Underline
            | Subtype::Squiggly
            | Subtype::StrikeOut
            | Subtype::Stamp
            | Subtype::Caret
            | Subtype::Ink
            | Subtype::FileAttachment
            | Subtype::Redact
    )
}

/// Synthesizes a pop-up for one annotation, when it deserves one.
///
/// The emptiness test runs on the **decoded** text, not the raw bytes: a bare
/// byte-order mark, or a mark followed by nothing but a language-code region,
/// both decode to nothing and suppress the pop-up even though the raw string
/// is non-empty.
fn create_popup<R: Resolve>(parent: &Annotation, page_width: f32, r: &R) -> Option<Annotation> {
    if !popup_appears_for(parent.subtype) {
        return None;
    }
    let contents = parent.dict.byte_string(obj_names::CONTENTS, r)?;
    if decode_text(&contents).is_empty() {
        return None;
    }

    let rect = popup_rect(geom::normalize(parent.rect), page_width);
    let title = parent.dict.byte_string(obj_names::T, r).unwrap_or_default();
    let dict = Dict::from_pairs([
        (obj_names::TYPE.clone(), Object::Name(names::ANNOT.clone())),
        (
            obj_names::SUBTYPE.clone(),
            Object::Name(Name::new(Subtype::Popup.as_bytes().to_vec())),
        ),
        (obj_names::T.clone(), Object::Str(PdfString::literal(title))),
        (
            obj_names::CONTENTS.clone(),
            Object::Str(PdfString::literal(contents)),
        ),
        (
            obj_names::RECT.clone(),
            Object::Array(pdfrum_object::Array::of([
                Object::from(geom::left(rect)),
                Object::from(geom::bottom(rect)),
                Object::from(geom::right(rect)),
                Object::from(geom::top(rect)),
            ])),
        ),
        (obj_names::F.clone(), Object::Int(0)),
    ]);
    Some(Annotation::read(&dict, r))
}

/// Where a synthesized 200×200 pop-up lands beside its parent.
fn popup_rect(parent: Rect, page_width: f32) -> Rect {
    let (left, bottom, right, top) = (
        geom::left(parent),
        geom::bottom(parent),
        geom::right(parent),
        geom::top(parent),
    );
    let base = geom::rect(0.0, 0.0, 200.0, 200.0);
    if left + 200.0 > page_width && bottom - 200.0 < 0.0 {
        // No room to the right and none below: hang it off the parent's
        // top-right corner instead.
        geom::translate(base, right - 200.0, top)
    } else {
        geom::translate(
            base,
            left.min(page_width - 200.0),
            (bottom - 200.0).max(0.0),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{AnnotList, popup_appears_for, popup_rect};
    use crate::annot::Subtype;
    use crate::geom;
    use pdfrum_object::{Array, Dict, Name, NoResolve, Object, PdfString};

    fn dict(pairs: &[(&str, Object)]) -> Dict {
        Dict::from_pairs(
            pairs
                .iter()
                .map(|(k, v)| (Name::from(*k), v.clone()))
                .collect::<Vec<_>>(),
        )
    }

    fn page_with(annots: &[Object]) -> Dict {
        dict(&[("Annots", Object::Array(Array::of(annots.to_vec())))])
    }

    fn text_annot(contents: &[u8]) -> Object {
        Object::Dict(dict(&[
            ("Subtype", Object::Name(Name::from("Text"))),
            ("Contents", Object::Str(PdfString::literal(contents))),
            (
                "Rect",
                Object::Array(Array::of([
                    Object::from(10.0_f32),
                    Object::from(300.0_f32),
                    Object::from(30.0_f32),
                    Object::from(320.0_f32),
                ])),
            ),
        ]))
    }

    #[test]
    fn a_text_annotation_with_content_gains_one_synthesized_popup() {
        let page = page_with(&[text_annot(b"Aa\xE4\xA0")]);
        let list = AnnotList::load(&page, 612.0, &NoResolve);
        assert_eq!(list.annots.len(), 1);
        assert_eq!(list.popups.len(), 1);
        // The raw bytes travel through verbatim.
        let popup = list.popups.first().map(|(_, popup)| popup);
        assert_eq!(
            popup
                .and_then(|popup| popup.dict.string(pdfrum_object::names::CONTENTS))
                .map(|s| s.as_bytes().to_vec()),
            Some(b"Aa\xE4\xA0".to_vec())
        );
    }

    #[test]
    fn emptiness_is_tested_after_decoding_not_before() {
        // Empty, a bare byte-order mark, and a mark followed by nothing but a
        // language-code region all decode to nothing.
        for contents in [
            &b""[..],
            &b"\xFE\xFF"[..],
            &b"\xFE\xFF\x00\x1Bja\x00\x1B"[..],
        ] {
            let page = page_with(&[text_annot(contents)]);
            let list = AnnotList::load(&page, 612.0, &NoResolve);
            assert_eq!(list.popups.len(), 0, "{contents:?}");
        }
    }

    #[test]
    fn a_popup_in_the_file_is_dropped_from_the_list() {
        let popup = Object::Dict(dict(&[("Subtype", Object::Name(Name::from("Popup")))]));
        let page = page_with(&[popup, text_annot(b"x")]);
        let list = AnnotList::load(&page, 612.0, &NoResolve);
        assert_eq!(list.annots.len(), 1);
        // The index it occupied in `/Annots` survives, which is what keeps
        // the dump and the overlay aligned.
        assert_eq!(list.source_indices, vec![1]);
    }

    #[test]
    fn free_text_and_widgets_get_no_popup() {
        assert!(!popup_appears_for(Subtype::FreeText));
        assert!(!popup_appears_for(Subtype::Widget));
        assert!(popup_appears_for(Subtype::Text));
        assert!(popup_appears_for(Subtype::Redact));
    }

    #[test]
    fn the_popup_hugs_the_parents_left_edge_until_the_page_runs_out() {
        // 690.511 wide, parent at left 500 with top 360.046: the left is
        // clamped to the page width less 200.
        let parent = geom::rect(500.0, 260.046, 520.0, 360.046);
        let placed = popup_rect(parent, 690.511);
        assert!((geom::left(placed) - 490.511).abs() < 1e-3, "{placed:?}");
        assert!((geom::bottom(placed) - 60.046).abs() < 1e-3);
    }

    #[test]
    fn a_bottom_right_parent_hangs_its_popup_off_its_own_corner() {
        // No room to the right and none below.
        let parent = geom::rect(500.0, 100.0, 560.0, 180.0);
        let placed = popup_rect(parent, 612.0);
        assert!((geom::left(placed) - 360.0).abs() < f32::EPSILON);
        assert!((geom::bottom(placed) - 180.0).abs() < f32::EPSILON);
    }
}
