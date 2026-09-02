//! What a document permits, decoded from the `/P` word.
//!
//! ISO 32000-1 table 22 numbers the bits from 1, and only eight of the
//! thirty-two carry meaning; the rest are reserved and must be preserved
//! rather than interpreted. The decode lives here, next to the `/P` value it
//! decodes, and not in the crates that ask the questions — that was the point
//! of `docs/design/idiomatic-api.md` §A.3, which found the facade spelling
//! `bits & 0x100` for a bit it had no business knowing the number of.

/// One bit's number in ISO 32000-1 table 22, **1-indexed as the table writes
/// it**.
///
/// The table's own numbering is the contract: a reader checking this file
/// against §7.6.4 table 22 reads "bit position 3" there and finds `3` here.
/// The shift is `bit - 1`, applied in exactly one place below, which is the
/// only arithmetic in this module.
const fn bit(word: u32, position: u32) -> bool {
    word & (1 << (position - 1)) != 0
}

/// The same, as a mask to build a word from.
const fn mask(position: u32) -> u32 {
    1 << (position - 1)
}

/// What a document's security handler permits.
///
/// Eight questions, not a bitfield — a caller asks "may I print?", never "is
/// bit 3 set?" (`docs/design/idiomatic-api.md` §6, which rules this a struct
/// of booleans rather than a fourth `bitflags`-shaped newtype precisely
/// because the answers do not compose into a set).
///
/// The reserved bits are *not* dropped: [`Permissions::from_bits`] ignores
/// them and [`Permissions::bits`] reconstructs only the eight it knows, so a
/// writer that must reproduce a document's `/P` verbatim keeps the original
/// word — which is what `pdfrum-edit` does, copying `/Encrypt` rather than
/// rebuilding it.
///
/// ```
/// use pdfrum_crypt::Permissions;
///
/// // `/P -1` — every bit set, which is what an unrestricted document says.
/// // (The standard handler forces `0xFFFF_FFFC` for a `/P` of -4, which
/// // differs only in the two reserved low bits and decodes the same.)
/// let all = Permissions::from_bits(0xFFFF_FFFF);
/// assert_eq!(all, Permissions::ALL);
/// assert!(all.print);
///
/// // Bit 3 alone: printing, and nothing else.
/// let print_only = Permissions::from_bits(0b100);
/// assert!(print_only.print);
/// assert!(!print_only.copy);
/// ```
// Eight booleans is exactly the shape `docs/design/idiomatic-api.md` §6 rules
// for this type, against the `bitflags`-newtype alternative it considers and
// rejects: table 22's bits are eight independent questions with reserved holes
// between them, not a set that composes, and `struct_excessive_bools`'s usual
// advice — collapse them into an enum or a flags type — is the design that was
// weighed and declined. The lint is right about most structs and wrong about
// this one.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Permissions {
    /// Bit 3 — print the document.
    ///
    /// Revision 3 and above qualify this: with [`print_high_quality`] clear,
    /// printing is permitted only in a degraded form.
    ///
    /// [`print_high_quality`]: Permissions::print_high_quality
    pub print: bool,
    /// Bit 4 — modify the contents by operations other than those controlled
    /// by bits 6, 9 and 11.
    pub modify: bool,
    /// Bit 5 — copy or otherwise extract text and graphics.
    pub copy: bool,
    /// Bit 6 — add or modify text annotations and fill in interactive form
    /// fields.
    ///
    /// The table couples the two: a document granting this grants both. Bit 9
    /// then grants form filling *without* annotation editing, which is why
    /// [`fill_form`] is a separate field and not implied by this one.
    ///
    /// [`fill_form`]: Permissions::fill_form
    pub annotate: bool,
    /// Bit 9 — fill in interactive form fields, including signature fields,
    /// even when bit 6 is clear.
    pub fill_form: bool,
    /// Bit 10 — extract text and graphics for accessibility.
    pub extract: bool,
    /// Bit 11 — assemble the document: insert, rotate or delete pages, and
    /// create bookmarks or thumbnails, even when bit 4 is clear.
    pub assemble: bool,
    /// Bit 12 — print at high resolution rather than as a degraded image.
    pub print_high_quality: bool,
}

impl Permissions {
    /// Every permission granted — an unencrypted document, and an
    /// owner-authenticated one.
    pub const ALL: Self = Self {
        print: true,
        modify: true,
        copy: true,
        annotate: true,
        fill_form: true,
        extract: true,
        assemble: true,
        print_high_quality: true,
    };

    /// Nothing granted.
    pub const NONE: Self = Self {
        print: false,
        modify: false,
        copy: false,
        annotate: false,
        fill_form: false,
        extract: false,
        assemble: false,
        print_high_quality: false,
    };

    /// Decode the `/P` word (ISO 32000-1 §7.6.4 table 22).
    ///
    /// Reserved bits are ignored rather than refused: a damaged or
    /// forward-dated file sets bits this table does not name, and the answer
    /// to "may I print?" does not depend on them.
    #[must_use]
    pub const fn from_bits(bits: u32) -> Self {
        Self {
            print: bit(bits, 3),
            modify: bit(bits, 4),
            copy: bit(bits, 5),
            annotate: bit(bits, 6),
            fill_form: bit(bits, 9),
            extract: bit(bits, 10),
            assemble: bit(bits, 11),
            print_high_quality: bit(bits, 12),
        }
    }

    /// The eight bits back as a word, for a writer that must emit `/P`.
    ///
    /// **Not** a round trip of [`from_bits`]: reserved bits the input carried
    /// are not here, because this type never held them. A save that must
    /// reproduce a document's `/P` exactly keeps the original word instead —
    /// which is what `pdfrum-edit` does today, copying the whole `/Encrypt`
    /// dictionary.
    ///
    /// [`from_bits`]: Permissions::from_bits
    #[must_use]
    pub const fn bits(self) -> u32 {
        let mut word = 0;
        if self.print {
            word |= mask(3);
        }
        if self.modify {
            word |= mask(4);
        }
        if self.copy {
            word |= mask(5);
        }
        if self.annotate {
            word |= mask(6);
        }
        if self.fill_form {
            word |= mask(9);
        }
        if self.extract {
            word |= mask(10);
        }
        if self.assemble {
            word |= mask(11);
        }
        if self.print_high_quality {
            word |= mask(12);
        }
        word
    }
}

#[cfg(test)]
mod tests {
    use super::Permissions;

    /// ISO 32000-1 §7.6.4 table 22, written as the table writes it: **bit
    /// numbers, 1-indexed**, never masks. A reviewer with the standard open
    /// checks this column against the page, and nothing here computes
    /// `1 << (n - 1)` — that is the code under test's job, and repeating it in
    /// the test would test the test.
    const TABLE_22: [(u32, &str); 8] = [
        (3, "print"),
        (4, "modify"),
        (5, "copy"),
        (6, "annotate"),
        (9, "fill_form"),
        (10, "extract"),
        (11, "assemble"),
        (12, "print_high_quality"),
    ];

    /// The field a name selects, so the table above can drive the assertions.
    fn field(p: Permissions, name: &str) -> bool {
        match name {
            "print" => p.print,
            "modify" => p.modify,
            "copy" => p.copy,
            "annotate" => p.annotate,
            "fill_form" => p.fill_form,
            "extract" => p.extract,
            "assemble" => p.assemble,
            "print_high_quality" => p.print_high_quality,
            other => panic!("no field {other}"),
        }
    }

    // Each bit, alone, grants exactly its own permission and no other. This is
    // the assertion that catches an off-by-one in the 1-indexing: bit 3 set
    // alone must read as `print`, not as `modify`.
    #[test]
    fn each_table_22_bit_grants_exactly_its_own_permission() {
        for (position, name) in TABLE_22 {
            let word = 1u32 << (position - 1);
            let p = Permissions::from_bits(word);
            for (_, other) in TABLE_22 {
                assert_eq!(
                    field(p, other),
                    other == name,
                    "bit {position} ({name}) should grant only {name}, but {other} read \
                     {}",
                    field(p, other)
                );
            }
        }
    }

    // `/P -1` — every bit set — is the unrestricted document, which is what
    // `SecurityHandler::Identity` reports and what an owner-unlocked handler
    // is forced to.
    #[test]
    fn every_bit_set_is_all() {
        assert_eq!(Permissions::from_bits(0xFFFF_FFFF), Permissions::ALL);
    }

    #[test]
    fn no_bit_set_is_none() {
        assert_eq!(Permissions::from_bits(0), Permissions::NONE);
    }

    // Reserved bits are ignored, not refused. Bits 1, 2, 7, 8 and 13 upward
    // carry no meaning in table 22; a file setting all of them and nothing
    // else grants nothing.
    #[test]
    fn reserved_bits_grant_nothing() {
        let named: u32 = TABLE_22
            .iter()
            .fold(0, |acc, (position, _)| acc | (1 << (position - 1)));
        assert_eq!(Permissions::from_bits(!named), Permissions::NONE);
    }

    // `bits()` reconstructs the eight named bits and only those, so it
    // round-trips a word that had no reserved bits set — and drops the ones it
    // did, which is documented and is why `pdfrum-edit` copies `/Encrypt`
    // rather than rebuilding it.
    #[test]
    fn bits_round_trips_the_named_bits_and_drops_the_rest() {
        let named: u32 = TABLE_22
            .iter()
            .fold(0, |acc, (position, _)| acc | (1 << (position - 1)));
        assert_eq!(Permissions::ALL.bits(), named);
        assert_eq!(Permissions::NONE.bits(), 0);
        assert_eq!(Permissions::from_bits(0xFFFF_FFFF).bits(), named);
        for (position, _) in TABLE_22 {
            let word = 1u32 << (position - 1);
            assert_eq!(Permissions::from_bits(word).bits(), word, "bit {position}");
        }
    }

    // The standard handler's own reading of `/P 4092`, which is the value
    // `docs` and the C++ both use as the worked example: bits 3 through 12
    // set, everything else clear. Table 22 names eight of those ten, and the
    // two it does not (7 and 8) are reserved.
    #[test]
    fn the_worked_example_p_4092_grants_everything_named() {
        assert_eq!(Permissions::from_bits(4092), Permissions::ALL);
    }
}
