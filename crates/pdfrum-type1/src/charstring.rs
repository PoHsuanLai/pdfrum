//! The Type 1 charstring interpreter (Type 1 specification §6).
//!
//! A charstring is a stack machine whose program is a byte string: values
//! 32–255 encode numbers, 0–31 encode operators, and `12 x` escapes to a
//! second operator page. The machine draws exactly one glyph and reports its
//! advance width.
//!
//! Three things make Type 1 charstrings more than a trivial decoder, and all
//! three are here because the Foxit fallback faces use them:
//!
//! - **`callothersubr`** is a callback into PostScript procedures the font
//!   ships. Numbers 0–3 are standardised (flex, hint replacement) and readers
//!   *emulate* them rather than run the PostScript; 14–18 are the Multiple
//!   Master blend, which folds `n` operand tuples into one using the weight
//!   vector. Results come back through `pop`.
//! - **`flex`** (othersubr 0/1/2) replaces seven `rmoveto`s that would
//!   otherwise be a jagged near-straight line with two curves.
//! - **`seac`** composes an accented glyph from two others named through
//!   `StandardEncoding`, positioned by the difference of their side bearings.
//!
//! The interpreter is a pure function of `(charstring bytes, environment)` to
//! a [`Glyph`]; it holds no state between glyphs.

use crate::blend::Blend;
use crate::encoding;
use pdfrum_common::kurbo::{BezPath, Point};

/// Maximum `callsubr` nesting. The specification says 10; broken fonts recurse
/// deeper and a cap is what stops a hostile one from exhausting the stack.
const MAX_DEPTH: u32 = 30;
/// The interpreter's operand stack is 48 entries in the specification; the
/// Multiple Master blend pushes up to `n_points * n_masters` at once, so this
/// is generous rather than exact.
const MAX_STACK: usize = 192;
/// Ceiling on emitted path segments, so a charstring that loops through
/// subroutines cannot grow a `BezPath` without bound.
const MAX_SEGMENTS: usize = 65_536;

/// Everything a charstring needs from the font around it.
///
/// A borrowed view rather than a reference to the font, because `seac` needs
/// to interpret *other* charstrings while the outer one is mid-flight — which
/// with a `&Type1Font` would be fine, but with the font's own outline cache in
/// scope would not.
#[derive(Clone, Copy)]
pub(crate) struct Env<'a> {
    /// The `/Subrs` array, already decrypted.
    pub subrs: &'a [Vec<u8>],
    /// Charstrings by glyph index, already decrypted.
    pub charstrings: &'a [Vec<u8>],
    /// Glyph index for a name — `seac` and nothing else.
    pub name_lookup: &'a dyn Fn(&str) -> Option<usize>,
    /// The active weight vector, empty for a non-Multiple-Master font.
    pub weights: &'a [f32],
    /// The font's Multiple-Master declaration, if any. Present so the
    /// interpreter can tell "no blend requested" from "blend requested with
    /// the wrong arity".
    pub blend: Option<&'a Blend>,
}

/// One interpreted glyph.
#[derive(Debug, Clone, PartialEq)]
pub struct Glyph {
    /// The outline in font units (before the `/FontMatrix`).
    pub path: BezPath,
    /// Advance width in font units, from `hsbw` or `sbw`.
    pub advance: f32,
    /// Left side bearing, which `seac` needs and metrics consumers want.
    pub left_side_bearing: f32,
}

/// Why interpretation stopped early. The partial path is kept regardless; this
/// only decides whether a diagnostic is recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Abort {
    /// Ran off the end without `endchar`.
    Truncated,
    /// An operator wanted operands that were not there.
    StackUnderflow,
    /// `callsubr` named a subroutine the font does not have.
    MissingSubr,
    /// Nesting cap hit.
    TooDeep,
    /// A blend was requested but the operand count did not match the weight
    /// vector.
    BadBlend,
    /// `seac` named a component the font does not have.
    BadSeac,
    /// The path grew past `MAX_SEGMENTS`.
    TooBig,
}

/// Interpret one charstring.
///
/// Returns the glyph and, when interpretation could not finish, why. The glyph
/// is always usable: a charstring that aborts halfway yields the outline built
/// so far, which is what a rasterizer wants and what FreeType produces.
pub(crate) fn interpret(code: &[u8], env: Env<'_>) -> (Glyph, Option<Abort>) {
    let mut m = Machine::new(env);
    let abort = m.run(code, 0).err();
    m.finish();
    (
        Glyph {
            path: m.path,
            advance: m.advance,
            left_side_bearing: m.lsb,
        },
        abort,
    )
}

struct Machine<'a> {
    env: Env<'a>,
    stack: Vec<f64>,
    /// The PostScript operand stack `callothersubr` results are `pop`ped from.
    ps_stack: Vec<f64>,
    path: BezPath,
    open: bool,
    current: Point,
    /// Where the current subpath started, so `closepath` is a real close.
    start: Point,
    advance: f32,
    lsb: f32,
    segments: usize,
    /// Non-`None` while a flex is being collected: the reference point plus
    /// the seven points othersubr 1/2 accumulate.
    flex: Option<Vec<Point>>,
    /// `seac` may only run once and ends the charstring.
    done: bool,
}

impl<'a> Machine<'a> {
    fn new(env: Env<'a>) -> Self {
        Self {
            env,
            stack: Vec::new(),
            ps_stack: Vec::new(),
            path: BezPath::new(),
            open: false,
            current: Point::ZERO,
            start: Point::ZERO,
            advance: 0.0,
            lsb: 0.0,
            segments: 0,
            flex: None,
            done: false,
        }
    }

    fn finish(&mut self) {
        if self.open {
            self.path.close_path();
            self.open = false;
        }
    }

    fn push(&mut self, v: f64) -> Result<(), Abort> {
        if self.stack.len() >= MAX_STACK {
            return Err(Abort::StackUnderflow);
        }
        self.stack.push(v);
        Ok(())
    }

    /// Take the last `n` operands, leaving the stack empty.
    ///
    /// Type 1 operators clear the stack, and — crucially — take their operands
    /// from the *bottom* when more were pushed than they need, because a
    /// hint-replacement subroutine leaves residue behind. FreeType reads from
    /// the bottom for the path operators; doing otherwise misplaces glyphs in
    /// exactly the fonts that use hint replacement.
    fn take(&mut self, n: usize) -> Result<Vec<f64>, Abort> {
        if self.stack.len() < n {
            self.stack.clear();
            return Err(Abort::StackUnderflow);
        }
        let args = self.stack.get(..n).unwrap_or_default().to_vec();
        self.stack.clear();
        Ok(args)
    }

    fn grow(&mut self) -> Result<(), Abort> {
        self.segments = self.segments.saturating_add(1);
        if self.segments > MAX_SEGMENTS {
            return Err(Abort::TooBig);
        }
        Ok(())
    }

    fn move_to(&mut self, p: Point) -> Result<(), Abort> {
        self.current = p;
        // Inside a flex, the seven `rmoveto`s are data points, not moves.
        if let Some(points) = self.flex.as_mut() {
            points.push(p);
            return Ok(());
        }
        self.grow()?;
        if self.open {
            self.path.close_path();
        }
        self.path.move_to(p);
        self.start = p;
        self.open = true;
        Ok(())
    }

    fn line_to(&mut self, p: Point) -> Result<(), Abort> {
        self.grow()?;
        if !self.open {
            self.path.move_to(self.current);
            self.start = self.current;
            self.open = true;
        }
        self.current = p;
        self.path.line_to(p);
        Ok(())
    }

    fn curve_to(&mut self, c1: Point, c2: Point, p: Point) -> Result<(), Abort> {
        self.grow()?;
        if !self.open {
            self.path.move_to(self.current);
            self.start = self.current;
            self.open = true;
        }
        self.current = p;
        self.path.curve_to(c1, c2, p);
        Ok(())
    }

    fn close(&mut self) {
        // Type 1's `closepath` closes the subpath but leaves the current point
        // where it is — a following `rmoveto` is relative to that, not to the
        // subpath start. Getting this wrong shifts every glyph with more than
        // one contour.
        if self.open {
            self.path.close_path();
            self.open = false;
        }
    }

    fn run(&mut self, code: &[u8], depth: u32) -> Result<(), Abort> {
        if depth > MAX_DEPTH {
            return Err(Abort::TooDeep);
        }
        let mut i = 0usize;
        while let Some(&b) = code.get(i) {
            i = i.saturating_add(1);
            match b {
                // Number encodings (Type 1 specification §6.2).
                32..=246 => self.push(f64::from(i16::from(b) - 139))?,
                247..=250 => {
                    let w = *code.get(i).ok_or(Abort::Truncated)?;
                    i = i.saturating_add(1);
                    self.push(f64::from((i16::from(b) - 247) * 256 + i16::from(w) + 108))?;
                }
                251..=254 => {
                    let w = *code.get(i).ok_or(Abort::Truncated)?;
                    i = i.saturating_add(1);
                    self.push(f64::from(-(i16::from(b) - 251) * 256 - i16::from(w) - 108))?;
                }
                255 => {
                    let bytes = code.get(i..i.saturating_add(4)).ok_or(Abort::Truncated)?;
                    i = i.saturating_add(4);
                    let mut v: i32 = 0;
                    for &x in bytes {
                        v = (v << 8) | i32::from(x);
                    }
                    self.push(f64::from(v))?;
                }
                12 => {
                    let esc = *code.get(i).ok_or(Abort::Truncated)?;
                    i = i.saturating_add(1);
                    if self.escaped(esc, depth)? {
                        return Ok(());
                    }
                }
                _ => {
                    if self.operator(b, code, &mut i, depth)? {
                        return Ok(());
                    }
                }
            }
            if self.done {
                return Ok(());
            }
        }
        // Falling off the end without `endchar` or `return` is how a great
        // many real subroutines finish; only a top-level charstring doing it
        // is worth reporting.
        if depth == 0 {
            return Err(Abort::Truncated);
        }
        Ok(())
    }

    /// One single-byte operator. `Ok(true)` means "stop this charstring".
    fn operator(&mut self, op: u8, code: &[u8], i: &mut usize, depth: u32) -> Result<bool, Abort> {
        match op {
            // hstem / vstem: hints, which we do not apply (we render filled
            // unhinted outlines) but must consume so the stack is clean.
            1 | 3 => {
                self.stack.clear();
            }
            4 => {
                // vmoveto: dy
                let a = self.take(1)?;
                let dy = a.first().copied().unwrap_or(0.0);
                self.move_to(Point::new(self.current.x, self.current.y + dy))?;
            }
            5 => {
                let a = self.take(2)?;
                let (dx, dy) = (arg(&a, 0), arg(&a, 1));
                self.line_to(Point::new(self.current.x + dx, self.current.y + dy))?;
            }
            6 => {
                let a = self.take(1)?;
                self.line_to(Point::new(self.current.x + arg(&a, 0), self.current.y))?;
            }
            7 => {
                let a = self.take(1)?;
                self.line_to(Point::new(self.current.x, self.current.y + arg(&a, 0)))?;
            }
            8 => {
                let a = self.take(6)?;
                self.relative_curve(
                    arg(&a, 0),
                    arg(&a, 1),
                    arg(&a, 2),
                    arg(&a, 3),
                    arg(&a, 4),
                    arg(&a, 5),
                )?;
            }
            9 => {
                self.stack.clear();
                self.close();
            }
            10 => {
                // callsubr: the index is the *last* operand, since a hint
                // replacement leaves `subr# 4 callothersubr pop callsubr` with
                // extra values below it.
                let idx = self.stack.pop().ok_or(Abort::StackUnderflow)?;
                let sub = usize::try_from(idx as i64)
                    .ok()
                    .and_then(|n| self.env.subrs.get(n))
                    .ok_or(Abort::MissingSubr)?;
                // Cloning the subroutine avoids borrowing `self.env` across
                // the `&mut self` call. Subroutines are tens of bytes.
                let sub = sub.clone();
                self.run(&sub, depth.saturating_add(1))?;
            }
            11 => return Ok(true), // return
            13 => {
                // hsbw: sbx wx
                let a = self.take(2)?;
                self.lsb = arg(&a, 0) as f32;
                self.advance = arg(&a, 1) as f32;
                // The origin moves to the left side bearing; every subsequent
                // relative move is from there.
                self.current = Point::new(arg(&a, 0), 0.0);
                self.start = self.current;
            }
            14 => {
                self.done = true;
                return Ok(true); // endchar
            }
            21 => {
                let a = self.take(2)?;
                self.move_to(Point::new(
                    self.current.x + arg(&a, 0),
                    self.current.y + arg(&a, 1),
                ))?;
            }
            22 => {
                let a = self.take(1)?;
                self.move_to(Point::new(self.current.x + arg(&a, 0), self.current.y))?;
            }
            30 => {
                // vhcurveto: dy1 dx2 dy2 dx3
                let a = self.take(4)?;
                self.relative_curve(0.0, arg(&a, 0), arg(&a, 1), arg(&a, 2), arg(&a, 3), 0.0)?;
            }
            31 => {
                // hvcurveto: dx1 dx2 dy2 dy3
                let a = self.take(4)?;
                self.relative_curve(arg(&a, 0), 0.0, arg(&a, 1), arg(&a, 2), 0.0, arg(&a, 3))?;
            }
            // 0, 2, 15..=20, 23..=29 are reserved. A font using one is
            // damaged; clearing the stack and carrying on recovers more than
            // aborting does, and matches FreeType's "unknown operator" arm.
            _ => {
                let _ = (code, i);
                self.stack.clear();
            }
        }
        Ok(false)
    }

    /// A `12 x` two-byte operator.
    fn escaped(&mut self, esc: u8, depth: u32) -> Result<bool, Abort> {
        match esc {
            6 => {
                // seac: asb adx ady bchar achar
                let a = self.take(5)?;
                self.seac(
                    arg(&a, 0),
                    arg(&a, 1),
                    arg(&a, 2),
                    arg(&a, 3),
                    arg(&a, 4),
                    depth,
                )?;
                self.done = true;
                return Ok(true);
            }
            7 => {
                // sbw: sbx sby wx wy
                let a = self.take(4)?;
                self.lsb = arg(&a, 0) as f32;
                self.advance = arg(&a, 2) as f32;
                self.current = Point::new(arg(&a, 0), arg(&a, 1));
                self.start = self.current;
            }
            12 => {
                // div: divisor stays on the stack, so this does *not* clear.
                let b = self.stack.pop().ok_or(Abort::StackUnderflow)?;
                let a = self.stack.pop().ok_or(Abort::StackUnderflow)?;
                self.push(if b == 0.0 { 0.0 } else { a / b })?;
            }
            16 => self.call_other_subr()?,
            17 => {
                // pop: take one result the othersubr left behind. A font that
                // pops more than was produced gets zero, which is what
                // FreeType hands back and keeps hint replacement working.
                let v = self.ps_stack.pop().unwrap_or(0.0);
                self.push(v)?;
            }
            33 => {
                // setcurrentpoint: x y — absolute, and used to resynchronise
                // after an othersubr moved the point out from under us.
                let a = self.take(2)?;
                self.current = Point::new(arg(&a, 0), arg(&a, 1));
            }
            // Everything else. `dotsection` (0), `vstem3` (1) and `hstem3`
            // (2) are hinting directives that we deliberately ignore — we
            // render filled unhinted outlines — and the rest are reserved.
            // Both cases must still *consume* their operands, or the next
            // path operator would read them as its own.
            _ => self.stack.clear(),
        }
        Ok(false)
    }

    /// `callothersubr`: `arg1 … argn n othersubr# callothersubr`.
    fn call_other_subr(&mut self) -> Result<(), Abort> {
        let index = self.stack.pop().ok_or(Abort::StackUnderflow)? as i64;
        let count = self.stack.pop().ok_or(Abort::StackUnderflow)?;
        let count = usize::try_from(count as i64).unwrap_or(0);
        if self.stack.len() < count {
            self.stack.clear();
            return Err(Abort::StackUnderflow);
        }
        let base = self.stack.len().saturating_sub(count);
        let args: Vec<f64> = self.stack.get(base..).unwrap_or_default().to_vec();
        self.stack.truncate(base);

        match index {
            0 => self.end_flex(&args),
            1 => {
                // Start collecting flex points; the reference point comes from
                // the first of the seven `rmoveto`s that follow.
                self.flex = Some(Vec::new());
                Ok(())
            }
            2 => Ok(()), // collect: the rmoveto handler is doing the work
            3 => {
                // Hint replacement. The font expects `3` back so the following
                // `pop callsubr` calls subr 3, which is a no-op by convention.
                self.ps_stack.push(3.0);
                Ok(())
            }
            14..=18 => self.blend(index, &args),
            _ => {
                // An othersubr we do not emulate. The convention is that its
                // arguments come straight back through `pop`, which keeps a
                // font using a private othersubr renderable.
                self.ps_stack.extend(args.iter().rev().copied());
                Ok(())
            }
        }
    }

    /// othersubr 0 — end of flex. Seven collected points become two curves.
    ///
    /// The published idiom is `flex_height end_x end_y 3 0 callothersubr`,
    /// followed by `pop pop setcurrentpoint`. The three arguments are advisory
    /// (the height is a hinting hint and the end point restates the seventh
    /// collected point), so the outline is built entirely from the collected
    /// points; the two values pushed back are what the following
    /// `setcurrentpoint` consumes.
    fn end_flex(&mut self, args: &[f64]) -> Result<(), Abort> {
        let points = self.flex.take().unwrap_or_default();
        // The first collected point is the flex *reference* point, which only
        // exists to tell a hinter how far the curve strays from a line; the
        // outline uses the six after it.
        let p = |n: usize| points.get(n).copied();
        let end = if let (Some(c1), Some(c2), Some(mid), Some(c3), Some(c4), Some(end)) =
            (p(1), p(2), p(3), p(4), p(5), p(6))
        {
            self.curve_to(c1, c2, mid)?;
            self.curve_to(c3, c4, end)?;
            self.current = end;
            end
        } else {
            // Too few points collected — a truncated flex. Fall back to a
            // straight line to wherever the last point was, so the contour
            // stays closed.
            if let Some(last) = points.last().copied() {
                self.line_to(last)?;
            }
            self.current
        };
        // Pushed y first so the font's `pop pop` reads x then y, which is the
        // order `setcurrentpoint` wants.
        self.ps_stack.push(end.y);
        self.ps_stack.push(end.x);
        let _ = args;
        Ok(())
    }

    /// othersubr 14–18 — the Multiple Master blend.
    ///
    /// The operand block is `k` base values followed by `k * (m-1)` deltas,
    /// where `k` is the point count this othersubr number implies and `m` the
    /// number of masters. Each result is `base + Σ delta_j * weight_{j+1}` —
    /// the first weight is not applied, because the base value *is* the first
    /// master's value.
    fn blend(&mut self, index: i64, args: &[f64]) -> Result<(), Abort> {
        let masters = self.env.weights.len();
        if masters < 2 || self.env.blend.is_none() {
            // Not a Multiple Master font, or its declaration was rejected.
            // Handing the arguments back is the only recovery that keeps a
            // charstring's arithmetic consistent.
            self.ps_stack.extend(args.iter().rev().copied());
            return Ok(());
        }
        // 14→1 point, 15→2, 16→3, 17→4, 18→6. The jump at 18 is in the
        // specification: it blends the six values of an `rrcurveto`.
        let points = match index {
            14 => 1usize,
            15 => 2,
            16 => 3,
            17 => 4,
            18 => 6,
            _ => return Err(Abort::BadBlend),
        };
        if args.len() != points.saturating_mul(masters) {
            return Err(Abort::BadBlend);
        }
        let mut blended = Vec::with_capacity(points);
        for k in 0..points {
            let mut v = args.get(k).copied().unwrap_or(0.0);
            for (j, &w) in self.env.weights.iter().enumerate().skip(1) {
                let delta = args
                    .get(
                        points
                            .saturating_add(k.saturating_mul(masters.saturating_sub(1)))
                            .saturating_add(j.saturating_sub(1)),
                    )
                    .copied()
                    .unwrap_or(0.0);
                v += delta * f64::from(w);
            }
            blended.push(v);
        }
        // Results come back through `pop`, last-pushed-first, so the font's
        // run of `pop`s reads them left to right.
        self.ps_stack.extend(blended.iter().rev().copied());
        Ok(())
    }

    /// `seac` — compose from two `StandardEncoding`-named glyphs.
    fn seac(
        &mut self,
        asb: f64,
        adx: f64,
        ady: f64,
        bchar: f64,
        achar: f64,
        depth: u32,
    ) -> Result<(), Abort> {
        let code = |v: f64| u8::try_from(v as i64).ok();
        let lookup = |v: f64| -> Option<Vec<u8>> {
            let name = encoding::standard_encoding_name(code(v)?)?;
            let gid = (self.env.name_lookup)(name)?;
            self.env.charstrings.get(gid).cloned()
        };
        let (Some(base), Some(accent)) = (lookup(bchar), lookup(achar)) else {
            return Err(Abort::BadSeac);
        };

        // The base is drawn at the origin, keeping its own side bearing and
        // advance.
        let outer_lsb = self.lsb;
        self.reset_for_component();
        self.run(&base, depth.saturating_add(1))?;
        self.finish();
        let base_advance = self.advance;

        // The accent is drawn shifted. `asb` is the accent's side bearing as
        // the *composing* glyph believed it to be, so the true offset corrects
        // for any disagreement with the accent's own `hsbw`.
        let accent_start = self.path.elements().len();
        self.reset_for_component();
        self.run(&accent, depth.saturating_add(1))?;
        self.finish();
        let shift = kurbo_translate(f64::from(outer_lsb) - asb + adx, ady);
        translate_from(&mut self.path, accent_start, shift);

        self.advance = base_advance;
        self.lsb = outer_lsb;
        Ok(())
    }

    /// Between `seac` components: keep the accumulated path, reset the pen.
    fn reset_for_component(&mut self) {
        self.stack.clear();
        self.ps_stack.clear();
        self.flex = None;
        self.open = false;
        self.current = Point::ZERO;
        self.start = Point::ZERO;
        self.done = false;
    }

    fn relative_curve(
        &mut self,
        dx1: f64,
        dy1: f64,
        dx2: f64,
        dy2: f64,
        dx3: f64,
        dy3: f64,
    ) -> Result<(), Abort> {
        let c1 = Point::new(self.current.x + dx1, self.current.y + dy1);
        let c2 = Point::new(c1.x + dx2, c1.y + dy2);
        let end = Point::new(c2.x + dx3, c2.y + dy3);
        self.curve_to(c1, c2, end)
    }
}

fn arg(args: &[f64], i: usize) -> f64 {
    args.get(i).copied().unwrap_or(0.0)
}

fn kurbo_translate(dx: f64, dy: f64) -> (f64, f64) {
    (dx, dy)
}

/// Shift every path element from `from` onward. `BezPath` has no range
/// transform, so this rebuilds the tail.
fn translate_from(path: &mut BezPath, from: usize, (dx, dy): (f64, f64)) {
    use pdfrum_common::kurbo::PathEl;
    let shift = |p: Point| Point::new(p.x + dx, p.y + dy);
    let tail: Vec<PathEl> = path
        .elements()
        .get(from..)
        .unwrap_or_default()
        .iter()
        .map(|el| match *el {
            PathEl::MoveTo(p) => PathEl::MoveTo(shift(p)),
            PathEl::LineTo(p) => PathEl::LineTo(shift(p)),
            PathEl::QuadTo(a, b) => PathEl::QuadTo(shift(a), shift(b)),
            PathEl::CurveTo(a, b, c) => PathEl::CurveTo(shift(a), shift(b), shift(c)),
            PathEl::ClosePath => PathEl::ClosePath,
        })
        .collect();
    path.truncate(from);
    for el in tail {
        path.push(el);
    }
}

#[cfg(test)]
#[allow(
    clippy::indexing_slicing,
    clippy::float_cmp,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::similar_names
)]
mod tests {
    use super::{Abort, Env, Glyph, interpret};
    use crate::eexec;
    use pdfrum_common::kurbo::Shape;

    /// Assemble a charstring from a readable description.
    ///
    /// Numbers are encoded the way a real font would, so the tests exercise
    /// the number decoder as well as the operators.
    fn cs(items: &[Item]) -> Vec<u8> {
        let mut out = Vec::new();
        for it in items {
            match *it {
                Item::N(v) => encode_number(&mut out, v),
                Item::Op(o) => out.push(o),
                Item::Esc(o) => out.extend_from_slice(&[12, o]),
            }
        }
        out
    }

    #[derive(Clone, Copy)]
    enum Item {
        N(i32),
        Op(u8),
        Esc(u8),
    }
    use Item::{Esc, N, Op};

    fn encode_number(out: &mut Vec<u8>, v: i32) {
        match v {
            -107..=107 => out.push((v + 139) as u8),
            108..=1131 => {
                let v = v - 108;
                out.push(((v >> 8) + 247) as u8);
                out.push((v & 0xFF) as u8);
            }
            -1131..=-108 => {
                let v = -v - 108;
                out.push(((v >> 8) + 251) as u8);
                out.push((v & 0xFF) as u8);
            }
            _ => {
                out.push(255);
                out.extend_from_slice(&v.to_be_bytes());
            }
        }
    }

    fn no_names(_: &str) -> Option<usize> {
        None
    }

    fn run(code: &[u8]) -> (Glyph, Option<Abort>) {
        run_with(code, &[], &[])
    }

    fn run_with(code: &[u8], subrs: &[Vec<u8>], charstrings: &[Vec<u8>]) -> (Glyph, Option<Abort>) {
        interpret(
            code,
            Env {
                subrs,
                charstrings,
                name_lookup: &no_names,
                weights: &[],
                blend: None,
            },
        )
    }

    #[test]
    fn hsbw_sets_the_origin_and_advance() {
        let (g, abort) = run(&cs(&[N(50), N(600), Op(13), Op(14)]));
        assert_eq!(abort, None);
        assert_eq!(g.advance, 600.0);
        assert_eq!(g.left_side_bearing, 50.0);
        // Nothing was drawn, but the pen sits at the side bearing: a following
        // rmoveto is relative to (50, 0).
        let (g, _) = run(&cs(&[
            N(50),
            N(600),
            Op(13),
            N(0),
            N(0),
            Op(21),
            N(10),
            Op(6),
            Op(9),
            Op(14),
        ]));
        assert_eq!(g.path.bounding_box().x0, 50.0);
        assert_eq!(g.path.bounding_box().x1, 60.0);
    }

    #[test]
    fn a_square_from_the_line_operators() {
        // rlineto, hlineto, vlineto and closepath.
        let (g, abort) = run(&cs(&[
            N(0),
            N(1000),
            Op(13),
            N(100),
            N(100),
            Op(21), // rmoveto
            N(200),
            Op(6), // hlineto
            N(200),
            Op(7), // vlineto
            N(-200),
            N(0),
            Op(5), // rlineto
            Op(9), // closepath
            Op(14),
        ]));
        assert_eq!(abort, None);
        let bb = g.path.bounding_box();
        assert_eq!((bb.x0, bb.y0, bb.x1, bb.y1), (100.0, 100.0, 300.0, 300.0));
        assert_eq!(g.path.elements().len(), 5); // move + 3 lines + close
    }

    #[test]
    fn rrcurveto_vhcurveto_hvcurveto_place_control_points() {
        use pdfrum_common::kurbo::{PathEl, Point};
        let (g, _) = run(&cs(&[
            N(0),
            N(1000),
            Op(13),
            N(0),
            N(0),
            Op(21),
            N(10),
            N(20),
            N(30),
            N(40),
            N(50),
            N(60),
            Op(8), // rrcurveto
            N(10),
            N(20),
            N(30),
            N(40),
            Op(30), // vhcurveto
            N(10),
            N(20),
            N(30),
            N(40),
            Op(31), // hvcurveto
            Op(14),
        ]));
        let els = g.path.elements();
        // rrcurveto from (0,0): c1=(10,20) c2=(40,60) end=(90,120)
        assert_eq!(
            els.get(1),
            Some(&PathEl::CurveTo(
                Point::new(10.0, 20.0),
                Point::new(40.0, 60.0),
                Point::new(90.0, 120.0)
            ))
        );
        // vhcurveto dy1=10 dx2=20 dy2=30 dx3=40, from (90,120):
        // c1=(90,130) c2=(110,160) end=(150,160)
        assert_eq!(
            els.get(2),
            Some(&PathEl::CurveTo(
                Point::new(90.0, 130.0),
                Point::new(110.0, 160.0),
                Point::new(150.0, 160.0)
            ))
        );
        // hvcurveto dx1=10 dx2=20 dy2=30 dy3=40, from (150,160):
        // c1=(160,160) c2=(180,190) end=(180,230)
        assert_eq!(
            els.get(3),
            Some(&PathEl::CurveTo(
                Point::new(160.0, 160.0),
                Point::new(180.0, 190.0),
                Point::new(180.0, 230.0)
            ))
        );
    }

    #[test]
    fn callsubr_and_return() {
        let subrs = vec![
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            cs(&[N(100), Op(6), Op(11)]), // subr 4: hlineto 100, return
        ];
        let (g, abort) = run_with(
            &cs(&[
                N(0),
                N(1000),
                Op(13),
                N(0),
                N(0),
                Op(21),
                N(4),
                Op(10),
                Op(14),
            ]),
            &subrs,
            &[],
        );
        assert_eq!(abort, None);
        assert_eq!(g.path.bounding_box().x1, 100.0);
    }

    #[test]
    fn a_missing_subr_aborts_but_keeps_the_path() {
        let (g, abort) = run(&cs(&[
            N(0),
            N(1000),
            Op(13),
            N(0),
            N(0),
            Op(21),
            N(50),
            Op(6),
            N(99),
            Op(10),
            Op(14),
        ]));
        assert_eq!(abort, Some(Abort::MissingSubr));
        assert_eq!(g.path.bounding_box().x1, 50.0);
    }

    #[test]
    fn div_leaves_a_quotient_on_the_stack() {
        let (g, _) = run(&cs(&[
            N(0),
            N(1000),
            Op(13),
            N(0),
            N(0),
            Op(21),
            N(300),
            N(3),
            Esc(12),
            Op(6),
            Op(14),
        ]));
        assert_eq!(g.path.bounding_box().x1, 100.0);
        // Division by zero yields zero rather than an infinity that would
        // poison the outline.
        let (g, _) = run(&cs(&[
            N(0),
            N(1000),
            Op(13),
            N(0),
            N(0),
            Op(21),
            N(300),
            N(0),
            Esc(12),
            Op(6),
            Op(14),
        ]));
        assert!(g.path.bounding_box().x1.is_finite());
    }

    #[test]
    fn setcurrentpoint_is_absolute() {
        let (g, _) = run(&cs(&[
            N(0),
            N(1000),
            Op(13),
            N(0),
            N(0),
            Op(21),
            N(500),
            N(500),
            Esc(33), // setcurrentpoint 500 500
            N(10),
            Op(6),
            Op(14),
        ]));
        let bb = g.path.bounding_box();
        assert_eq!((bb.x1, bb.y1), (510.0, 500.0));
    }

    #[test]
    fn flex_becomes_two_curves() {
        use pdfrum_common::kurbo::PathEl;
        // The canonical shape: othersubr 1, seven rmovetos, othersubr 0.
        let mut items = vec![N(0), N(1000), Op(13), N(0), N(0), Op(21)];
        items.extend([N(0), N(1), Esc(16)]); // 0 1 callothersubr (start flex)
        for (dx, dy) in [
            (10, 10),
            (10, 10),
            (10, 10),
            (10, 10),
            (10, 10),
            (10, 10),
            (10, 10),
        ] {
            items.extend([N(0), N(2), Esc(16), N(dx), N(dy), Op(21)]);
        }
        // flex_height end_x end_y 3 0 callothersubr, then pop pop setcurrentpoint.
        items.extend([N(50), N(70), N(70), N(3), N(0), Esc(16)]);
        items.extend([Esc(17), Esc(17), Esc(33)]);
        items.push(Op(14));
        let (g, abort) = run(&cs(&items));
        assert_eq!(abort, None);
        // Two curves, no intervening moves: the seven rmovetos were absorbed.
        let curves = g
            .path
            .elements()
            .iter()
            .filter(|e| matches!(e, PathEl::CurveTo(..)))
            .count();
        assert_eq!(curves, 2);
        assert_eq!(
            g.path
                .elements()
                .iter()
                .filter(|e| matches!(e, PathEl::MoveTo(_)))
                .count(),
            1
        );
    }

    #[test]
    fn hint_replacement_returns_three_and_calls_the_subr() {
        // `subr# 1 3 callothersubr pop callsubr` is the published idiom; the
        // `3` that comes back through `pop` selects subr 3, conventionally a
        // no-op, and the outline must be unaffected.
        let subrs = vec![Vec::new(), Vec::new(), Vec::new(), cs(&[Op(11)])];
        let (g, abort) = run_with(
            &cs(&[
                N(0),
                N(1000),
                Op(13),
                N(0),
                N(0),
                Op(21),
                N(3),
                N(1),
                N(3),
                Esc(16), // 3 1 3 callothersubr
                Esc(17),
                Op(10), // pop callsubr
                N(70),
                Op(6),
                Op(14),
            ]),
            &subrs,
            &[],
        );
        assert_eq!(abort, None);
        assert_eq!(g.path.bounding_box().x1, 70.0);
    }

    #[test]
    fn seac_composes_two_glyphs_with_the_side_bearing_correction() {
        // Glyph 1 is the base (a unit square at x=0), glyph 2 the accent
        // (a unit square at x=0 with its own side bearing of 20).
        let base = cs(&[
            N(0),
            N(500),
            Op(13),
            N(0),
            N(0),
            Op(21),
            N(100),
            Op(6),
            N(100),
            Op(7),
            Op(9),
            Op(14),
        ]);
        let accent = cs(&[
            N(20),
            N(300),
            Op(13),
            N(0),
            N(0),
            Op(21),
            N(50),
            Op(6),
            N(50),
            Op(7),
            Op(9),
            Op(14),
        ]);
        let charstrings = vec![Vec::new(), base, accent];
        // StandardEncoding: 'A' is 65, 'B' is 66.
        let lookup = |n: &str| match n {
            "A" => Some(1usize),
            "B" => Some(2usize),
            _ => None,
        };
        let (g, abort) = interpret(
            // 0 500 hsbw  20 300 100 65 66 seac
            &cs(&[
                N(0),
                N(500),
                Op(13),
                N(20),
                N(300),
                N(100),
                N(65),
                N(66),
                Esc(6),
            ]),
            Env {
                subrs: &[],
                charstrings: &charstrings,
                name_lookup: &lookup,
                weights: &[],
                blend: None,
            },
        );
        assert_eq!(abort, None);
        // The base keeps the composite's advance. The accent draws itself from
        // its own side bearing (20) and is then shifted by
        // `adx - asb + outer_lsb` = 300 - 20 + 0 = 280, so its 50-wide box
        // spans 300..350 in x and 100..150 in y.
        assert_eq!(g.advance, 500.0);
        let bb = g.path.bounding_box();
        assert_eq!((bb.x0, bb.y0), (0.0, 0.0));
        assert_eq!((bb.x1, bb.y1), (350.0, 150.0));
    }

    #[test]
    fn seac_naming_a_missing_component_aborts_cleanly() {
        let (_, abort) = run(&cs(&[
            N(0),
            N(500),
            Op(13),
            N(0),
            N(0),
            N(0),
            N(65),
            N(66),
            Esc(6),
        ]));
        assert_eq!(abort, Some(Abort::BadSeac));
    }

    #[test]
    fn multiple_master_blend_interpolates_the_operands() {
        use crate::blend::{AxisKind, Blend, DesignMap};
        use pdfrum_common::Diagnostics;
        let mut d = Diagnostics::default();
        let blend = Blend::new(
            vec![AxisKind::Weight],
            vec![vec![0.0], vec![1.0]],
            vec![DesignMap {
                knots: vec![(0.0, 0.0), (1.0, 1.0)],
            }],
            vec![0.5, 0.5],
            &mut d,
        )
        .expect("consistent");

        // Two masters, one point: `base delta 1 14 callothersubr pop`.
        // At weight (0.25, 0.75) the result is base + delta * 0.75.
        let code = cs(&[
            N(0),
            N(1000),
            Op(13),
            N(0),
            N(0),
            Op(21),
            N(100),
            N(200),
            N(2),
            N(14),
            Esc(16), // 100 200 2 14 callothersubr
            Esc(17), // pop -> 100 + 200*0.75 = 250
            Op(6),   // hlineto
            Op(14),
        ]);
        let (g, abort) = interpret(
            &code,
            Env {
                subrs: &[],
                charstrings: &[],
                name_lookup: &no_names,
                weights: &[0.25, 0.75],
                blend: Some(&blend),
            },
        );
        assert_eq!(abort, None);
        assert!((g.path.bounding_box().x1 - 250.0).abs() < 1e-9);

        // The same charstring at the other extreme moves the outline.
        let (g2, _) = interpret(
            &code,
            Env {
                subrs: &[],
                charstrings: &[],
                name_lookup: &no_names,
                weights: &[1.0, 0.0],
                blend: Some(&blend),
            },
        );
        assert!((g2.path.bounding_box().x1 - 100.0).abs() < 1e-9);
    }

    #[test]
    fn a_blend_with_the_wrong_arity_aborts() {
        use crate::blend::{AxisKind, Blend, DesignMap};
        use pdfrum_common::Diagnostics;
        let mut d = Diagnostics::default();
        let blend = Blend::new(
            vec![AxisKind::Weight],
            vec![vec![0.0], vec![1.0]],
            vec![DesignMap {
                knots: vec![(0.0, 0.0), (1.0, 1.0)],
            }],
            vec![0.5, 0.5],
            &mut d,
        )
        .expect("consistent");
        // othersubr 14 wants 1 point x 2 masters = 2 operands; give it 3.
        let (_, abort) = interpret(
            &cs(&[
                N(0),
                N(1000),
                Op(13),
                N(1),
                N(2),
                N(3),
                N(3),
                N(14),
                Esc(16),
                Op(14),
            ]),
            Env {
                subrs: &[],
                charstrings: &[],
                name_lookup: &no_names,
                weights: &[0.5, 0.5],
                blend: Some(&blend),
            },
        );
        assert_eq!(abort, Some(Abort::BadBlend));
    }

    #[test]
    fn closepath_leaves_the_current_point_alone() {
        // Two contours: the second `rmoveto` is relative to where the pen was
        // when `closepath` ran, not to the first contour's start.
        let (g, _) = run(&cs(&[
            N(0),
            N(1000),
            Op(13),
            N(100),
            N(0),
            Op(21),
            N(100),
            Op(6), // pen at (200, 0)
            Op(9), // closepath
            N(50),
            N(50),
            Op(21), // -> (250, 50), not (150, 50)
            N(10),
            Op(6),
            Op(14),
        ]));
        let bb = g.path.bounding_box();
        assert_eq!(bb.x1, 260.0);
    }

    #[test]
    fn recursion_is_capped() {
        // Subr 0 calls itself forever.
        let subrs = vec![cs(&[N(0), Op(10), Op(11)])];
        let (_, abort) = run_with(
            &cs(&[N(0), N(1000), Op(13), N(0), Op(10), Op(14)]),
            &subrs,
            &[],
        );
        assert_eq!(abort, Some(Abort::TooDeep));
    }

    #[test]
    fn unknown_operators_clear_the_stack_and_carry_on() {
        let (g, abort) = run(&cs(&[
            N(0),
            N(1000),
            Op(13),
            N(0),
            N(0),
            Op(21),
            N(1),
            N(2),
            Op(23),
            N(80),
            Op(6),
            Op(14),
        ]));
        assert_eq!(abort, None);
        assert_eq!(g.path.bounding_box().x1, 80.0);
    }

    #[test]
    fn the_four_byte_number_encoding_round_trips() {
        let (g, _) = run(&cs(&[
            N(0),
            N(1000),
            Op(13),
            N(0),
            N(0),
            Op(21),
            N(70000),
            Op(6),
            Op(14),
        ]));
        assert_eq!(g.path.bounding_box().x1, 70000.0);
        let (g, _) = run(&cs(&[
            N(0),
            N(1000),
            Op(13),
            N(0),
            N(0),
            Op(21),
            N(-70000),
            Op(6),
            Op(14),
        ]));
        assert_eq!(g.path.bounding_box().x0, -70000.0);
    }

    #[test]
    fn a_truncated_charstring_reports_it() {
        // 255 needs four more bytes; only two are there.
        let (_, abort) = run(&[255, 0, 1]);
        assert_eq!(abort, Some(Abort::Truncated));
        // And an entirely empty one.
        let (g, abort) = run(&[]);
        assert_eq!(abort, Some(Abort::Truncated));
        assert!(g.path.is_empty());
    }

    #[test]
    fn encrypted_charstrings_decode_through_the_cipher() {
        // The integration the real font takes: charstrings arrive encrypted
        // with seed 4330 and four bytes of lead-in.
        let plain = {
            let mut v = vec![0u8; 4];
            v.extend(cs(&[
                N(0),
                N(1000),
                Op(13),
                N(0),
                N(0),
                Op(21),
                N(42),
                Op(6),
                Op(14),
            ]));
            v
        };
        let cipher = eexec::encrypt(&plain, eexec::CHARSTRING_SEED);
        let decoded = eexec::decrypt(&cipher, eexec::CHARSTRING_SEED, 4);
        let (g, abort) = run(&decoded);
        assert_eq!(abort, None);
        assert_eq!(g.path.bounding_box().x1, 42.0);
    }
}
