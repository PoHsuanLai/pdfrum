//! The commit cascade: turning an edited field back into a stored value.
//!
//! Six steps in a fixed order, and the order is normative:
//!
//! ```text
//! is_changed → keystroke_commit → validate → save → calculate → format
//! ```
//!
//! Without scripts every hook takes its permissive answer, so a commit
//! reduces to "the value changed, store it" — but the shape is the full one
//! rather than a reduced one, which is what lets a scripting implementation
//! drop in without a redesign.
//!
//! # The quirk that must not be tidied
//!
//! **A rejected commit still loses focus.** When a validation hook refuses,
//! the field is reverted to its stored value and the commit reports
//! *success* — so the caller carries on and drops focus exactly as it would
//! after an accepted commit. A user who typed something a script rejected
//! finds the caret gone and their typing discarded.
//!
//! This is not what a reader expects, it is not what other viewers do, and
//! many files are written assuming the opposite. It is reproduced
//! deliberately, and [`CommitOutcome`] is shaped to say so: `committed` and
//! `reverted` are separate answers precisely because "the commit finished"
//! and "the value was stored" are different questions here.
//!
//! Without scripts the rejection branch is unreachable — nothing can refuse —
//! so this costs nothing today and becomes visible the moment scripts arrive.

use crate::cascade::{Cascade, FieldRef, FieldWrites};

/// What a commit did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitOutcome {
    /// Whether the cascade ran to the end. **True even when a hook refused**,
    /// which is what makes focus proceed either way.
    pub committed: bool,
    /// Whether a hook refused and the field was put back to its stored value.
    pub reverted: bool,
    /// The value now stored, when one was stored.
    pub stored: Option<String>,
    /// A display string a formatting hook produced, which does not change the
    /// stored value.
    pub display: Option<String>,
    /// Values a calculation asked to be written to other fields.
    pub writes: Vec<(u32, String)>,
}

impl CommitOutcome {
    /// The answer for a field whose value had not changed: nothing ran.
    #[must_use]
    pub fn unchanged() -> CommitOutcome {
        CommitOutcome {
            committed: true,
            reverted: false,
            stored: None,
            display: None,
            writes: Vec::new(),
        }
    }
}

/// Runs the cascade for one field.
///
/// `stored` is what the document currently holds and `edited` is what the
/// user has; when they agree nothing runs at all, which is the first gate and
/// the reason an unchanged field costs nothing to blur through.
pub fn run(
    field: &FieldRef,
    stored: &str,
    edited: &str,
    cascade: &mut dyn Cascade,
    max_calculate_depth: u32,
) -> CommitOutcome {
    if stored == edited {
        return CommitOutcome::unchanged();
    }

    // A refusal at either gate reverts the field and reports success, so the
    // caller's focus handling proceeds exactly as it would on an acceptance.
    if !cascade.keystroke_commit(field, edited) || !cascade.validate(field, edited) {
        return CommitOutcome {
            committed: true,
            reverted: true,
            stored: None,
            display: None,
            writes: Vec::new(),
        };
    }

    let stored_value = edited.to_string();

    let mut writes = FieldWrites::with_max_depth(max_calculate_depth);
    cascade.calculate(&mut writes, field);
    let writes: Vec<(u32, String)> = writes.writes().map(|(f, v)| (f, v.to_string())).collect();

    // Format runs last and changes only what is shown, never what is stored.
    let display = cascade.format(field, &stored_value);

    CommitOutcome {
        committed: true,
        reverted: false,
        stored: Some(stored_value),
        display,
        writes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cascade::{Keystroke, KeystrokeOutcome, NoScripts};

    fn field() -> FieldRef {
        FieldRef {
            name: "Text Box".to_string(),
            index: 0,
        }
    }

    /// Nothing runs when the value has not moved.
    #[test]
    fn an_unchanged_field_runs_no_hook() {
        struct Counting {
            calls: u32,
        }
        impl Cascade for Counting {
            fn validate(&mut self, _f: &FieldRef, _v: &str) -> bool {
                self.calls += 1;
                true
            }
        }

        let mut cascade = Counting { calls: 0 };
        let outcome = run(&field(), "same", "same", &mut cascade, 8);

        assert_eq!(cascade.calls, 0);
        assert!(outcome.committed);
        assert!(!outcome.reverted);
        assert_eq!(outcome.stored, None);
    }

    /// Without scripts the value is simply stored.
    #[test]
    fn a_changed_field_stores_its_new_value() {
        let outcome = run(&field(), "old", "new", &mut NoScripts, 8);
        assert!(outcome.committed);
        assert!(!outcome.reverted);
        assert_eq!(outcome.stored.as_deref(), Some("new"));
        assert_eq!(outcome.display, None, "no formatting without scripts");
        assert!(outcome.writes.is_empty(), "no calculation without scripts");
    }

    /// The quirk: a refusal reports the commit as finished, so the caller
    /// drops focus exactly as it would have on an acceptance.
    #[test]
    fn a_refused_commit_still_reports_success() {
        struct Refusing;
        impl Cascade for Refusing {
            fn validate(&mut self, _f: &FieldRef, _v: &str) -> bool {
                false
            }
        }

        let outcome = run(&field(), "old", "new", &mut Refusing, 8);
        assert!(
            outcome.committed,
            "a refusal must not report the commit as failed"
        );
        assert!(outcome.reverted, "…but it must say the value was put back");
        assert_eq!(outcome.stored, None, "and nothing was stored");
    }

    /// Either gate refusing has the same shape.
    #[test]
    fn the_keystroke_gate_refuses_the_same_way() {
        struct Refusing;
        impl Cascade for Refusing {
            fn keystroke_commit(&mut self, _f: &FieldRef, _v: &str) -> bool {
                false
            }
        }

        let outcome = run(&field(), "old", "new", &mut Refusing, 8);
        assert!(outcome.committed);
        assert!(outcome.reverted);
        assert_eq!(outcome.stored, None);
    }

    /// The order is normative, so it is asserted rather than assumed.
    #[test]
    fn the_hooks_run_in_the_specified_order() {
        #[derive(Default)]
        struct Recording {
            seen: Vec<&'static str>,
        }
        impl Cascade for Recording {
            fn keystroke(&mut self, _f: &FieldRef, c: Keystroke) -> KeystrokeOutcome {
                self.seen.push("keystroke");
                KeystrokeOutcome::Accept(c)
            }
            fn keystroke_commit(&mut self, _f: &FieldRef, _v: &str) -> bool {
                self.seen.push("keystroke_commit");
                true
            }
            fn validate(&mut self, _f: &FieldRef, _v: &str) -> bool {
                self.seen.push("validate");
                true
            }
            fn calculate(&mut self, _w: &mut FieldWrites, _t: &FieldRef) {
                self.seen.push("calculate");
            }
            fn format(&mut self, _f: &FieldRef, _v: &str) -> Option<String> {
                self.seen.push("format");
                None
            }
        }

        let mut cascade = Recording::default();
        run(&field(), "old", "new", &mut cascade, 8);

        assert_eq!(
            cascade.seen,
            vec!["keystroke_commit", "validate", "calculate", "format"]
        );
    }

    /// A refusal stops the cascade there: nothing after the refusing gate
    /// runs.
    #[test]
    fn a_refusal_stops_the_cascade() {
        #[derive(Default)]
        struct Recording {
            seen: Vec<&'static str>,
        }
        impl Cascade for Recording {
            fn validate(&mut self, _f: &FieldRef, _v: &str) -> bool {
                self.seen.push("validate");
                false
            }
            fn calculate(&mut self, _w: &mut FieldWrites, _t: &FieldRef) {
                self.seen.push("calculate");
            }
            fn format(&mut self, _f: &FieldRef, _v: &str) -> Option<String> {
                self.seen.push("format");
                None
            }
        }

        let mut cascade = Recording::default();
        run(&field(), "old", "new", &mut cascade, 8);
        assert_eq!(cascade.seen, vec!["validate"]);
    }

    /// Formatting changes what is shown and not what is stored — the whole
    /// point of a separate display string.
    #[test]
    fn a_display_string_does_not_change_the_stored_value() {
        struct Formatting;
        impl Cascade for Formatting {
            fn format(&mut self, _f: &FieldRef, value: &str) -> Option<String> {
                Some(format!("${value}.00"))
            }
        }

        let outcome = run(&field(), "0", "1234", &mut Formatting, 8);
        assert_eq!(outcome.stored.as_deref(), Some("1234"));
        assert_eq!(outcome.display.as_deref(), Some("$1234.00"));
    }

    #[test]
    fn a_calculation_reaches_the_outcome() {
        struct Calculating;
        impl Cascade for Calculating {
            fn calculate(&mut self, writes: &mut FieldWrites, _t: &FieldRef) {
                writes.set(7, "total");
            }
        }

        let outcome = run(&field(), "old", "new", &mut Calculating, 8);
        assert_eq!(outcome.writes, vec![(7, "total".to_string())]);
    }

    /// The cascade takes its hooks behind a reference, which is the one place
    /// in the crate that needs the seam to be a trait at all.
    #[test]
    fn the_cascade_is_taken_by_reference() {
        let mut owned = NoScripts;
        let cascade: &mut dyn Cascade = &mut owned;
        let outcome = run(&field(), "a", "b", cascade, 8);
        assert_eq!(outcome.stored.as_deref(), Some("b"));
    }
}
