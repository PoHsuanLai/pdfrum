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
//! # A rejected commit keeps focus — where we diverge from the oracle
//!
//! **In PDFium a refused commit still loses focus, and that is a defect.**
//! `CFFL_FormField::CommitData` reverts the edit and returns `true` on both
//! refusal paths (`fpdfsdk/formfiller/cffl_formfield.cpp:525-531`), which
//! makes a rejection indistinguishable from an acceptance to its only
//! caller; `KillFocusForAnnot`'s sole guard is that return value
//! (`:306`), so `pWnd->KillFocus()` (`:311`) and `EscapeFiller()` (`:324`)
//! run either way. A user who typed something a validation script rejected
//! finds the caret gone and their typing discarded, with no way to correct
//! it — which defeats the purpose of a validation hook.
//!
//! ISO 32000-1 §12.7.5.3 gives the Validate event the job of *rejecting the
//! value*, not of ending the interaction, and pdf.js implements the rule with
//! the comment to prove it: on the not-valid branch it sends
//! `focus: true, // Stay in the field.`
//! (`src/scripting_api/event.js:277`). Under PLAN.md's oracle-bug rule we
//! implement the correct behaviour: **a refused commit reverts the edit and
//! keeps focus**, so the user can fix what the script objected to.
//!
//! [`CommitOutcome`] is shaped to say all three things separately, because
//! "the commit finished", "the value was stored" and "focus should move on"
//! are three different questions: `committed`, `reverted` and
//! [`CommitOutcome::keeps_focus`].
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
    ///
    /// Meaningful only when [`committed`](CommitOutcome::committed) and not
    /// [`reverted`](CommitOutcome::reverted): a commit that ran no hooks
    /// because the value had not moved says nothing about what the field
    /// shows, and a reader must not take its `None` for "show the raw value"
    /// — `AfterValueChange` is what calls `OnFormat`, and it runs on a
    /// *change* (`fpdfsdk/cpdfsdk_interactiveform.cpp:575-588`).
    /// [`CommitOutcome::formats`] is that question asked directly.
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

    /// Whether the field that just committed should **keep** the keyboard.
    ///
    /// True exactly when a hook refused. See the module documentation: the
    /// oracle drops focus here and it is a defect
    /// (`cffl_formfield.cpp:525-531` versus pdf.js
    /// `src/scripting_api/event.js:277`).
    #[must_use]
    pub fn keeps_focus(&self) -> bool {
        self.reverted
    }

    /// Whether this outcome **decides** what the field displays.
    ///
    /// The distinction [`display`](CommitOutcome::display) alone cannot make:
    /// a `None` from a commit that ran means "no formatter, draw the raw
    /// value" and must erase any earlier answer, while a `None` from a commit
    /// that never ran — because the value had not moved, or because a gate
    /// refused — means nothing at all and must leave the field showing what
    /// it was showing. Only the first is `AfterValueChange`'s
    /// `ResetFieldAppearance(pField, OnFormat(pField))`
    /// (`fpdfsdk/cpdfsdk_interactiveform.cpp:588`).
    #[must_use]
    pub fn formats(&self) -> bool {
        self.committed && !self.reverted && self.stored.is_some()
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

    // A refusal at either gate reverts the field and reports the commit as
    // finished — but, unlike the oracle, it does not let focus move on.
    //
    // [oracle-bug] `CFFL_FormField::CommitData` returns `true` from both
    // refusal paths (`fpdfsdk/formfiller/cffl_formfield.cpp:525-531`), and
    // that return value is `KillFocusForAnnot`'s only guard (`:306`), so a
    // rejected value loses the caret exactly as an accepted one does.
    // ISO 32000-1 §12.7.5.3 makes Validate reject the *value*; pdf.js keeps
    // the field with the comment to prove it — `focus: true, // Stay in the
    // field.`, `src/scripting_api/event.js:277`. `keeps_focus` is that fix.
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
            index: Some(0),
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

    /// A refusal reports the commit as finished — and **keeps the field**,
    /// which is where we diverge from the oracle on purpose.
    ///
    /// PDFium's `CommitData` returns `true` on this path
    /// (`cffl_formfield.cpp:525-531`), which is its caller's only guard
    /// (`:306`), so the caret goes and the typing with it. pdf.js keeps the
    /// field at `src/scripting_api/event.js:277`, commented `// Stay in the
    /// field.`, and ISO 32000-1 §12.7.5.3 agrees: Validate rejects the value,
    /// not the interaction.
    #[test]
    fn a_refused_commit_reports_success_and_keeps_the_field() {
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
        assert!(
            outcome.keeps_focus(),
            "[oracle-bug] the user must be able to correct what was rejected"
        );
    }

    /// Either gate refusing has the same shape, focus included.
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
        assert!(outcome.keeps_focus());
    }

    /// An accepted commit lets focus go, which is the ordinary path and the
    /// one that must not change.
    #[test]
    fn an_accepted_commit_lets_focus_move_on() {
        assert!(!run(&field(), "old", "new", &mut NoScripts, 8).keeps_focus());
        assert!(!CommitOutcome::unchanged().keeps_focus());
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
