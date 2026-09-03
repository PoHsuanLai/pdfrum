//! `SASLprep` (RFC 4013), the string preparation ISO 32000-2 §7.6.4.3.3
//! (Algorithm 2.A) puts in front of a revision-6 password.
//!
//! `SASLprep` is a *profile* of stringprep (RFC 3454): four steps, in order —
//! map, normalise, prohibit, check bidi. What each step is made of comes from
//! RFC 3454's appendix tables, and RFC 4013 §2 names which of them this
//! profile uses:
//!
//! 1. **Mapping** (§2.1). Every non-ASCII space (table C.1.2) becomes
//!    `U+0020`; every character "commonly mapped to nothing" (table B.1) is
//!    deleted.
//! 2. **Normalisation** (§2.2). Unicode **NFKC**.
//! 3. **Prohibited output** (§2.3). Tables C.1.2 (the non-ASCII spaces the
//!    mapping did not already remove — none survive it, but the check is what
//!    the profile says), C.2.1 and C.2.2 (control characters), C.3 (private
//!    use), C.4 (non-characters), C.5 (surrogates), C.6 (inappropriate for
//!    plain text), C.7 (inappropriate for canonical representation), C.8
//!    (change display / deprecated) and C.9 (tagging).
//! 4. **Bidirectional check** (§2.4, the rule in RFC 3454 §6). A string
//!    containing a `RandALCat` character may contain no `LCat` character, and must
//!    both begin and end with a `RandALCat` character.
//!
//! RFC 4013 §2.5 additionally forbids **unassigned** code points in *stored*
//! strings while permitting them in a query. A password being checked against
//! an existing `/U` or `/O` entry is a query, not a stored string, so
//! unassigned code points are permitted here. This crate never *creates* a
//! password — there is no API to change a document's password — so the
//! stored-string direction has no site to be wrong at.
//!
//! # Why the failure modes are not errors here
//!
//! [`saslprep`] returns `None` when a step 3 or step 4 check rejects the
//! input. The caller treats that as "this candidate does not exist", not as
//! "authentication failed" — a password the specification would refuse is
//! still tried raw, because a producer that skipped `SASLprep` hashed the raw
//! bytes and its file must still open. That is also what pdf.js does, from
//! the other end: its `sasl_prep.js` omits steps 3 and 4 outright and relies
//! on the hash comparison to reject a bad candidate.
//!
//! # The Unicode version
//!
//! Stringprep is defined against Unicode 3.2. The `unicode-normalization`
//! crate normalises against whatever version it ships, and the code-point
//! tables below are transcribed from RFC 3454 as written. Both pdf.js
//! (`sasl_prep.js`, its `TODO`) and this module accept that skew: the
//! characters where 3.2 and a modern version disagree about NFKC are not ones
//! that appear in passwords, and freezing a second normalisation table to fix
//! it would cost more than the divergence does.

use unicode_normalization::UnicodeNormalization;

/// RFC 3454 table C.1.2 — non-ASCII space characters.
///
/// Step 1 maps every one of these to `U+0020`; step 3 then prohibits any that
/// survived, which after the mapping is none. Both uses read this one table.
const NON_ASCII_SPACES: &[char] = &[
    '\u{00A0}', '\u{1680}', '\u{2000}', '\u{2001}', '\u{2002}', '\u{2003}', '\u{2004}', '\u{2005}',
    '\u{2006}', '\u{2007}', '\u{2008}', '\u{2009}', '\u{200A}', '\u{200B}', '\u{202F}', '\u{205F}',
    '\u{3000}',
];

/// RFC 3454 table B.1 — characters commonly mapped to nothing.
///
/// Step 1 deletes these. `U+200B` is in this table *and* in C.1.2; the
/// mapping order in RFC 4013 §2.1 puts the space mapping first, so it becomes
/// a space rather than vanishing. pdf.js resolves the same overlap the same
/// way (`sasl_prep.js`, its `if`/`else if`).
const MAPPED_TO_NOTHING: &[char] = &[
    '\u{00AD}', '\u{034F}', '\u{1806}', '\u{180B}', '\u{180C}', '\u{180D}', '\u{200B}', '\u{200C}',
    '\u{200D}', '\u{2060}', '\u{FE00}', '\u{FE01}', '\u{FE02}', '\u{FE03}', '\u{FE04}', '\u{FE05}',
    '\u{FE06}', '\u{FE07}', '\u{FE08}', '\u{FE09}', '\u{FE0A}', '\u{FE0B}', '\u{FE0C}', '\u{FE0D}',
    '\u{FE0E}', '\u{FE0F}', '\u{FEFF}',
];

/// Prepare `password` per RFC 4013, or `None` if a prohibited character or a
/// bidirectional violation makes it unpreparable.
///
/// The result is the prepared *string*; the caller encodes it as UTF-8 and
/// truncates the bytes, because ISO 32000-2 §7.6.4.3.3 truncates after the
/// encoding, not before it.
#[must_use]
pub(crate) fn saslprep(password: &str) -> Option<String> {
    let mapped: String = password
        .chars()
        .filter_map(|c| {
            if NON_ASCII_SPACES.contains(&c) {
                Some(' ')
            } else if MAPPED_TO_NOTHING.contains(&c) {
                None
            } else {
                Some(c)
            }
        })
        .collect();

    let normalized: String = mapped.nfkc().collect();

    if normalized.chars().any(is_prohibited) {
        return None;
    }
    if !bidi_ok(&normalized) {
        return None;
    }

    Some(normalized)
}

/// Whether `c` falls in one of the tables RFC 4013 §2.3 prohibits.
///
/// The arms are in the RFC's own order so each can be read against its table.
fn is_prohibited(c: char) -> bool {
    // C.1.2 — non-ASCII space characters. Unreachable after step 1's mapping;
    // present because the profile lists it, and because a future caller that
    // skips the mapping must still be refused.
    if NON_ASCII_SPACES.contains(&c) {
        return true;
    }
    let code = u32::from(c);

    // C.2.1 — ASCII control characters.
    let c21 = matches!(code, 0x0000..=0x001F | 0x007F);
    // C.2.2 — non-ASCII control characters.
    let c22 = matches!(code,
        0x0080..=0x009F | 0x06DD | 0x070F | 0x180E | 0x200C | 0x200D
        | 0x2028 | 0x2029 | 0x2060..=0x2063 | 0x206A..=0x206F | 0xFEFF
        | 0xFFF9..=0xFFFC | 0x1D173..=0x1D17A);
    // C.3 — private use.
    let c3 = matches!(code, 0xE000..=0xF8FF | 0xF_0000..=0xF_FFFD | 0x0010_0000..=0x0010_FFFD);
    // C.4 — non-character code points: the `FDD0..FDEF` block plus the last
    // two code points of every plane.
    let c4 = matches!(code, 0xFDD0..=0xFDEF) || (code & 0xFFFE) == 0xFFFE;
    // C.5 — surrogate codes. Unreachable through `char`, which cannot hold
    // one; written out so the table is complete against the RFC.
    let c5 = matches!(code, 0xD800..=0xDFFF);
    // C.6 — inappropriate for plain text.
    let c6 = matches!(code, 0xFFF9..=0xFFFD);
    // C.7 — inappropriate for canonical representation (the ideographic
    // description characters).
    let c7 = matches!(code, 0x2FF0..=0x2FFB);
    // C.8 — change display properties, or are deprecated.
    let c8 = matches!(code, 0x0340 | 0x0341 | 0x200E | 0x200F | 0x202A..=0x202E | 0x206A..=0x206F);
    // C.9 — tagging characters.
    let c9 = matches!(code, 0xE0001 | 0xE0020..=0xE007F);

    c21 || c22 || c3 || c4 || c5 || c6 || c7 || c8 || c9
}

/// The RFC 3454 §6 bidirectional rule, as RFC 4013 §2.4 adopts it.
///
/// A string holding a `RandALCat` character may hold no `LCat` character, and must
/// begin and end with `RandALCat`. A string with no `RandALCat` character passes
/// unconditionally, whatever it holds.
fn bidi_ok(s: &str) -> bool {
    if !s.chars().any(is_rand_al_cat) {
        return true;
    }
    if s.chars().any(is_l_cat) {
        return false;
    }
    let mut chars = s.chars();
    let first = chars.next();
    let last = chars.next_back().or(first);
    first.is_some_and(is_rand_al_cat) && last.is_some_and(is_rand_al_cat)
}

/// RFC 3454 table D.1 — characters with a bidirectional property of "R" or
/// "AL".
fn is_rand_al_cat(c: char) -> bool {
    matches!(u32::from(c),
        0x05BE | 0x05C0 | 0x05C3 | 0x05D0..=0x05EA | 0x05F0..=0x05F4
        | 0x061B | 0x061F | 0x0621..=0x063A | 0x0640..=0x064A
        | 0x066D..=0x066F | 0x0671..=0x06D5 | 0x06DD | 0x06E5 | 0x06E6
        | 0x06FA..=0x06FE | 0x0700..=0x070D | 0x0710 | 0x0712..=0x072C
        | 0x0780..=0x07A5 | 0x07B1 | 0x200F | 0xFB1D | 0xFB1F..=0xFB28
        | 0xFB2A..=0xFB36 | 0xFB38..=0xFB3C | 0xFB3E | 0xFB40 | 0xFB41
        | 0xFB43 | 0xFB44 | 0xFB46..=0xFBB1 | 0xFBD3..=0xFD3D
        | 0xFD50..=0xFD8F | 0xFD92..=0xFDC7 | 0xFDF0..=0xFDFC
        | 0xFE70..=0xFE74 | 0xFE76..=0xFEFC)
}

/// RFC 3454 table D.2 — characters with a bidirectional property of "L".
///
/// The table is enormous (every Latin, Greek, Cyrillic, CJK … letter), so it
/// is expressed as its defining property rather than transcribed: a character
/// is `LCat` when Unicode gives it a strong left-to-right direction. The ranges
/// below are the ones a password plausibly reaches, and the fallback covers
/// the rest by exclusion — anything neither `RandALCat`, nor a digit, nor
/// punctuation, nor a mark, nor whitespace, and above the ASCII controls, is
/// treated as `LCat`.
///
/// The rule only ever *rejects* a candidate, and a rejected candidate is still
/// tried raw, so an over- or under-inclusive edge costs a candidate rather
/// than an unlock.
fn is_l_cat(c: char) -> bool {
    if is_rand_al_cat(c) {
        return false;
    }
    // The bidi-neutral classes a password can contain: ASCII digits and
    // punctuation, the space, combining marks, and the format characters
    // step 3 has already refused.
    if c.is_ascii_digit() || c.is_whitespace() || c.is_ascii_punctuation() {
        return false;
    }
    if u32::from(c) < 0x0041 {
        return false;
    }
    c.is_alphabetic() || c.is_numeric()
}

#[cfg(test)]
mod tests {
    use super::saslprep;

    // RFC 4013 §3's own examples, in the order the RFC lists them.
    #[test]
    fn the_rfc_4013_examples() {
        // #1 SOFT HYPHEN mapped to nothing.
        assert_eq!(saslprep("I\u{00AD}X").as_deref(), Some("IX"));
        // #2 no transformation.
        assert_eq!(saslprep("user").as_deref(), Some("user"));
        // #3 case preserved — SASLprep does not casefold.
        assert_eq!(saslprep("USER").as_deref(), Some("USER"));
        // #4 output is NFKC, so the feminine ordinal folds to "a".
        assert_eq!(saslprep("\u{00AA}").as_deref(), Some("a"));
        // #5 the Roman numeral nine folds to "IX".
        assert_eq!(saslprep("\u{2168}").as_deref(), Some("IX"));
        // #6 prohibited: a control character.
        assert_eq!(saslprep("\u{0007}"), None);
        // #7 bidirectional check fails: Arabic followed by an ASCII digit.
        assert_eq!(saslprep("\u{0627}\u{0031}"), None);
    }

    #[test]
    fn a_non_ascii_space_becomes_u0020() {
        assert_eq!(saslprep("a\u{00A0}b").as_deref(), Some("a b"));
        assert_eq!(saslprep("a\u{3000}b").as_deref(), Some("a b"));
        // U+200B is in both tables; the space mapping runs first.
        assert_eq!(saslprep("a\u{200B}b").as_deref(), Some("a b"));
    }

    #[test]
    fn nfkc_composes_a_decomposed_sequence() {
        // "cafe" + COMBINING ACUTE and "café" prepare to the same string,
        // which is the whole point of the normalisation step.
        assert_eq!(
            saslprep("cafe\u{0301}").as_deref(),
            saslprep("caf\u{00E9}").as_deref()
        );
        assert_eq!(saslprep("cafe\u{0301}").as_deref(), Some("caf\u{00E9}"));
    }

    // A bidi string that satisfies the rule is accepted: all-RandALCat, so it
    // begins and ends with one and holds no LCat.
    #[test]
    fn an_all_arabic_password_passes_the_bidi_rule() {
        assert_eq!(
            saslprep("\u{0627}\u{0628}").as_deref(),
            Some("\u{0627}\u{0628}")
        );
    }

    #[test]
    fn mixing_the_two_directions_fails() {
        // RandALCat plus LCat, in either order.
        assert_eq!(saslprep("\u{0627}a"), None);
        assert_eq!(saslprep("a\u{0627}"), None);
    }

    #[test]
    fn a_purely_latin_password_never_reaches_the_bidi_rule() {
        assert_eq!(saslprep("pa55w0rd!").as_deref(), Some("pa55w0rd!"));
    }

    #[test]
    fn the_empty_password_prepares_to_itself() {
        assert_eq!(saslprep("").as_deref(), Some(""));
    }

    // One member of each prohibited table, checked at the table rather than
    // through `saslprep` — three of them (C.1.2's spaces, and the B.1 members
    // that also sit in C.2.2) never reach step 3, because step 1 has already
    // mapped or deleted them.
    #[test]
    fn every_prohibited_table_names_a_member() {
        for (table, c) in [
            ("C.1.2", '\u{00A0}'),
            ("C.2.1", '\u{0007}'),
            ("C.2.2", '\u{0085}'),
            ("C.3", '\u{E000}'),
            ("C.4", '\u{FDD0}'),
            ("C.4 plane end", '\u{FFFE}'),
            ("C.5", '\u{D7FF}'),
            ("C.6", '\u{FFFD}'),
            ("C.7", '\u{2FF0}'),
            ("C.8", '\u{202A}'),
            ("C.9", '\u{E0020}'),
        ] {
            // U+D7FF stands in for C.5: `char` cannot hold a surrogate, so the
            // arm is unreachable and the nearest non-surrogate must *not* be
            // prohibited — the assertion is inverted for that one row.
            let expected = table != "C.5";
            assert_eq!(
                super::is_prohibited(c),
                expected,
                "{table} U+{:04X}",
                u32::from(c)
            );
        }
    }

    // The tables step 1 does not touch reject through `saslprep` itself.
    #[test]
    fn a_prohibited_character_makes_the_whole_password_unpreparable() {
        for c in ['\u{0007}', '\u{0085}', '\u{E000}', '\u{2FF0}', '\u{202A}'] {
            assert_eq!(
                saslprep(&format!("pass{c}word")),
                None,
                "{:04X}",
                u32::from(c)
            );
        }
    }
}
