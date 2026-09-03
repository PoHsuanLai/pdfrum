//! The input vocabulary: what a caller hands the engine.
//!
//! These are *semantic* events, in page space, with typed keys, typed
//! modifiers and `kurbo` points.
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
/// crop box — in this crate's own `f32`.
///
/// **Private, deliberately.** The public vocabulary is [`kurbo::Point`], and
/// this is what [`crate::route::apply`] narrows it to on the way in — the same
/// place the oracle narrows its own `double` pair.
/// Every geometric comparison in this crate is `f32` against widget edges
/// that `page::to_rect` already rounded to `f32`: an `f64` point meeting one
/// of those changes inclusive-edge behaviour and can move a caret across a
/// glyph boundary, which is why the narrowing is at the entry function and
/// not one layer further in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Point {
    /// Distance right of the crop box's left edge.
    pub(crate) x: f32,
    /// Distance **up** from the crop box's bottom edge.
    pub(crate) y: f32,
}

impl Point {
    /// A point at the given page-space coordinates.
    pub(crate) fn new(x: f32, y: f32) -> Point {
        Point { x, y }
    }

    /// The narrowing: a caller's `f64` page-space point onto this crate's.
    ///
    /// The whole of the `f64`/`f32` boundary, in one function, called from
    /// one place. A page coordinate past `f32`'s exact range has already lost
    /// its meaning, so rounding it loses nothing that was still there.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "page coordinates beyond f32 have already lost meaning, and every \
                  geometric query in this crate is f32 — see the type's own docs"
    )]
    pub(crate) fn narrow(at: kurbo::Point) -> Point {
        Point::new(at.x as f32, at.y as f32)
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

/// A key on a keyboard, as an event reports it.
///
/// The named variants are the keys the form layer *decides on* — navigation,
/// editing, and the three accelerator letters — plus the two modifier keys a
/// host reports as keys in their own right. Everything else is [`Key::Other`],
/// the arm that says "the form layer does not decide on this".
///
/// [`Key::from_virtual`] and [`Key::virtual_code`] are the boundary with a
/// host's own event queue, which speaks in bare integers.
///
/// ```
/// use pdfrum_form::Key;
///
/// assert_eq!(Key::from_virtual(0x09), Key::Tab);
/// assert_eq!(Key::Tab.virtual_code(), 0x09);
/// // A code the form layer does not branch on round-trips too.
/// assert_eq!(Key::from_virtual(0x70), Key::Other(0x70));
/// assert_eq!(Key::Other(0x70).virtual_code(), 0x70);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Key {
    /// No key. Also what a selection-clearing delete is rewritten to, which
    /// is why the text field branches on it rather than ignoring it.
    Unknown,
    /// Backspace.
    Backspace,
    /// Tab — focus traversal.
    Tab,
    /// Line feed. Distinct from [`Key::Return`], which is the carriage
    /// return a host sends for the Enter key.
    Newline,
    /// Carriage return — activates a widget, or commits a single-line field.
    Return,
    /// Escape — discards an in-progress edit.
    Escape,
    /// Space — activates a widget.
    Space,
    /// Page up. Not handled by the edit control.
    PageUp,
    /// Page down. Not handled by the edit control.
    PageDown,
    /// End of line, or of the document with the accelerator held.
    End,
    /// Start of line, or of the document with the accelerator held.
    Home,
    /// Caret left.
    Left,
    /// Caret up.
    Up,
    /// Caret right.
    Right,
    /// Caret down.
    Down,
    /// Insert. Not handled.
    Insert,
    /// Forward delete.
    Delete,
    /// The letter A — select-all with the accelerator.
    A,
    /// The letter Y — redo with the accelerator, off Apple.
    Y,
    /// The letter Z — undo, or redo with shift.
    Z,
    /// The shift key reported as a key in its own right. Never consumed.
    Shift,
    /// The control key reported as a key in its own right. Never consumed.
    Control,
    /// Any other key, by the code a host reported it under.
    ///
    /// Never carries a code a named variant already names: [`Key::from_virtual`]
    /// is the only way one is built from an integer, and it maps the named
    /// codes first.
    Other(u16),
}

impl Key {
    /// The key a host's virtual-key code names.
    ///
    /// Total, and the inverse of [`Key::virtual_code`]: a code no variant
    /// names becomes [`Key::Other`] carrying it unchanged.
    #[must_use]
    pub const fn from_virtual(code: u16) -> Key {
        match code {
            0x00 => Key::Unknown,
            0x08 => Key::Backspace,
            0x09 => Key::Tab,
            0x0A => Key::Newline,
            0x0D => Key::Return,
            0x10 => Key::Shift,
            0x11 => Key::Control,
            0x1B => Key::Escape,
            0x20 => Key::Space,
            0x21 => Key::PageUp,
            0x22 => Key::PageDown,
            0x23 => Key::End,
            0x24 => Key::Home,
            0x25 => Key::Left,
            0x26 => Key::Up,
            0x27 => Key::Right,
            0x28 => Key::Down,
            0x2D => Key::Insert,
            0x2E => Key::Delete,
            0x41 => Key::A,
            0x59 => Key::Y,
            0x5A => Key::Z,
            other => Key::Other(other),
        }
    }

    /// The virtual-key code this key is reported under.
    ///
    /// The inverse of [`Key::from_virtual`] over every value that function
    /// can produce.
    #[must_use]
    pub const fn virtual_code(self) -> u16 {
        match self {
            Key::Unknown => 0x00,
            Key::Backspace => 0x08,
            Key::Tab => 0x09,
            Key::Newline => 0x0A,
            Key::Return => 0x0D,
            Key::Shift => 0x10,
            Key::Control => 0x11,
            Key::Escape => 0x1B,
            Key::Space => 0x20,
            Key::PageUp => 0x21,
            Key::PageDown => 0x22,
            Key::End => 0x23,
            Key::Home => 0x24,
            Key::Left => 0x25,
            Key::Up => 0x26,
            Key::Right => 0x27,
            Key::Down => 0x28,
            Key::Insert => 0x2D,
            Key::Delete => 0x2E,
            Key::A => 0x41,
            Key::Y => 0x59,
            Key::Z => 0x5A,
            Key::Other(code) => code,
        }
    }
}

/// The modifier bits carried by an event (`FWL_EVENTFLAG`).
///
/// A hand-written bitflag newtype: nine constants and a handful of
/// operations, sharing the algebra of `pdfrum_font::FontFlags` and
/// `pdfrum_doc::AnnotFlags`, unknown-bit retention included.
///
/// ```
/// use pdfrum_form::Modifiers;
///
/// let m = Modifiers::SHIFT | Modifiers::CONTROL;
/// assert!(m.contains(Modifiers::SHIFT));
/// assert!(!m.without(Modifiers::SHIFT).contains(Modifiers::SHIFT));
/// assert_eq!(Modifiers::from_bits(1 << 30).bits(), 1 << 30);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct Modifiers(u32);

impl Modifiers {
    /// No modifiers held.
    pub const NONE: Self = Self(0);
    /// Shift.
    pub const SHIFT: Self = Self(1 << 0);
    /// Control.
    pub const CONTROL: Self = Self(1 << 1);
    /// Alt.
    pub const ALT: Self = Self(1 << 2);
    /// Meta — Command on Apple keyboards.
    pub const META: Self = Self(1 << 3);
    /// The key came from the numeric keypad.
    pub const KEYPAD: Self = Self(1 << 4);
    /// The key is repeating because it is held down.
    pub const AUTO_REPEAT: Self = Self(1 << 5);
    /// The left mouse button is down.
    pub const LEFT_BUTTON: Self = Self(1 << 6);
    /// The middle mouse button is down.
    pub const MIDDLE_BUTTON: Self = Self(1 << 7);
    /// The right mouse button is down.
    pub const RIGHT_BUTTON: Self = Self(1 << 8);

    /// The raw modifier word, including any bit this type does not name.
    #[must_use]
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// The word as a host reported it. **Unknown bits are retained.**
    #[must_use]
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    /// Whether every bit of `other` is set here.
    ///
    /// [`Modifiers::NONE`] is contained in everything, which is what makes
    /// `contains` the wrong question to ask about "no modifiers held" — use
    /// `== Modifiers::NONE` for that.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Both sets of bits.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// A copy with `other`'s bits set. An alias for [`Modifiers::union`].
    #[must_use]
    pub const fn with(self, other: Self) -> Self {
        self.union(other)
    }

    /// The bits of `self` that are not in `other`.
    #[must_use]
    pub const fn without(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }

    /// Whether no bit at all is set.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl std::ops::BitOr for Modifiers {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self {
        self.union(rhs)
    }
}

/// One input event.
///
/// Points are [`kurbo::Point`] — page space, PDF user space, y-**up**, origin
/// at the crop box, and `f64`. That is the same vocabulary `Page::crop_box`
/// speaks and the same one the oracle's own entry points take
/// (`FORM_OnMouseMove(.., double page_x, double page_y)`); the crate narrows
/// to its private `f32` point in [`crate::route::apply`] and nowhere else.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Event {
    /// The pointer moved. Drives hover enter/exit and extends a live drag.
    MouseMove {
        /// Where, in page space.
        at: kurbo::Point,
        /// Which modifiers were held.
        modifiers: Modifiers,
    },
    /// A mouse button went down.
    MouseDown {
        /// Which button.
        button: Button,
        /// Where, in page space.
        at: kurbo::Point,
        /// Which modifiers were held.
        modifiers: Modifiers,
    },
    /// A mouse button came up.
    MouseUp {
        /// Which button.
        button: Button,
        /// Where, in page space.
        at: kurbo::Point,
        /// Which modifiers were held.
        modifiers: Modifiers,
    },
    /// A double click. Carries no button because the grammar rejects any
    /// button but the left one.
    DoubleClick {
        /// Where, in page space.
        at: kurbo::Point,
        /// Which modifiers were held.
        modifiers: Modifiers,
    },
    /// The wheel turned. Deltas are notches, negative `y` meaning down.
    MouseWheel {
        /// Where the pointer was, in page space.
        at: kurbo::Point,
        /// Horizontal and vertical notches.
        delta: (i32, i32),
        /// Which modifiers were held.
        modifiers: Modifiers,
    },
    /// Focus was requested at a point, without a click.
    Focus {
        /// Where, in page space.
        at: kurbo::Point,
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

    /// The `f64`-to-`f32` hazard, pinned at the boundary that answers it.
    ///
    /// [`crate::route::apply`] narrows an [`Event`]'s `f64` point to this
    /// `f32` one before any comparison. The interior then compares against
    /// widget edges `page::to_rect` already rounded the same way, so an
    /// on-the-edge click stays on the edge. If the narrowing ever moved
    /// deeper — an `f64` reaching `hit::contains` or `Plate::to_widget` —
    /// the value it met would be a *different* number from the one this
    /// pins, and `hit.rs`'s `containment_includes_every_edge` would start
    /// disagreeing with a caller who clicked exactly on a boundary.
    #[expect(
        clippy::float_cmp,
        reason = "bit-exactness is the assertion: a tolerance would pass under \
                  precisely the half-migration this test exists to forbid"
    )]
    #[test]
    fn a_fractional_coordinate_is_narrowed_before_any_comparison() {
        // A value with a fractional part that `f32` cannot hold exactly.
        let at = kurbo::Point::new(10.1, 713.7);
        let narrowed = Point::narrow(at);

        // What the interior sees is the `f32` nearest the caller's `f64` —
        // and it is *not* the caller's value, which is the whole point.
        assert_eq!(narrowed.x, 10.1_f32);
        assert_eq!(narrowed.y, 713.7_f32);
        assert!(f64::from(narrowed.x) != at.x, "10.1 is not exact in f32");

        // And it is exactly what `page::to_rect` produces for the same
        // number, so an edge written `10.1` in the file and a click at
        // `10.1` from the host meet as equals.
        let edge = crate::page::to_rect(kurbo::Rect::new(10.1, 713.7, 20.0, 800.0));
        assert_eq!(narrowed.x, edge.left);
        assert_eq!(narrowed.y, edge.bottom);
        assert!(
            crate::hit::contains(edge, narrowed.x, narrowed.y),
            "a click exactly on a fractional edge is inside it"
        );
    }

    /// An integer coordinate — which is all an `.evt` script can write, since
    /// its parser is a hand-rolled `atoi` — survives the widening and the
    /// narrowing unchanged. This is why no golden moved.
    #[expect(
        clippy::float_cmp,
        reason = "exactness is the assertion — this is why no golden moved"
    )]
    #[test]
    fn an_integer_coordinate_round_trips_exactly() {
        for value in [0.0_f64, 1.0, 312.0, -450.0, 9999.0] {
            let narrowed = Point::narrow(kurbo::Point::new(value, value));
            assert_eq!(f64::from(narrowed.x), value);
            assert_eq!(f64::from(narrowed.y), value);
        }
    }

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
        assert_eq!(Modifiers::SHIFT.bits(), 1);
        assert_eq!(Modifiers::CONTROL.bits(), 2);
        assert_eq!((Modifiers::SHIFT | Modifiers::CONTROL).bits(), 3);
        assert_eq!(Modifiers::ALT.bits(), 4);
        assert_eq!(Modifiers::META.bits(), 8);
    }

    #[test]
    fn unknown_modifier_bits_round_trip() {
        let f = Modifiers::from_bits((1 << 30) | Modifiers::SHIFT.bits());
        assert_eq!(f.bits(), (1 << 30) | 1);
        assert!(f.contains(Modifiers::SHIFT));
        assert!(!f.contains(Modifiers::CONTROL));
    }

    #[test]
    fn modifier_set_algebra() {
        let m = Modifiers::SHIFT | Modifiers::CONTROL | Modifiers::ALT;
        assert!(m.contains(Modifiers::SHIFT | Modifiers::ALT));
        assert!(m.contains(Modifiers::NONE));
        assert!(!m.contains(Modifiers::SHIFT | Modifiers::META));
        assert_eq!(
            m.without(Modifiers::CONTROL),
            Modifiers::SHIFT | Modifiers::ALT
        );
        assert_eq!(Modifiers::NONE.with(Modifiers::META), Modifiers::META);
        assert!(Modifiers::NONE.is_empty());
        assert!(!m.is_empty());
    }

    /// The codes are a wire format — a host's virtual-key word and the
    /// `.evt` grammar's integers both name them numerically — so the table
    /// is pinned rather than merely round-tripped.
    #[test]
    fn key_codes_are_the_virtual_key_codes() {
        assert_eq!(Key::Tab.virtual_code(), 0x09);
        assert_eq!(Key::Return.virtual_code(), 0x0D);
        assert_eq!(Key::Delete.virtual_code(), 0x2E);
        assert_eq!(Key::A.virtual_code(), 0x41);
        assert_eq!(Key::Z.virtual_code(), 0x5A);
    }

    /// Every named variant survives the trip out to a code and back, and so
    /// does an `Other` the table does not name. The list is written out
    /// rather than iterated because a variant added without a table row is
    /// exactly the mistake this catches.
    #[test]
    fn every_key_round_trips_through_its_virtual_code() {
        let named = [
            Key::Unknown,
            Key::Backspace,
            Key::Tab,
            Key::Newline,
            Key::Return,
            Key::Escape,
            Key::Space,
            Key::PageUp,
            Key::PageDown,
            Key::End,
            Key::Home,
            Key::Left,
            Key::Up,
            Key::Right,
            Key::Down,
            Key::Insert,
            Key::Delete,
            Key::A,
            Key::Y,
            Key::Z,
            Key::Shift,
            Key::Control,
        ];
        for key in named {
            assert_eq!(Key::from_virtual(key.virtual_code()), key, "{key:?}");
        }
        // Distinct codes stay distinct: a table that mapped two variants to
        // one code would pass the loop above and fail here.
        let mut codes: Vec<u16> = named.iter().map(|k| k.virtual_code()).collect();
        codes.sort_unstable();
        let count = codes.len();
        codes.dedup();
        assert_eq!(codes.len(), count, "two variants share a virtual code");
    }

    /// The `.evt` corpus sends F-keys, digits and the clipboard letters
    /// precisely to check that nothing consumes them. They must arrive.
    #[test]
    fn undecided_codes_arrive_as_other_unchanged() {
        for code in [0x70_u16, 0x30, 0x43, 0x56, 0x58, 0xFFFF] {
            assert_eq!(Key::from_virtual(code), Key::Other(code));
            assert_eq!(Key::Other(code).virtual_code(), code);
        }
    }
}
