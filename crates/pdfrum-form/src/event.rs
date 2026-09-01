//! The input vocabulary: what a caller hands the engine (SPEC §15.5).
//!
//! These are *semantic* events, in page space, with typed keys and modifiers.
//! They are deliberately not the `.evt` grammar's own types: that file format
//! parses integers with `atoi` and emits one verb for a key-down/key-up pair,
//! which is a faithful description of a text file and a poor description of
//! what a form does. The two layers meet in one conversion function, which
//! lives with the parser.
//!
//! Two variants a reader may go looking for are absent by derivation rather
//! than by omission. There is no key-up: the oracle's entry point for it is
//! documented as permanently unimplemented and returns false, so an API that
//! modelled it would invite callers to send an event that cannot do anything.
//! And there is no idle tick: a blank line in an event script does not pump
//! the host's message loop, and nothing in a script-free build observes one.

/// A position in page space — PDF user space, y-**up**, origin at the page's
/// crop box.
///
/// Coordinates are `f32` even though an event script can only write integers,
/// because the entry points take doubles and the ported assertions click at
/// fractional positions.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    /// Distance right of the crop box's left edge.
    pub x: f32,
    /// Distance **up** from the crop box's bottom edge.
    pub y: f32,
}

impl Point {
    /// A point at the given page-space coordinates.
    #[must_use]
    pub fn new(x: f32, y: f32) -> Point {
        Point { x, y }
    }
}

/// Which mouse button an event came from.
///
/// The right button is representable because event scripts contain it, and
/// the correct response to those lines is to consume nothing: outside XFA
/// builds — which are declined — the right-button entry points do nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    /// The primary button.
    Left,
    /// The secondary button. Never has an effect.
    Right,
}

/// A virtual key code.
///
/// A newtype over the raw code rather than an enum, because the wire format
/// admits any integer and the ported assertions deliberately send codes the
/// form layer does not decide on — F1, digits, letters — to check that they
/// are *not* consumed. An enum would have to carry an `Other(u16)` arm that
/// every match would then have to handle anyway.
///
/// The constants below are the codes the form layer actually branches on;
/// everything else falls through as "not a navigation key".
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Key(pub u16);

impl Key {
    /// No key. Also what a selection-clearing delete is rewritten to.
    pub const UNKNOWN: Key = Key(0x00);
    /// Backspace.
    pub const BACK: Key = Key(0x08);
    /// Tab — focus traversal.
    pub const TAB: Key = Key(0x09);
    /// Line feed.
    pub const NEWLINE: Key = Key(0x0A);
    /// Clear.
    pub const CLEAR: Key = Key(0x0C);
    /// Carriage return.
    pub const RETURN: Key = Key(0x0D);
    /// Escape — discards an in-progress edit.
    pub const ESCAPE: Key = Key(0x1B);
    /// Space.
    pub const SPACE: Key = Key(0x20);
    /// Page up. Not handled by the edit control.
    pub const PRIOR: Key = Key(0x21);
    /// Page down. Not handled by the edit control.
    pub const NEXT: Key = Key(0x22);
    /// End of line, or of the document with the accelerator held.
    pub const END: Key = Key(0x23);
    /// Start of line, or of the document with the accelerator held.
    pub const HOME: Key = Key(0x24);
    /// Caret left.
    pub const LEFT: Key = Key(0x25);
    /// Caret up.
    pub const UP: Key = Key(0x26);
    /// Caret right.
    pub const RIGHT: Key = Key(0x27);
    /// Caret down.
    pub const DOWN: Key = Key(0x28);
    /// Insert.
    pub const INSERT: Key = Key(0x2D);
    /// Forward delete.
    pub const DELETE: Key = Key(0x2E);
    /// The letter A — select-all with the accelerator.
    pub const A: Key = Key(0x41);
    /// The letter Y — redo with the accelerator, off Apple.
    pub const Y: Key = Key(0x59);
    /// The letter Z — undo, or redo with shift.
    pub const Z: Key = Key(0x5A);
    /// The shift key reported as a key in its own right. Never consumed.
    pub const SHIFT: Key = Key(0x10);
    /// The control key reported as a key in its own right. Never consumed.
    pub const CONTROL: Key = Key(0x11);
}

/// The modifier bits carried by an event.
///
/// A hand-written bitflag newtype rather than a dependency: the dependency
/// manifest is closed and this is nine constants and two operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct Modifiers(pub u32);

impl Modifiers {
    /// No modifiers held.
    pub const NONE: Modifiers = Modifiers(0);
    /// Shift.
    pub const SHIFT: Modifiers = Modifiers(1 << 0);
    /// Control.
    pub const CONTROL: Modifiers = Modifiers(1 << 1);
    /// Alt.
    pub const ALT: Modifiers = Modifiers(1 << 2);
    /// Meta — Command on Apple keyboards.
    pub const META: Modifiers = Modifiers(1 << 3);
    /// The key came from the numeric keypad.
    pub const KEYPAD: Modifiers = Modifiers(1 << 4);
    /// The key is repeating because it is held down.
    pub const AUTO_REPEAT: Modifiers = Modifiers(1 << 5);
    /// The left mouse button is down.
    pub const LEFT_BUTTON: Modifiers = Modifiers(1 << 6);
    /// The middle mouse button is down.
    pub const MIDDLE_BUTTON: Modifiers = Modifiers(1 << 7);
    /// The right mouse button is down.
    pub const RIGHT_BUTTON: Modifiers = Modifiers(1 << 8);

    /// Whether every bit of `other` is set here.
    ///
    /// [`Modifiers::NONE`] is contained in everything, which is what makes
    /// `contains` the wrong question to ask about "no modifiers held" — use
    /// `== Modifiers::NONE` for that.
    #[must_use]
    pub fn contains(self, other: Modifiers) -> bool {
        self.0 & other.0 == other.0
    }

    /// Both sets of bits.
    #[must_use]
    pub fn union(self, other: Modifiers) -> Modifiers {
        Modifiers(self.0 | other.0)
    }

    /// The bits of `self` that are not in `other`.
    #[must_use]
    pub fn without(self, other: Modifiers) -> Modifiers {
        Modifiers(self.0 & !other.0)
    }

    /// Whether no bit at all is set.
    #[must_use]
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl std::ops::BitOr for Modifiers {
    type Output = Modifiers;

    fn bitor(self, rhs: Modifiers) -> Modifiers {
        self.union(rhs)
    }
}

/// One input event.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Event {
    /// The pointer moved. Drives hover enter/exit and extends a live drag.
    MouseMove {
        /// Where, in page space.
        at: Point,
        /// Which modifiers were held.
        modifiers: Modifiers,
    },
    /// A mouse button went down.
    MouseDown {
        /// Which button.
        button: Button,
        /// Where, in page space.
        at: Point,
        /// Which modifiers were held.
        modifiers: Modifiers,
    },
    /// A mouse button came up.
    MouseUp {
        /// Which button.
        button: Button,
        /// Where, in page space.
        at: Point,
        /// Which modifiers were held.
        modifiers: Modifiers,
    },
    /// A double click. Carries no button because the grammar rejects any
    /// button but the left one.
    DoubleClick {
        /// Where, in page space.
        at: Point,
        /// Which modifiers were held.
        modifiers: Modifiers,
    },
    /// The wheel turned. Deltas are notches, negative `y` meaning down.
    MouseWheel {
        /// Where the pointer was, in page space.
        at: Point,
        /// Horizontal and vertical notches.
        delta: (i32, i32),
        /// Which modifiers were held.
        modifiers: Modifiers,
    },
    /// Focus was requested at a point, without a click.
    Focus {
        /// Where, in page space.
        at: Point,
        /// Which modifiers were held.
        modifiers: Modifiers,
    },
    /// A key went down. Navigation and shortcuts arrive here, never as text.
    KeyDown {
        /// Which key.
        key: Key,
        /// Which modifiers were held.
        modifiers: Modifiers,
    },
    /// A character was typed. Text arrives here, never as a key-down.
    ///
    /// This split is the single most load-bearing fact in the event model:
    /// typing sends only this, and the accelerator shortcuts are decided only
    /// on the key-down path. A character that arrives here with the
    /// accelerator held is deliberately *not* a shortcut.
    Char {
        /// The character typed.
        ch: char,
        /// Which modifiers were held.
        modifiers: Modifiers,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modifiers_contains_is_subset_not_equality() {
        let both = Modifiers::SHIFT | Modifiers::CONTROL;
        assert!(both.contains(Modifiers::SHIFT));
        assert!(both.contains(Modifiers::CONTROL));
        assert!(both.contains(Modifiers::NONE));
        assert!(!both.contains(Modifiers::ALT));
        assert!(!Modifiers::SHIFT.contains(both));
    }

    #[test]
    fn modifiers_none_is_empty_and_everything_contains_it() {
        assert!(Modifiers::NONE.is_empty());
        assert!(!Modifiers::SHIFT.is_empty());
        assert!(Modifiers::NONE.contains(Modifiers::NONE));
    }

    #[test]
    fn modifiers_without_removes_only_named_bits() {
        let all = Modifiers::SHIFT | Modifiers::CONTROL | Modifiers::ALT;
        assert_eq!(
            all.without(Modifiers::CONTROL),
            Modifiers::SHIFT | Modifiers::ALT
        );
    }

    /// The bit values are a wire format, not an internal choice: an event
    /// script's modifier field and the ported link-action assertions both
    /// name them numerically.
    #[test]
    fn modifier_bits_are_the_documented_wire_values() {
        assert_eq!(Modifiers::SHIFT.0, 1);
        assert_eq!(Modifiers::CONTROL.0, 2);
        assert_eq!((Modifiers::SHIFT | Modifiers::CONTROL).0, 3);
        assert_eq!(Modifiers::ALT.0, 4);
        assert_eq!(Modifiers::META.0, 8);
    }

    #[test]
    fn key_constants_are_the_virtual_key_codes() {
        assert_eq!(Key::TAB.0, 0x09);
        assert_eq!(Key::RETURN.0, 0x0D);
        assert_eq!(Key::DELETE.0, 0x2E);
        assert_eq!(Key::A.0, 0x41);
        assert_eq!(Key::Z.0, 0x5A);
    }
}
