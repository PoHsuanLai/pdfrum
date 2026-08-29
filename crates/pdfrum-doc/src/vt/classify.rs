//! The character classifier the line breaker consults.
//!
//! A 128-entry bit table for ASCII plus explicit range and set tests above
//! it. Everything here is a pure function of one code point; there is no
//! state and no locale.
//!
//! **One of these predicates carries a transcribed bug.** In
//! [`is_punctuation`]'s Latin-1 arm every term is an equality test except one,
//! which is written `<=` — so every code point from `0x80` to `0x94`
//! inclusive is punctuation. That changes where lines may break in Latin-1
//! text, it is visible in rendered output, and it is ported as written.

/// Bit flags in the ASCII table.
const LATIN: u8 = 0x01;
const OPEN_PUNCT: u8 = 0x04;
const PUNCT: u8 = 0x08;
const CONNECTIVE: u8 = 0x20;

/// The ASCII classification table, transcribed entry for entry.
///
/// Read it as data, not as a claim about ASCII: the first row assigns flags
/// to control characters, and the digits carry `0x02` rather than the Latin
/// bit — which no predicate here tests, since [`is_digit`] is a range check.
#[rustfmt::skip]
const ASCII: [u8; 128] = [
    0x00, 0x0C, 0x08, 0x0C, 0x08, 0x00, 0x20, 0x00, // 00-07
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // 08-0F
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // 10-17
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // 18-1F
    0x00, 0x08, 0x08, 0x00, 0x10, 0x00, 0x00, 0x28, // 20-27
    0x0C, 0x08, 0x00, 0x00, 0x28, 0x28, 0x28, 0x28, // 28-2F
    0x02, 0x02, 0x02, 0x02, 0x02, 0x02, 0x02, 0x02, // 30-37: digits
    0x02, 0x02, 0x08, 0x08, 0x00, 0x00, 0x00, 0x08, // 38-3F
    0x00, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, // 40-47
    0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, // 48-4F
    0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, // 50-57
    0x01, 0x01, 0x01, 0x0C, 0x00, 0x08, 0x00, 0x00, // 58-5F
    0x00, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, // 60-67
    0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, // 68-6F
    0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, // 70-77
    0x01, 0x01, 0x01, 0x0C, 0x00, 0x08, 0x00, 0x00, // 78-7F
];

/// Whether an ASCII code point carries a flag.
fn ascii_flag(word: u32, flag: u8) -> bool {
    usize::try_from(word)
        .ok()
        .and_then(|index| ASCII.get(index))
        .is_some_and(|bits| bits & flag != 0)
}

/// Whether a code point falls in any of the given inclusive ranges.
fn in_ranges(word: u32, ranges: &[(u32, u32)]) -> bool {
    ranges
        .iter()
        .any(|(low, high)| word >= *low && word <= *high)
}

/// Latin script, broadly: ASCII letters and digits plus the Latin supplements
/// and their full-width forms.
#[must_use]
pub fn is_latin(word: u32) -> bool {
    if word <= 0x7F {
        return ascii_flag(word, LATIN);
    }
    in_ranges(
        word,
        &[
            (0x00C0, 0x00FF),
            (0x0100, 0x024F),
            (0x1E00, 0x1EFF),
            (0x2C60, 0x2C7F),
            (0xA720, 0xA7FF),
            (0xFF21, 0xFF3A),
            (0xFF41, 0xFF5A),
        ],
    )
}

/// An ASCII digit.
#[must_use]
pub fn is_digit(word: u32) -> bool {
    (0x0030..=0x0039).contains(&word)
}

/// Chinese, Japanese or Korean script, including the ideographic iteration
/// marks and half-width katakana.
#[must_use]
pub fn is_cjk(word: u32) -> bool {
    if in_ranges(
        word,
        &[
            (0x1100, 0x11FF),
            (0x2E80, 0x2FFF),
            (0x3040, 0x9FBF),
            (0xAC00, 0xD7AF),
            (0xF900, 0xFAFF),
            (0xFE30, 0xFE4F),
            (0x20000, 0x2A6DF),
            (0x2F800, 0x2FA1F),
        ],
    ) {
        return true;
    }
    // Inside the CJK symbols block only a named handful count.
    if (0x3000..=0x303F).contains(&word) {
        return matches!(word, 0x3005 | 0x3006 | 0x3021..=0x3029 | 0x3031..=0x3035);
    }
    (0xFF66..=0xFF9D).contains(&word)
}

/// Punctuation, for line-breaking purposes.
///
/// The Latin-1 arm's `word <= 0x0094` term is transcribed as written: it
/// makes every code point from `0x80` through `0x94` punctuation, where every
/// neighbouring term is an equality test. Reproducing it is the point.
#[must_use]
pub fn is_punctuation(word: u32) -> bool {
    if word <= 0x7F {
        return ascii_flag(word, PUNCT);
    }
    if (0x0080..=0x00FF).contains(&word) {
        // The `<= 0x0094` term subsumes the four equality tests written
        // before it and sweeps in everything from 0x80 up. Transcribed.
        return word <= 0x0094 || matches!(word, 0x0096 | 0x00B4 | 0x00B8);
    }
    if (0x2000..=0x206F).contains(&word) {
        return matches!(
            word,
            0x2010..=0x2013 | 0x2018..=0x201F | 0x2032..=0x2037 | 0x203C..=0x203E | 0x2044
        );
    }
    if (0x3000..=0x303F).contains(&word) {
        return matches!(
            word,
            0x3001..=0x3003 | 0x3005 | 0x3009..=0x301B | 0x301D..=0x301F
        );
    }
    if (0xFE50..=0xFE6F).contains(&word) {
        return (0xFE50..=0xFE5E).contains(&word) || word == 0xFE63;
    }
    if (0xFF00..=0xFFEF).contains(&word) {
        return matches!(
            word,
            0xFF01 | 0xFF02 | 0xFF07..=0xFF09 | 0xFF0C | 0xFF0E | 0xFF0F
                | 0xFF1A | 0xFF1B | 0xFF1F | 0xFF3B | 0xFF3D | 0xFF40
                | 0xFF5B..=0xFF5D | 0xFF61..=0xFF65 | 0xFF9E | 0xFF9F
        );
    }
    false
}

/// A symbol that joins the characters on either side of it.
///
/// Only `0x06` carries the bit in the table, so this is effectively dead —
/// kept because the line-break rules consult it and removing the term would
/// silently change their shape.
#[must_use]
pub fn is_connective_symbol(word: u32) -> bool {
    word <= 0x7F && ascii_flag(word, CONNECTIVE)
}

/// An opening bracket or quotation mark, which binds to what follows.
#[must_use]
pub fn is_open_style_punctuation(word: u32) -> bool {
    if word <= 0x7F {
        return ascii_flag(word, OPEN_PUNCT);
    }
    matches!(
        word,
        0x300A
            | 0x300C
            | 0x300E
            | 0x3010
            | 0x3014
            | 0x3016
            | 0x3018
            | 0x301A
            | 0xFF08
            | 0xFF3B
            | 0xFF5B
            | 0xFF62
    )
}

/// A currency sign.
#[must_use]
pub fn is_currency_symbol(word: u32) -> bool {
    matches!(
        word,
        0x0024 | 0x0080 | 0x00A2..=0x00A5 | 0x20A0..=0x20CF | 0xFE69 | 0xFF04 | 0xFFE0 | 0xFFE1
            | 0xFFE5 | 0xFFE6
    )
}

/// A symbol that binds to the number after it — the currency signs plus the
/// numero sign.
#[must_use]
pub fn is_prefix_symbol(word: u32) -> bool {
    is_currency_symbol(word) || word == 0x2116
}

/// A space, ordinary or ideographic.
#[must_use]
pub fn is_space(word: u32) -> bool {
    word == 0x0020 || word == 0x3000
}

/// Whether a line may break between two adjacent characters.
///
/// The rungs are tried in order and the first that applies decides; the
/// ordering is what gives `$5` its unbreakable bond and lets CJK break
/// anywhere:
///
/// 1. Two letters or digits never break apart.
/// 2. Never break *before* a space or a punctuation mark.
/// 3. Never break either side of a connective symbol.
/// 4. Always break *after* a space or a punctuation mark.
/// 5. Never break after a prefix symbol — `$` binds to what follows.
/// 6. Always break before a prefix symbol or a CJK character.
/// 7. Always break after a CJK character.
#[must_use]
pub fn need_division(previous: u32, current: u32) -> bool {
    if (is_latin(previous) || is_digit(previous)) && (is_latin(current) || is_digit(current)) {
        return false;
    }
    if is_space(current) || is_punctuation(current) {
        return false;
    }
    if is_connective_symbol(previous) || is_connective_symbol(current) {
        return false;
    }
    if is_space(previous) || is_punctuation(previous) {
        return true;
    }
    if is_prefix_symbol(previous) {
        return false;
    }
    if is_prefix_symbol(current) || is_cjk(current) {
        return true;
    }
    is_cjk(previous)
}

#[cfg(test)]
mod tests {
    use super::{
        is_cjk, is_connective_symbol, is_currency_symbol, is_digit, is_latin,
        is_open_style_punctuation, is_prefix_symbol, is_punctuation, is_space, need_division,
    };

    #[test]
    fn every_latin_one_code_point_from_eighty_to_ninety_four_is_punctuation() {
        // The transcribed `<=` term, reproduced deliberately.
        for word in 0x80..=0x94_u32 {
            assert!(is_punctuation(word), "{word:#04X}");
        }
        // Just past it the equality tests take over, and they are sparse.
        assert!(!is_punctuation(0x95));
        assert!(is_punctuation(0x96));
        assert!(is_punctuation(0x00B4));
        assert!(!is_punctuation(0x00A1));
    }

    #[test]
    fn ascii_classification_matches_the_table() {
        assert!(is_latin(u32::from(b'A')));
        assert!(is_latin(u32::from(b'z')));
        // The digits carry their own bit, not the Latin one — which only
        // matters because the break rules test both.
        assert!(!is_latin(u32::from(b'7')));
        assert!(is_digit(u32::from(b'7')));
        assert!(!is_digit(u32::from(b'A')));
        assert!(is_punctuation(u32::from(b'.')));
        assert!(is_punctuation(u32::from(b',')));
        assert!(is_punctuation(u32::from(b'?')));
        assert!(is_open_style_punctuation(u32::from(b'(')));
        assert!(is_open_style_punctuation(u32::from(b'[')));
        assert!(is_open_style_punctuation(u32::from(b'{')));
        assert!(!is_open_style_punctuation(u32::from(b')')));
        assert!(is_space(0x20));
        assert!(is_space(0x3000));
    }

    #[test]
    fn the_connective_bit_covers_one_control_and_five_separators() {
        // Everything the table marks connective, and nothing else.
        let expected: &[u32] = &[
            0x06,
            u32::from(b'\''),
            u32::from(b','),
            u32::from(b'-'),
            u32::from(b'.'),
            u32::from(b'/'),
        ];
        for word in 0..=0x7F_u32 {
            assert_eq!(
                is_connective_symbol(word),
                expected.contains(&word),
                "{word:#04X}"
            );
        }
        // Nothing above ASCII qualifies at all.
        assert!(!is_connective_symbol(0x3000));
    }

    #[test]
    fn the_cjk_symbols_block_admits_only_a_named_handful() {
        assert!(is_cjk(0x3005));
        assert!(is_cjk(0x3021));
        assert!(is_cjk(0x3035));
        assert!(!is_cjk(0x3000));
        assert!(!is_cjk(0x3010));
        // The main ideographic ranges are wholesale.
        assert!(is_cjk(0x4E00));
        assert!(is_cjk(0xAC00));
        assert!(is_cjk(0x20000));
    }

    #[test]
    fn currency_and_prefix_symbols_differ_by_one_code_point() {
        assert!(is_currency_symbol(0x0024));
        assert!(is_currency_symbol(0x20AC));
        assert!(!is_currency_symbol(0x2116));
        assert!(is_prefix_symbol(0x2116));
        assert!(is_prefix_symbol(0x0024));
    }

    #[test]
    fn each_break_rung_fires_where_it_should() {
        // 1. Inside a word.
        assert!(!need_division(u32::from(b'a'), u32::from(b'b')));
        assert!(!need_division(u32::from(b'1'), u32::from(b'a')));
        // 2. Never before a space or punctuation.
        assert!(!need_division(u32::from(b'a'), u32::from(b' ')));
        assert!(!need_division(u32::from(b'a'), u32::from(b'.')));
        // 3. Never either side of a connective — and `.` is one, which is
        // why it never opens a break even though rung 4 would allow it.
        assert!(!need_division(0x06, 0x4E00));
        assert!(!need_division(0x4E00, 0x06));
        assert!(!need_division(u32::from(b'.'), u32::from(b'a')));
        // 4. Always after a space or a non-connective punctuation mark.
        assert!(need_division(u32::from(b' '), u32::from(b'a')));
        assert!(need_division(u32::from(b'?'), u32::from(b'a')));
        // 5. A currency sign binds to what follows.
        assert!(!need_division(0x0024, u32::from(b'5')));
        // 6/7. CJK breaks on either side.
        assert!(need_division(u32::from(b'a'), 0x4E00));
        assert!(need_division(0x4E00, u32::from(b'a')));
        assert!(need_division(0x4E00, 0x4E01));
    }
}
