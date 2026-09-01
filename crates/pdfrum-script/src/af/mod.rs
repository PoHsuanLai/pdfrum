//! Acrobat `AF*` field helpers as pure functions.
//!
//! Every function here is a transformation of text (plus, for dates, an
//! explicit "now" in milliseconds). There is no JavaScript engine, no form
//! session, and no field lookup.
//!
//! # What the host is asked for comes back as data
//!
//! Two things Acrobat does through its host a pure function cannot do: raise
//! an alert, and recolour the target field's text. Both are *returned* rather
//! than performed, as [`AfEffects`] — a list of [`AfAlert`]s and an optional
//! [`AfColor`]. That is why the negative styles that carry their sign in the
//! colour rather than in a minus sign are fully reproduced here: the string
//! half is in the outcome and the colour half is in the effects, and the
//! caller applies the colour once it has a field to apply it to.
//!
//! An alert also travels with an error. Rejecting a keystroke and throwing are
//! different answers, and the one path that does *both* — throw an exception
//! **and** notify the user — returns the alert alongside the error as
//! [`Thrown`], rather than dropping the notification on the floor.

mod calc;
mod date;
mod merge;
mod number;
mod percent;
mod range;
mod special;

pub use crate::error::{AfAlert, AfColor, AfEffects, AlertMessage, Error};
pub use calc::{
    SimpleOp, af_make_number, af_simple, af_simple_calculate, af_simple_calculate_texts,
    af_split_field_list,
};
pub use date::{
    DATE_FORMATS, TIME_FORMATS, af_date_format, af_date_format_ex, af_date_keystroke,
    af_date_keystroke_ex, af_parse_date_ex, af_time_format, af_time_format_ex, af_time_keystroke,
    af_time_keystroke_ex,
};
pub use merge::{af_extract_nums, af_merge_change};
pub use number::{af_number_format, af_number_keystroke};
pub use percent::{af_percent_format, af_percent_keystroke};
pub use range::af_range_validate;
pub use special::{
    af_special_format, af_special_keystroke, af_special_keystroke_ex, is_reserved_mask_char,
    mask_satisfied,
};

/// An exception, together with anything the same call still asked of the host.
///
/// `AFNumber_Keystroke` on a non-numeric commit both notifies the user and
/// throws; the transcript records the notification line *and* the thrown text,
/// so an `Err` that carried only the message would lose half the answer.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("{error}")]
pub struct Thrown {
    /// The exception Acrobat raises.
    pub error: Error,
    /// Alerts and colour requested before the throw. Usually empty.
    pub effects: AfEffects,
}

impl Thrown {
    /// An exception with nothing asked of the host.
    #[must_use]
    pub fn bare(error: Error) -> Self {
        Self {
            error,
            effects: AfEffects::none(),
        }
    }

    /// An exception that also asked `caller` to notify the user.
    #[must_use]
    pub fn alerting(error: Error, caller: &'static str, message: AlertMessage) -> Self {
        Self {
            error,
            effects: AfEffects::alert(caller, message),
        }
    }
}

impl From<Error> for Thrown {
    fn from(error: Error) -> Self {
        Self::bare(error)
    }
}

/// The value a field holds or a keystroke proposes: always text at this layer.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AfValue {
    /// Field contents as Unicode text.
    pub text: String,
}

impl AfValue {
    /// Wrap `text`.
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }
}

impl From<&str> for AfValue {
    fn from(s: &str) -> Self {
        Self {
            text: s.to_string(),
        }
    }
}

impl From<String> for AfValue {
    fn from(text: String) -> Self {
        Self { text }
    }
}

/// What a format function did to the field value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AfOutcome {
    /// The field value should become this string.
    Formatted(String),
    /// The action is rejected (`event.rc = false`).
    Rejected,
    /// Leave the field as it is.
    Unchanged,
}

/// A format function's whole answer: what happened to the value, plus what was
/// asked of the host.
#[derive(Debug, Clone, PartialEq)]
pub struct AfFormat {
    /// What to do with the field value.
    pub outcome: AfOutcome,
    /// Alerts to raise and a colour to apply, if any.
    pub effects: AfEffects,
}

impl AfFormat {
    /// A new value, nothing asked of the host.
    #[must_use]
    pub fn formatted(value: impl Into<String>) -> Self {
        Self {
            outcome: AfOutcome::Formatted(value.into()),
            effects: AfEffects::none(),
        }
    }

    /// Leave the value alone, nothing asked of the host.
    #[must_use]
    pub fn unchanged() -> Self {
        Self {
            outcome: AfOutcome::Unchanged,
            effects: AfEffects::none(),
        }
    }

    /// Reject, and ask `caller` to notify the user.
    #[must_use]
    pub fn rejected(caller: &'static str, message: AlertMessage) -> Self {
        Self {
            outcome: AfOutcome::Rejected,
            effects: AfEffects::alert(caller, message),
        }
    }

    /// Attach a text colour to this answer.
    #[must_use]
    pub fn with_color(mut self, color: crate::error::AfColor) -> Self {
        self.effects.text_color = Some(color);
        self
    }

    /// The alert this answer carries, if it carries exactly one.
    #[must_use]
    pub fn alert(&self) -> Option<&AfAlert> {
        match self.effects.alerts.as_slice() {
            [only] => Some(only),
            _ => None,
        }
    }
}

/// The slice of Acrobat's `event` object an `AF*` keystroke function reads.
///
/// Modelled field by field rather than as a merged string, because the
/// functions genuinely branch on all five: `will_commit` picks an entirely
/// different code path, `sel_start` / `sel_end` decide where `change` lands and
/// (when negative) how much of `value` survives, and `change` alone is what a
/// mask rewrites.
///
/// `sel_start` of `-1` is "no selection", exactly as upstream spells it, and is
/// load-bearing: the merge treats a negative start as "keep everything".
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Keystroke {
    /// `event.value` — the field contents before this key.
    pub value: String,
    /// `event.change` — the proposed insertion.
    pub change: String,
    /// `event.selStart` (character index; `-1` is "no selection").
    pub sel_start: i32,
    /// `event.selEnd`.
    pub sel_end: i32,
    /// `event.willCommit` — the last keystroke before validation, when the
    /// whole value is checked rather than the single insertion.
    pub will_commit: bool,
    /// `event.fieldFull`. No `AF*` function reads it; carried so the engine
    /// binding has one event record rather than two.
    pub field_full: bool,
}

impl Keystroke {
    /// A commit of `value` (`willCommit = true`, empty change).
    #[must_use]
    pub fn commit(value: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            will_commit: true,
            ..Self::default()
        }
    }

    /// A non-commit keystroke inserting `change` at the caret.
    #[must_use]
    pub fn insert(value: impl Into<String>, change: impl Into<String>, caret: i32) -> Self {
        Self {
            value: value.into(),
            change: change.into(),
            sel_start: caret,
            sel_end: caret,
            will_commit: false,
            field_full: false,
        }
    }

    /// A non-commit keystroke replacing the characters in `sel_start..sel_end`.
    #[must_use]
    pub fn replace(
        value: impl Into<String>,
        change: impl Into<String>,
        sel_start: i32,
        sel_end: i32,
    ) -> Self {
        Self {
            value: value.into(),
            change: change.into(),
            sel_start,
            sel_end,
            will_commit: false,
            field_full: false,
        }
    }

    /// The value this keystroke would produce, per `AFMergeChange`.
    #[must_use]
    pub fn merged(&self) -> String {
        if self.will_commit {
            self.value.clone()
        } else {
            merge_change(&self.value, &self.change, self.sel_start, self.sel_end)
        }
    }
}

/// What a keystroke function decided about the keystroke.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeystrokeOutcome {
    /// Accept. `value` / `change` are `Some` when the event field was rewritten.
    Accept {
        /// New `event.value`, if rewritten.
        value: Option<String>,
        /// New `event.change`, if rewritten (mask literals).
        change: Option<String>,
    },
    /// `event.rc = false`.
    Reject,
}

/// A keystroke function's whole answer: the decision, plus what was asked of
/// the host.
#[derive(Debug, Clone, PartialEq)]
pub struct KeystrokeResult {
    /// Accept (with any rewritten event fields) or reject.
    pub outcome: KeystrokeOutcome,
    /// Alerts to raise, if any. Keystroke functions never ask for a colour.
    pub effects: AfEffects,
}

impl KeystrokeResult {
    /// Accept without rewriting anything.
    #[must_use]
    pub fn accept() -> Self {
        Self {
            outcome: KeystrokeOutcome::Accept {
                value: None,
                change: None,
            },
            effects: AfEffects::none(),
        }
    }

    /// Accept, rewriting `event.value`.
    #[must_use]
    pub fn accept_value(value: impl Into<String>) -> Self {
        Self {
            outcome: KeystrokeOutcome::Accept {
                value: Some(value.into()),
                change: None,
            },
            effects: AfEffects::none(),
        }
    }

    /// Accept, rewriting `event.change` (a mask substituted its literals).
    #[must_use]
    pub fn accept_change(change: impl Into<String>) -> Self {
        Self {
            outcome: KeystrokeOutcome::Accept {
                value: None,
                change: Some(change.into()),
            },
            effects: AfEffects::none(),
        }
    }

    /// Reject silently — `event.rc = false` with no notification, which is what
    /// the numeric keystroke guards do when a character simply does not belong.
    #[must_use]
    pub fn reject() -> Self {
        Self {
            outcome: KeystrokeOutcome::Reject,
            effects: AfEffects::none(),
        }
    }

    /// Reject and ask `caller` to notify the user.
    #[must_use]
    pub fn reject_with(caller: &'static str, message: AlertMessage) -> Self {
        Self {
            outcome: KeystrokeOutcome::Reject,
            effects: AfEffects::alert(caller, message),
        }
    }

    /// Whether this answer rejects the keystroke.
    #[must_use]
    pub fn is_reject(&self) -> bool {
        matches!(self.outcome, KeystrokeOutcome::Reject)
    }

    /// The rewritten `event.value`, if there is one.
    #[must_use]
    pub fn value(&self) -> Option<&str> {
        match &self.outcome {
            KeystrokeOutcome::Accept { value, .. } => value.as_deref(),
            KeystrokeOutcome::Reject => None,
        }
    }

    /// The rewritten `event.change`, if there is one.
    #[must_use]
    pub fn change(&self) -> Option<&str> {
        match &self.outcome {
            KeystrokeOutcome::Accept { change, .. } => change.as_deref(),
            KeystrokeOutcome::Reject => None,
        }
    }

    /// The alert this answer carries, if it carries exactly one.
    #[must_use]
    pub fn alert(&self) -> Option<&AfAlert> {
        match self.effects.alerts.as_slice() {
            [only] => Some(only),
            _ => None,
        }
    }
}

/// Merge `change` into `value` using the selection, matching `CalcMergedString`.
#[must_use]
pub fn merge_change(value: &str, change: &str, sel_start: i32, sel_end: i32) -> String {
    let chars: Vec<char> = value.chars().collect();
    // The prefix is taken by an unsigned count, so a negative selection start
    // becomes an enormous one and the whole value is kept.
    let prefix: String = match usize::try_from(sel_start) {
        Ok(start) => chars.iter().take(start).collect(),
        Err(_) => value.to_string(),
    };
    // The suffix, by contrast, is guarded against a negative end and a stale
    // one, so either leaves nothing behind the insertion.
    let postfix: String = match usize::try_from(sel_end) {
        Ok(end) if end < chars.len() => chars.iter().skip(end).collect(),
        _ => String::new(),
    };
    let mut out = prefix;
    out.push_str(change);
    out.push_str(&postfix);
    out
}

/// Clamp `value` into `0..size`, else 0 — `WithinBoundsOrZero`.
#[must_use]
pub(crate) fn within_bounds_or_zero(value: i32, size: usize) -> usize {
    if value >= 0
        && let Ok(v) = usize::try_from(value)
        && v < size
    {
        v
    } else {
        0
    }
}

/// `ValidStyleOrZero`: styles outside `0..4` become 0.
#[must_use]
pub(crate) fn valid_style_or_zero(style: i32) -> i32 {
    let idx = within_bounds_or_zero(style, 4);
    i32::try_from(idx).unwrap_or(0)
}

pub(crate) fn is_style_with_digit_separator(style: i32) -> bool {
    style == 0 || style == 2
}

pub(crate) fn digit_separator_for_style(style: i32) -> char {
    if style == 0 { ',' } else { '.' }
}

pub(crate) fn is_style_with_apostrophe_separator(style: i32) -> bool {
    style >= 4
}

pub(crate) fn is_style_with_comma_decimal_mark(style: i32) -> bool {
    style == 2 || style == 3
}

pub(crate) fn decimal_mark_for_style(style: i32) -> char {
    if is_style_with_comma_decimal_mark(style) {
        ','
    } else {
        '.'
    }
}
