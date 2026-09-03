//! Errors, alerts and colour effects this crate can return.
//!
//! `Err` is for the cases that throw a JavaScript exception; a keystroke
//! *rejection* is [`crate::af::AfOutcome::Rejected`] /
//! [`crate::af::KeystrokeOutcome::Reject`] instead, not an error.
//!
//! **The message strings are API, not diagnostics.** Acrobat's table has no
//! localisation layer and the conformance transcripts compare the thrown text
//! character for character, so `Display` reproduces it verbatim — including
//! the two entries that carry **no trailing period**
//! ([`Error::NoEventHandler`] and [`Error::DateKeystrokeArity`]).
//! Changing any string here changes observable behaviour.

/// What went wrong in an AF* / util.* helper.
///
/// Each variant's `Display` is the exact text Acrobat throws.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// Wrong argument count. Raised by the engine binding rather than by the
    /// pure functions here, but named so the binding has one place to reach for.
    #[error("Incorrect number of parameters passed to function.")]
    ParamCount,
    /// `Incorrect parameter value.`
    #[error("Incorrect parameter value.")]
    Value,
    /// `Incorrect parameter type.`
    #[error("Incorrect parameter type.")]
    Type,
    /// `The input value is invalid.`
    #[error("The input value is invalid.")]
    InvalidInput,
    /// `The input value is too long.`
    #[error("The input value is too long.")]
    TooLong,
    /// `The input value can't be parsed as a valid date/time (…).`
    #[error("The input value can't be parsed as a valid date/time ({format}).")]
    ParseDate {
        /// The format string that failed to match, interpolated verbatim.
        format: String,
    },
    /// `Operation not supported.`
    #[error("Operation not supported.")]
    NotSupported,
    /// `Object no longer exists.` — the answer when a format or keystroke
    /// function runs with no event value in scope.
    #[error("Object no longer exists.")]
    BadObject,
    /// `The second parameter can't be converted to a Date.`
    #[error("The second parameter can't be converted to a Date.")]
    NotADate,
    /// `The second parameter is an invalid Date.`
    #[error("The second parameter is an invalid Date.")]
    InvalidDate,
    /// `No event handler` — a bare literal, **no trailing period**, raised only
    /// by `AFNumber_Format` when there is no event value.
    #[error("No event handler")]
    NoEventHandler,
    /// `AFDate_KeystrokeEx's parameter size not correct` — also a bare literal
    /// with **no trailing period**. `AFTime_KeystrokeEx` raises this same text,
    /// naming `AFDate_KeystrokeEx`, because it forwards to it.
    #[error("AFDate_KeystrokeEx's parameter size not correct")]
    DateKeystrokeArity,
}

/// The icon every `AF*` alert uses (`JSPLATFORM_ALERT_ICON_STATUS`).
pub const ALERT_ICON_STATUS: u8 = 3;
/// The button set every `AF*` alert uses (`JSPLATFORM_ALERT_BUTTON_OK`).
pub const ALERT_BUTTON_OK: u8 = 0;

/// The text of a host-visible alert, without the caller or the icon.
///
/// Separate from [`AfAlert`] so the message and the framing that names which
/// function raised it stay one record each.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AlertMessage {
    /// `The input value is invalid.`
    InvalidInput,
    /// `The input value is too long.`
    TooLong,
    /// `The input value can't be parsed as a valid date/time ({format}).`
    ParseDate {
        /// Format string interpolated into the message.
        format: String,
    },
    /// `The input value must be greater than or equal to {min} and less than or
    /// equal to {max}.`
    RangeBetween {
        /// Lower bound, as the **raw argument text**, not the parsed double: the
        /// message is filled from the JS value, so a bound passed as `2` reads
        /// `2` and never `2.0`.
        min: String,
        /// Upper bound, as the raw argument text.
        max: String,
    },
    /// `The input value must be greater than or equal to {min}.`
    RangeGreater {
        /// Lower bound, as the raw argument text.
        min: String,
    },
    /// `The input value must be less than or equal to {max}.`
    RangeLess {
        /// Upper bound, as the raw argument text.
        max: String,
    },
}

impl core::fmt::Display for AlertMessage {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidInput => f.write_str("The input value is invalid."),
            Self::TooLong => f.write_str("The input value is too long."),
            Self::ParseDate { format } => write!(
                f,
                "The input value can't be parsed as a valid date/time ({format})."
            ),
            Self::RangeBetween { min, max } => write!(
                f,
                "The input value must be greater than or equal to {min} and less than or equal to {max}."
            ),
            Self::RangeGreater { min } => {
                write!(f, "The input value must be greater than or equal to {min}.")
            }
            Self::RangeLess { max } => {
                write!(f, "The input value must be less than or equal to {max}.")
            }
        }
    }
}

/// One alert an `AF*` function asked the host to raise.
///
/// The host renders this as `{caller}[icon={icon},type={button}]: {message}`,
/// which is the shape the golden transcripts compare, so `caller`, `icon` and
/// `button` are part of the observable answer and not decoration. Every `AF*`
/// alert uses [`ALERT_ICON_STATUS`] and [`ALERT_BUTTON_OK`]; `caller` is the
/// Acrobat name of the function that raised it, which is **not** always the
/// function the script called — `AFTime_KeystrokeEx` forwards to
/// `AFDate_KeystrokeEx` and the alert is attributed to the callee.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AfAlert {
    /// The Acrobat name of the raising function, e.g. `AFRange_Validate`.
    pub caller: &'static str,
    /// The message text.
    pub message: AlertMessage,
    /// Always [`ALERT_ICON_STATUS`].
    pub icon: u8,
    /// Always [`ALERT_BUTTON_OK`].
    pub button: u8,
}

impl AfAlert {
    /// An alert from `caller` carrying `message`, with the icon and button
    /// every `AF*` alert uses.
    #[must_use]
    pub fn new(caller: &'static str, message: AlertMessage) -> Self {
        Self {
            caller,
            message,
            icon: ALERT_ICON_STATUS,
            button: ALERT_BUTTON_OK,
        }
    }
}

impl core::fmt::Display for AfAlert {
    /// The transcript line, verbatim: `AFRange_Validate[icon=3,type=0]: …`.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "{}[icon={},type={}]: {}",
            self.caller, self.icon, self.button, self.message
        )
    }
}

/// The colour space of a colour an `AF*` function asked a field to take.
///
/// Matches the `["T"] / ["G",g] / ["RGB",r,g,b] / ["CMYK",c,m,y,k]` array
/// encoding Acrobat's `color` object uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AfColorSpace {
    /// `["T"]` — no components read.
    Transparent,
    /// `["G", g]` — one component.
    Gray,
    /// `["RGB", r, g, b]` — three components.
    Rgb,
    /// `["CMYK", c, m, y, k]` — four components.
    Cmyk,
}

/// A text colour an `AF*` function asked the *target field* to take.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AfColor {
    /// Which of `components` carry meaning.
    pub space: AfColorSpace,
    /// Components in `0.0..=1.0`, in the order the space names them.
    pub components: [f32; 4],
}

impl AfColor {
    /// Opaque red, the colour the red negative styles ask for on a negative
    /// value.
    pub const RED: Self = Self {
        space: AfColorSpace::Rgb,
        components: [1.0, 0.0, 0.0, 0.0],
    };
    /// Opaque black, the colour those same styles ask for on a non-negative
    /// value.
    pub const BLACK: Self = Self {
        space: AfColorSpace::Rgb,
        components: [0.0, 0.0, 0.0, 0.0],
    };
}

/// What an `AF*` function did besides producing a value — the two things the
/// oracle does through the host that a pure function cannot.
///
/// Both come back as data so the host can apply them on its own schedule. An
/// empty `AfEffects` is a valid and common answer: with no form-fill
/// environment upstream the alert is a no-op, and only the two red negative
/// styles ever ask for a colour.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AfEffects {
    /// Alerts to raise, in the order they were raised.
    pub alerts: Vec<AfAlert>,
    /// A text colour the function asked the target field to take.
    ///
    /// `AFNumber_Format`'s negative styles 1 (red) and 3 (parenthesised red)
    /// are the only producers. The oracle writes the colour back only when it
    /// differs from the field's current colour, which needs the field; this is
    /// reported unconditionally and the host does the comparison.
    pub text_color: Option<AfColor>,
}

impl AfEffects {
    /// No alerts, no colour.
    #[must_use]
    pub fn none() -> Self {
        Self::default()
    }

    /// Just one alert.
    #[must_use]
    pub fn alert(caller: &'static str, message: AlertMessage) -> Self {
        Self {
            alerts: vec![AfAlert::new(caller, message)],
            text_color: None,
        }
    }

    /// Just a colour.
    #[must_use]
    pub fn color(color: AfColor) -> Self {
        Self {
            alerts: Vec::new(),
            text_color: Some(color),
        }
    }

    /// Whether nothing at all was asked of the host.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.alerts.is_empty() && self.text_color.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The strings the golden transcript compares character for character.
    #[test]
    fn error_display_is_the_oracle_message_table() {
        let cases: &[(Error, &str)] = &[
            (
                Error::ParamCount,
                "Incorrect number of parameters passed to function.",
            ),
            (Error::Value, "Incorrect parameter value."),
            (Error::Type, "Incorrect parameter type."),
            (Error::InvalidInput, "The input value is invalid."),
            (Error::TooLong, "The input value is too long."),
            (Error::NotSupported, "Operation not supported."),
            (Error::BadObject, "Object no longer exists."),
            (
                Error::NotADate,
                "The second parameter can't be converted to a Date.",
            ),
            (
                Error::InvalidDate,
                "The second parameter is an invalid Date.",
            ),
        ];
        for (err, expected) in cases {
            assert_eq!(err.to_string(), *expected, "{err:?}");
        }
    }

    /// The two bare literals carry no trailing period, unlike every table row.
    #[test]
    fn the_two_bare_literals_have_no_trailing_period() {
        assert_eq!(Error::NoEventHandler.to_string(), "No event handler");
        assert_eq!(
            Error::DateKeystrokeArity.to_string(),
            "AFDate_KeystrokeEx's parameter size not correct"
        );
    }

    #[test]
    fn parse_date_interpolates_the_format_verbatim() {
        let e = Error::ParseDate {
            format: "blooey".into(),
        };
        assert_eq!(
            e.to_string(),
            "The input value can't be parsed as a valid date/time (blooey)."
        );
    }

    /// The notification lines, exactly as the transcript records them.
    #[test]
    fn alert_display_is_the_transcript_notification_line() {
        let cases: &[(AfAlert, &str)] = &[
            (
                AfAlert::new("AFNumber_Keystroke", AlertMessage::InvalidInput),
                "AFNumber_Keystroke[icon=3,type=0]: The input value is invalid.",
            ),
            (
                AfAlert::new("AFSpecial_KeystrokeEx", AlertMessage::TooLong),
                "AFSpecial_KeystrokeEx[icon=3,type=0]: The input value is too long.",
            ),
            (
                AfAlert::new(
                    "AFDate_KeystrokeEx",
                    AlertMessage::ParseDate {
                        format: "m/d".into(),
                    },
                ),
                "AFDate_KeystrokeEx[icon=3,type=0]: The input value can't be parsed as a valid date/time (m/d).",
            ),
            (
                AfAlert::new(
                    "AFRange_Validate",
                    AlertMessage::RangeBetween {
                        min: "2".into(),
                        max: "4".into(),
                    },
                ),
                "AFRange_Validate[icon=3,type=0]: The input value must be greater than or equal to 2 and less than or equal to 4.",
            ),
            (
                AfAlert::new(
                    "AFRange_Validate",
                    AlertMessage::RangeGreater { min: "2".into() },
                ),
                "AFRange_Validate[icon=3,type=0]: The input value must be greater than or equal to 2.",
            ),
            (
                AfAlert::new(
                    "AFRange_Validate",
                    AlertMessage::RangeLess { max: "4".into() },
                ),
                "AFRange_Validate[icon=3,type=0]: The input value must be less than or equal to 4.",
            ),
        ];
        for (alert, expected) in cases {
            assert_eq!(alert.to_string(), *expected);
        }
    }
}
