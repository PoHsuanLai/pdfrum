//! `AFSpecial_Format` / `AFSpecial_Keystroke` / `AFSpecial_KeystrokeEx`.

use super::{AfFormat, Keystroke, KeystrokeResult};
use crate::error::AlertMessage;
use crate::parse::{is_ascii_alnum, is_ascii_alpha, is_decimal_digit};
use crate::printx::util_printx;

/// The name the keystroke functions report as the alert's caller.
///
/// `AFSpecial_Keystroke` forwards to `AFSpecial_KeystrokeEx`, and the alert is
/// attributed to the callee — so both spell this same name.
const CALLER: &str = "AFSpecial_KeystrokeEx";

/// Whether `change` is allowed where the mask holds `mask`.
///
/// Four characters are placeholders — a digit, a letter, a letter-or-digit, and
/// `X`, which accepts **anything at all**, including punctuation and space.
/// Every other mask character is a literal that only matches itself.
#[must_use]
pub fn mask_satisfied(change: char, mask: char) -> bool {
    match mask {
        '9' => is_decimal_digit(change),
        'A' => is_ascii_alpha(change),
        'O' => is_ascii_alnum(change),
        'X' => true,
        _ => change == mask,
    }
}

/// Whether `ch` is one of the four placeholders rather than a literal.
///
/// A literal in the mask is *substituted into* the keystroke, which is how
/// typing the fifth digit of a zip-plus-four produces the hyphen.
#[must_use]
pub fn is_reserved_mask_char(ch: char) -> bool {
    matches!(ch, '9' | 'A' | 'O' | 'X')
}

/// `AFSpecial_Format(psf)` — the four canned masks: 0 zip, 1 zip-plus-four,
/// 2 phone, 3 social security number.
///
/// The phone mask is chosen by first running the value through a bare
/// ten-digit probe: only if ten digits survive does it get an area code.
///
/// An unrecognised selector leaves the mask empty, and an empty mask produces
/// an empty value — so `AFSpecial_Format(9)` blanks the field rather than
/// leaving it alone. The punctuation in these masks is literal and is planted
/// even when no digits reach it, which is why formatting an empty value under
/// mask 3 yields `--`.
#[must_use]
pub fn af_special_format(value: &str, kind: i32) -> AfFormat {
    /// Digits alone, to count how many the value can supply.
    const PHONE_PROBE: &str = "9999999999";
    let format = match kind {
        0 => "99999",
        1 => "99999-9999",
        2 => {
            if util_printx(PHONE_PROBE, value).chars().count() >= 10 {
                "(999) 999-9999"
            } else {
                "999-9999"
            }
        }
        3 => "999-99-9999",
        _ => "",
    };
    AfFormat::formatted(util_printx(format, value))
}

/// `AFSpecial_KeystrokeEx(mask)` — check a keystroke, or a whole value, against
/// an arbitrary mask.
///
/// An empty mask accepts everything unconditionally.
///
/// On commit the entire value is walked against the mask and must both fit and
/// match to its full length: too long notifies "too long", anything else
/// notifies "invalid". A value *shorter* than the mask is invalid, since the
/// walk stops before reaching the mask's end.
///
/// On an ordinary keystroke only the insertion is walked, starting at the
/// caret, and each literal the mask holds is written *into* the change — that
/// substitution is the whole point, and is what makes the punctuation appear as
/// the user types. A character that fails its placeholder rejects silently;
/// running past the mask's end notifies "too long".
#[must_use]
pub fn af_special_keystroke_ex(event: &Keystroke, mask: &str) -> KeystrokeResult {
    if mask.is_empty() {
        return KeystrokeResult::accept();
    }
    let mask_chars: Vec<char> = mask.chars().collect();
    let val_chars: Vec<char> = event.value.chars().collect();

    if event.will_commit {
        if val_chars.is_empty() {
            return KeystrokeResult::accept();
        }
        if val_chars.len() > mask_chars.len() {
            return KeystrokeResult::reject_with(CALLER, AlertMessage::TooLong);
        }
        let matched = val_chars
            .iter()
            .zip(mask_chars.iter())
            .take_while(|(v, m)| mask_satisfied(**v, **m))
            .count();
        if matched == mask_chars.len() {
            return KeystrokeResult::accept();
        }
        return KeystrokeResult::reject_with(CALLER, AlertMessage::InvalidInput);
    }

    let mut change_chars: Vec<char> = event.change.chars().collect();
    if change_chars.is_empty() {
        return KeystrokeResult::accept();
    }

    // The mask is indexed by an unsigned caret, so a selection start of -1
    // lands past the end of every mask and the keystroke is refused as too
    // long — which is the answer, since there is nowhere for it to go.
    let Ok(mut mask_index) = usize::try_from(event.sel_start) else {
        return KeystrokeResult::reject_with(CALLER, AlertMessage::TooLong);
    };

    // What the field would hold afterwards: everything already there, plus the
    // insertion, less whatever the selection replaces.
    let replaced = usize::try_from(event.sel_end.saturating_sub(event.sel_start)).unwrap_or(0);
    let combined_len = val_chars
        .len()
        .saturating_add(change_chars.len())
        .saturating_sub(replaced);
    if combined_len > mask_chars.len() || mask_index >= mask_chars.len() {
        return KeystrokeResult::reject_with(CALLER, AlertMessage::TooLong);
    }

    for slot in &mut change_chars {
        let Some(w_mask) = mask_chars.get(mask_index).copied() else {
            return KeystrokeResult::reject_with(CALLER, AlertMessage::TooLong);
        };
        // A literal in the mask overwrites what was typed, so the punctuation
        // appears whatever key produced it.
        if !is_reserved_mask_char(w_mask) {
            *slot = w_mask;
        }
        if !mask_satisfied(*slot, w_mask) {
            return KeystrokeResult::reject();
        }
        mask_index = mask_index.saturating_add(1);
    }
    KeystrokeResult::accept_change(change_chars.into_iter().collect::<String>())
}

/// `AFSpecial_Keystroke(psf)` — the same four selectors as
/// [`af_special_format`], but **digits only**.
///
/// These masks carry no punctuation at all: the hyphens and parentheses are
/// added later, when the value is formatted. Conflating the two sets would make
/// a user unable to type past the first literal, so they are deliberately
/// separate tables.
///
/// The phone selector picks its length from how much has been typed so far,
/// which is why a seven-digit local number and a ten-digit long-distance one
/// both pass.
#[must_use]
pub fn af_special_keystroke(event: &Keystroke, kind: i32) -> KeystrokeResult {
    /// Beyond this many characters a phone number is assumed to carry an area
    /// code.
    const LOCAL_PHONE_LEN: usize = 7;
    let format = match kind {
        0 => "99999",
        1 | 3 => "999999999",
        2 => {
            let typed = event.value.chars().count() + event.change.chars().count();
            if typed > LOCAL_PHONE_LEN {
                "9999999999"
            } else {
                "9999999"
            }
        }
        _ => "",
    };
    af_special_keystroke_ex(event, format)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::af::AfOutcome;

    fn fmt(value: &str, kind: i32) -> String {
        match af_special_format(value, kind).outcome {
            AfOutcome::Formatted(s) => s,
            other => unreachable!("AFSpecial_Format answered {other:?}"),
        }
    }

    /// Every `AFSpecial_Format` line the golden transcript records.
    #[test]
    fn special_format_transcript_lines() {
        assert_eq!(fmt("", 0), "");
        assert_eq!(fmt("", 1), "-");
        assert_eq!(fmt("", 2), "-");
        assert_eq!(fmt("", 3), "--");
        assert_eq!(fmt("0123456789", 0), "01234");
        assert_eq!(fmt("0123456789", 1), "01234-5678");
        assert_eq!(fmt("0123456789", 2), "(012) 345-6789");
        assert_eq!(fmt("0123456789", 3), "012-34-5678");
    }

    /// Fewer than ten digits and the area code is dropped.
    #[test]
    fn the_phone_mask_is_chosen_by_a_digit_probe() {
        assert_eq!(fmt("5551212", 2), "555-1212");
        assert_eq!(fmt("5551212345", 2), "(555) 121-2345");
        // Junk between the digits does not count against the probe.
        assert_eq!(fmt("(555) 121-2345", 2), "(555) 121-2345");
    }

    /// An unrecognised selector leaves the mask empty, and an empty mask blanks
    /// the value rather than leaving it alone.
    #[test]
    fn an_unknown_selector_blanks_the_value() {
        assert_eq!(fmt("0123456789", 9), "");
        assert_eq!(fmt("0123456789", -1), "");
    }

    /// `X` takes anything; the other three placeholders are narrow.
    #[test]
    fn the_four_placeholders() {
        assert!(mask_satisfied('5', '9'));
        assert!(!mask_satisfied('a', '9'));
        assert!(mask_satisfied('a', 'A'));
        assert!(mask_satisfied('Z', 'A'));
        assert!(!mask_satisfied('1', 'A'));
        assert!(mask_satisfied('1', 'O'));
        assert!(mask_satisfied('a', 'O'));
        assert!(!mask_satisfied('-', 'O'));
        for anything in ['-', ' ', '5', 'q', '\u{e9}'] {
            assert!(mask_satisfied(anything, 'X'), "{anything:?}");
        }
        assert!(mask_satisfied('-', '-'));
        assert!(!mask_satisfied('x', '-'));

        for ch in ['9', 'A', 'O', 'X'] {
            assert!(is_reserved_mask_char(ch), "{ch}");
        }
        for ch in ['-', '(', '8', 'B'] {
            assert!(!is_reserved_mask_char(ch), "{ch}");
        }
    }

    /// The six `AFSpecial_KeystrokeEx` lines the transcript records, each with
    /// the notification it does or does not produce.
    #[test]
    fn keystroke_ex_commit_transcript_lines() {
        let cases: &[(&str, &str, Option<&str>)] = &[
            ("12345", "", None),
            (
                "123",
                "9999",
                Some("AFSpecial_KeystrokeEx[icon=3,type=0]: The input value is invalid."),
            ),
            (
                "12345",
                "9999",
                Some("AFSpecial_KeystrokeEx[icon=3,type=0]: The input value is too long."),
            ),
            (
                "abcd",
                "9999",
                Some("AFSpecial_KeystrokeEx[icon=3,type=0]: The input value is invalid."),
            ),
            ("1234", "9999", None),
            ("abcd", "XXXX", None),
        ];
        for &(value, mask, expected) in cases {
            let out = af_special_keystroke_ex(&Keystroke::commit(value), mask);
            assert_eq!(
                out.alert().map(ToString::to_string).as_deref(),
                expected,
                "{value:?} against {mask:?}"
            );
            assert_eq!(out.is_reject(), expected.is_some(), "{value:?}");
        }
    }

    /// A value shorter than the mask is invalid, not merely incomplete.
    #[test]
    fn a_short_value_fails_on_commit() {
        let out = af_special_keystroke_ex(&Keystroke::commit("1"), "9999");
        assert!(out.is_reject());
        // An empty value is the exception: nothing to check.
        assert!(!af_special_keystroke_ex(&Keystroke::commit(""), "9999").is_reject());
    }

    /// The mask's literals are written into the keystroke, which is how the
    /// punctuation appears while typing.
    #[test]
    fn a_literal_in_the_mask_overwrites_the_keystroke() {
        let out = af_special_keystroke_ex(&Keystroke::insert("01234", "5", 5), "99999-9999");
        assert_eq!(out.change(), Some("-"));
        // A placeholder leaves the keystroke alone.
        let out = af_special_keystroke_ex(&Keystroke::insert("0123", "4", 4), "99999-9999");
        assert_eq!(out.change(), Some("4"));
    }

    /// Typing past the mask's end notifies "too long"; a character that simply
    /// does not fit its placeholder rejects in silence.
    #[test]
    fn too_long_notifies_and_a_bad_character_does_not() {
        let out = af_special_keystroke_ex(&Keystroke::insert("1234", "5", 4), "9999");
        assert!(out.is_reject());
        assert_eq!(
            out.alert().map(ToString::to_string).as_deref(),
            Some("AFSpecial_KeystrokeEx[icon=3,type=0]: The input value is too long.")
        );

        let out = af_special_keystroke_ex(&Keystroke::insert("12", "a", 2), "9999");
        assert!(out.is_reject());
        assert!(out.effects.is_empty());
    }

    /// A selection makes room, so replacing two characters with two is not too
    /// long even when the field is already full.
    #[test]
    fn a_selection_makes_room_for_the_insertion() {
        let out = af_special_keystroke_ex(&Keystroke::replace("1234", "99", 0, 2), "9999");
        assert_eq!(out.change(), Some("99"));
    }

    /// An empty mask accepts anything at all.
    #[test]
    fn an_empty_mask_accepts_everything() {
        for event in [
            Keystroke::commit("anything at all"),
            Keystroke::insert("x", "y", 1),
        ] {
            assert!(!af_special_keystroke_ex(&event, "").is_reject());
        }
    }

    /// The keystroke masks are digits only — the punctuation belongs to the
    /// format masks and must not leak into these.
    #[test]
    fn the_keystroke_masks_carry_no_punctuation() {
        // Nine digits pass selector 1, where the *format* mask would demand a
        // hyphen at index five.
        assert!(!af_special_keystroke(&Keystroke::commit("123456789"), 1).is_reject());
        assert!(af_special_keystroke(&Keystroke::commit("01234-5678"), 1).is_reject());
        assert!(!af_special_keystroke(&Keystroke::commit("12345"), 0).is_reject());
        assert!(!af_special_keystroke(&Keystroke::commit("123456789"), 3).is_reject());
    }

    /// The phone selector's length follows how much has been typed.
    #[test]
    fn the_phone_keystroke_mask_grows_with_the_value() {
        assert!(!af_special_keystroke(&Keystroke::commit("5551212"), 2).is_reject());
        assert!(!af_special_keystroke(&Keystroke::commit("5551212345"), 2).is_reject());
    }

    /// An unknown selector yields an empty mask, which accepts everything.
    #[test]
    fn an_unknown_keystroke_selector_accepts_everything() {
        assert!(!af_special_keystroke(&Keystroke::commit("abc"), 65).is_reject());
    }
}
