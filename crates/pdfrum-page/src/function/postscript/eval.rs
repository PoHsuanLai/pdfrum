//! The calculator's stack machine.
//!
//! **Every error path here is silent.** Pushing past the hundred-slot stack
//! drops the value; popping an empty stack yields 0; dividing by zero yields
//! 0; a malformed `if` aborts its own procedure and no more. There is no
//! error channel in the C++ at all, and a `Result`-returning engine would
//! reject documents PDFium renders — so this one records a
//! [`Diagnostic`](pdfrum_common::Diagnostic) and carries on.
//!
//! Booleans are plain floats, 1.0 and 0.0, and truthiness goes through the
//! **saturating** integer conversion — so `0.5` truncates to 0 and reads as
//! *false*.

use super::op::PsOp;
use super::{Proc, ProcItem};

/// Stack slots. PDFium's `kPSEngineStackSize`.
pub(super) const STACK_SIZE: usize = 100;

/// Degrees per radian, at the precision the C++ uses.
const PI: f32 = std::f32::consts::PI;

/// The calculator's stack, plus what it noticed going wrong.
pub(super) struct Machine {
    stack: [f32; STACK_SIZE],
    count: usize,
    /// Set when a push was dropped or a pop underflowed.
    pub(super) abused_stack: bool,
    /// Set when an `if` or `ifelse` was not preceded by the procedures it
    /// needs.
    pub(super) malformed_proc: bool,
}

impl Machine {
    pub(super) fn new() -> Self {
        Self {
            stack: [0.0; STACK_SIZE],
            count: 0,
            abused_stack: false,
            malformed_proc: false,
        }
    }

    /// Push, **dropping the value** when the stack is full.
    fn push(&mut self, v: f32) {
        if self.count >= STACK_SIZE {
            self.abused_stack = true;
            return;
        }
        if let Some(slot) = self.stack.get_mut(self.count) {
            *slot = v;
        }
        self.count += 1;
    }

    /// Pop, yielding **0** when the stack is empty.
    fn pop(&mut self) -> f32 {
        if self.count == 0 {
            self.abused_stack = true;
            return 0.0;
        }
        self.count -= 1;
        self.stack.get(self.count).copied().unwrap_or(0.0)
    }

    /// Pop as a saturating integer, with NaN reading as 0.
    fn pop_int(&mut self) -> i32 {
        saturate(self.pop())
    }

    /// How many values are on the stack.
    pub(super) fn len(&self) -> usize {
        self.count
    }

    /// The value `depth` below the top.
    fn peek(&self, depth: usize) -> f32 {
        self.count
            .checked_sub(depth + 1)
            .and_then(|i| self.stack.get(i))
            .copied()
            .unwrap_or(0.0)
    }

    /// Push the function's inputs, in order.
    pub(super) fn push_inputs(&mut self, inputs: &[f32]) {
        for v in inputs {
            self.push(*v);
        }
    }

    /// Take the outputs off the top of the stack **in reverse**, so the
    /// topmost value is the *last* output. Residue below them is ignored.
    ///
    /// Returns false when the stack holds fewer values than there are
    /// outputs, which is the one condition the C++ reports as failure.
    pub(super) fn take_outputs(&mut self, out: &mut [f32], outputs: usize) -> bool {
        if self.count < outputs {
            return false;
        }
        for i in 0..outputs {
            let v = self.pop();
            if let Some(slot) = out.get_mut(outputs - i - 1) {
                *slot = v;
            }
        }
        true
    }

    /// Run a procedure's instructions in order.
    ///
    /// Returns false when a structural check failed, which aborts *this*
    /// procedure only — a nested branch's failure is discarded by its caller.
    pub(super) fn run(&mut self, proc: &Proc) -> bool {
        for (i, item) in proc.items.iter().enumerate() {
            match item {
                // Inert until an `if`/`ifelse` reaches back for it.
                ProcItem::Proc(_) => {}
                ProcItem::Const(v) => self.push(*v),
                ProcItem::Op(PsOp::If) => {
                    // The procedure is located **lexically**, by position,
                    // never popped from the stack.
                    let Some(ProcItem::Proc(body)) =
                        i.checked_sub(1).and_then(|p| proc.items.get(p))
                    else {
                        self.malformed_proc = true;
                        return false;
                    };
                    if self.pop_int() != 0 {
                        // A nested failure does not abort the parent.
                        let _ = self.run(body);
                    }
                }
                ProcItem::Op(PsOp::IfElse) => {
                    let (Some(ProcItem::Proc(first)), Some(ProcItem::Proc(second))) = (
                        i.checked_sub(2).and_then(|p| proc.items.get(p)),
                        i.checked_sub(1).and_then(|p| proc.items.get(p)),
                    ) else {
                        self.malformed_proc = true;
                        return false;
                    };
                    // True selects the **first** procedure.
                    let body = if self.pop_int() != 0 { first } else { second };
                    let _ = self.run(body);
                }
                ProcItem::Op(op) => self.apply(*op),
            }
        }
        true
    }

    /// One operator. Never fails: `DoOperator` has no error channel.
    #[expect(
        clippy::too_many_lines,
        reason = "the operator table is a flat dispatch by design: one arm \
                  per operator, each a line or two"
    )]
    fn apply(&mut self, op: PsOp) {
        match op {
            // Commutative operators pop `d1` first; order-sensitive ones pop
            // the right-hand operand first. The distinction only shows in a
            // stack trace, but it is what the C++ does.
            PsOp::Add => {
                let (d1, d2) = (self.pop(), self.pop());
                self.push(d1 + d2);
            }
            PsOp::Sub => {
                let (d2, d1) = (self.pop(), self.pop());
                self.push(d1 - d2);
            }
            PsOp::Mul => {
                let (d1, d2) = (self.pop(), self.pop());
                self.push(d1 * d2);
            }
            PsOp::Div => {
                let (d2, d1) = (self.pop(), self.pop());
                // Zero, not an infinity.
                self.push(if d2 == 0.0 { 0.0 } else { d1 / d2 });
            }
            PsOp::Idiv => {
                let (i2, i1) = (self.pop_int(), self.pop_int());
                self.push(from_int(i1.checked_div(i2).unwrap_or(0)));
            }
            PsOp::Mod => {
                let (i2, i1) = (self.pop_int(), self.pop_int());
                self.push(from_int(i1.checked_rem(i2).unwrap_or(0)));
            }
            PsOp::Neg => {
                let v = self.pop();
                self.push(-v);
            }
            PsOp::Abs => {
                let v = self.pop();
                self.push(v.abs());
            }
            PsOp::Ceiling => {
                let v = self.pop();
                self.push(v.ceil());
            }
            PsOp::Floor => {
                let v = self.pop();
                self.push(v.floor());
            }
            PsOp::Round => {
                let v = self.pop();
                self.push(round_half_up(v));
            }
            // A saturating integer round trip, not `trunc()`.
            PsOp::Truncate | PsOp::Cvi => {
                let v = self.pop_int();
                self.push(from_int(v));
            }
            PsOp::Sqrt => {
                let v = self.pop();
                self.push(v.sqrt());
            }
            PsOp::Sin => {
                let v = self.pop();
                self.push((v * PI / 180.0).sin());
            }
            PsOp::Cos => {
                let v = self.pop();
                self.push((v * PI / 180.0).cos());
            }
            PsOp::Atan => {
                let (d2, d1) = (self.pop(), self.pop());
                let mut r = d1.atan2(d2) * 180.0 / PI;
                if r < 0.0 {
                    r += 360.0;
                }
                self.push(r);
            }
            PsOp::Exp => {
                let (d2, d1) = (self.pop(), self.pop());
                self.push(d1.powf(d2));
            }
            PsOp::Ln => {
                let v = self.pop();
                self.push(v.ln());
            }
            PsOp::Log => {
                let v = self.pop();
                self.push(v.log10());
            }
            PsOp::Eq | PsOp::Ne | PsOp::Gt | PsOp::Ge | PsOp::Lt | PsOp::Le => {
                let (d2, d1) = (self.pop(), self.pop());
                #[expect(
                    clippy::float_cmp,
                    reason = "PostScript's `eq` and `ne` compare floats exactly"
                )]
                let truth = match op {
                    PsOp::Eq => d1 == d2,
                    PsOp::Ne => d1 != d2,
                    PsOp::Gt => d1 > d2,
                    PsOp::Ge => d1 >= d2,
                    PsOp::Lt => d1 < d2,
                    // `le`, plus the arms this match cannot reach.
                    _ => d1 <= d2,
                };
                self.push(if truth { 1.0 } else { 0.0 });
            }
            PsOp::And | PsOp::Or | PsOp::Xor => {
                let (i1, i2) = (self.pop_int(), self.pop_int());
                self.push(from_int(match op {
                    PsOp::And => i1 & i2,
                    PsOp::Or => i1 | i2,
                    _ => i1 ^ i2,
                }));
            }
            // Logical, yielding 0 or 1 — not a bitwise complement.
            PsOp::Not => {
                let v = self.pop_int();
                self.push(if v == 0 { 1.0 } else { 0.0 });
            }
            PsOp::Bitshift => {
                let shift = self.pop_int();
                let value = self.pop_int();
                let result = if shift > 0 {
                    u32::try_from(shift)
                        .ok()
                        .and_then(|s| value.checked_shl(s).filter(|_| s < 32))
                } else {
                    shift
                        .checked_neg()
                        .and_then(|s| u32::try_from(s).ok())
                        .and_then(|s| value.checked_shr(s).filter(|_| s < 32))
                };
                self.push(from_int(result.unwrap_or(0)));
            }
            PsOp::True => self.push(1.0),
            PsOp::False => self.push(0.0),
            PsOp::Pop => {
                self.pop();
            }
            PsOp::Exch => {
                let (a, b) = (self.pop(), self.pop());
                self.push(a);
                self.push(b);
            }
            PsOp::Dup => {
                let v = self.pop();
                self.push(v);
                self.push(v);
            }
            PsOp::Copy => {
                let n = self.pop_int();
                // Every out-of-range case is a silent no-op, with `n` already
                // consumed.
                let Ok(n) = usize::try_from(n) else { return };
                if n > self.count || self.count + n > STACK_SIZE {
                    return;
                }
                // Snapshot before pushing: the sources move under the
                // destination otherwise, and `2 copy` would duplicate the
                // first value twice.
                let mut source = [0f32; STACK_SIZE];
                for i in 0..n {
                    if let Some(slot) = source.get_mut(i) {
                        *slot = self.peek(n - i - 1);
                    }
                }
                for i in 0..n {
                    self.push(source.get(i).copied().unwrap_or(0.0));
                }
            }
            PsOp::Index => {
                let n = self.pop_int();
                let Ok(n) = usize::try_from(n) else { return };
                if n >= self.count {
                    // A no-op that pushes nothing, so the stack net-shrinks.
                    return;
                }
                let v = self.peek(n);
                self.push(v);
            }
            PsOp::Roll => {
                let j = self.pop_int();
                let n = self.pop_int();
                if j == 0 || n == 0 || self.count == 0 {
                    return;
                }
                let Ok(n) = usize::try_from(n) else { return };
                if n > self.count {
                    return;
                }
                // Normalize `j` into `(-n, 0]`, so a positive count rolls
                // towards the top.
                let n_i = i32::try_from(n).unwrap_or(i32::MAX);
                let mut j = j.checked_rem(n_i).unwrap_or(0);
                if j > 0 {
                    j -= n_i;
                }
                let Some(slice) = self
                    .count
                    .checked_sub(n)
                    .and_then(|from| self.stack.get_mut(from..self.count))
                else {
                    return;
                };
                let by = usize::try_from(-j).unwrap_or(0) % n.max(1);
                slice.rotate_left(by);
            }
            // `cvr` is genuinely nothing, since every value is already a
            // float; the other four are handled by `run`, which sees them in
            // the context they need.
            PsOp::Cvr | PsOp::If | PsOp::IfElse => {}
        }
    }
}

/// `saturated_cast<int>`: NaN reads as 0, everything else saturates.
fn saturate(v: f32) -> i32 {
    if v.is_nan() {
        return 0;
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "Rust's float-to-int cast already saturates, which is what \
                  saturated_cast does"
    )]
    let out = v as i32;
    out
}

/// The widening back to float, with the precision loss the C++ has.
#[expect(
    clippy::cast_precision_loss,
    reason = "matching the C++'s int-to-float widening"
)]
fn from_int(v: i32) -> f32 {
    v as f32
}

/// PDFium's rounding: **half-way values always go up**, so −5.5 rounds to
/// −5, not to −6. NaN becomes 0, and a value within half a unit of `f32::MAX`
/// saturates there rather than overflowing.
#[must_use]
pub(super) fn round_half_up(x: f32) -> f32 {
    if x.is_nan() {
        return 0.0;
    }
    if x > f32::MAX - 0.5 {
        return f32::MAX;
    }
    (x + 0.5).floor()
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

    use super::{round_half_up, saturate};

    #[test]
    fn rounding_is_half_up_not_half_even() {
        assert!((round_half_up(5.5) - 6.0).abs() < 1e-6);
        // The asymmetry: −5.5 rounds *up* to −5.
        assert!((round_half_up(-5.5) + 5.0).abs() < 1e-6);
        assert!((round_half_up(5.4) - 5.0).abs() < 1e-6);
        assert!((round_half_up(-5.6) + 6.0).abs() < 1e-6);
        assert!(round_half_up(f32::NAN).abs() < 1e-6);
        assert!((round_half_up(f32::MAX) - f32::MAX).abs() < 1e-6);
        assert!(
            round_half_up(f32::INFINITY).is_infinite() || round_half_up(f32::INFINITY) == f32::MAX
        );
    }

    #[test]
    fn integer_conversion_saturates_and_maps_nan_to_zero() {
        assert_eq!(saturate(5.9), 5);
        assert_eq!(saturate(-5.9), -5);
        assert_eq!(saturate(f32::NAN), 0);
        assert_eq!(saturate(1e30), i32::MAX);
        assert_eq!(saturate(-1e30), i32::MIN);
        // Truthiness truncates, so 0.5 reads as false.
        assert_eq!(saturate(0.5), 0);
    }
}
