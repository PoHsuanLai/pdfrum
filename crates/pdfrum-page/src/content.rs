//! `parse_content`: content bytes to a list of typed operators.
//!
//! Infallible by contract (SPEC.md §7): an operator nobody recognises, an
//! operator with the wrong number of operands, an unterminated string — all
//! become a diagnostic and are skipped, never an error. This mirrors PDFium's
//! interpreter, which has no failure path at all in its dispatch loop.
//!
//! # The operand ring
//!
//! Operands accumulate in a **16-slot ring** that evicts its oldest entry
//! when full ([`OperandRing`]). A content stream pushing twenty numbers
//! before one operator leaves the operator seeing the last sixteen with the
//! indices renumbered — silently, exactly as PDFium does. Accessors index
//! from the *newest* operand, so index 0 is the last one pushed.
//!
//! # Two sub-loops bypass the ring
//!
//! `m` enters a fast path that reads path operators straight from the
//! tokenizer ([`parse_path_run`]), and `BI` consumes an inline image whole
//! ([`crate::inline_image`]). Both rewind the cursor when they meet something
//! they do not handle, so no operand is ever lost.

use crate::inline_image;
use crate::ops::{Dispatch, Op};
use crate::tokenize::{ContentLexer, Element};
use kurbo::Point;
use pdfrum_common::{DiagKind, Diagnostics, Limits, Severity};
use pdfrum_object::{Name, Object};
use smallvec::SmallVec;

/// Slots in the operand ring. PDFium's `kParamBufSize`.
pub(crate) const RING_SIZE: usize = 16;

/// One pending operand.
///
/// Numbers and names stay unboxed because that is how they arrive and how
/// most operators read them; anything else is already an [`Object`].
#[derive(Debug, Clone, PartialEq, Default)]
enum Operand {
    #[default]
    Empty,
    Number(f32),
    Name(Name),
    Object(Object),
}

/// The 16-slot operand ring, with oldest-eviction.
///
/// A plain record: `push` and the four accessors are all it does. The
/// eviction rule and the "a missing operand reads as zero" rule live here and
/// nowhere else.
#[derive(Debug, Clone)]
pub(crate) struct OperandRing {
    slots: [Operand; RING_SIZE],
    start: usize,
    count: usize,
    /// Set when eviction actually dropped an operand, so the caller can
    /// record one diagnostic per operator rather than one per push.
    overflowed: bool,
}

impl Default for OperandRing {
    fn default() -> Self {
        Self {
            slots: std::array::from_fn(|_| Operand::Empty),
            start: 0,
            count: 0,
            overflowed: false,
        }
    }
}

impl OperandRing {
    /// Append an operand, evicting the oldest when the ring is full.
    fn push(&mut self, value: Operand) {
        let at = if self.count == RING_SIZE {
            // Full: the oldest slot is reused and `start` advances, so the
            // count stays pinned at sixteen and the indices renumber.
            self.overflowed = true;
            let at = self.start;
            self.start = (self.start + 1) % RING_SIZE;
            at
        } else {
            let at = (self.start + self.count) % RING_SIZE;
            self.count += 1;
            at
        };
        if let Some(slot) = self.slots.get_mut(at) {
            *slot = value;
        }
    }

    /// Drop every operand. Runs after each keyword, recognised or not.
    fn clear(&mut self) {
        self.start = 0;
        self.count = 0;
        self.overflowed = false;
        for slot in &mut self.slots {
            *slot = Operand::Empty;
        }
    }

    /// How many operands are pending, capped at [`RING_SIZE`].
    pub(crate) fn len(&self) -> usize {
        self.count
    }

    /// Whether the ring evicted anything since the last [`Self::clear`].
    fn overflowed(&self) -> bool {
        self.overflowed
    }

    /// The slot `index` operands back from the newest.
    fn slot(&self, index: usize) -> Option<&Operand> {
        if index >= self.count {
            return None;
        }
        let at = (self.start + self.count - index - 1) % RING_SIZE;
        self.slots.get(at)
    }

    /// The number at `index`, or **0** when there is no such operand.
    ///
    /// A non-numeric operand also reads as 0, matching `GetNumber`.
    pub(crate) fn number(&self, index: usize) -> f32 {
        match self.slot(index) {
            Some(Operand::Number(n)) => *n,
            Some(Operand::Object(o)) => o.number().unwrap_or(0.0),
            _ => 0.0,
        }
    }

    /// The number at `index` as an integer, or `None` when it is outside a
    /// C `int`. Callers use `.unwrap_or(0)`, matching `.value_or(0)`.
    pub(crate) fn integer(&self, index: usize) -> Option<i64> {
        let v = self.number(index);
        if v.is_nan() || !(-2_147_483_648.0..=2_147_483_647.0).contains(&v) {
            return None;
        }
        #[expect(
            clippy::cast_possible_truncation,
            reason = "the range check above is exactly the C++'s int check"
        )]
        Some(v as i64)
    }

    /// The bytes at `index`, or empty when there is no such operand.
    ///
    /// A name yields its text, a string its bytes, a number its formatting,
    /// anything else empty — `GetString`'s behaviour.
    pub(crate) fn string(&self, index: usize) -> Vec<u8> {
        match self.slot(index) {
            Some(Operand::Name(n)) => n.as_bytes().to_vec(),
            Some(Operand::Number(n)) => pdfrum_object::fmt_number(*n).into_bytes(),
            Some(Operand::Object(o)) => o.to_byte_string(),
            _ => Vec::new(),
        }
    }

    /// The object at `index`, materializing an unboxed slot, or
    /// [`Object::Null`] when there is no such operand.
    pub(crate) fn object(&self, index: usize) -> Object {
        match self.slot(index) {
            Some(Operand::Number(n)) => Object::Real(*n),
            Some(Operand::Name(n)) => Object::Name(n.clone()),
            Some(Operand::Object(o)) => o.clone(),
            _ => Object::Null,
        }
    }

    /// Whether the operand at `index` is a name, which is how `SCN`/`scn`
    /// decide between a colour set and a pattern.
    pub(crate) fn is_name(&self, index: usize) -> bool {
        matches!(
            self.slot(index),
            Some(Operand::Name(_) | Operand::Object(Object::Name(_)))
        )
    }

    /// The newest `count` numbers **in source order**, oldest first —
    /// `GetNumbers`.
    pub(crate) fn numbers(&self, count: usize) -> SmallVec<[f32; 4]> {
        (0..count).rev().map(|i| self.number(i)).collect()
    }

    /// Every operand **except the newest**, in source order — `GetNamedColors`.
    ///
    /// Not `numbers(len - 1)`: that would take the newest `len - 1` operands,
    /// which for `scn` means the pattern name and all but the *oldest*
    /// component. The C++ reads `GetNumber(param_count_ - i - 1)`, which skips
    /// slot zero and keeps every component. The difference shows up only when
    /// a pattern actually paints, and it shows up as the wrong colour: an
    /// uncoloured pattern under `1 1 1 /P1 scn` takes `[1, 1, 0]` and paints
    /// yellow where it should paint white.
    pub(crate) fn named_numbers(&self) -> SmallVec<[f32; 4]> {
        (1..self.len()).rev().map(|i| self.number(i)).collect()
    }
}

/// Parse content-stream bytes into operators.
///
/// Never fails: unrecognised operators, wrong operand counts and truncated
/// constructs each record a [`Diagnostic`](pdfrum_common::Diagnostic) and are
/// skipped. The returned list is exactly what
/// [`build_page`](crate::build_page) folds.
///
/// ```
/// use pdfrum_common::{Diagnostics, Limits};
/// use pdfrum_page::{Op, parse_content};
///
/// let mut diags = Diagnostics::default();
/// let ops = parse_content(b"1 0 0 1 10 20 cm 0 0 m 5 5 l S", &Limits::default(), &mut diags);
/// assert_eq!(ops.len(), 4);
/// assert!(matches!(ops[0], Op::Concat(_)));
/// assert!(matches!(ops[3], Op::Stroke()));
/// assert!(diags.is_empty());
/// ```
#[must_use]
pub fn parse_content(bytes: &[u8], limits: &Limits, diags: &mut Diagnostics) -> Vec<Op> {
    let mut lexer = ContentLexer::new(bytes);
    let mut ring = OperandRing::default();
    let mut ops = Vec::new();
    loop {
        match lexer.next_element() {
            Element::Eof => return ops,
            Element::Number(n) => ring.push(Operand::Number(n)),
            Element::Name(n) => ring.push(Operand::Name(n)),
            Element::Object(o) => ring.push(Operand::Object(o)),
            Element::Keyword(word) => {
                dispatch(&word, &mut lexer, &mut ring, &mut ops, limits, diags);
                // Operands never survive an operator, recognised or not.
                ring.clear();
            }
        }
    }
}

/// Handle one keyword: the two sub-loops first, then the table.
fn dispatch(
    word: &[u8],
    lexer: &mut ContentLexer<'_>,
    ring: &mut OperandRing,
    ops: &mut Vec<Op>,
    limits: &Limits,
    diags: &mut Diagnostics,
) {
    let at = Some(lexer.pos() as u64);
    if ring.overflowed() {
        diags.record(Severity::Suspicious, DiagKind::OperandsDropped, at);
    }

    if word == b"BI" {
        // A `None` means the image was abandoned; `read` has already rewound
        // and recorded why, and `BI` itself does nothing.
        if let Some(image) = inline_image::read(lexer, limits, diags) {
            ops.push(Op::InlineImage(Box::new(image)));
        }
        return;
    }

    match Op::from_ring(word, ring) {
        Dispatch::Op(op) => {
            let is_move = matches!(op, Op::MoveTo(_));
            ops.push(op);
            if is_move {
                // `m` enters PDFium's path fast loop, which reads further
                // path operators straight from the tokenizer.
                parse_path_run(lexer, ops);
            }
        }
        Dispatch::GuardFailed => {
            diags.record(Severity::Suspicious, DiagKind::OperandCountMismatch, at);
        }
        Dispatch::NotAnOperator => {
            diags.record(Severity::Suspicious, DiagKind::UnknownOperator, at);
            ops.push(Op::Unknown(word.into()));
        }
    }
}

/// PDFium's `ParsePathObject`: the sub-loop `m` enters.
///
/// It reads numbers into a six-slot buffer and recognises only the seven path
/// operators, so operands here are in **source order** rather than the ring's
/// reverse order. Two details are load-bearing:
///
/// - a **seventh** number in one run is silently dropped, because the C++
///   `break`s out of its `switch` and keeps looping;
/// - anything else — an unrecognised keyword, a non-number element, the end
///   of data — **rewinds** to just after the last path operator, so the main
///   loop re-reads the operands it had already consumed.
fn parse_path_run(lexer: &mut ContentLexer<'_>, ops: &mut Vec<Op>) {
    let mut params = [0f32; 6];
    let mut n = 0usize;
    let mut last_pos = lexer.pos();
    loop {
        let element = lexer.next_element();
        match element {
            Element::Number(v) => {
                if n < 6 {
                    if let Some(slot) = params.get_mut(n) {
                        *slot = v;
                    }
                    n += 1;
                }
                // A seventh number is consumed and dropped.
            }
            Element::Keyword(word) => {
                let p = |i: usize| f64::from(params.get(i).copied().unwrap_or(0.0));
                let op = match &*word {
                    b"m" => Op::MoveTo(Point::new(p(0), p(1))),
                    b"l" => Op::LineTo(Point::new(p(0), p(1))),
                    b"c" => Op::CurveTo(
                        Point::new(p(0), p(1)),
                        Point::new(p(2), p(3)),
                        Point::new(p(4), p(5)),
                    ),
                    b"v" => Op::CurveToV(Point::new(p(0), p(1)), Point::new(p(2), p(3))),
                    b"y" => Op::CurveToY(Point::new(p(0), p(1)), Point::new(p(2), p(3))),
                    b"h" => Op::ClosePath(),
                    b"re" => Op::Rectangle(
                        params.first().copied().unwrap_or(0.0),
                        params.get(1).copied().unwrap_or(0.0),
                        params.get(2).copied().unwrap_or(0.0),
                        params.get(3).copied().unwrap_or(0.0),
                    ),
                    _ => {
                        lexer.seek(last_pos);
                        return;
                    }
                };
                ops.push(op);
                n = 0;
                last_pos = lexer.pos();
            }
            _ => {
                lexer.seek(last_pos);
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    // Test fixtures quote the oracle's own vectors, compare floats exactly
    // where the behaviour being pinned is exact, and index arrays whose
    // length the fixture itself fixes.
    #![allow(
        clippy::unreadable_literal,
        clippy::float_cmp,
        clippy::indexing_slicing,
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        reason = "test fixtures quote oracle vectors verbatim and compare exactly"
    )]

    use super::{OperandRing, RING_SIZE, parse_content};
    use crate::ops::Op;
    use kurbo::Point;
    use pdfrum_common::{DiagKind, Diagnostics, Limits};

    fn parse(src: &[u8]) -> (Vec<Op>, Diagnostics) {
        let mut diags = Diagnostics::default();
        let ops = parse_content(src, &Limits::default(), &mut diags);
        (ops, diags)
    }

    #[test]
    fn twenty_operands_leave_the_last_sixteen_renumbered() {
        // `1 2 … 20 cm` — the matrix reads the six newest, which are 15..=20.
        use std::fmt::Write as _;
        let mut src = String::new();
        for i in 1..=20 {
            let _ = write!(src, "{i} ");
        }
        src.push_str("cm");
        let (ops, diags) = parse(src.as_bytes());
        let Some(Op::Concat(m)) = ops.first() else {
            panic!("expected cm, got {ops:?}");
        };
        assert_eq!(m.as_coeffs(), [15.0, 16.0, 17.0, 18.0, 19.0, 20.0]);
        assert!(diags.contains(&DiagKind::OperandsDropped));
    }

    #[test]
    fn missing_operands_read_as_zero_unless_the_row_guards() {
        // `w` has no guard: zero operands means a zero line width.
        let (ops, _) = parse(b"w");
        assert_eq!(ops, vec![Op::SetLineWidth(0.0)]);

        // `rg` guards on exactly three, so two operands skip it entirely.
        let (ops, diags) = parse(b"1 2 rg");
        assert!(ops.is_empty());
        assert!(diags.contains(&DiagKind::OperandCountMismatch));

        // `m` guards on exactly two.
        let (ops, _) = parse(b"5 m");
        assert!(ops.is_empty());
    }

    #[test]
    fn an_unknown_operator_clears_the_operands() {
        let (ops, diags) = parse(b"1 2 3 zzz 4 w");
        assert_eq!(ops.len(), 2);
        assert!(matches!(&ops[0], Op::Unknown(w) if &**w == b"zzz"));
        // The `4` after it is the only operand `w` sees.
        assert_eq!(ops[1], Op::SetLineWidth(4.0));
        assert!(diags.contains(&DiagKind::UnknownOperator));
    }

    #[test]
    fn the_path_fast_loop_rewinds_on_an_unknown_keyword() {
        let (ops, _) = parse(b"0 0 m 1 1 l 2 2 3 zzz");
        assert_eq!(
            ops,
            vec![
                Op::MoveTo(Point::new(0.0, 0.0)),
                Op::LineTo(Point::new(1.0, 1.0)),
                Op::Unknown(Box::from(&b"zzz"[..])),
            ]
        );
    }

    #[test]
    fn the_path_fast_loop_drops_a_seventh_number() {
        // `1 2 3 4 5 6 7 c` keeps the first six; the seventh vanishes.
        let (ops, _) = parse(b"0 0 m 1 2 3 4 5 6 7 c");
        assert_eq!(
            ops,
            vec![
                Op::MoveTo(Point::new(0.0, 0.0)),
                Op::CurveTo(
                    Point::new(1.0, 2.0),
                    Point::new(3.0, 4.0),
                    Point::new(5.0, 6.0)
                ),
            ]
        );
    }

    #[test]
    fn the_path_fast_loop_reads_operands_in_source_order() {
        let (ops, _) = parse(b"1 2 m 3 4 l");
        assert_eq!(
            ops,
            vec![
                Op::MoveTo(Point::new(1.0, 2.0)),
                Op::LineTo(Point::new(3.0, 4.0)),
            ]
        );
    }

    #[test]
    fn rectangle_reads_x_y_width_height_oldest_first() {
        let (ops, _) = parse(b"10 20 30 40 re");
        assert_eq!(ops, vec![Op::Rectangle(10.0, 20.0, 30.0, 40.0)]);
    }

    #[test]
    fn a_named_scn_keeps_every_component_and_drops_only_the_name() {
        // `GetNamedColors` reads `GetNumber(param_count_ - i - 1)`, which
        // skips slot zero — the name — and keeps all three components.
        // Taking the newest `len - 1` instead drops the *oldest* component
        // and reads the name as a zero, which turns white into yellow on
        // every uncoloured pattern painted through a three-component base.
        let (ops, _) = parse(b"1 1 1 /P1 scn");
        let Some(Op::SetFillColorN(c)) = ops.first() else {
            panic!("expected a named scn, got {ops:?}");
        };
        assert_eq!(&c.values[..], &[1.0, 1.0, 1.0]);
        assert_eq!(
            c.pattern.as_ref().map(|n| n.as_bytes().to_vec()),
            Some(b"P1".to_vec())
        );
    }

    #[test]
    fn an_unnamed_scn_keeps_every_operand() {
        let (ops, _) = parse(b"0.25 0.5 0.75 scn");
        let Some(Op::SetFillColorN(c)) = ops.first() else {
            panic!("expected an scn, got {ops:?}");
        };
        assert_eq!(&c.values[..], &[0.25, 0.5, 0.75]);
        assert!(c.pattern.is_none());
    }

    #[test]
    fn a_bare_named_scn_has_no_components_at_all() {
        // A `/P1 scn` with no operands before it: one slot, all name.
        let (ops, _) = parse(b"/P1 scn");
        let Some(Op::SetFillColorN(c)) = ops.first() else {
            panic!("expected a named scn, got {ops:?}");
        };
        assert!(c.values.is_empty(), "got {:?}", c.values);
    }

    #[test]
    fn ring_evicts_oldest_and_renumbers() {
        let mut ring = OperandRing::default();
        for i in 0..20 {
            ring.push(super::Operand::Number(i as f32));
        }
        assert_eq!(ring.len(), RING_SIZE);
        // Index 0 is the newest (19), index 15 the oldest survivor (4).
        assert!((ring.number(0) - 19.0).abs() < f32::EPSILON);
        assert!((ring.number(15) - 4.0).abs() < f32::EPSILON);
        // Past the count, everything is zero.
        assert!(ring.number(16).abs() < f32::EPSILON);
        assert!(ring.string(16).is_empty());
        assert!(ring.object(16).is_null());
    }

    #[test]
    fn numbers_come_back_in_source_order() {
        let mut ring = OperandRing::default();
        for i in 1..=4 {
            ring.push(super::Operand::Number(i as f32));
        }
        assert_eq!(&ring.numbers(4)[..], &[1.0, 2.0, 3.0, 4.0]);
        assert_eq!(&ring.numbers(2)[..], &[3.0, 4.0]);
    }
}
