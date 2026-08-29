//! Link annotations (ISO 32000-1 §12.5.6.5) and hit-testing a page for one.
//!
//! The per-page list keeps a **null placeholder for every non-link entry** in
//! `/Annots`, which looks wasteful until you see what it buys: the z-order a
//! hit test reports is the index in the *full* array, so a caller can turn a
//! hit straight back into an annotation index. Losing the placeholders would
//! renumber every link after the first non-link.

use kurbo::Point;
use pdfrum_common::{Diagnostics, Limits};
use pdfrum_object::{Dict, Resolve, names as obj_names};

use crate::geom;
use crate::names;
use crate::nav::action::Action;
use crate::nav::dest::Dest;

/// A link annotation.
#[derive(Debug, Clone, PartialEq)]
pub struct Link {
    /// The annotation dictionary.
    pub dict: Dict,
}

impl Link {
    /// Wraps a dictionary as a link.
    #[must_use]
    pub fn new(dict: Dict) -> Link {
        Link { dict }
    }

    /// The link's `/Rect`, **not** normalized.
    #[must_use]
    pub fn rect<R: Resolve>(&self, r: &R) -> kurbo::Rect {
        self.dict.rect(obj_names::RECT, r)
    }

    /// The link's action, when it has one.
    #[must_use]
    pub fn action<R: Resolve>(&self, r: &R) -> Option<Action> {
        self.dict.dict(names::A, r).map(Action::new)
    }

    /// Where the link goes.
    ///
    /// `/Dest` is tried first and the action's destination only when that
    /// yields no array — the same two-rung fallback a viewer applies.
    #[must_use]
    pub fn dest<R: Resolve>(
        &self,
        catalog: &Dict,
        r: &R,
        limits: &Limits,
        diags: &mut Diagnostics,
    ) -> Dest {
        let own = self.dict.get(names::DEST, r).map(|d| d.get().clone());
        let dest = Dest::create(catalog, own.as_ref(), r, limits, diags);
        if dest.array.is_some() {
            return dest;
        }
        self.action(r)
            .map(|action| action.dest(catalog, r, limits, diags))
            .unwrap_or_default()
    }
}

/// A page's links, with a gap where every non-link annotation sits.
#[must_use]
pub fn page_links<R: Resolve>(page: &Dict, r: &R) -> Vec<Option<Link>> {
    let Some(array) = page.array(obj_names::ANNOTS, r) else {
        return Vec::new();
    };
    (0..array.len())
        .map(|index| {
            let dict = array.dict_at(index, r)?;
            // Read coercively, so a *string* `/Subtype (Link)` is a link.
            let subtype = dict.byte_string(obj_names::SUBTYPE, r)?;
            (subtype == b"Link").then(|| Link::new(dict))
        })
        .collect()
}

/// The topmost link containing a point, with its index in `/Annots`.
///
/// The scan runs **backwards**, so the last annotation in painting order —
/// the one on top — wins a tie. Containment is inclusive on all four edges.
#[must_use]
pub fn link_at_point<R: Resolve>(
    links: &[Option<Link>],
    point: Point,
    r: &R,
) -> Option<(usize, Link)> {
    for (index, slot) in links.iter().enumerate().rev() {
        let Some(link) = slot else { continue };
        if geom::contains(link.rect(r), point) {
            return Some((index, link.clone()));
        }
    }
    None
}

/// Finds the next link at or after `start`, ignoring the placeholders.
///
/// This is the plain enumeration a caller walking every link wants; it does
/// not preserve z-order gaps because it reports each link's own index.
#[must_use]
pub fn enumerate_links<R: Resolve>(page: &Dict, start: usize, r: &R) -> Option<(usize, Link)> {
    let array = page.array(obj_names::ANNOTS, r)?;
    for index in start..array.len() {
        let Some(dict) = array.dict_at(index, r) else {
            continue;
        };
        if dict.byte_string(obj_names::SUBTYPE, r).as_deref() == Some(b"Link") {
            return Some((index, Link::new(dict)));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{link_at_point, page_links};
    use kurbo::Point;
    use pdfrum_object::{Array, Dict, Name, NoResolve, Object, PdfString};

    fn dict(pairs: &[(&str, Object)]) -> Dict {
        Dict::from_pairs(
            pairs
                .iter()
                .map(|(k, v)| (Name::from(*k), v.clone()))
                .collect::<Vec<_>>(),
        )
    }

    fn annot(subtype: Object, rect: [f32; 4]) -> Object {
        Object::Dict(dict(&[
            ("Subtype", subtype),
            ("Rect", Object::Array(Array::of(rect.map(Object::from)))),
        ]))
    }

    #[test]
    fn non_links_leave_gaps_so_the_z_order_stays_the_annots_index() {
        let page = dict(&[(
            "Annots",
            Object::Array(Array::of([
                annot(Object::Name(Name::from("Square")), [0.0, 0.0, 10.0, 10.0]),
                annot(Object::Name(Name::from("Link")), [0.0, 0.0, 10.0, 10.0]),
            ])),
        )]);
        let links = page_links(&page, &NoResolve);
        assert_eq!(links.len(), 2);
        assert!(links.first().is_some_and(Option::is_none));
        assert!(links.get(1).is_some_and(Option::is_some));
        assert_eq!(
            link_at_point(&links, Point::new(5.0, 5.0), &NoResolve).map(|(i, _)| i),
            Some(1)
        );
    }

    #[test]
    fn a_string_subtype_still_names_a_link() {
        let page = dict(&[(
            "Annots",
            Object::Array(Array::of([annot(
                Object::Str(PdfString::literal(b"Link")),
                [0.0, 0.0, 1.0, 1.0],
            )])),
        )]);
        assert!(
            page_links(&page, &NoResolve)
                .first()
                .is_some_and(Option::is_some)
        );
    }

    #[test]
    fn the_topmost_link_wins_an_overlap() {
        let page = dict(&[(
            "Annots",
            Object::Array(Array::of([
                annot(Object::Name(Name::from("Link")), [0.0, 0.0, 20.0, 20.0]),
                annot(Object::Name(Name::from("Link")), [0.0, 0.0, 20.0, 20.0]),
            ])),
        )]);
        let links = page_links(&page, &NoResolve);
        assert_eq!(
            link_at_point(&links, Point::new(1.0, 1.0), &NoResolve).map(|(i, _)| i),
            Some(1)
        );
    }

    #[test]
    fn containment_is_inclusive_on_the_edges() {
        let page = dict(&[(
            "Annots",
            Object::Array(Array::of([annot(
                Object::Name(Name::from("Link")),
                [10.0, 10.0, 20.0, 20.0],
            )])),
        )]);
        let links = page_links(&page, &NoResolve);
        assert!(link_at_point(&links, Point::new(10.0, 10.0), &NoResolve).is_some());
        assert!(link_at_point(&links, Point::new(20.0, 20.0), &NoResolve).is_some());
        assert!(link_at_point(&links, Point::new(9.9, 15.0), &NoResolve).is_none());
    }

    #[test]
    fn a_page_with_no_annots_has_no_links() {
        assert!(page_links(&Dict::new(), &NoResolve).is_empty());
    }
}
