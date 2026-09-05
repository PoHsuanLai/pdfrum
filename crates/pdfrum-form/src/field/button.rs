//! Push buttons: the one control with no value at all.
//!
//! A push button stores nothing and commits nothing. It has a pressed
//! appearance and an action, and the action is the whole point of it.
//!
//! # The asserted-broken behaviour
//!
//! Pressing Return on a focused push button **does not fire its action**, and
//! the event is reported as not consumed. That is what the oracle does today,
//! it is what its own tests assert, and both sites carry an upstream bug
//! reference saying the numbers "should" be one and true. It is reproduced
//! as-asserted: the goldens contain this behaviour, and an implementation
//! that fixed it would fail the tests that pin it. When upstream fixes it,
//! the diff here will be a recognizable one-line change rather than a mystery.

/// A push button's interaction state.
///
/// ```
/// use pdfrum_form::field::ButtonState;
///
/// let mut state = ButtonState::new();
/// assert!(!state.pressed);
/// state.pressed = true;
/// assert!(state.pressed);
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ButtonState {
    /// Whether the button is showing its pressed appearance.
    pub pressed: bool,
}

impl ButtonState {
    /// A released button.
    ///
    /// ```
    /// use pdfrum_form::field::ButtonState;
    ///
    /// assert!(!ButtonState::new().pressed);
    /// ```
    #[must_use]
    pub fn new() -> ButtonState {
        ButtonState::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_button_starts_released() {
        assert!(!ButtonState::new().pressed);
    }
}
