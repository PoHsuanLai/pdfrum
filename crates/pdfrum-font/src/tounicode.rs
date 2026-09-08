//! `/ToUnicode` CMaps — the component text extraction's byte-exactness rests
//! on.
//!
//! A `/ToUnicode` stream is a CMap program whose `bfchar` and `bfrange`
//! sections map character codes to Unicode. Reproducing PDFium here means
//! reproducing its *rejection* rules and its *collision* policy, not just its
//! happy path: a block whose declared count disagrees with its contents is
//! discarded whole, a code above `0xFFFF` invalidates its entire block, and
//! where two entries collide the numerically smaller value wins in **both**
//! directions.

use pdfrum_cmap::{CharCode, CidSet, Words};
use pdfrum_common::{DiagKind, Diagnostics, Limits, Severity, hex_digit};
use smallvec::SmallVec;
use std::collections::{BTreeMap, HashMap};

/// A code above this invalidates the whole `bfchar`/`bfrange` block it
/// appears in — not just that entry (`kCidLimit`).
const CID_LIMIT: u32 = 0xffff;

/// The declared-entry cap. The specification says at most 100 per block;
/// PDFium deliberately tolerates far more because real files violate it, and
/// caps at a value chosen to keep a fuzzer from stalling
/// (`kOutOfSpecBFLimit`). Ported verbatim: it is a rejection files depend on.
const OUT_OF_SPEC_BF_LIMIT: i64 = 160_000;

/// The widest a committed `bfrange` can be, in codes.
///
/// Not a limit we impose: the high-code mask in [`handle_bfrange`] takes the
/// high code's low byte and the rest of the low code, which pins both ends of
/// a range inside one 256-code block. [`ToUnicode::window`] relies on it to
/// bound how far back a search must look.
const MAX_RUN_SPAN: u32 = 256;

/// A `/ToUnicode` map: character code → Unicode, and back.
///
/// The stored value is *either* a single UTF-16 code unit *or* a packed index
/// into the multi-character table (`index << 16 | 0xFFFF`). That packing has an
/// observable consequence PDFium never fixed and we reproduce: a code mapping
/// to exactly U+FFFF is indistinguishable from a multi-character entry and is
/// read back as one, usually yielding nothing at all.
#[derive(Debug, Clone, Default)]
pub struct ToUnicode {
    /// The one-code-at-a-time entries: charcode → packed value, ordered so
    /// iteration is deterministic.
    ///
    /// A contiguous `bfrange` does *not* land here; it is kept whole in
    /// [`runs`](Self::runs). Every read folds the two stores together under
    /// the same lowest-value-wins rule the C++ applies at insertion time.
    singles: BTreeMap<u32, u32>,
    /// packed value → charcode, for the [`singles`](Self::singles) only.
    /// Keyed on the *packed* value, so a multi-char entry's reverse key is its
    /// indicator rather than any real character.
    reverse_singles: BTreeMap<u32, u32>,
    /// The contiguous `bfrange` runs, kept as runs rather than expanded,
    /// **sorted by `low`** — [`seal`](Self::seal) puts them in that order once
    /// the program is fully parsed.
    ///
    /// An identity `/ToUnicode` is 256 of these covering 65,536 codes;
    /// expanding them cost 65,536 `BTreeMap` insertions in each direction per
    /// font load. Reordering them is safe because the collision rule is `min`,
    /// which is commutative and associative: folding the runs in any order
    /// yields exactly the value the C++'s sequential inserts leave behind.
    runs: Vec<Run>,
    /// The same runs sorted by `start`, the index the reverse direction
    /// searches. Also built by [`seal`](Self::seal).
    runs_by_start: Vec<Run>,
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

/// One committed contiguous `bfrange`: codes `low..=high` map to consecutive
/// values starting at `start`.
///
/// This is the `<lo> <hi> <start>` form, and the only one worth keeping whole:
/// the array and incrementing forms carry a destination *string* per code and
/// have nothing to compress. `high` is always within `low`'s 256-code block —
/// the high-code mask in [`handle_bfrange`] guarantees it — so `len` fits a
/// `u16` with room to spare and no arithmetic here can overflow.
#[derive(Debug, Clone, Copy)]
struct Run {
    low: u32,
    high: u32,
    start: u32,
}

impl Run {
    /// The value this run assigns to `code`, or `None` when `code` is outside
    /// it.
    ///
    /// `wrapping_add` mirrors the C++'s `value++` on a `uint32_t`. It cannot
    /// actually wrap: [`string_to_units`] caps a unit at `0xFFFF` and the mask
    /// caps the span at 256, so the largest value a run reaches is `0x100FE`.
    const fn value_at(self, code: u32) -> Option<u32> {
        if code < self.low || code > self.high {
            return None;
        }
        Some(self.start.wrapping_add(code - self.low))
    }

    /// The code this run assigns `value` to, or `None` when no code in it
    /// does. The inverse of [`value_at`](Self::value_at).
    const fn code_at(self, value: u32) -> Option<u32> {
        let offset = value.wrapping_sub(self.start);
        if offset > self.high - self.low {
            return None;
        }
        Some(self.low + offset)
    }

    /// Every `(code, value)` pair the run stands for, in ascending code order.
    fn pairs(self) -> impl Iterator<Item = (u32, u32)> {
        (self.low..=self.high).map(move |code| (code, self.start.wrapping_add(code - self.low)))
    }
}

impl ToUnicode {
    /// The stored value for `code`: the smallest any store offers, or `None`
    /// when nothing maps it.
    ///
    /// **The collision rule, once.** `InsertIntoMaps` keeps
    /// `min(existing, destcode)` for the forward direction
    /// (`cpdf_tounicodemap.cpp`, `map_.insert` then
    /// `it->second = std::min(it->second, destcode)`), so a code mapped twice
    /// reads back as the numerically smaller value regardless of which entry
    /// came first. Taking the minimum across the singles and every covering
    /// run reproduces that exactly, because `min` does not care about order.
    fn forward(&self, code: u32) -> Option<u32> {
        let from_runs =
            Self::window(&self.runs, |run| run.low, code).filter_map(|run| run.value_at(code));
        self.singles
            .get(&code)
            .copied()
            .into_iter()
            .chain(from_runs)
            .min()
    }

    /// The slice of a `key`-sorted run list that can possibly contain `target`.
    ///
    /// A run's span is at most [`MAX_RUN_SPAN`] codes wide — the high-code mask
    /// in [`handle_bfrange`] forces `low` and `high` into one 256-code block —
    /// so a run containing `target` must have `key(run)` in
    /// `target - 255 ..= target`. Binary-searching to the start of that window
    /// and walking it is what keeps a 256-run identity CMap at a couple of
    /// comparisons per lookup instead of 256.
    fn window<K: Fn(&Run) -> u32>(runs: &[Run], key: K, target: u32) -> impl Iterator<Item = &Run> {
        let first = target.saturating_sub(MAX_RUN_SPAN - 1);
        let start = runs.partition_point(|run| key(run) < first);
        runs.get(start..)
            .unwrap_or_default()
            .iter()
            .take_while(move |run| key(run) <= target)
    }

    /// The charcode `value` reverses to: the smallest any store offers, or
    /// `None`.
    ///
    /// The mirror rule, from the same function:
    /// `reverse_map_.insert({destcode, code})` then
    /// `reverse_it->second = std::min(reverse_it->second, code)` — the
    /// *smallest code* wins for one value, again order-independently.
    fn reverse_code(&self, value: u32) -> Option<u32> {
        let from_runs = Self::window(&self.runs_by_start, |run| run.start, value)
            .filter_map(|run| run.code_at(value));
        self.reverse_singles
            .get(&value)
            .copied()
            .into_iter()
            .chain(from_runs)
            .min()
    }

    /// Every `(value, charcode)` the reverse map holds, ascending by value and
    /// with the lowest-code rule already applied.
    fn reverse_entries(&self) -> impl Iterator<Item = (u32, u32)> + '_ {
        let mut merged: BTreeMap<u32, u32> = self.reverse_singles.clone();
        for run in &self.runs {
            for (code, value) in run.pairs() {
                merged
                    .entry(value)
                    .and_modify(|c| *c = (*c).min(code))
                    .or_insert(code);
            }
        }
        merged.into_iter()
    }

    /// The Unicode sequence a character code maps to, empty when unmapped.
    ///
    /// Two divergences from the C++ live here, both forced by `char` being a
    /// Unicode scalar where PDFium's `wchar_t` is not:
    ///
    /// - A **valid surrogate pair** is combined into one `char`.
    /// - An **unpaired surrogate** becomes U+FFFD.
    ///
    /// A stored value of `0x10000` or above is masked to its low 16 bits
    /// before the U+FFFF test, so a code can map to a single NUL — measured
    /// against the oracle, not inferred.
    #[must_use]
    pub fn lookup(&self, code: CharCode) -> SmallVec<[char; 2]> {
        let Some(value) = self.forward(code.0) else {
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
        CharCode(self.reverse_code(unicode as u32).unwrap_or(0))
    }

    /// Every `(unicode, charcode)` the reverse map holds, in ascending
    /// Unicode order.
    ///
    /// Multi-character destinations are skipped: their reverse key is the
    /// packed indicator `index << 16 | 0xFFFF`, not a character, so they are
    /// unreachable through [`reverse`](Self::reverse) as well. Values that
    /// are not Unicode scalars (unpaired surrogates, which the C++'s
    /// `wchar_t` map holds and Rust's `char` cannot) are skipped for the same
    /// reason a lookup would yield U+FFFD for them.
    pub fn reverse_pairs(&self) -> impl Iterator<Item = (char, u32)> + '_ {
        self.reverse_entries()
            .filter_map(|(unicode, code)| Some((char::from_u32(unicode)?, code)))
    }

    /// Whether the map holds nothing at all — neither entries nor a registry
    /// fallback.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.singles.is_empty() && self.runs.is_empty() && self.base_set == CidSet::Unknown
    }

    /// The number of committed code→Unicode entries.
    ///
    /// A run counts as the codes it covers, not as one entry: the tests state
    /// their expectations in the C++'s expanded terms and must keep reading
    /// the same numbers. Codes covered by both a run and a single are counted
    /// once.
    #[cfg(test)]
    #[must_use]
    pub fn len(&self) -> usize {
        let mut codes: std::collections::BTreeSet<u32> = self.singles.keys().copied().collect();
        for run in &self.runs {
            codes.extend(run.low..=run.high);
        }
        codes.len()
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
    /// collision tests of the former working note are stated in terms of it and no other function
    /// exposes the reverse map's multiplicity.
    #[cfg(test)]
    fn unicode_count(&self, charcode: u32) -> usize {
        self.reverse_entries()
            .filter(|&(_, c)| c == charcode)
            .count()
    }

    /// Insert one code with the **lowest-value-wins** collision policy, in
    /// both directions (`InsertIntoMaps`, the former working note).
    ///
    /// Only the *forward* half is recorded eagerly here; the cross-store
    /// minimum against the runs is taken on read, which is the same value
    /// because `min` is order-independent.
    fn insert(&mut self, code: u32, destcode: u32) {
        self.singles
            .entry(code)
            .and_modify(|v| *v = (*v).min(destcode))
            .or_insert(destcode);
        self.reverse_singles
            .entry(destcode)
            .and_modify(|c| *c = (*c).min(code))
            .or_insert(code);
    }

    /// Commit a contiguous `bfrange` whole, without expanding it.
    ///
    /// Equivalent to `insert(code, start + (code - low))` for every code in
    /// the run — which is what the C++ does — because both directions resolve
    /// by `min` on read.
    fn insert_run(&mut self, run: Run) {
        self.runs.push(run);
    }

    /// Put the runs into the two sorted orders the searches need. Called once,
    /// by [`parse`], when the program has been read to the end.
    fn seal(&mut self) {
        self.runs.sort_unstable_by_key(|run| run.low);
        self.runs_by_start.clone_from(&self.runs);
        self.runs_by_start.sort_unstable_by_key(|run| run.start);
    }

    /// The packed indicator the *next* multi-character entry would use.
    fn multi_char_indicator(&self) -> u32 {
        u32::try_from(self.multi_char.len())
            .ok()
            .and_then(|n| n.checked_mul(0x10000))
            .and_then(|n| n.checked_add(0xffff))
            .unwrap_or(0)
    }

    /// Commit one destination string for one code (`SetCode`, the former working note).
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
/// PDFium keeps unpaired surrogates as values, which `char` cannot hold, so
/// they become U+FFFD here. A *valid* pair combines normally,
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
        // low surrogate; anything else is a lone surrogate.
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
    map.seal();
    map
}

/// Scan a `<hex>` character code, **validating** as it goes.
///
/// Different from the embedded-CMap parser's code reader, which is
/// deliberately lax: this one rejects a non-hex byte and a `u32` overflow
/// outright, and those rejections invalidate whole blocks.
fn string_to_code(word: &[u8]) -> Option<u32> {
    if word.len() <= 2 || word.first() != Some(&b'<') || word.last() != Some(&b'>') {
        return None;
    }
    let mut code: u32 = 0;
    for &c in word.get(1..word.len() - 1)? {
        if is_pdf_whitespace(c) {
            continue;
        }
        let digit = hex_digit(c)?;
        code = code.checked_mul(16)?.checked_add(u32::from(digit))?;
    }
    Some(code)
}

/// Scan a `<hex>` destination into code units.
///
/// **Never fails**; it returns whatever complete groups of four hex digits it
/// read before running out or hitting a non-hex byte. A trailing partial group
/// is discarded, so every unit this produces is at most `0xFFFF` — a UTF-16
/// code unit, even though the storage is wider.
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
        let Some(digit) = hex_digit(c) else {
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
/// **exactly** — too few is as fatal as too many.
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
    /// `<lo> <hi> <start>` — consecutive values from a single unit. Carries
    /// the [`Run`] it commits as: the collected and the stored form are the
    /// same three numbers, so there is nothing to translate.
    Consecutive(Run),
    /// `<lo> <hi> <multi>` — the destination string incremented per code.
    Incremented { low: u32, dests: Vec<Vec<u32>> },
}

/// `beginbfrange` … `endbfrange`.
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
            ranges.push(Range::Consecutive(Run {
                low: lowcode,
                high: highcode,
                start: *single,
            }));
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
        Range::Consecutive(run) => {
            // Stored whole. Plain `u32` arithmetic with no clamping: a start
            // near 0xFFFF walks straight through the multi-character
            // indicator and out the far side, where `lookup`'s low-16-bit
            // mask takes over.
            map.insert_run(*run);
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
/// **This is a base-2³² increment.** PDFium's `wchar_t` is 32 bits on the
/// platform the oracle is built for, so `0xFFFF + 1` is `0x10000` — larger,
/// not wrapped — and the carry arm never fires. Since [`string_to_units`]
/// emits a fresh unit every four hex digits, no element starts above
/// `0xFFFF` either, which makes the
/// carry arm **unreachable from any input at all**. It is written out anyway
/// because the algorithm has one and a reader should be able to see why it
/// never runs. Measured against the oracle, not inferred.
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

/// Invert a `/ToUnicode` CMap: Unicode scalar → the character code that maps
/// to it, for every code the program reaches.
///
/// The whole map is built once, rather than a reverse lookup per character.
/// A writer that embeds a font with a *caller-supplied* `/ToUnicode` needs
/// exactly this to turn text into codes: the caller's CMap is the only
/// statement of what its codes mean, and the font program's own cmap is not
/// it.
///
/// Where several codes map to one Unicode value the **numerically smallest**
/// code wins, the same collision policy the forward direction uses.
/// Multi-character destinations are unreachable — the reverse map is keyed on
/// the packed stored value, and a multi-character entry's key is an indicator
/// rather than any real character.
#[must_use]
pub fn invert_to_unicode(
    bytes: &[u8],
    limits: &Limits,
    diags: &mut Diagnostics,
) -> HashMap<char, u32> {
    parse(bytes, limits, diags)
        .reverse_pairs()
        .filter(|(_, code)| *code != 0)
        .collect()
}
