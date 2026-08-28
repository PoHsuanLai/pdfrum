//! The embedded-CMap program reader: a word-at-a-time state machine that
//! turns a CMap stream into a decoder and a charcode→CID table
//! (ISO 32000-1 §9.7.5.3).
//!
//! Three things about it are worth knowing before reading the code, because
//! they are what makes a real file's CMap behave the way it does:
//!
//! - **Operators are recognised before operands.** The operator tests run
//!   first and unconditionally, so a literal `begincidrange` appearing where a
//!   CID was expected resets the machine instead of being read as a number.
//!   A truncated `begincidchar` block therefore cannot corrupt the block after
//!   it.
//! - **`usecmap` does nothing.** The operator is recognised and discarded: a
//!   CMap that inherits a predefined base and overrides a handful of codes
//!   gets *none* of the base, only its own overrides. This matches the oracle
//!   and is what text extraction is measured against (SPEC.md §6); the
//!   inheritance that *is* implemented is the predefined tables' own static
//!   chain. A diagnostic records each ignored operator so the loss is visible.
//! - **Nothing here fails.** Every malformed construct is skipped, clamped or
//!   ignored, and the CMap that comes out is whatever the program managed to
//!   say.

use pdfrum_common::{DiagKind, Diagnostics, Limits, Severity};

use crate::decode::{CodeRange, Decoder};
use crate::ids::{CidSet, CodingScheme};

/// A charcode range whose codes do not fit the dense table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CidRange {
    pub(crate) start_code: u32,
    pub(crate) end_code: u32,
    pub(crate) start_cid: u16,
}

/// Codes below this are held in a dense table; the rest become [`CidRange`]s.
pub(crate) const DIRECT_TABLE_SIZE: usize = 0x1_0000;

/// Which operand the machine is currently collecting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    Start,
    CidChar,
    CidRange,
    Registry,
    Ordering,
    Supplement,
    WMode,
    CodeSpaceRange,
}

/// The result of reading a CMap program.
pub(crate) struct Parsed {
    pub(crate) decoder: Decoder,
    pub(crate) direct: DirectTable,
    pub(crate) additional: Vec<CidRange>,
    pub(crate) charset: CidSet,
    pub(crate) vertical: bool,
}

/// The machine's mutable state. Kept separate from [`Parsed`] so the CMap is
/// built once, at the end, from settled values.
struct Builder<'a> {
    status: Status,
    /// How many operands of the current construct have been seen.
    code_seq: u32,
    code_points: [u32; 4],
    /// Codespace ranges committed by a block that declared more than one.
    ranges: Vec<CodeRange>,
    /// Ranges read but not yet committed; see [`Builder::end_codespace`].
    pending_ranges: Vec<CodeRange>,
    additional: Vec<CidRange>,
    direct: DirectTable,
    last_word: &'a [u8],
    scheme: CodingScheme,
    charset: CidSet,
    vertical: bool,
    /// Set once `max_cmap_ranges` is hit, so the diagnostic is recorded once.
    ranges_capped: bool,
}

/// A zeroed dense table, allocated on the heap.
///
/// The table is 128 KiB and every embedded CMap gets one, even a program that
/// maps nothing — matching the original, whose "was a table ever allocated"
/// predicate distinguishes embedded CMaps from predefined ones and is read by
/// the CID font's glyph lookup. Allocating lazily would change that predicate,
/// so the allocation is unconditional.
fn zeroed_table() -> DirectTable {
    DirectTable(vec![0u16; DIRECT_TABLE_SIZE])
}

/// The dense charcode→CID table of an embedded CMap: exactly
/// [`DIRECT_TABLE_SIZE`] entries, heap-allocated, never re-sized.
///
/// A newtype rather than a `Box<[u16; N]>` because 128 KiB is far too large to
/// pass through the stack even momentarily, which building the array type
/// would require.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DirectTable(Vec<u16>);

impl DirectTable {
    /// The CID stored for `code`, or `None` when the code is past the table.
    pub(crate) fn get(&self, code: u32) -> Option<u16> {
        usize::try_from(code)
            .ok()
            .and_then(|i| self.0.get(i))
            .copied()
    }

    fn set(&mut self, code: u32, cid: u16) -> bool {
        let Ok(i) = usize::try_from(code) else {
            return false;
        };
        match self.0.get_mut(i) {
            Some(slot) => {
                *slot = cid;
                true
            }
            None => false,
        }
    }
}

/// Read a CMap program.
pub(crate) fn parse(bytes: &[u8], limits: &Limits, diags: &mut Diagnostics) -> Parsed {
    let mut b = Builder {
        status: Status::Start,
        code_seq: 0,
        code_points: [0; 4],
        ranges: Vec::new(),
        pending_ranges: Vec::new(),
        additional: Vec::new(),
        direct: zeroed_table(),
        last_word: &[],
        scheme: CodingScheme::default(),
        charset: CidSet::Unknown,
        vertical: false,
        ranges_capped: false,
    };
    for word in crate::lexer::Words::new(bytes) {
        b.feed(word, limits, diags);
    }
    b.finish(diags)
}

impl<'a> Builder<'a> {
    fn feed(&mut self, word: &'a [u8], limits: &Limits, diags: &mut Diagnostics) {
        match word {
            b"begincidchar" => {
                self.status = Status::CidChar;
                self.code_seq = 0;
            }
            b"begincidrange" => {
                self.status = Status::CidRange;
                self.code_seq = 0;
            }
            b"endcidrange" | b"endcidchar" => self.status = Status::Start,
            b"/WMode" => self.status = Status::WMode,
            b"/Registry" => self.status = Status::Registry,
            b"/Ordering" => self.status = Status::Ordering,
            b"/Supplement" => self.status = Status::Supplement,
            b"begincodespacerange" => {
                self.status = Status::CodeSpaceRange;
                self.code_seq = 0;
            }
            b"usecmap" => diags.record(Severity::Suspicious, DiagKind::CMapUsecmapIgnored, None),
            _ => match self.status {
                Status::CidChar | Status::CidRange => self.handle_cid(word, limits, diags),
                Status::Registry | Status::Supplement => self.status = Status::Start,
                Status::Ordering => {
                    self.charset = crate::charset_from_ordering(operand_value(word));
                    self.status = Status::Start;
                }
                Status::WMode => {
                    self.vertical = get_code(word) != 0;
                    self.status = Status::Start;
                }
                Status::CodeSpaceRange => self.handle_codespace(word, limits, diags),
                Status::Start => {}
            },
        }
        // Unconditional, including for words that were skipped: the codespace
        // pairing reads `last_word` and must see the real previous token.
        self.last_word = word;
    }

    /// Collect the two operands of a `cidchar` or the three of a `cidrange`,
    /// then write the mapping.
    fn handle_cid(&mut self, word: &'a [u8], limits: &Limits, diags: &mut Diagnostics) {
        let is_char = self.status == Status::CidChar;
        let seq = self.code_seq as usize;
        // The operand count is bounded by the reset below; the guard exists so
        // no input can reach past the buffer regardless.
        let Some(slot) = self.code_points.get_mut(seq) else {
            self.code_seq = 0;
            diags.record(Severity::Suspicious, DiagKind::CMapOperandOverflow, None);
            return;
        };
        *slot = get_code(word);
        self.code_seq += 1;
        let required = if is_char { 2 } else { 3 };
        if self.code_seq < required {
            return;
        }
        let cp = self.code_points;
        let (start, end, cid) = if is_char {
            (cp[0], cp[0], cp[1] as u16)
        } else {
            (cp[0], cp[1], cp[2] as u16)
        };
        self.code_seq = 0;

        if usize::try_from(end).is_ok_and(|e| e < DIRECT_TABLE_SIZE) {
            // A reversed range writes nothing at all, which is also what the
            // C++'s `for (code = start; code <= end; ++code)` does.
            if start > end {
                diags.record(Severity::Suspicious, DiagKind::CMapReversedRange, None);
                return;
            }
            for code in start..=end {
                // Last write wins: a later range overrides an earlier one.
                let mapped = (u32::from(cid).wrapping_add(code).wrapping_sub(start)) as u16;
                if !self.direct.set(code, mapped) {
                    break;
                }
            }
        } else if self.additional.len() >= limits.max_cmap_ranges {
            if !self.ranges_capped {
                self.ranges_capped = true;
                diags.record(Severity::Suspicious, DiagKind::CMapRangeLimit, None);
            }
        } else {
            self.additional.push(CidRange {
                start_code: start,
                end_code: end,
                start_cid: cid,
            });
        }
    }

    /// Collect codespace-range operands in pairs, and decide the coding scheme
    /// when the block ends.
    fn handle_codespace(&mut self, word: &'a [u8], limits: &Limits, diags: &mut Diagnostics) {
        if word != b"endcodespacerange" {
            // A token that is not a hex string is skipped *without* advancing
            // the pair counter, so one piece of garbage does not shift every
            // range after it by one.
            if word.first() != Some(&b'<') {
                return;
            }
            if self.code_seq % 2 == 1
                && let Some(range) = get_code_range(self.last_word, word, diags)
            {
                if self.pending_ranges.len() >= limits.max_cmap_ranges {
                    if !self.ranges_capped {
                        self.ranges_capped = true;
                        diags.record(Severity::Suspicious, DiagKind::CMapRangeLimit, None);
                    }
                } else {
                    self.pending_ranges.push(range);
                }
            }
            self.code_seq += 1;
            return;
        }
        self.end_codespace(diags);
    }

    /// `endcodespacerange`: pick the coding scheme from how many ranges the
    /// CMap has declared *in total*, across every block so far.
    ///
    /// The one-range case is the odd one and it is deliberate: the range's
    /// bounds are thrown away and only its width survives, as a one- or
    /// two-byte scheme. A CMap declaring the single range `<00>`–`<7F>`
    /// therefore decodes byte `0xFF` just as happily as byte `0x00` — the
    /// bounds never reach the decoder. And a lone *three*- or four-byte range
    /// yields a **one-byte** scheme, because only a width of exactly 2 maps to
    /// the two-byte scheme.
    fn end_codespace(&mut self, diags: &mut Diagnostics) {
        let segs = self.ranges.len() + self.pending_ranges.len();
        if segs == 1 {
            let width = self
                .ranges
                .first()
                .or(self.pending_ranges.first())
                .map_or(0, |r| r.char_size);
            self.scheme = if width == 2 {
                CodingScheme::TwoBytes
            } else {
                CodingScheme::OneByte
            };
            // `pending_ranges` is deliberately not drained here.
            diags.record(Severity::Recovered, DiagKind::CMapCodespaceDropped, None);
        } else if segs > 1 {
            self.scheme = CodingScheme::MixedFourBytes;
            self.ranges.append(&mut self.pending_ranges);
        }
        self.status = Status::Start;
    }

    fn finish(mut self, diags: &mut Diagnostics) -> Parsed {
        // The four-byte ranges only mean anything to a four-byte decoder, and
        // codes at or above the dense table's end are only reachable through
        // one, so both are discarded for any other scheme.
        let additional = if self.scheme == CodingScheme::MixedFourBytes {
            self.additional.sort_by_key(|r| r.end_code);
            self.additional
        } else {
            if !self.additional.is_empty() {
                diags.record(
                    Severity::Suspicious,
                    DiagKind::CMapWideMappingsDropped,
                    None,
                );
            }
            Vec::new()
        };
        let decoder = match self.scheme {
            CodingScheme::OneByte => Decoder::OneByte,
            CodingScheme::MixedFourBytes => Decoder::MixedFourBytes {
                ranges: self.ranges,
            },
            // A CMap program never produces a mixed-two-byte scheme — only
            // the predefined name table does — so that arm shares the
            // two-byte default rather than inventing an empty leading-byte
            // set, which would be an unrepresentable decoder.
            CodingScheme::TwoBytes | CodingScheme::MixedTwoBytes => Decoder::TwoBytes,
        };
        Parsed {
            decoder,
            direct: self.direct,
            additional,
            charset: self.charset,
            vertical: self.vertical,
        }
    }
}

/// The operand value of a `/Registry`-style word: everything after its first
/// two bytes.
///
/// This is a blunt slice, not a string decoder, and it is worth being explicit
/// about the consequence. `/Ordering`'s value is normally written as the
/// PostScript string `(Japan1)`, which the lexer hands over whole; dropping
/// two bytes gives `apan1)`, which matches no collection name. So an embedded
/// CMap written the ordinary way does **not** set its own charset, and the
/// font's `/CIDSystemInfo` is what ends up deciding the collection. Only an
/// unusual spelling — a hex string, say — lines the bytes up to match. The
/// oracle behaves this way and files are laid out against it.
fn operand_value(word: &[u8]) -> &[u8] {
    word.get(2..).unwrap_or_default()
}

/// Read a number: `<...>` is hexadecimal, anything else decimal.
///
/// Both forms stop at the first byte that is not a digit of their base and
/// return what they accumulated, so `12d` is 12 and `<A2z` is 0xA2 — and, more
/// to the point, the lexer's `<A2>` token stops at its own closing bracket. A
/// value that overflows 32 bits yields 0, not a truncated value.
pub(crate) fn get_code(word: &[u8]) -> u32 {
    let (digits, radix) = match word.first() {
        None => return 0,
        Some(&b'<') => (word.get(1..).unwrap_or_default(), 16u32),
        Some(_) => (word, 10),
    };
    let mut num = 0u32;
    for &b in digits {
        let Some(d) = char::from(b).to_digit(radix) else {
            break;
        };
        // Overflow discards the whole number rather than wrapping.
        let Some(next) = num.checked_mul(radix).and_then(|n| n.checked_add(d)) else {
            return 0;
        };
        num = next;
    }
    num
}

/// One hex digit's value, or 0 for anything else — a non-hex byte inside a
/// codespace bound contributes zero rather than rejecting the range.
fn hex_digit(b: u8) -> u8 {
    char::from(b).to_digit(16).unwrap_or(0) as u8
}

/// Build a codespace range from a `<lower>` / `<upper>` token pair.
///
/// The width comes from the *lower* token alone: the bytes between `<` and the
/// first `>` (or the end of the token, if it has no `>`), halved. A width
/// above four rejects the range. The upper bound is read from the second token
/// at the same positions, and any position past its end contributes the digit
/// `0` — which is why pairing `<a1>` with an empty token gives an upper bound
/// of 0 rather than 0xFF.
pub(crate) fn get_code_range(
    first: &[u8],
    second: &[u8],
    diags: &mut Diagnostics,
) -> Option<CodeRange> {
    if first.first() != Some(&b'<') {
        return None;
    }
    let close = first
        .iter()
        .position(|&b| b == b'>')
        .filter(|&i| i >= 1)
        .unwrap_or(first.len());
    if close == first.len() {
        // An unterminated bound still yields a range, at whatever width the
        // digits it has imply.
        diags.record(Severity::Suspicious, DiagKind::CMapTruncatedCodespace, None);
    }
    let char_size = (close.saturating_sub(1)) / 2;
    if char_size > 4 {
        return None;
    }
    let mut range = CodeRange {
        char_size: u8::try_from(char_size).unwrap_or(4),
        lower: [0; 4],
        upper: [0; 4],
    };
    for i in 0..char_size {
        let hi = first.get(i * 2 + 1).copied().map_or(0, hex_digit);
        let lo = first.get(i * 2 + 2).copied().map_or(0, hex_digit);
        if let Some(slot) = range.lower.get_mut(i) {
            *slot = hi * 16 + lo;
        }
        let hi = second.get(i * 2 + 1).copied().map_or(0, hex_digit);
        let lo = second.get(i * 2 + 2).copied().map_or(0, hex_digit);
        if let Some(slot) = range.upper.get_mut(i) {
            *slot = hi * 16 + lo;
        }
    }
    Some(range)
}

#[cfg(test)]
mod tests {
    use super::{get_code, get_code_range, operand_value};
    use pdfrum_common::Diagnostics;

    /// Every assertion of the oracle's own `GetCode` unit test.
    #[test]
    fn get_code_matches_the_pinned_assertions() {
        for (input, want) in [
            (&b""[..], 0u32),
            (b"<", 0),
            (b"<c2", 194),
            (b"<A2", 162),
            (b"<Af2", 2802),
            (b"<A2z", 162),
            (b"12", 12),
            (b"12d", 12),
            (b"128", 128),
            (b"<FFFFFFFF", 4_294_967_295),
            // One digit too many: the whole value is discarded, not truncated.
            (b"<100000000", 0),
        ] {
            assert_eq!(get_code(input), want, "GetCode({input:?})");
        }
    }

    /// The lexer delivers hex tokens with their closing bracket, and the scan
    /// stops there.
    #[test]
    fn get_code_stops_at_the_closing_bracket() {
        assert_eq!(get_code(b"<A2>"), 162);
        assert_eq!(get_code(b"<20>"), 0x20);
        assert_eq!(get_code(b"<>"), 0);
        assert_eq!(get_code(b"1234567890123"), 0);
        assert_eq!(get_code(b"abc"), 0);
    }

    /// Every assertion of the oracle's own `GetCodeRange` unit test.
    #[test]
    fn get_code_range_matches_the_pinned_assertions() {
        let mut d = Diagnostics::default();
        assert!(get_code_range(b"", b"", &mut d).is_none());
        assert!(get_code_range(b"A", b"", &mut d).is_none());
        // A width of five is rejected outright.
        assert!(get_code_range(b"<aaaaaaaaaa>", b"", &mut d).is_none());

        let r = get_code_range(b"<12345678>", b"<87654321>", &mut d).unwrap();
        assert_eq!(r.char_size, 4);
        assert_eq!(r.lower, [18, 52, 86, 120]);
        assert_eq!(r.upper, [135, 101, 67, 33]);

        let r = get_code_range(b"<a1>", b"<F3>", &mut d).unwrap();
        assert_eq!(r.char_size, 1);
        assert_eq!(r.lower[0], 161);
        assert_eq!(r.upper[0], 243);

        // A short upper bound pads with the digit zero, not with 0xFF.
        let r = get_code_range(b"<a1>", b"", &mut d).unwrap();
        assert_eq!(r.char_size, 1);
        assert_eq!(r.lower[0], 161);
        assert_eq!(r.upper[0], 0);
    }

    /// An unterminated lower bound still yields a range, at the width its
    /// digits imply, and says so through the diagnostic channel.
    #[test]
    fn unterminated_bounds_still_produce_a_range() {
        let mut d = Diagnostics::default();
        let r = get_code_range(b"<a1", b"<f3>", &mut d).unwrap();
        assert_eq!(r.char_size, 1);
        assert_eq!(r.lower[0], 0xa1);
        assert_eq!(r.upper[0], 0xf3);
        assert!(d.contains(&pdfrum_common::DiagKind::CMapTruncatedCodespace));
    }

    /// Non-hex bytes inside a bound read as zero rather than rejecting it.
    #[test]
    fn non_hex_digits_read_as_zero() {
        let mut d = Diagnostics::default();
        let r = get_code_range(b"<zz>", b"<zz>", &mut d).unwrap();
        assert_eq!(r.char_size, 1);
        assert_eq!(r.lower[0], 0);
        assert_eq!(r.upper[0], 0);
    }

    #[test]
    fn width_zero_ranges_are_representable() {
        let mut d = Diagnostics::default();
        let r = get_code_range(b"<>", b"<>", &mut d).unwrap();
        assert_eq!(r.char_size, 0);
    }

    /// The two-byte slice that makes `/Ordering (Japan1)` a no-op.
    #[test]
    fn operand_value_drops_two_bytes() {
        assert_eq!(operand_value(b"(Japan1)"), b"apan1)");
        assert_eq!(operand_value(b"ab"), b"");
        assert_eq!(operand_value(b""), b"");
        assert_eq!(operand_value(b"abJapan1"), b"Japan1");
    }
}
