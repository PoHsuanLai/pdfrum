//! Check boxes and radio buttons: the two controls whose value is a state
//! name rather than text.
//!
//! They share a machine and differ in one line. A check box **toggles**; a
//! radio button **sets**, unconditionally, and there is no way to un-select
//! one by clicking it.
//!
//! Two rules that look like bugs and are not: **a read-only control consumes
//! the keystroke and does nothing** (consumption and effect are independent),
//! and **neither control handles arrow keys at all**.

/// The off state's appearance name, which every check box and radio button
/// shares.
///
/// A file may name the *on* state anything; this is the only reserved
/// spelling.
///
/// ```
/// use pdfrum_form::field::{ToggleState, toggle::OFF_STATE};
///
/// let mut state = ToggleState::new(OFF_STATE, "Yes");
/// state.set_checked(true);
/// assert_eq!(state.state, "Yes");
/// ```
pub const OFF_STATE: &str = "Off";

/// A check box or radio button's interaction state.
///
/// One fact: which appearance state the control is showing.
///
/// ```
/// use pdfrum_form::field::{ToggleKind, ToggleState, activate, toggle::OFF_STATE};
///
/// let mut state = ToggleState::new(OFF_STATE, "On");
/// assert!(!state.is_checked());
/// activate(&mut state, ToggleKind::Check, false);
/// assert_eq!(state.state, "On");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ToggleState {
    /// The appearance state name currently selected — the "on" name when the
    /// control is checked, [`OFF_STATE`] when it is not.
    pub state: String,
    /// The name this control shows when checked, from its appearance
    /// dictionary. Empty when the control offers no on state at all.
    pub on_state: String,
    /// Which **control** of the field is the checked one, by its raw
    /// `/Annots` index, once one has been chosen.
    ///
    /// A field's controls share one state here, which is right for a check
    /// box (a field with one kid) and not enough for a radio group, where the
    /// clicked control shows its own on state and every other control of the
    /// field shows `Off`. This records the half of that a shared state *can*
    /// express — **which** control is on — so a sibling's appearance can be
    /// answered `Off` without a second state record.
    ///
    /// `None` before any control has been activated, which is the state a
    /// group loaded from a file with no `/V` is in.
    pub checked_control: Option<crate::session::AnnotId>,
}

impl ToggleState {
    /// A control showing `state`, whose checked appearance is `on_state`.
    ///
    /// An empty `on_state` means the control offers no checked appearance at
    /// all, and it can then never be checked.
    ///
    /// ```
    /// use pdfrum_form::field::{ToggleState, toggle::OFF_STATE};
    ///
    /// let state = ToggleState::new(OFF_STATE, "On");
    /// assert!(!state.is_checked());
    /// assert_eq!(state.checked_control, None, "nothing has been clicked yet");
    /// ```
    #[must_use]
    pub fn new(state: impl Into<String>, on_state: impl Into<String>) -> ToggleState {
        ToggleState {
            state: state.into(),
            on_state: on_state.into(),
            checked_control: None,
        }
    }

    /// Whether the control is currently checked.
    ///
    /// An empty state reads as clear rather than as ambiguous: a widget with
    /// no appearance state at all is not checked.
    ///
    /// ```
    /// use pdfrum_form::field::ToggleState;
    ///
    /// assert!(ToggleState::new("On", "On").is_checked());
    /// assert!(!ToggleState::new("Off", "On").is_checked());
    /// assert!(!ToggleState::new("", "On").is_checked());
    /// ```
    #[must_use]
    pub fn is_checked(&self) -> bool {
        self.state != OFF_STATE && !self.state.is_empty()
    }

    /// The appearance state one **control** of this field should draw.
    ///
    /// The per-control answer a shared [`ToggleState`] can give: the clicked
    /// control shows its own on state and every other control of the same
    /// field shows `Off`.
    ///
    /// Three cases, and the middle one is why this is not simply
    /// [`Self::state`]:
    ///
    /// - **Nothing has been clicked** ([`Self::checked_control`] is `None`):
    ///   `None`, meaning "read the widget's own `/AS`" — a group loaded from a
    ///   file must render exactly as the file wrote it, kid by kid.
    /// - **This control is the chosen one**: its own on state, which for a
    ///   radio group is a name only this kid carries.
    /// - **A sibling was chosen**: [`OFF_STATE`], whatever the file's `/AS`
    ///   still says.
    ///
    /// ```
    /// use pdfrum_form::AnnotId;
    /// use pdfrum_form::field::{ToggleState, toggle::OFF_STATE};
    ///
    /// let clicked = AnnotId::new(0u32, 1);
    /// let sibling = AnnotId::new(0u32, 2);
    ///
    /// let mut state = ToggleState::new(OFF_STATE, "Choice1");
    /// // Nothing clicked yet: read each widget's own `/AS`.
    /// assert_eq!(state.state_for_control(clicked), None);
    ///
    /// state.set_checked(true);
    /// state.checked_control = Some(clicked);
    /// assert_eq!(state.state_for_control(clicked), Some("Choice1"));
    /// assert_eq!(state.state_for_control(sibling), Some(OFF_STATE));
    /// ```
    // A check box is a field with one control, so `control` is always the
    // chosen one once anything has been clicked and the answer is its own
    // state either way.
    #[must_use]
    pub fn state_for_control(&self, control: crate::session::AnnotId) -> Option<&str> {
        let chosen = self.checked_control?;
        if chosen == control {
            Some(&self.state)
        } else {
            Some(OFF_STATE)
        }
    }

    /// Sets the control checked or clear.
    ///
    /// Checking a control with no on state leaves it clear: there is no
    /// appearance to show.
    ///
    /// ```
    /// use pdfrum_form::field::{ToggleState, toggle::OFF_STATE};
    ///
    /// let mut none = ToggleState::new(OFF_STATE, "");
    /// none.set_checked(true);
    /// assert!(!none.is_checked());
    /// ```
    pub fn set_checked(&mut self, checked: bool) {
        if checked && !self.on_state.is_empty() {
            self.state.clone_from(&self.on_state);
        } else {
            self.state = OFF_STATE.to_string();
        }
    }

    /// Flips the control. What a click on a check box does.
    ///
    /// ```
    /// use pdfrum_form::field::{ToggleState, toggle::OFF_STATE};
    ///
    /// let mut state = ToggleState::new(OFF_STATE, "On");
    /// state.toggle();
    /// assert!(state.is_checked());
    /// state.toggle();
    /// assert!(!state.is_checked());
    /// ```
    pub fn toggle(&mut self) {
        let checked = self.is_checked();
        self.set_checked(!checked);
    }
}

/// Which of the two controls a widget is, for the one line where they differ.
///
/// ```
/// use pdfrum_form::field::{ToggleKind, ToggleState, activate, toggle::OFF_STATE};
///
/// // A check box flips; a radio button sets and never clears.
/// let mut check = ToggleState::new(OFF_STATE, "On");
/// activate(&mut check, ToggleKind::Check, false);
/// activate(&mut check, ToggleKind::Check, false);
/// assert!(!check.is_checked());
///
/// let mut radio = ToggleState::new(OFF_STATE, "On");
/// activate(&mut radio, ToggleKind::Radio, false);
/// activate(&mut radio, ToggleKind::Radio, false);
/// assert!(radio.is_checked());
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToggleKind {
    /// A check box: activation flips it.
    Check,
    /// A radio button: activation sets it, and never clears it.
    Radio,
}

/// Activates a control — what a click or a Return or Space does.
///
/// Returns whether the state moved. A read-only control never moves, but its
/// caller still reports the event as consumed.
///
/// ```
/// use pdfrum_form::field::{ToggleKind, ToggleState, activate, toggle::OFF_STATE};
///
/// let mut state = ToggleState::new(OFF_STATE, "On");
/// assert!(activate(&mut state, ToggleKind::Check, false));
/// assert!(state.is_checked());
///
/// // A radio button re-activated is a no-op, not a clear.
/// let mut radio = ToggleState::new(OFF_STATE, "On");
/// activate(&mut radio, ToggleKind::Radio, false);
/// assert!(!activate(&mut radio, ToggleKind::Radio, false));
/// assert!(radio.is_checked());
///
/// // A read-only control does not move.
/// let mut fixed = ToggleState::new(OFF_STATE, "On");
/// assert!(!activate(&mut fixed, ToggleKind::Check, true));
/// assert!(!fixed.is_checked());
/// ```
pub fn activate(state: &mut ToggleState, kind: ToggleKind, read_only: bool) -> bool {
    if read_only {
        return false;
    }
    let before = state.state.clone();
    match kind {
        ToggleKind::Check => state.toggle(),
        // A radio button sets. Clicking a selected one again leaves it
        // selected; only a sibling can clear it.
        ToggleKind::Radio => state.set_checked(true),
    }
    state.state != before
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check() -> ToggleState {
        ToggleState::new(OFF_STATE, "On")
    }

    #[test]
    fn a_check_box_flips_each_time_it_is_activated() {
        let mut state = check();
        assert!(!state.is_checked());

        assert!(activate(&mut state, ToggleKind::Check, false));
        assert!(state.is_checked());
        assert_eq!(state.state, "On");

        assert!(activate(&mut state, ToggleKind::Check, false));
        assert!(!state.is_checked());
        assert_eq!(state.state, OFF_STATE);
    }

    /// The asymmetry with a check box, and the whole reason the two share a
    /// machine rather than a function.
    #[test]
    fn a_radio_button_cannot_be_cleared_by_activating_it_again() {
        let mut state = check();
        assert!(activate(&mut state, ToggleKind::Radio, false));
        assert!(state.is_checked());

        // Activating it again is a no-op, not a clear.
        assert!(!activate(&mut state, ToggleKind::Radio, false));
        assert!(state.is_checked());
    }

    /// The event is consumed by the caller; the state does not move. Both
    /// halves are asserted upstream.
    #[test]
    fn a_read_only_control_does_not_move() {
        let mut state = check();
        assert!(!activate(&mut state, ToggleKind::Check, true));
        assert!(!state.is_checked());

        let mut radio = check();
        assert!(!activate(&mut radio, ToggleKind::Radio, true));
        assert!(!radio.is_checked());
    }

    #[test]
    fn a_control_with_no_on_state_stays_clear() {
        let mut state = ToggleState::new(OFF_STATE, "");
        assert!(!activate(&mut state, ToggleKind::Check, false));
        assert!(!state.is_checked());
    }

    /// A file may name the on state anything; "Off" is the only reserved
    /// spelling.
    #[test]
    fn the_on_state_name_comes_from_the_file() {
        let mut state = ToggleState::new(OFF_STATE, "Yes");
        state.set_checked(true);
        assert_eq!(state.state, "Yes");
        assert!(state.is_checked());

        state.set_checked(false);
        assert_eq!(state.state, OFF_STATE);
    }

    /// An empty state is not a checked one — a widget with no appearance
    /// state at all reads as clear rather than as ambiguous.
    #[test]
    fn an_empty_state_reads_as_clear() {
        assert!(!ToggleState::new("", "On").is_checked());
    }
}
