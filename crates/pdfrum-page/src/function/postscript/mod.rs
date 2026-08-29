//! Type 4: the PostScript calculator (ISO 32000-1 §7.10.5).
//!
//! A program is one outer `{ … }` procedure. Parsing is strict about
//! exactly two things — the program must **begin** with `{`, and an
//! unterminated `{` is a failure — and lenient about everything else: an
//! unrecognised token becomes the constant `0.0`.
//!
//! Nesting is capped at 128, compared with `>`, so **129 levels** including
//! the outer procedure are accepted and the 130th is refused. Execution
//! recursion has no separate limit; it is bounded by that parse depth.

mod eval;
mod op;

pub use op::PsOp;

use super::Common;
use crate::names;
use eval::{Machine, STACK_SIZE};
use pdfrum_common::{DiagKind, Diagnostics, Limits, Severity};
use pdfrum_filters::decode_chain;
use pdfrum_object::{Resolve, Stream};

/// Nesting cap, compared with `>` so 128 is accepted.
pub const MAX_NESTING: u32 = 128;

/// One instruction inside a procedure.
#[derive(Debug, Clone, PartialEq)]
enum ProcItem {
    /// A nested `{ … }` block.
    Proc(Proc),
    /// A literal value.
    Const(f32),
    /// A named operator.
    Op(PsOp),
}

/// A `{ … }` block.
#[derive(Debug, Clone, PartialEq, Default)]
struct Proc {
    items: Vec<ProcItem>,
}

/// A type 4 function.
#[derive(Debug, Clone, PartialEq)]
pub struct PostScript {
    /// `/Domain`.
    pub domain: Box<[f32]>,
    /// `/Range`, which is required for this type.
    pub range: Box<[f32]>,
    /// How many outputs, from `/Range`.
    pub outputs: usize,
    /// The parsed program.
    program: Proc,
}

impl PostScript {
    /// Load from a stream, parsing the program.
    pub(super) fn load<R: Resolve>(
        stream: &Stream,
        common: &Common,
        r: &R,
        limits: &Limits,
        diags: &mut Diagnostics,
    ) -> Option<Self> {
        // `/Range` is required for this type.
        let outputs = common.outputs();
        if outputs == 0 {
            return None;
        }
        let _ = names::FUNCTION_TYPE;
        let source = decode_chain(stream, 0, r, limits, diags).data;
        let program = parse(&source)?;
        Some(Self {
            domain: common.domain.clone(),
            range: common.range.clone(),
            outputs,
            program,
        })
    }

    /// Evaluate.
    ///
    /// The only condition that reports failure is the stack holding fewer
    /// values than there are outputs; every other misbehaviour is absorbed
    /// silently, exactly as the C++ absorbs it.
    pub(super) fn eval(&self, input: &[f32], out: &mut [f32]) -> bool {
        let mut machine = Machine::new();
        machine.push_inputs(input);
        let _ = machine.run(&self.program);
        machine.take_outputs(out, self.outputs)
    }

    /// Evaluate, reporting what the engine noticed going wrong.
    ///
    /// The plain [`Self::eval`] path swallows these, which is the behaviour
    /// contract; this variant exists so a caller that owns a diagnostics sink
    /// can record them.
    pub fn eval_with_diagnostics(
        &self,
        input: &[f32],
        out: &mut [f32],
        diags: &mut Diagnostics,
    ) -> bool {
        let mut machine = Machine::new();
        machine.push_inputs(input);
        let _ = machine.run(&self.program);
        if machine.abused_stack {
            diags.record(Severity::Suspicious, DiagKind::PostScriptStackAbuse, None);
        }
        if machine.malformed_proc {
            diags.record(
                Severity::Suspicious,
                DiagKind::PostScriptMalformedProc,
                None,
            );
        }
        machine.take_outputs(out, self.outputs)
    }

    /// How many values the program leaves on the stack for the given input,
    /// for tests that need to see the machine rather than the function.
    #[must_use]
    pub fn stack_depth_after(&self, input: &[f32]) -> usize {
        let mut machine = Machine::new();
        machine.push_inputs(input);
        let _ = machine.run(&self.program);
        machine.len()
    }
}

/// Split a program into words, treating `{` and `}` as one-byte tokens and
/// skipping `%` comments to end of line.
struct Words<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Iterator for Words<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let b = self.data.get(self.pos).copied()?;
            if matches!(b, 0x00 | 0x09 | 0x0A | 0x0C | 0x0D | 0x20) {
                self.pos += 1;
            } else if b == b'%' {
                while let Some(c) = self.data.get(self.pos).copied() {
                    self.pos += 1;
                    if c == b'\r' || c == b'\n' {
                        break;
                    }
                }
            } else {
                break;
            }
        }
        let start = self.pos;
        let first = self.data.get(self.pos).copied()?;
        if matches!(
            first,
            b'{' | b'}' | b'[' | b']' | b'(' | b')' | b'<' | b'>' | b'/'
        ) {
            self.pos += 1;
            return self.data.get(start..self.pos);
        }
        while let Some(b) = self.data.get(self.pos).copied() {
            if matches!(b, 0x00 | 0x09 | 0x0A | 0x0C | 0x0D | 0x20)
                || matches!(
                    b,
                    b'{' | b'}' | b'[' | b']' | b'(' | b')' | b'<' | b'>' | b'/' | b'%'
                )
            {
                break;
            }
            self.pos += 1;
        }
        self.data.get(start..self.pos)
    }
}

/// Parse a program: a single outer procedure.
fn parse(source: &[u8]) -> Option<Proc> {
    let mut words = Words {
        data: source,
        pos: 0,
    };
    // The program **must** begin with `{`.
    if words.next()? != b"{" {
        return None;
    }
    parse_proc(&mut words, 0)
}

/// Parse the body of a procedure whose `{` has already been consumed.
fn parse_proc(words: &mut Words<'_>, depth: u32) -> Option<Proc> {
    if depth > MAX_NESTING {
        return None;
    }
    let mut items = Vec::new();
    loop {
        // End of data with the brace still open is a parse failure.
        let word = words.next()?;
        match word {
            b"}" => return Some(Proc { items }),
            b"{" => items.push(ProcItem::Proc(parse_proc(words, depth + 1)?)),
            _ => items.push(match PsOp::from_name(word) {
                Some(op) => ProcItem::Op(op),
                // An unrecognised token is a constant, and one that will not
                // parse as a number is the constant zero.
                None => ProcItem::Const(parse_number(word)),
            }),
        }
    }
}

/// `StringToFloat`: a decimal reading that yields 0.0 for anything else.
fn parse_number(word: &[u8]) -> f32 {
    std::str::from_utf8(word)
        .ok()
        .and_then(|s| s.parse::<f32>().ok())
        .filter(|v| v.is_finite())
        .unwrap_or(0.0)
}

/// Build a function from a bare program, for tests and for callers that hold
/// a program rather than a stream.
#[must_use]
pub fn parse_program(source: &[u8], domain: &[f32], range: &[f32]) -> Option<PostScript> {
    Some(PostScript {
        domain: domain.into(),
        range: range.into(),
        outputs: range.len() / 2,
        program: parse(source)?,
    })
}

/// The stack size the engine runs with, exposed for tests that probe the
/// overflow behaviour.
#[must_use]
pub fn stack_size() -> usize {
    STACK_SIZE
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

    use super::{MAX_NESTING, PostScript, PsOp, parse_program, stack_size};

    fn run(program: &str, inputs: &[f32], outputs: usize) -> Vec<f32> {
        let range: Vec<f32> = (0..outputs).flat_map(|_| [-1e30, 1e30]).collect();
        let domain: Vec<f32> = (0..inputs.len()).flat_map(|_| [-1e30, 1e30]).collect();
        let f = parse_program(program.as_bytes(), &domain, &range).expect("parses");
        let mut out = vec![0.0f32; outputs];
        assert!(f.eval(inputs, &mut out), "evaluation should succeed");
        out
    }

    fn one(program: &str) -> f32 {
        run(program, &[], 1).first().copied().unwrap_or(f32::NAN)
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn arithmetic_basics() {
        assert!(close(one("{100 200 add}"), 300.0));
        assert!(close(one("{100 150 sub}"), -50.0));
        assert!(close(one("{5 120 mul}"), 600.0));
        assert!(close(one("{15 10 div}"), 1.5));
        assert!(close(one("{15 10 idiv}"), 1.0));
        assert!(close(one("{15 10 mod}"), 5.0));
        assert!(close(one("{-5 neg}"), 5.0));
        assert!(close(one("{-5 abs}"), 5.0));
    }

    #[test]
    fn division_by_zero_yields_zero_everywhere() {
        assert!(close(one("{100 0 idiv}"), 0.0));
        assert!(close(one("{100 0 mod}"), 0.0));
        assert!(close(one("{100 0 div}"), 0.0));
    }

    #[test]
    fn rounding_operators() {
        assert!(close(one("{5.5 round}"), 6.0));
        // Half-way values round *up*, so −5.5 becomes −5.
        assert!(close(one("{-5.5 round}"), -5.0));
        assert!(close(one("{5.9 ceiling}"), 6.0));
        assert!(close(one("{-5.1 ceiling}"), -5.0));
        assert!(close(one("{5.9 floor}"), 5.0));
        assert!(close(one("{-5.1 floor}"), -6.0));
        assert!(close(one("{5.9 truncate}"), 5.0));
        assert!(close(one("{-5.9 truncate}"), -5.0));
        assert!(close(one("{5.9 cvi}"), 5.0));
    }

    #[test]
    fn truncate_saturates_rather_than_overflowing() {
        // crbug 42270316: `f32(i32::MAX) * -1.5` saturates to `i32::MIN`,
        // whose float value is `-f32(i32::MAX) - 1`.
        let program = format!("{{{} truncate}}", (i32::MAX as f32) * -1.5);
        let got = one(&program);
        assert!(got <= -2.0e9, "got {got}");
        assert!(got.is_finite());
    }

    #[test]
    fn comparisons_push_one_or_zero() {
        for (program, want) in [
            ("{0 0 eq}", 1.0),
            ("{0 1 eq}", 0.0),
            ("{0 0 ne}", 0.0),
            ("{0 1 ne}", 1.0),
            ("{255 1 gt}", 1.0),
            ("{-1 0 gt}", 0.0),
            ("{0 0 ge}", 1.0),
            ("{-1 0 lt}", 1.0),
            ("{255 1 le}", 0.0),
        ] {
            assert!(close(one(program), want), "{program} should give {want}");
        }
    }

    #[test]
    fn logic_operators_are_bitwise_except_not() {
        assert!(close(one("{true}"), 1.0));
        assert!(close(one("{false}"), 0.0));
        assert!(close(one("{1 1 and}"), 1.0));
        assert!(close(one("{1 0 and}"), 0.0));
        assert!(close(one("{1 0 or}"), 1.0));
        assert!(close(one("{1 1 xor}"), 0.0));
        assert!(close(one("{6 3 and}"), 2.0));
        // `not` is logical, not a bitwise complement: 1 gives 0, not −2.
        assert!(close(one("{0 not}"), 1.0));
        assert!(close(one("{1 not}"), 0.0));
    }

    #[test]
    fn maths_functions_work_in_degrees() {
        assert!(close(one("{2 sqrt}"), std::f32::consts::SQRT_2));
        assert!(close(one("{60 sin}"), 0.8660254));
        assert!(close(one("{60 cos}"), 0.5));
        assert!(close(one("{1 1 atan}"), 45.0));
        assert!(close(one("{10 3 exp}"), 1000.0));
        assert!(close(one("{1000 log}"), 3.0));
        assert!(close(one("{10 ln}"), std::f32::consts::LN_10));
    }

    #[test]
    fn atan_normalizes_into_zero_to_three_sixty() {
        let got = one("{-1 -1 atan}");
        assert!((0.0..360.0).contains(&got), "got {got}");
        assert!(close(got, 225.0));
    }

    #[test]
    fn unknown_tokens_become_the_constant_zero() {
        assert!(close(one("{invalid}"), 0.0));
        assert!(close(one("{55}"), 55.0));
        assert!(close(one("{123.4}"), 123.4));
        assert!(close(one("{-5}"), -5.0));
        assert!(PsOp::from_name(b"invalid").is_none());
    }

    #[test]
    fn a_program_must_begin_with_a_brace_and_be_terminated() {
        assert!(parse_program(b"100 200 add", &[], &[0.0, 1.0]).is_none());
        assert!(parse_program(b"{100 200 add", &[], &[0.0, 1.0]).is_none());
        assert!(parse_program(b"", &[], &[0.0, 1.0]).is_none());
        assert!(parse_program(b"{}", &[], &[0.0, 1.0]).is_some());
    }

    #[test]
    fn nesting_is_accepted_to_the_cap_and_refused_beyond() {
        let build = |levels: u32| {
            let mut s = String::new();
            for _ in 0..levels {
                s.push('{');
            }
            for _ in 0..levels {
                s.push('}');
            }
            s
        };
        // The outer procedure is level one, so `MAX_NESTING + 1` levels fit.
        let ok = build(MAX_NESTING + 1);
        assert!(parse_program(ok.as_bytes(), &[], &[0.0, 1.0]).is_some());
        let too_deep = build(MAX_NESTING + 2);
        assert!(parse_program(too_deep.as_bytes(), &[], &[0.0, 1.0]).is_none());
    }

    #[test]
    fn if_and_ifelse_locate_their_procedures_lexically() {
        assert!(close(one("{1 {7} if}"), 7.0));
        // A false condition runs nothing, so the earlier constant survives.
        assert!(close(one("{3 0 {7} if}"), 3.0));
        // True selects the **first** procedure.
        assert!(close(one("{1 {7} {9} ifelse}"), 7.0));
        assert!(close(one("{0 {7} {9} ifelse}"), 9.0));
    }

    #[test]
    fn truthiness_truncates_so_a_half_reads_as_false() {
        assert!(close(one("{3 0.5 {7} if}"), 3.0));
        assert!(close(one("{1.5 {7} if}"), 7.0));
    }

    #[test]
    fn a_malformed_if_aborts_only_its_own_procedure() {
        // An `if` with no preceding procedure aborts the procedure it is in,
        // so the trailing `9` is never pushed. The abort itself is *not* an
        // error — `Execute`'s return value is discarded — so evaluation
        // succeeds with whatever the stack happens to hold.
        let f = parse_program(b"{5 9 if 7}", &[], &[-1e30, 1e30]).expect("parses");
        let mut out = [0.0f32];
        assert!(f.eval(&[], &mut out));
        // The structural check runs *before* the condition is popped, so
        // nothing was consumed and the `7` after the abort never ran: the
        // stack still holds `5 9`, and the single output takes the top.
        assert!(close(out[0], 9.0), "got {}", out[0]);
        // Inside a branch, a structural failure does not abort the parent.
        assert!(close(one("{4 1 {if} if}"), 4.0));
    }

    #[test]
    fn stack_overflow_drops_pushes_silently() {
        let mut program = String::from("{");
        for _ in 0..stack_size() + 5 {
            program.push_str("1 ");
        }
        program.push('}');
        let f = parse_program(program.as_bytes(), &[], &[-1e30, 1e30]).expect("parses");
        assert_eq!(f.stack_depth_after(&[]), stack_size());
        let mut out = [0.0f32];
        assert!(f.eval(&[], &mut out));
    }

    #[test]
    fn copy_index_and_roll_bounds_cases() {
        // `copy` with an out-of-range count is a no-op that still eats `n`.
        assert!(close(one("{7 5 copy}"), 7.0));
        assert!(close(one("{7 -1 copy}"), 7.0));
        assert!(close(one("{7 0 copy}"), 7.0));
        // A legal copy duplicates.
        assert_eq!(run("{1 2 2 copy}", &[], 4), vec![1.0, 2.0, 1.0, 2.0]);
        // `index` 0 is the top.
        assert!(close(one("{9 8 0 index}"), 8.0));
        // Out of range pushes nothing, so the stack net-shrinks.
        assert!(close(one("{9 8 5 index}"), 8.0));
        // `roll` rotates towards the top for a positive count.
        assert_eq!(run("{1 2 3 3 1 roll}", &[], 3), vec![3.0, 1.0, 2.0]);
        assert_eq!(run("{1 2 3 3 -1 roll}", &[], 3), vec![2.0, 3.0, 1.0]);
        // Degenerate counts are no-ops.
        assert_eq!(run("{1 2 3 0 0 roll}", &[], 3), vec![1.0, 2.0, 3.0]);
    }

    #[test]
    fn bitshift_collapses_every_overflow_to_zero() {
        assert!(close(one("{1 4 bitshift}"), 16.0));
        assert!(close(one("{16 -4 bitshift}"), 1.0));
        // Arithmetic, so a negative value keeps its sign.
        assert!(close(one("{-16 -2 bitshift}"), -4.0));
        // A shift past the word width, and INT_MIN's negation, both give 0.
        assert!(close(one("{1 99 bitshift}"), 0.0));
        assert!(close(one(&format!("{{1 {} bitshift}}", i32::MIN)), 0.0));
    }

    #[test]
    fn outputs_come_off_the_top_of_the_stack_in_reverse() {
        // The topmost value is the *last* output.
        assert_eq!(run("{1 2 3}", &[], 3), vec![1.0, 2.0, 3.0]);
        // Residue below the outputs is ignored.
        assert_eq!(run("{9 9 1 2}", &[], 2), vec![1.0, 2.0]);
    }

    #[test]
    fn too_few_stack_values_is_the_one_reported_failure() {
        let f = parse_program(b"{1}", &[], &[0.0, 1.0, 0.0, 1.0]).expect("parses");
        let mut out = [0.0f32; 2];
        assert!(!f.eval(&[], &mut out));
    }

    #[test]
    fn diagnostics_report_what_the_silent_paths_swallowed() {
        let mut program = String::from("{");
        for _ in 0..stack_size() + 5 {
            program.push_str("1 ");
        }
        program.push('}');
        let f: PostScript = parse_program(program.as_bytes(), &[], &[-1e30, 1e30]).expect("parses");
        let mut diags = pdfrum_common::Diagnostics::default();
        let mut out = [0.0f32];
        f.eval_with_diagnostics(&[], &mut out, &mut diags);
        assert!(diags.contains(&pdfrum_common::DiagKind::PostScriptStackAbuse));
    }
}
