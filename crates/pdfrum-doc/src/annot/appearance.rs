//! Finding an annotation's appearance stream, and placing it on the page.
//!
//! The lookup is a ladder every rung of which has a trap, so it is written
//! once and called from everywhere:
//!
//! 1. `/AP` must be a dictionary.
//! 2. The mode's entry (`/N`, `/R`, `/D`); when the caller allows it, a
//!    **missing key** — not an unusable value — falls back to `/N`.
//! 3. A stream there is the answer.
//! 4. Otherwise it must be a dictionary of states, chosen by `/AS`.
//! 5. With no `/AS`, `/V` on the annotation, then `/V` on its immediate
//!    `/Parent` — one level, not the full inherited-attribute walk — and the
//!    chosen name must actually be a key of the state dictionary or the state
//!    is `Off`.

use kurbo::Affine;
use pdfrum_object::{Dict, Name, Object, Resolve, Stream};

use crate::annot::Annotation;
use crate::geom;
use crate::names;

/// Which appearance a lookup wants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ApMode {
    /// The normal appearance (`/N`).
    #[default]
    Normal,
    /// The rollover appearance (`/R`).
    Rollover,
    /// The pressed appearance (`/D`).
    Down,
}

impl ApMode {
    /// The `/AP` sub-key this mode reads.
    fn key(self) -> &'static Name {
        match self {
            ApMode::Normal => names::N,
            ApMode::Rollover => names::R,
            ApMode::Down => names::D,
        }
    }
}

/// Resolves an annotation's appearance stream.
///
/// `fallback_to_normal` decides what a missing rollover or down entry means.
/// With it set, a **missing key** falls back to `/N`; a key that is present
/// but holds nothing usable does **not** — it suppresses the fallback and the
/// lookup comes back empty.
#[must_use]
pub fn annot_ap<R: Resolve>(
    dict: &Dict,
    mode: ApMode,
    fallback_to_normal: bool,
    r: &R,
) -> Option<Stream> {
    let ap = dict.dict(names::AP, r)?;
    let mut entry = mode.key();
    if fallback_to_normal && !ap.contains_key(entry) {
        entry = names::N;
    }
    let sub = ap.get(entry, r)?.get().clone();
    if let Object::Stream(stream) = sub {
        return Some(stream);
    }
    let states = sub.as_dict()?;

    let mut state = dict.byte_string(names::AS, r).unwrap_or_default();
    if state.is_empty() {
        // Both `/AS` and `/V` are read coercively, so a name `/V /Yes` and a
        // string `/V (Yes)` choose the same state.
        let mut value = dict.byte_string(names::V, r).unwrap_or_default();
        if value.is_empty() {
            value = dict
                .dict(names::PARENT, r)
                .and_then(|parent| parent.byte_string(names::V, r))
                .unwrap_or_default();
        }
        // A value naming a state the dictionary lacks falls back to `Off`,
        // not to whichever single on-state exists.
        state = if !value.is_empty() && states.contains_key(&Name::new(value.clone())) {
            value
        } else {
            names::OFF.as_bytes().to_vec()
        };
    }
    states.stream(&Name::new(state), r)
}

/// Whether an annotation already carries a usable normal appearance.
///
/// The test is that `/AP /N` reads as a **dictionary** — and a stream answers
/// with its own dictionary, so the overwhelmingly common "a stream is there"
/// case suppresses generation just as a multi-state dictionary does. Only a
/// missing `/AP`, a missing `/N`, or a scalar `/N` leaves the door open.
#[must_use]
pub fn has_appearance<R: Resolve>(dict: &Dict, r: &R) -> bool {
    dict.dict(names::AP, r)
        .and_then(|ap| ap.dict(names::N, r))
        .is_some()
}

/// The transform placing an annotation's appearance form on the page.
///
/// The form's `/BBox`, mapped through its own `/Matrix`, is fitted into the
/// annotation's **normalized** rectangle. A `NoRotate` annotation on a
/// rotated page then counter-rotates about the rectangle's **top-left**
/// corner, which is where the anchor comes from.
#[must_use]
pub(crate) fn annot_matrix(
    annot: &Annotation,
    form_dict: &Dict,
    page_quarter_turns: u8,
    user_to_device: Affine,
    r: &impl Resolve,
) -> Affine {
    let form_matrix = form_dict.matrix(names::MATRIX, r);
    let form_bbox = geom::transform_rect(form_matrix, form_dict.rect(names::BBOX, r));
    let rect = geom::normalize(annot.rect);
    let mut matrix = geom::match_rect(rect, form_bbox);

    if annot.flags.no_rotate() && page_quarter_turns != 0 {
        let (ox, oy) = (rect.x0, rect.y1);
        let angle = std::f64::consts::FRAC_PI_2 * f64::from(page_quarter_turns);
        matrix = matrix
            * Affine::translate((-ox, -oy))
            * Affine::rotate(angle)
            * Affine::translate((ox, oy));
    }
    matrix * user_to_device
}

#[cfg(test)]
mod tests {
    use super::{ApMode, annot_ap, has_appearance};
    use pdfrum_object::{ByteSpan, Dict, Name, NoResolve, Object, PdfString, Stream};

    fn dict(pairs: &[(&str, Object)]) -> Dict {
        Dict::from_pairs(
            pairs
                .iter()
                .map(|(k, v)| (Name::from(*k), v.clone()))
                .collect::<Vec<_>>(),
        )
    }

    fn stream(marker: &[u8]) -> Object {
        Object::Stream(Stream::new(Dict::new(), ByteSpan::from(marker.to_vec())))
    }

    fn found(annot: &Dict, mode: ApMode, fallback: bool) -> Option<Vec<u8>> {
        annot_ap(annot, mode, fallback, &NoResolve).map(|s| s.data.as_bytes().to_vec())
    }

    #[test]
    fn no_appearance_dictionary_finds_nothing() {
        assert_eq!(found(&Dict::new(), ApMode::Normal, true), None);
        assert_eq!(
            found(&dict(&[("AP", Object::Int(4))]), ApMode::Normal, true),
            None
        );
    }

    #[test]
    fn a_direct_stream_is_the_answer() {
        let annot = dict(&[("AP", Object::Dict(dict(&[("N", stream(b"body"))])))]);
        assert_eq!(found(&annot, ApMode::Normal, true), Some(b"body".to_vec()));
    }

    #[test]
    fn the_mode_fallback_tests_presence_not_usability() {
        let ap_missing_r = dict(&[("AP", Object::Dict(dict(&[("N", stream(b"n"))])))]);
        assert_eq!(
            found(&ap_missing_r, ApMode::Rollover, true),
            Some(b"n".to_vec())
        );
        assert_eq!(found(&ap_missing_r, ApMode::Rollover, false), None);

        // A present-but-null `/R` suppresses the fallback and yields nothing.
        let ap_null_r = dict(&[(
            "AP",
            Object::Dict(dict(&[("N", stream(b"n")), ("R", Object::Null)])),
        )]);
        assert_eq!(found(&ap_null_r, ApMode::Rollover, true), None);
    }

    #[test]
    fn a_state_dictionary_is_chosen_by_the_appearance_state() {
        let states = dict(&[("Yes", stream(b"on")), ("Off", stream(b"off"))]);
        let with_as = dict(&[
            (
                "AP",
                Object::Dict(dict(&[("N", Object::Dict(states.clone()))])),
            ),
            ("AS", Object::Name(Name::from("Yes"))),
        ]);
        assert_eq!(found(&with_as, ApMode::Normal, true), Some(b"on".to_vec()));

        // With no `/AS`, `/V` on the annotation decides — coercively, so a
        // string works as well as a name.
        let with_v = dict(&[
            (
                "AP",
                Object::Dict(dict(&[("N", Object::Dict(states.clone()))])),
            ),
            ("V", Object::Str(PdfString::literal(b"Yes"))),
        ]);
        assert_eq!(found(&with_v, ApMode::Normal, true), Some(b"on".to_vec()));

        // A `/V` naming a state the dictionary lacks falls back to `Off`,
        // not to the only on-state.
        let wrong_v = dict(&[
            (
                "AP",
                Object::Dict(dict(&[("N", Object::Dict(states.clone()))])),
            ),
            ("V", Object::Name(Name::from("Nope"))),
        ]);
        assert_eq!(found(&wrong_v, ApMode::Normal, true), Some(b"off".to_vec()));

        // With neither, `Off` again.
        let bare = dict(&[("AP", Object::Dict(dict(&[("N", Object::Dict(states))])))]);
        assert_eq!(found(&bare, ApMode::Normal, true), Some(b"off".to_vec()));
    }

    #[test]
    fn the_parent_is_consulted_one_level_only() {
        let states = dict(&[("Yes", stream(b"on")), ("Off", stream(b"off"))]);
        let grandparent = dict(&[("V", Object::Name(Name::from("Yes")))]);
        let parent = dict(&[("Parent", Object::Dict(grandparent))]);
        let annot = dict(&[
            (
                "AP",
                Object::Dict(dict(&[("N", Object::Dict(states.clone()))])),
            ),
            ("Parent", Object::Dict(parent)),
        ]);
        // The grandparent's `/V` is invisible to this ladder.
        assert_eq!(found(&annot, ApMode::Normal, true), Some(b"off".to_vec()));

        let direct_parent = dict(&[("V", Object::Name(Name::from("Yes")))]);
        let annot = dict(&[
            ("AP", Object::Dict(dict(&[("N", Object::Dict(states))]))),
            ("Parent", Object::Dict(direct_parent)),
        ]);
        assert_eq!(found(&annot, ApMode::Normal, true), Some(b"on".to_vec()));
    }

    #[test]
    fn a_non_stream_state_value_yields_nothing() {
        let states = dict(&[("Yes", Object::Int(5))]);
        let annot = dict(&[
            ("AP", Object::Dict(dict(&[("N", Object::Dict(states))]))),
            ("AS", Object::Name(Name::from("Yes"))),
        ]);
        assert_eq!(found(&annot, ApMode::Normal, true), None);
    }

    #[test]
    fn a_stream_normal_appearance_counts_as_having_one() {
        let with_stream = dict(&[("AP", Object::Dict(dict(&[("N", stream(b"x"))])))]);
        assert!(has_appearance(&with_stream, &NoResolve));

        let with_states = dict(&[(
            "AP",
            Object::Dict(dict(&[("N", Object::Dict(dict(&[("Yes", stream(b"x"))])))])),
        )]);
        assert!(has_appearance(&with_states, &NoResolve));

        // A scalar `/N` leaves generation open, as does a missing `/AP`.
        let scalar = dict(&[("AP", Object::Dict(dict(&[("N", Object::Int(5))])))]);
        assert!(!has_appearance(&scalar, &NoResolve));
        assert!(!has_appearance(&Dict::new(), &NoResolve));
    }
}
