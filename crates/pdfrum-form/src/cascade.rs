//! The four points where a field's `/AA` scripts can intervene.
//!
//! # The defaults are not stubs
//!
//! This is worth stating plainly, because the natural reading of a trait
//! whose every method has a default is "the real implementation is missing".
//! It is not. A PDF viewer built without a JavaScript engine runs the
//! *identical* commit path — same functions, same order, the same action
//! record built and populated, the action tree still walked — with exactly
//! three value-mutation points inert: the accept flag never goes false, the
//! change string is never rewritten, and calculate and format return at their
//! first line.
//!
//! So [`NoScripts`] is not "the real thing minus scripts". It is the real
//! thing with the identity cascade, and the method defaults below *are* that
//! behaviour, bit for bit. A scripting implementation substitutes a different
//! value for one parameter and changes no call site.
//!
//! # Why four methods and not five, or three
//!
//! There are precisely four gates: a keystroke may be rewritten or rejected
//! as it is typed; a commit may be rejected; other fields may be recalculated
//! from the committed one; and a display string may be produced that does not
//! change the stored value. The first two are the same action dictionary
//! distinguished only by whether a commit is imminent, which is why they are
//! two methods over one key.

/// A field, named the way a script would name it.
///
/// Carries the identity a script needs to talk about the field, not a
/// reference to it: nothing here can be used to reach back into the session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldRef {
    /// The field's fully qualified name.
    pub name: String,
    /// Its index in the form's field list.
    pub index: u32,
}

/// A keystroke offered to the keystroke hook.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keystroke {
    /// The text being inserted — one character for a typed key, a whole run
    /// for a paste, empty for a deletion.
    pub change: String,
    /// The field's text before the change.
    pub value: String,
    /// Where the replaced range starts, as a character index.
    pub selection_start: u32,
    /// Where it ends, as a character index.
    pub selection_end: u32,
}

/// What the keystroke hook decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeystrokeOutcome {
    /// Take the keystroke, possibly with the change text rewritten.
    Accept(Keystroke),
    /// Drop the keystroke. The field is left as it was.
    Reject,
}

/// Values a calculation wants written to other fields.
///
/// A recursion budget rides along, because a calculation that triggers
/// another calculation is the shape a script uses to build an infinite loop.
/// The budget belongs to the implementation rather than to the seam: the
/// script-free cascade never spends any of it.
#[derive(Debug, Clone, Default)]
pub struct FieldWrites {
    writes: Vec<(u32, String)>,
    depth: u32,
    max_depth: u32,
}

impl FieldWrites {
    /// A write sink that permits `max_depth` nested calculations.
    #[must_use]
    pub fn with_max_depth(max_depth: u32) -> FieldWrites {
        FieldWrites {
            writes: Vec::new(),
            depth: 0,
            max_depth,
        }
    }

    /// Records a new value for a field. Ignored past the recursion budget.
    pub fn set(&mut self, field: u32, value: impl Into<String>) {
        if self.depth <= self.max_depth {
            self.writes.push((field, value.into()));
        }
    }

    /// The recorded writes, in the order they were made.
    pub fn writes(&self) -> impl Iterator<Item = (u32, &str)> {
        self.writes.iter().map(|(f, v)| (*f, v.as_str()))
    }

    /// Whether nothing was written.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.writes.is_empty()
    }

    /// How deep the current calculation is nested.
    #[must_use]
    pub fn depth(&self) -> u32 {
        self.depth
    }

    /// Whether one more level of nesting is permitted.
    #[must_use]
    pub fn can_recurse(&self) -> bool {
        self.depth < self.max_depth
    }

    /// Enters one level of nesting. Returns whether the budget allowed it.
    pub fn enter(&mut self) -> bool {
        if !self.can_recurse() {
            return false;
        }
        self.depth += 1;
        true
    }

    /// Leaves one level of nesting.
    pub fn leave(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }
}

/// The script hooks a commit passes through.
///
/// One implementation ships here — [`NoScripts`] — and its behaviour is the
/// method defaults below. See the module documentation for why those are the
/// specification rather than a placeholder.
pub trait Cascade {
    /// The keystroke hook, with no commit imminent: may rewrite or reject
    /// what is being typed.
    fn keystroke(&mut self, _field: &FieldRef, change: Keystroke) -> KeystrokeOutcome {
        KeystrokeOutcome::Accept(change)
    }

    /// The keystroke hook with a commit imminent: may reject the commit.
    fn keystroke_commit(&mut self, _field: &FieldRef, _value: &str) -> bool {
        true
    }

    /// The validation hook: may reject the commit.
    fn validate(&mut self, _field: &FieldRef, _value: &str) -> bool {
        true
    }

    /// The calculation hook: may rewrite other fields' values.
    fn calculate(&mut self, _writes: &mut FieldWrites, _trigger: &FieldRef) {}

    /// The format hook: may return a display string that does not change the
    /// stored value.
    ///
    /// `None` means "display the raw value", which is exactly what a build
    /// without scripts does — and exactly why such a build shows `1234` where
    /// a formatting script would show a currency amount.
    fn format(&mut self, _field: &FieldRef, _value: &str) -> Option<String> {
        None
    }
}

/// The script-free cascade, and this crate's only implementation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NoScripts;

impl Cascade for NoScripts {}

#[cfg(test)]
mod tests {
    use super::*;

    fn field() -> FieldRef {
        FieldRef {
            name: "Text Box".to_string(),
            index: 0,
        }
    }

    fn keystroke(change: &str) -> Keystroke {
        Keystroke {
            change: change.to_string(),
            value: String::new(),
            selection_start: 0,
            selection_end: 0,
        }
    }

    /// The permissive answers are the contract, not a placeholder: a build
    /// without scripts accepts every keystroke and every commit.
    #[test]
    fn the_script_free_cascade_accepts_everything_unchanged() {
        let mut cascade = NoScripts;
        assert_eq!(
            cascade.keystroke(&field(), keystroke("A")),
            KeystrokeOutcome::Accept(keystroke("A"))
        );
        assert!(cascade.keystroke_commit(&field(), "anything"));
        assert!(cascade.validate(&field(), "anything"));
    }

    /// No cross-field recalculation happens without scripts.
    #[test]
    fn the_script_free_cascade_writes_no_other_field() {
        let mut cascade = NoScripts;
        let mut writes = FieldWrites::with_max_depth(4);
        cascade.calculate(&mut writes, &field());
        assert!(writes.is_empty());
    }

    /// A field displays its raw value: this is the visible gap a formatting
    /// script would close.
    #[test]
    fn the_script_free_cascade_formats_nothing() {
        let mut cascade = NoScripts;
        assert_eq!(cascade.format(&field(), "1234"), None);
    }

    #[test]
    fn writes_are_recorded_in_order() {
        let mut writes = FieldWrites::with_max_depth(4);
        writes.set(2, "two");
        writes.set(1, "one");
        let seen: Vec<_> = writes.writes().collect();
        assert_eq!(seen, vec![(2, "two"), (1, "one")]);
    }

    /// The budget is what stops a calculation that triggers a calculation.
    #[test]
    fn nesting_stops_at_the_budget() {
        let mut writes = FieldWrites::with_max_depth(2);
        assert!(writes.enter());
        assert!(writes.enter());
        assert!(!writes.enter(), "budget should be exhausted");
        assert_eq!(writes.depth(), 2);

        writes.leave();
        assert!(writes.can_recurse());
    }

    /// The trait is object safe: the commit path holds exactly one of these
    /// behind a reference, which is the whole reason it is a trait.
    #[test]
    fn the_seam_is_object_safe() {
        let mut owned = NoScripts;
        let cascade: &mut dyn Cascade = &mut owned;
        assert!(cascade.validate(&field(), "x"));
    }
}
