//! `/ToUnicode` CMaps — the component text extraction's byte-exactness rests
//! on.
//!
//! A `/ToUnicode` stream is a CMap program whose `bfchar` and `bfrange`
//! sections map character codes to Unicode. Reproducing PDFium here means
//! reproducing its *rejection* rules and its *collision* policy, not just its
//! happy path: a block whose declared count disagrees with its contents is
//! discarded whole, a code above `0xFFFF` invalidates its entire block, and
//! where two entries collide the numerically smaller value wins in **both**
//! directions (`docs/design/pdfrum-font.md` §1.6).

use pdfrum_cmap::{CharCode, CidSet, Words};
use pdfrum_common::{DiagKind, Diagnostics, Limits, Severity};
use smallvec::SmallVec;
use std::collections::BTreeMap;

/// A code above this invalidates the whole `bfchar`/`bfrange` block it
/// appears in — not just that entry (`kCidLimit`).
const CID_LIMIT: u32 = 0xffff;

/// The declared-entry cap. The specification says at most 100 per block;
/// PDFium deliberately tolerates far more because real files violate it, and
/// caps at a value chosen to keep a fuzzer from stalling
/// (`kOutOfSpecBFLimit`). Ported verbatim: it is a rejection files depend on.
const OUT_OF_SPEC_BF_LIMIT: i64 = 160_000;

/// A `/ToUnicode` map: character code → Unicode, and back.
///
/// The stored value is *either* a single UTF-16 code unit *or* a packed index
/// into the multi-character table (`index << 16 | 0xFFFF`). That packing has an
/// observable consequence PDFium never fixed and we reproduce: a code mapping
/// to exactly U+FFFF is indistinguishable from a multi-character entry and is
/// read back as one, usually yielding nothing at all.
#[derive(Debug, Clone, Default)]
pub struct ToUnicode {
    /// charcode → packed value, ordered so iteration is deterministic.
    map: BTreeMap<u32, u32>,
    /// packed value → charcode. Keyed on the *packed* value, so a multi-char
    /// entry's reverse key is its indicator rather than any real character.
    reverse_map: BTreeMap<u32, u32>,
    /// The destination strings of multi-character entries.
    ///
    /// Stored as `u32`, not `u16`: the scanner emits UTF-16 code units, but
    /// the incrementing `bfrange` form then adds to them in the C++'s 32-bit
    /// `wchar_t`, so a unit really can exceed `0xFFFF` and reach text output
    /// as a value no UTF-16 unit could hold. Measured, not inferred.
    multi_char: Vec<Vec<u32>>,
    /// The registry whose CID→Unicode table answers a miss, set by a
    /// `/Adobe-*-UCS2` token in the program.
    base_set: CidSet,
}

impl ToUnicode {
    /// The Unicode sequence a character code maps to, empty when unmapped.
    ///
    /// Two divergences from the C++ live here, both forced by `char` being a
    /// Unicode scalar where PDFium's `wchar_t` is not:
    ///
    /// - A **valid surrogate pair** is combined into one `char`.
    /// - An **unpaired surrogate** becomes U+FFFD (divergence D3).
    ///
    /// A stored value of `0x10000` or above is masked to its low 16 bits
    /// before the U+FFFF test, so a code can map to a single NUL — measured
    /// against the oracle, not inferred (see `docs/status/pdfrum-font.md`).
    #[must_use]
    pub fn lookup(&self, code: CharCode) -> SmallVec<[char; 2]> {
        let Some(&value) = self.map.get(&code.0) else {
            // A miss consults the registry table, which yields a character
            // even for an unmapped CID — PDFium returns a one-element string
            // holding NUL in that case, and callers read non-empty as success.
            if self.base_set == CidSet::Unknown {
                return SmallVec::new();
            }
            let cid = pdfrum_cmap::Cid(u16::try_from(code.0 & 0xffff).unwrap_or(0));
            let ch = pdfrum_cmap::unicode_from_cid(self.base_set, cid).unwrap_or('\0');
            return SmallVec::from_slice(&[ch]);
        };

        let unit = value & 0xffff;
        if unit != 0xffff {
            return units_to_chars(&[unit]);
        }
        let index = (value >> 16) as usize;
        self.multi_char
            .get(index)
            .map_or_else(SmallVec::new, |units| units_to_chars(units))
    }

    /// The character code that maps to `unicode`, or `CharCode(0)` on a miss.
    ///
    /// Zero doubles as "not found", exactly as `ReverseLookup` leaves it. Note
    /// the map is keyed on the *stored* value, so a multi-character entry is
    /// unreachable through this function even when [`lookup`](Self::lookup)
    /// returns it correctly.
    #[must_use]
    pub fn reverse(&self, unicode: char) -> CharCode {
        CharCode(
            self.reverse_map
                .get(&(unicode as u32))
                .copied()
                .unwrap_or(0),
        )
    }

    /// Every `(unicode, charcode)` the reverse map holds, in ascending
    /// Unicode order.
    ///
    /// Multi-character destinations are skipped: their reverse key is the
    /// packed indicator `index << 16 | 0xFFFF`, not a character, so they are
    /// unreachable through [`reverse`](Self::reverse) as well. Values that
    /// are not Unicode scalars (unpaired surrogates, which the C++'s
    /// `wchar_t` map holds and Rust's `char` cannot) are skipped for the same
    /// reason a lookup would yield U+FFFD for them (divergence D3).
    pub fn reverse_pairs(&self) -> impl Iterator<Item = (char, u32)> + '_ {
        self.reverse_map
            .iter()
            .filter_map(|(&unicode, &code)| Some((char::from_u32(unicode)?, code)))
    }

    /// Whether the map holds nothing at all — neither entries nor a registry
    /// fallback.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.map.is_empty() && self.base_set == CidSet::Unknown
    }

    /// The number of committed code→Unicode entries.
    #[cfg(test)]
    #[must_use]
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// The registry whose CID→Unicode table answers a lookup miss.
    #[cfg(test)]
    #[must_use]
    pub fn base_set(&self) -> CidSet {
        self.base_set
    }

    /// How many reverse-map entries point at `charcode`.
    ///
    /// The oracle's `GetUnicodeCountByCharcodeForTesting`, kept because the
    /// collision tests of §1.6 are stated in terms of it and no other function
    /// exposes the reverse map's multiplicity.
    #[cfg(test)]
    fn unicode_count(&self, charcode: u32) -> usize {
        self.reverse_map
            .values()
            .filter(|&&c| c == charcode)
            .count()
    }

    /// Insert with the **lowest-value-wins** collision policy, in both
    /// directions (`InsertIntoMaps`, §1.6.1).
    fn insert(&mut self, code: u32, destcode: u32) {
        self.map
            .entry(code)
            .and_modify(|v| *v = (*v).min(destcode))
            .or_insert(destcode);
        self.reverse_map
            .entry(destcode)
            .and_modify(|c| *c = (*c).min(code))
            .or_insert(code);
    }

    /// The packed indicator the *next* multi-character entry would use.
    fn multi_char_indicator(&self) -> u32 {
        u32::try_from(self.multi_char.len())
            .ok()
            .and_then(|n| n.checked_mul(0x10000))
            .and_then(|n| n.checked_add(0xffff))
            .unwrap_or(0)
    }

    /// Commit one destination string for one code (`SetCode`, §1.6.1).
    fn set_code(&mut self, srccode: u32, dest: &[u32]) {
        match dest {
            [] => {}
            [single] => self.insert(srccode, *single),
            multi => {
                self.insert(srccode, self.multi_char_indicator());
                // The push happens unconditionally, *after* the insert. When
                // the insert lost the `min` race the string is unreachable —
                // but removing it would shift every later indicator index, so
                // the waste is load-bearing.
                self.multi_char.push(multi.to_vec());
            }
        }
    }
}

/// Combine stored code units into characters.
///
/// Units at or below `0xFFFF` are UTF-16 and are combined as such; a unit
/// above it is already a scalar value (only the incrementing `bfrange` form
/// produces one) and is taken directly.
///
/// Divergence D3: PDFium keeps unpaired surrogates as values, which `char`
/// cannot hold, so they become U+FFFD here. A *valid* pair combines normally,
/// which is what the `NonBmpUnicodeLookup` assertion pins.
fn units_to_chars(units: &[u32]) -> SmallVec<[char; 2]> {
    let mut out = SmallVec::new();
    let mut i = 0;
    while let Some(&unit) = units.get(i) {
        i += 1;
        if unit > 0xffff {
            out.push(char::from_u32(unit).unwrap_or(char::REPLACEMENT_CHARACTER));
            continue;
        }
        // A high surrogate takes the next unit with it, when that unit is a
        // low surrogate; anything else is a lone surrogate (D3).
        if (0xd800..0xdc00).contains(&unit)
            && let Some(&low @ 0xdc00..=0xdfff) = units.get(i)
        {
            i += 1;
            let scalar = 0x1_0000 + ((unit - 0xd800) << 10) + (low - 0xdc00);
            out.push(char::from_u32(scalar).unwrap_or(char::REPLACEMENT_CHARACTER));
            continue;
        }
        out.push(char::from_u32(unit).unwrap_or(char::REPLACEMENT_CHARACTER));
    }
    out
}

/// Parse a decoded `/ToUnicode` stream.
///
/// Never fails: an unreadable program yields an empty map. The `limits` and
/// `diags` arguments exist for the damage channel — a block rejected by the
/// count check is recorded, because silently losing every character mapping in
/// a file is exactly the kind of loss the diagnostics channel is for.
#[must_use]
pub fn parse(bytes: &[u8], limits: &Limits, diags: &mut Diagnostics) -> ToUnicode {
    let mut map = ToUnicode::default();
    let mut words = Words::new(bytes);
    let mut previous: Vec<u8> = Vec::new();

    // The loop reads a word, and a handler may consume many more and hand back
    // the token it stopped on — which becomes the next `previous`, so a count
    // token is always the word immediately before `beginbfchar`/`beginbfrange`.
    let mut pending = words.next().map(<[u8]>::to_vec);
    while let Some(word) = pending.take() {
        if word.is_empty() {
            break;
        }
        let next = match word.as_slice() {
            b"beginbfchar" => Some(handle_bfchar(
                &mut words, &previous, limits, &mut map, diags,
            )),
            b"beginbfrange" => Some(handle_bfrange(
                &mut words, &previous, limits, &mut map, diags,
            )),
            b"/Adobe-Korea1-UCS2" => {
                map.base_set = CidSet::Korea1;
                None
            }
            b"/Adobe-Japan1-UCS2" => {
                map.base_set = CidSet::Japan1;
                None
            }
            b"/Adobe-CNS1-UCS2" => {
                map.base_set = CidSet::Cns1;
                None
            }
            b"/Adobe-GB1-UCS2" => {
                map.base_set = CidSet::Gb1;
                None
            }
            _ => None,
        };
        previous = next.unwrap_or(word);
        pending = words.next().map(<[u8]>::to_vec);
    }
    map
}

/// Scan a `<hex>` character code, **validating** as it goes.
///
/// Different from the embedded-CMap parser's code reader, which is
/// deliberately lax: this one rejects a non-hex byte and a `u32` overflow
/// outright, and those rejections invalidate whole blocks (§1.6.3).
fn string_to_code(word: &[u8]) -> Option<u32> {
    if word.len() <= 2 || word.first() != Some(&b'<') || word.last() != Some(&b'>') {
        return None;
    }
    let mut code: u32 = 0;
    for &c in word.get(1..word.len() - 1)? {
        if is_pdf_whitespace(c) {
            continue;
        }
        let digit = hex_value(c)?;
        code = code.checked_mul(16)?.checked_add(u32::from(digit))?;
    }
    Some(code)
}

/// Scan a `<hex>` destination into code units.
///
/// **Never fails**; it returns whatever complete groups of four hex digits it
/// read before running out or hitting a non-hex byte. A trailing partial group
/// is discarded, so every unit this produces is at most `0xFFFF` — a UTF-16
/// code unit, even though the storage is wider (§1.6.4).
fn string_to_units(word: &[u8]) -> Vec<u32> {
    if word.len() <= 2 || word.first() != Some(&b'<') || word.last() != Some(&b'>') {
        return Vec::new();
    }
    let Some(body) = word.get(1..word.len() - 1) else {
        return Vec::new();
    };
    let mut result = Vec::new();
    let mut byte_pos = 0u8;
    let mut ch: u32 = 0;
    for &c in body {
        if is_pdf_whitespace(c) {
            continue;
        }
        let Some(digit) = hex_value(c) else {
            break;
        };
        ch = ch * 16 + u32::from(digit);
        byte_pos += 1;
        if byte_pos == 4 {
            result.push(ch);
            byte_pos = 0;
            ch = 0;
        }
    }
    result
}

/// PDF whitespace as the ToUnicode scanners define it — note this set is
/// *not* the CMap lexer's, which additionally treats `0x80` and `0xFF` as
/// separators.
fn is_pdf_whitespace(c: u8) -> bool {
    matches!(c, 0x00 | 0x09 | 0x0A | 0x0C | 0x0D | 0x20)
}

fn hex_value(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// The declared count, and whether it is usable at all.
///
/// A count token that is not a number parses as 0, which is a *valid* count of
/// zero — so any entry at all then makes the block's collected length exceed
/// it and the block is discarded.
fn declared_count(previous: &[u8]) -> (bool, usize) {
    let raw = parse_int(previous);
    let valid = (0..=OUT_OF_SPEC_BF_LIMIT).contains(&raw);
    (valid, if valid { raw as usize } else { 0 })
}

/// PDFium's `StringToInt`: leading sign, then digits, stopping at the first
/// non-digit; anything unparsable is 0.
fn parse_int(word: &[u8]) -> i64 {
    let (negative, digits) = match word.first() {
        Some(b'-') => (true, word.get(1..).unwrap_or_default()),
        Some(b'+') => (false, word.get(1..).unwrap_or_default()),
        _ => (false, word),
    };
    let mut value: i64 = 0;
    for &c in digits {
        let Some(d) = c.checked_sub(b'0').filter(|d| *d <= 9) else {
            break;
        };
        let Some(next) = value
            .checked_mul(10)
            .and_then(|v| v.checked_add(i64::from(d)))
        else {
            return 0;
        };
        value = next;
    }
    if negative { -value } else { value }
}

/// `beginbfchar` … `endbfchar`, with the two-phase count check.
///
/// Nothing is committed unless the collected count equals the declared one
/// **exactly** — too few is as fatal as too many (§1.6.5).
fn handle_bfchar(
    words: &mut Words<'_>,
    previous: &[u8],
    limits: &Limits,
    map: &mut ToUnicode,
    diags: &mut Diagnostics,
) -> Vec<u8> {
    let (mut is_valid, expected) = declared_count(previous);
    let mut collected: Vec<(u32, Vec<u32>)> = Vec::new();
    let mut last = Vec::new();

    while let Some(word) = words.next() {
        if word.is_empty() || word == b"endbfchar" {
            last = word.to_vec();
            break;
        }
        if !is_valid {
            // Keep draining so the outer loop resumes after `endbfchar`.
            continue;
        }
        match string_to_code(word) {
            // A code above the CID limit invalidates the *whole block*, not
            // just this entry.
            Some(code) if code <= CID_LIMIT => {
                let Some(dest) = words.next() else { break };
                collected.push((code, string_to_units(dest)));
                if collected.len() > expected || collected.len() > limits.max_array_len {
                    is_valid = false;
                }
            }
            _ => is_valid = false,
        }
    }

    if is_valid && collected.len() == expected {
        for (code, dest) in &collected {
            map.set_code(*code, dest);
        }
    } else if !collected.is_empty() || expected != 0 {
        diags.record(Severity::Suspicious, DiagKind::ToUnicodeBlockRejected, None);
    }
    last
}

/// One collected `bfrange`, before the count check decides whether to commit.
enum Range {
    /// `<lo> <hi> [<a> <b> …]` — one destination string per code.
    Array { low: u32, dests: Vec<Vec<u32>> },
    /// `<lo> <hi> <start>` — consecutive values from a single unit.
    Consecutive { low: u32, high: u32, start: u32 },
    /// `<lo> <hi> <multi>` — the destination string incremented per code.
    Incremented { low: u32, dests: Vec<Vec<u32>> },
}

/// `beginbfrange` … `endbfrange` (§1.6.6).
fn handle_bfrange(
    words: &mut Words<'_>,
    previous: &[u8],
    limits: &Limits,
    map: &mut ToUnicode,
    diags: &mut Diagnostics,
) -> Vec<u8> {
    let (mut is_valid, expected) = declared_count(previous);
    let mut ranges: Vec<Range> = Vec::new();
    let mut last = Vec::new();

    while let Some(word) = words.next() {
        if word.is_empty() || word == b"endbfrange" {
            last = word.to_vec();
            break;
        }
        if !is_valid {
            continue;
        }

        let Some(lowcode) = string_to_code(word) else {
            is_valid = false;
            continue;
        };
        let Some(high_word) = words.next() else { break };
        let Some(highraw) = string_to_code(high_word) else {
            is_valid = false;
            continue;
        };

        // *The high-code mask.* The declared high code contributes only its
        // low byte; the rest comes from `lowcode`, forcing the range into one
        // 256-code block. A range crossing a block boundary is silently
        // truncated, and `<0001> <10000>` inverts into an invalid range that
        // discards the entire section.
        let highcode = (lowcode & 0xffff_ff00) | (highraw & 0xff);
        if lowcode > CID_LIMIT || highcode > CID_LIMIT || lowcode > highcode {
            is_valid = false;
            continue;
        }
        let span = (highcode - lowcode) as usize + 1;

        let Some(third) = words.next() else { break };
        if third == b"[" {
            // The array's words are consumed *unconditionally*, before the
            // count check, so a runaway range in a malformed file eats up to
            // 256 tokens — bounded by the mask above, which is why the mask
            // doubles as a safety property.
            let mut dests = Vec::with_capacity(span.min(256));
            for _ in 0..span {
                let Some(w) = words.next() else { break };
                dests.push(string_to_units(w));
            }
            ranges.push(Range::Array {
                low: lowcode,
                dests,
            });
            if ranges.len() > expected {
                is_valid = false;
                continue;
            }
            // A closing bracket is required *after* the words, and anything
            // else — including `}` — discards the block.
            match words.next() {
                Some(b"]") => {}
                _ => is_valid = false,
            }
            continue;
        }

        let dest = string_to_units(third);
        if let [single] = dest.as_slice() {
            ranges.push(Range::Consecutive {
                low: lowcode,
                high: highcode,
                start: *single,
            });
        } else {
            let mut dests = Vec::with_capacity(span.min(256));
            dests.push(dest);
            for _ in lowcode + 1..=highcode {
                let next = dests.last().map_or_else(Vec::new, |d| string_data_add(d));
                dests.push(next);
            }
            ranges.push(Range::Incremented {
                low: lowcode,
                dests,
            });
        }
        if ranges.len() > expected || ranges.len() > limits.max_array_len {
            is_valid = false;
        }
    }

    if is_valid && ranges.len() == expected {
        for range in &ranges {
            commit_range(range, map);
        }
    } else if !ranges.is_empty() || expected != 0 {
        diags.record(Severity::Suspicious, DiagKind::ToUnicodeBlockRejected, None);
    }
    last
}

fn commit_range(range: &Range, map: &mut ToUnicode) {
    match range {
        Range::Array { low, dests } => {
            for (i, dest) in dests.iter().enumerate() {
                let Some(code) = u32::try_from(i).ok().and_then(|i| low.checked_add(i)) else {
                    break;
                };
                map.set_code(code, dest);
            }
        }
        Range::Consecutive { low, high, start } => {
            // Plain `u32` arithmetic with no clamping: a start near 0xFFFF
            // walks straight through the multi-character indicator and out the
            // far side, where `lookup`'s low-16-bit mask takes over.
            let mut value = *start;
            for code in *low..=*high {
                map.insert(code, value);
                value = value.wrapping_add(1);
            }
        }
        Range::Incremented { low, dests } => {
            for (i, dest) in dests.iter().enumerate() {
                let Some(code) = u32::try_from(i).ok().and_then(|i| low.checked_add(i)) else {
                    break;
                };
                map.set_code(code, dest);
            }
        }
    }
}

/// Increment a destination string by one, as the multi-destination `bfrange`
/// form does per code.
///
/// **This is a base-2³² increment, not the base-65536 one the design brief
/// describes.** PDFium's `wchar_t` is 32 bits on the platform the oracle is
/// built for, so `0xFFFF + 1` is `0x10000` — larger, not wrapped — and the
/// carry arm never fires. Since [`string_to_units`] emits a fresh unit every
/// four hex digits, no element starts above `0xFFFF` either, which makes the
/// carry arm **unreachable from any input at all**. It is written out anyway
/// because the algorithm has one and a reader should be able to see why it
/// never runs. Measured against the oracle, not inferred; the probe and its
/// output are recorded in `docs/status/pdfrum-font.md`.
fn string_data_add(units: &[u32]) -> Vec<u32> {
    let mut out: Vec<u32> = Vec::with_capacity(units.len() + 1);
    let mut value: u32 = 1;
    for &unit in units.iter().rev() {
        let ch = unit.wrapping_add(value);
        if ch < unit {
            // Unreachable: `unit <= u32::MAX - 1` for every value the scanner
            // can produce, and `value` is 0 or 1.
            out.push(0);
        } else {
            out.push(ch);
            value = 0;
        }
    }
    if value != 0 {
        out.push(value);
    }
    out.reverse();
    out
}

#[cfg(test)]
#[path = "tounicode_tests.rs"]
mod tests;
