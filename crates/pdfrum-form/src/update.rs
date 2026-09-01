//! What applying an event hands back.
//!
//! This is the crate's **entire** output channel, and its shape is the design
//! decision the module exists to record: nothing is pushed at a caller.
//!
//! The C++ this replaces fills a thirty-slot table of function pointers and
//! calls out through it — invalidate this rectangle, set that cursor, start a
//! timer, focus changed, follow this link. Half the complexity of its widget
//! layer exists to feed the first of those: a dirty-rectangle accumulator, a
//! per-line rectangle push, a one-unit inflation, and a lifetime protocol
//! repeated four times so the callback target can be torn down mid-call.
//!
//! All of it goes. `apply` **returns** what changed, and the caller re-renders
//! whatever it likes — which is exactly what a golden comparison does anyway,
//! since it diffs whole pages. Dirty rectangles describe pixels to someone
//! else; a library that hands back appearance streams has nobody to describe
//! them to.
//!
//! Two things become better rather than merely smaller in the process.
//! Actions come back as a **request** the caller may inspect, ignore or
//! perform, instead of a callback that has already fired by the time anyone
//! could object. And a focus change is an ordinary observation in the
//! returned list rather than a callback that only exists in some versions of
//! the embedding interface — which is why the version split upstream has here
//! has no analogue.

use pdfrum_doc::GeneratedAp;
use pdfrum_doc::nav::Action;

use crate::event::Modifiers;
use crate::session::AnnotId;

/// One thing that changed, and what the caller may do about it.
#[derive(Debug, Clone, PartialEq)]
pub struct AppearanceUpdate {
    /// Which annotation it concerns.
    pub annot: AnnotId,
    /// What happened to it.
    pub kind: UpdateKind,
}

impl AppearanceUpdate {
    /// An update about one annotation.
    #[must_use]
    pub fn new(annot: AnnotId, kind: UpdateKind) -> AppearanceUpdate {
        AppearanceUpdate { annot, kind }
    }
}

/// What happened to an annotation.
#[derive(Debug, Clone, PartialEq)]
pub enum UpdateKind {
    /// A committed value produced a new appearance stream.
    ///
    /// The unfocused path: the field is drawn from what it now stores.
    Regenerated(Box<GeneratedAp>),
    /// A focused field's live editor state, with its caret and selection
    /// band.
    ///
    /// The focused path. Which of these two a field takes is the whole seam
    /// between appearance generation and interaction, and it is one `if`.
    LiveEdit(Box<GeneratedAp>),
    /// The widget has no generated appearance any more: it falls back to
    /// the **file's own** `/AP`.
    ///
    /// # This is not "draw nothing"
    ///
    /// The distinction matters because the two are easy to conflate and only
    /// one of them is expressible. A caller applies these updates by keying
    /// an appearance overlay, and *absence* from that overlay already means
    /// "use whatever the file declares". So this variant says: drop any
    /// appearance an earlier event generated for this widget, and let the
    /// file's own stream show through again.
    ///
    /// Suppressing a widget that has an `/AP` — painting nothing where the
    /// file says something — is a different instruction, and this cannot
    /// carry it: an overlay keyed by presence has no way to spell a positive
    /// "blank". Nothing here needs it. A widget that genuinely draws nothing
    /// is one whose field type gets no appearance at all — a signature, or a
    /// type the classifier could not name — and those never receive
    /// interaction state, so they never produce an update of any kind. If a
    /// later milestone needs real suppression, it needs a new variant *and* a
    /// positive marker in the overlay, not a re-reading of this one.
    RevertedToFileAppearance,
    /// An action fired and the caller decides whether to perform it.
    ///
    /// The modifiers held when it fired ride along, because a link's action
    /// is expected to see them.
    ActionRequested {
        /// What was asked for.
        action: Box<Action>,
        /// Which modifiers were held.
        modifiers: Modifiers,
    },
    /// Focus moved.
    ///
    /// An observation rather than a callback, which is why there is no
    /// version of this interface that omits it.
    FocusChanged {
        /// What had focus before, if anything.
        from: Option<AnnotId>,
        /// What has it now, if anything.
        to: Option<AnnotId>,
    },
}

impl UpdateKind {
    /// Whether this update carries a new appearance stream, by either path.
    #[must_use]
    pub fn appearance(&self) -> Option<&GeneratedAp> {
        match self {
            UpdateKind::Regenerated(ap) | UpdateKind::LiveEdit(ap) => Some(ap),
            _ => None,
        }
    }

    /// Whether this update is the focused-field path.
    #[must_use]
    pub fn is_live_edit(&self) -> bool {
        matches!(self, UpdateKind::LiveEdit(_))
    }
}

/// What applying an event produced.
///
/// Both halves of the answer, because the oracle's single boolean conflates
/// them: `consumed` says the event was handled — which is **not** the same as
/// saying it had an effect, since a read-only control consumes a keystroke
/// and does nothing — and `updates` says what changed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Response {
    /// Whether the event was handled.
    pub consumed: bool,
    /// What changed, in the order it changed.
    pub updates: Vec<AppearanceUpdate>,
}

impl Response {
    /// An event nothing handled.
    #[must_use]
    pub fn ignored() -> Response {
        Response {
            consumed: false,
            updates: Vec::new(),
        }
    }

    /// An event that was handled but changed nothing to draw.
    ///
    /// The answer a read-only control gives: it took the keystroke and
    /// declined to act on it.
    #[must_use]
    pub fn consumed() -> Response {
        Response {
            consumed: true,
            updates: Vec::new(),
        }
    }

    /// An event that was handled and changed something.
    #[must_use]
    pub fn with(updates: Vec<AppearanceUpdate>) -> Response {
        Response {
            consumed: true,
            updates,
        }
    }

    /// Adds one update, keeping the order.
    pub fn push(&mut self, update: AppearanceUpdate) {
        self.updates.push(update);
    }

    /// Folds another response in, keeping both orders and consuming if either
    /// did.
    pub fn absorb(&mut self, other: Response) {
        self.consumed |= other.consumed;
        self.updates.extend(other.updates);
    }

    /// The actions the caller has been asked to perform, in order.
    pub fn actions(&self) -> impl Iterator<Item = (&Action, Modifiers)> {
        self.updates.iter().filter_map(|u| match &u.kind {
            UpdateKind::ActionRequested { action, modifiers } => {
                Some((action.as_ref(), *modifiers))
            }
            _ => None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::AnnotId;
    use pdfrum_object::Dict;

    fn annot() -> AnnotId {
        AnnotId::new(0, 3)
    }

    fn action() -> Action {
        Action::new(Dict::new())
    }

    fn appearance() -> GeneratedAp {
        GeneratedAp {
            stream: Vec::new(),
            bbox: kurbo::Rect::ZERO,
            matrix: kurbo::Affine::IDENTITY,
            resources: Dict::new(),
            rect_override: None,
            as_override: None,
        }
    }

    /// Consumption and effect are independent, which is the distinction the
    /// oracle's single boolean cannot make.
    #[test]
    fn an_event_can_be_consumed_without_changing_anything() {
        let response = Response::consumed();
        assert!(response.consumed);
        assert!(response.updates.is_empty());

        let ignored = Response::ignored();
        assert!(!ignored.consumed);
        assert!(ignored.updates.is_empty());
    }

    #[test]
    fn folding_responses_keeps_both_orders() {
        let mut first = Response::with(vec![AppearanceUpdate::new(
            annot(),
            UpdateKind::RevertedToFileAppearance,
        )]);
        let second = Response::with(vec![AppearanceUpdate::new(
            AnnotId::new(0, 4),
            UpdateKind::FocusChanged {
                from: None,
                to: Some(AnnotId::new(0, 4)),
            },
        )]);

        first.absorb(second);
        let order: Vec<AnnotId> = first.updates.iter().map(|u| u.annot).collect();
        assert_eq!(order, vec![annot(), AnnotId::new(0, 4)]);
    }

    /// Folding an ignored response into a consuming one keeps it consuming.
    #[test]
    fn absorbing_an_ignored_response_does_not_un_consume() {
        let mut consumed = Response::consumed();
        consumed.absorb(Response::ignored());
        assert!(consumed.consumed);

        let mut ignored = Response::ignored();
        ignored.absorb(Response::consumed());
        assert!(ignored.consumed, "either half consuming is enough");
    }

    /// Actions come back as requests in order, with the modifiers that were
    /// held — which is what a link's action expects to see.
    #[test]
    fn actions_come_back_in_order_with_their_modifiers() {
        let mut response = Response::default();
        for modifiers in [
            Modifiers::NONE,
            Modifiers::CONTROL,
            Modifiers::SHIFT,
            Modifiers::SHIFT | Modifiers::CONTROL,
        ] {
            response.push(AppearanceUpdate::new(
                annot(),
                UpdateKind::ActionRequested {
                    action: Box::new(action()),
                    modifiers,
                },
            ));
        }

        let seen: Vec<u32> = response.actions().map(|(_, m)| m.0).collect();
        // The raw bit values are part of the contract, not an internal choice.
        assert_eq!(seen, vec![0, 2, 1, 3]);
    }

    #[test]
    fn a_response_with_no_actions_yields_none() {
        let response = Response::with(vec![AppearanceUpdate::new(
            annot(),
            UpdateKind::RevertedToFileAppearance,
        )]);
        assert_eq!(response.actions().count(), 0);
    }

    /// Reverting carries no appearance, which is what makes it mean "use the
    /// file's own" rather than "use this blank one".
    #[test]
    fn reverting_carries_no_appearance_of_its_own() {
        let reverted = UpdateKind::RevertedToFileAppearance;
        assert!(reverted.appearance().is_none());
        assert!(!reverted.is_live_edit());

        // And it is a distinct answer from generating one, which is the whole
        // point: a caller keys an overlay by presence, so these two take
        // different branches.
        let generated = UpdateKind::Regenerated(Box::new(appearance()));
        assert_ne!(reverted, generated);
        assert!(generated.appearance().is_some());
    }

    /// The focused and unfocused paths are distinguishable, because which one
    /// a field took is the question a golden difference asks.
    #[test]
    fn the_two_appearance_paths_are_distinguishable() {
        let live = UpdateKind::LiveEdit(Box::new(appearance()));
        let regenerated = UpdateKind::Regenerated(Box::new(appearance()));

        assert!(live.is_live_edit());
        assert!(!regenerated.is_live_edit());
        assert!(live.appearance().is_some());
        assert!(regenerated.appearance().is_some());
        assert!(UpdateKind::RevertedToFileAppearance.appearance().is_none());
    }
}
