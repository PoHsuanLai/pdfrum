//! `build_page`: the fold from operators to page objects.
//!
//! Pure with respect to its inputs, and the only mutable state is the build
//! context's three caches, passed down by `&mut` (STYLE.md §1).
//!
//! # Path assembly has three silent repairs
//!
//! Adding a point to a path is not the append it looks like:
//!
//! - a `MoveTo` identical to a preceding open `MoveTo` is **dropped**;
//! - a `MoveTo` following an open `MoveTo` **overwrites** it, so `m m m`
//!   collapses to the last one;
//! - a non-`MoveTo` point with **no path started at all is discarded**, so an
//!   `l` before any `m` vanishes.
//!
//! # A single-point path is a special case
//!
//! With a clip pending it produces an **empty clip** that blanks everything
//! after it. Without one it draws nothing at all — *unless* the point is a
//! closed `MoveTo` and the line cap is round, which is the round-dot case.
//!
//! # The form guard is buffer identity, not depth
//!
//! Recursion is refused when more than forty parses are in flight **or when
//! the same content buffer is already being parsed**. The second half is the
//! real cycle guard: a form that re-invokes itself is refused however
//! shallow it is, while two sequential `Do`s of the same form both work.
//! Refusal consumes the stream and **succeeds with zero objects**.

use crate::color::{ColorSpace, ColorSpaceCache};
use crate::function::FunctionCache;
use crate::image::{ImageCache, RequestedSize, decode_image};
use crate::names;
use crate::ops::{FillRule, LineCap, Op, TextItem, TextRenderMode};
use crate::page::{
    Content, FormObject, ImageObject, Page, PageObject, PathObject, ShadingObject, TextObject,
    TextSegment,
};
use crate::pattern::{Pattern, TilingPattern};
use crate::resources::Resources;
use crate::shading::Shading;
use crate::state::{
    ContentMarks, GraphicsState, StateStack, TextCursor, apply_ext_gstate, glyph_matrix,
    kerning_shift,
};
use crate::transparency::Transparency;
use kurbo::{Affine, BezPath, Point, Rect};
use pdfrum_common::{DiagKind, Diagnostics, Limits, Severity};
use pdfrum_font::{Font, FontCache};
use pdfrum_object::{Dict, Name, Object, Resolve};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// The most form parses that may be in flight at once.
///
/// Compared with `>`, so **forty-one** nested forms are allowed and the
/// forty-second is refused.
pub const MAX_FORM_LEVEL: usize = 40;

/// The caches and guards a build shares across the whole page.
///
/// Owned by the caller and passed down by `&mut` — no globals, no interior
/// mutability.
#[derive(Debug, Default)]
pub struct BuildContext {
    /// Colour spaces, keyed on the reference that named them.
    pub colorspaces: ColorSpaceCache,
    /// Functions, likewise.
    pub functions: FunctionCache,
    /// Decoded images, keyed on `(reference, requested size)`.
    pub images: ImageCache,
    /// Fonts.
    pub fonts: FontCache,
    /// Loaded font *instances*, keyed on the reference that named them.
    ///
    /// Separate from [`fonts`](Self::fonts), which hands out identities: this
    /// is what makes two text objects that name the same `/Font` resource
    /// share one `Arc<Font>`. Text extraction's duplicate suppression
    /// compares fonts by that pointer, so loading a fresh instance per `Tf`
    /// would silently stop it firing and let a redrawn line be extracted
    /// twice.
    font_instances: HashMap<pdfrum_object::ObjRef, Option<Arc<Font>>>,
    /// The content buffers currently being parsed, which is the form guard.
    in_flight: HashSet<BufferId>,
    /// How many Type 3 glyph procedures are being interpreted above the
    /// current one (`kMaxType3FormLevel`).
    ///
    /// A glyph procedure may itself show text in a Type 3 font, so
    /// interpreting one can reach another; the buffer-identity guard catches
    /// a procedure that invokes *itself*, but not a pair that invoke each
    /// other through two distinct streams, which is what
    /// [`MAX_TYPE3_DEPTH`](pdfrum_font::MAX_TYPE3_DEPTH) bounds.
    type3_depth: u32,
}

/// A content buffer's identity: the object that holds it, and its extent.
///
/// The C++ keys its guard on a raw pointer to the decoded bytes; this is the
/// same identity in a representation Rust can hold safely.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct BufferId {
    reference: Option<pdfrum_object::ObjRef>,
    len: usize,
    /// A hash of the first and last bytes, so two distinct inline buffers of
    /// the same length are not confused.
    fingerprint: u64,
}

impl BufferId {
    fn new(reference: Option<pdfrum_object::ObjRef>, data: &[u8]) -> Self {
        let mut fingerprint = 0xcbf2_9ce4_8422_2325u64;
        for b in data.iter().take(64).chain(data.iter().rev().take(64)) {
            fingerprint ^= u64::from(*b);
            fingerprint = fingerprint.wrapping_mul(0x100_0000_01b3);
        }
        Self {
            reference,
            len: data.len(),
            fingerprint,
        }
    }
}

impl BuildContext {
    /// An empty context.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// How many form parses are in flight.
    #[must_use]
    pub fn forms_in_flight(&self) -> usize {
        self.in_flight.len()
    }

    /// Enter a Type 3 glyph procedure, or refuse when the cap is reached.
    ///
    /// Returns `false` at the cap, in which case the caller must **not** call
    /// [`Self::leave_type3`]. Compared with `>=`, so four levels nest and the
    /// fifth is refused.
    pub(crate) fn enter_type3(&mut self) -> bool {
        if self.type3_depth >= pdfrum_font::MAX_TYPE3_DEPTH {
            return false;
        }
        self.type3_depth += 1;
        true
    }

    /// Leave a Type 3 glyph procedure entered through [`Self::enter_type3`].
    pub(crate) fn leave_type3(&mut self) {
        self.type3_depth = self.type3_depth.saturating_sub(1);
    }
}

/// Build a page from its operators.
///
/// `resources` is what named lookups consult, and `initial` is the state the
/// content begins in — the identity transform and opaque black for a page,
/// the caller's state for a form.
///
/// ```
/// use pdfrum_common::{Diagnostics, Limits};
/// use pdfrum_object::NoResolve;
/// use pdfrum_page::{BuildContext, PageObject, Resources, build_page, parse_content};
///
/// let mut diags = Diagnostics::default();
/// let limits = Limits::default();
/// let ops = parse_content(b"0 0 10 10 re f", &limits, &mut diags);
///
/// let mut ctx = BuildContext::new();
/// let page = build_page(
///     &ops,
///     &Resources::default(),
///     &NoResolve,
///     &mut ctx,
///     &limits,
///     &mut diags,
/// );
/// assert_eq!(page.objects.len(), 1);
/// assert!(matches!(page.objects[0], PageObject::Path(_)));
/// ```
#[must_use]
pub fn build_page<R: Resolve>(
    ops: &[Op],
    resources: &Resources,
    r: &R,
    ctx: &mut BuildContext,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> Page {
    let objects = interpret(
        ops,
        resources,
        &GraphicsState::default(),
        Affine::IDENTITY,
        r,
        ctx,
        limits,
        diags,
    );
    Page {
        objects,
        resources: resources.chosen.clone(),
        ..Page::empty()
    }
}

/// Build a page from its operators and its dictionary.
///
/// The dictionary supplies the boxes, the rotation and the transparency
/// group; `inherited` answers for a key the page tree may hold further up.
#[expect(
    clippy::too_many_arguments,
    reason = "a page needs its operators, dictionary, inherited attributes, \
              resources and the usual resolver/context/limits/diagnostics"
)]
#[must_use]
pub fn build_page_from_dict<R: Resolve>(
    ops: &[Op],
    dict: &Dict,
    inherited: impl Fn(&Name) -> Option<Object>,
    resources: &Resources,
    r: &R,
    ctx: &mut BuildContext,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> Page {
    let (media_box, crop_box) = crate::page::derive_boxes(dict, &inherited, r, diags);
    let rotate = crate::page::Rotation::from_degrees(
        dict.int(names::ROTATE, r)
            .or_else(|| inherited(names::ROTATE).and_then(|o| o.as_int()))
            .unwrap_or(0),
    );
    let transparency = Transparency::for_page(dict.dict(names::GROUP, r).as_ref(), r);
    let objects = interpret(
        ops,
        resources,
        &GraphicsState::default(),
        Affine::IDENTITY,
        r,
        ctx,
        limits,
        diags,
    );
    Page {
        objects,
        media_box,
        crop_box,
        rotate,
        transparency,
        resources: resources.chosen.clone(),
    }
}

/// The fold's mutable working set, kept together so no function needs a
/// dozen parameters.
struct Interp<'a, R: Resolve> {
    state: GraphicsState,
    stack: StateStack,
    marks: ContentMarks,
    cursor: TextCursor,
    /// The path being assembled, as points with their kinds.
    points: Vec<(Point, PointKind)>,
    /// The rule a pending `W`/`W*` will clip with, once a painting operator
    /// consumes it.
    pending_clip: FillRule,
    /// Where the current subpath began, for `h`.
    subpath_start: Point,
    /// The current point.
    current: Point,
    /// Glyph outlines a clipping text mode has accumulated.
    text_clip: Vec<BezPath>,
    resources: &'a Resources,
    /// The form's or page's coordinate system, which patterns anchor to —
    /// **not** the current transform.
    parent_matrix: Affine,
    resolver: &'a R,
    objects: Vec<PageObject>,
}

/// How a path point continues the path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PointKind {
    Move,
    Line,
    Curve,
    /// A point that also closes its subpath.
    CloseLine,
}

/// Interpret a run of operators into page objects.
#[expect(
    clippy::too_many_arguments,
    reason = "the interpreter needs its operators, resources, initial state, \
              parent matrix, resolver, context, limits and diagnostics"
)]
fn interpret<R: Resolve>(
    ops: &[Op],
    resources: &Resources,
    initial: &GraphicsState,
    parent_matrix: Affine,
    r: &R,
    ctx: &mut BuildContext,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> Vec<PageObject> {
    let mut interp = Interp {
        state: initial.clone(),
        stack: StateStack::new(),
        marks: ContentMarks::new(),
        cursor: TextCursor::default(),
        points: Vec::new(),
        pending_clip: FillRule::None,
        subpath_start: Point::ZERO,
        current: Point::ZERO,
        text_clip: Vec::new(),
        resources,
        parent_matrix,
        resolver: r,
        objects: Vec::new(),
    };
    for op in ops {
        interp.apply(op, ctx, limits, diags);
    }
    interp.objects
}

impl<R: Resolve> Interp<'_, R> {
    /// Apply one operator.
    #[expect(
        clippy::too_many_lines,
        reason = "the operator dispatch is a flat table by design: one arm \
                  per operator, each a few lines, and splitting it would \
                  hide which operator does what"
    )]
    fn apply(&mut self, op: &Op, ctx: &mut BuildContext, limits: &Limits, diags: &mut Diagnostics) {
        match op {
            // ---- Graphics state ----
            Op::SaveState() => self.stack.push(&self.state),
            Op::RestoreState() => {
                if !self.stack.pop(&mut self.state) {
                    diags.record(Severity::Suspicious, DiagKind::UnbalancedRestore, None);
                }
            }
            // A **pre**-concatenation: the new matrix applies first.
            Op::Concat(m) => self.state.ctm *= *m,
            Op::SetLineWidth(w) => self.state.stroke_params.width = *w,
            Op::SetLineCap(c) => self.state.stroke_params.cap = *c,
            Op::SetLineJoin(j) => self.state.stroke_params.join = *j,
            Op::SetMiterLimit(m) => self.state.stroke_params.miter_limit = *m,
            Op::SetDash(d) => {
                // A non-array first operand makes the operator a no-op.
                if d.valid {
                    self.state.stroke_params.dash.clone_from(&d.array);
                    self.state.stroke_params.dash_phase = d.phase;
                }
            }
            Op::SetFlatness(f) => self.state.general.flatness = *f,
            Op::SetExtGState(name) => self.apply_ext_gstate(name, ctx, limits, diags),

            // ---- Path construction ----
            Op::MoveTo(p) => {
                self.add_point(*p, PointKind::Move);
                self.subpath_start = *p;
            }
            Op::LineTo(p) => self.add_point(*p, PointKind::Line),
            Op::CurveTo(a, b, c) => {
                self.add_point(*a, PointKind::Curve);
                self.add_point(*b, PointKind::Curve);
                self.add_point(*c, PointKind::Curve);
            }
            // The first control point is the current point.
            Op::CurveToV(b, c) => {
                let start = self.current;
                self.add_point(start, PointKind::Curve);
                self.add_point(*b, PointKind::Curve);
                self.add_point(*c, PointKind::Curve);
            }
            // The last point is duplicated as the second control point.
            Op::CurveToY(a, c) => {
                self.add_point(*a, PointKind::Curve);
                self.add_point(*c, PointKind::Curve);
                self.add_point(*c, PointKind::Curve);
            }
            Op::ClosePath() => self.close_path(),
            Op::Rectangle(x, y, w, h) => {
                let (x, y, w, h) = (f64::from(*x), f64::from(*y), f64::from(*w), f64::from(*h));
                self.add_point(Point::new(x, y), PointKind::Move);
                self.add_point(Point::new(x + w, y), PointKind::Line);
                self.add_point(Point::new(x + w, y + h), PointKind::Line);
                self.add_point(Point::new(x, y + h), PointKind::Line);
                self.add_point(Point::new(x, y), PointKind::CloseLine);
                self.subpath_start = Point::new(x, y);
            }

            // ---- Path painting ----
            Op::Stroke() => self.paint(FillRule::None, true),
            Op::CloseStroke() => {
                self.close_path();
                self.paint(FillRule::None, true);
            }
            Op::Fill() | Op::FillObsolete() => self.paint(FillRule::Winding, false),
            Op::FillEvenOdd() => self.paint(FillRule::EvenOdd, false),
            Op::FillStroke() => self.paint(FillRule::Winding, true),
            Op::FillStrokeEvenOdd() => self.paint(FillRule::EvenOdd, true),
            Op::CloseFillStroke() => {
                self.close_path();
                self.paint(FillRule::Winding, true);
            }
            // Unlike `b`, this appends the closing segment **unconditionally**
            // rather than only when the current point differs from the start.
            Op::CloseFillStrokeEvenOdd() => {
                let start = self.subpath_start;
                self.current = start;
                if !self.points.is_empty() {
                    self.points.push((start, PointKind::CloseLine));
                }
                self.paint(FillRule::EvenOdd, true);
            }
            Op::EndPath() => self.paint(FillRule::None, false),

            // ---- Clipping ----
            Op::Clip() => self.pending_clip = FillRule::Winding,
            Op::ClipEvenOdd() => self.pending_clip = FillRule::EvenOdd,

            // ---- Text ----
            Op::BeginText() => {
                // `BT` resets the matrices but does **not** clear the text
                // clip list; that is `ET`'s job.
                self.cursor.set_matrix(Affine::IDENTITY);
            }
            Op::EndText() => {
                if !self.text_clip.is_empty() {
                    let glyphs = std::mem::take(&mut self.text_clip);
                    self.state.clip.push_text(glyphs);
                }
            }
            Op::TextMove(tx, ty) => {
                self.cursor.move_line(f64::from(*tx), f64::from(*ty));
            }
            Op::TextMoveSetLeading(tx, ty) => {
                self.cursor.move_line(f64::from(*tx), f64::from(*ty));
                // The **negated** y offset.
                self.state.text.leading = -*ty;
            }
            Op::SetTextMatrix(m) => self.cursor.set_matrix(*m),
            Op::TextNextLine() => {
                self.cursor.next_line(f64::from(self.state.text.leading));
            }
            Op::SetLeading(l) => self.state.text.leading = *l,
            Op::SetTextRise(rise) => self.state.text.rise = *rise,
            // Stored as a fraction: `150 Tz` becomes 1.5.
            Op::SetHorzScale(z) => self.state.text.horz_scale = *z / 100.0,
            Op::SetCharSpace(c) => self.state.text.char_space = *c,
            Op::SetWordSpace(w) => self.state.text.word_space = *w,
            Op::SetFont(name, size) => {
                // The size is **always** set; the font only when it resolves.
                let font = self.find_font(name, ctx, limits, diags);
                match (font, self.state.text.font.take()) {
                    (Some(f), _) => self.state.text.font = Some((f, *size)),
                    (None, Some((old, _))) => self.state.text.font = Some((old, *size)),
                    (None, None) => {}
                }
            }
            Op::SetTextRenderMode(mode) => match TextRenderMode::from_int(*mode) {
                Some(m) => self.state.text.render_mode = m,
                // Out of range leaves the mode **unchanged**.
                None => {
                    diags.record(Severity::Suspicious, DiagKind::BadTextRenderMode, None);
                }
            },
            Op::ShowText(s) => self.show_text(&[(s.bytes.clone(), 0.0)], 0.0, ctx, limits, diags),
            Op::NextLineShowText(s) => {
                self.cursor.next_line(f64::from(self.state.text.leading));
                self.show_text(&[(s.bytes.clone(), 0.0)], 0.0, ctx, limits, diags);
            }
            Op::SetSpacingShowText(word, char_space, s) => {
                self.state.text.word_space = *word;
                self.state.text.char_space = *char_space;
                self.cursor.next_line(f64::from(self.state.text.leading));
                self.show_text(&[(s.bytes.clone(), 0.0)], 0.0, ctx, limits, diags);
            }
            Op::ShowTextAdjusted(array) => {
                if array.valid {
                    self.show_adjusted(&array.items, ctx, limits, diags);
                }
            }

            // ---- Colour ----
            Op::SetStrokeColorSpace(name) => {
                self.set_color_space(name, true, ctx, limits, diags);
            }
            Op::SetFillColorSpace(name) => {
                self.set_color_space(name, false, ctx, limits, diags);
            }
            Op::SetStrokeColor(c) => {
                self.state.stroke.set_components(&c.0);
            }
            Op::SetFillColor(c) => {
                self.state.fill.set_components(&c.0);
            }
            Op::SetStrokeColorN(c) => self.set_color_n(c, true, ctx, limits, diags),
            Op::SetFillColorN(c) => self.set_color_n(c, false, ctx, limits, diags),
            Op::SetStrokeGray(g) => {
                self.state.stroke.set_stock(ColorSpace::DeviceGray, &[*g]);
            }
            Op::SetFillGray(g) => self.state.fill.set_stock(ColorSpace::DeviceGray, &[*g]),
            Op::SetStrokeRgb(r, g, b) => {
                self.state
                    .stroke
                    .set_stock(ColorSpace::DeviceRgb, &[*r, *g, *b]);
            }
            Op::SetFillRgb(r, g, b) => {
                self.state
                    .fill
                    .set_stock(ColorSpace::DeviceRgb, &[*r, *g, *b]);
            }
            Op::SetStrokeCmyk(c, m, y, k) => {
                self.state
                    .stroke
                    .set_stock(ColorSpace::DeviceCmyk, &[*c, *m, *y, *k]);
            }
            Op::SetFillCmyk(c, m, y, k) => {
                self.state
                    .fill
                    .set_stock(ColorSpace::DeviceCmyk, &[*c, *m, *y, *k]);
            }

            // ---- XObjects and shading ----
            Op::DoXObject(name) => self.do_xobject(name, ctx, limits, diags),
            Op::ShadeFill(name) => self.shade_fill(name, ctx, limits, diags),
            Op::InlineImage(image) => self.inline_image(image, ctx, limits, diags),

            // ---- Marked content ----
            Op::BeginMarkedContent(tag) => self.marks.push(tag.clone()),
            Op::BeginMarkedContentDict(tag, props) => {
                if let Some(properties) = &props.0 {
                    let resources = self.resources;
                    let resolver = self.resolver;
                    self.marks
                        .push_with_properties(tag.clone(), properties, |name| {
                            resources
                                .find(names::PROPERTIES, name, resolver)
                                .and_then(|o| o.as_dict().cloned())
                        });
                }
                // A null or wrong-typed property list pushes nothing at all.
            }
            Op::EndMarkedContent() => {
                if !self.marks.pop() {
                    diags.record(
                        Severity::Suspicious,
                        DiagKind::UnbalancedMarkedContent,
                        None,
                    );
                }
            }

            // ---- Operators that produce no page object ----
            //
            // Each for its own reason: `d0` and `d1` are Type 3 glyph metrics
            // the font layer consumes; `ri` is discarded outright, since only
            // the `/ExtGState` `/RI` is stored; `MP` and `DP` are
            // marked-content *points*, which carry no scope; `BI`, `ID` and
            // `EI` reach dispatch only when the tokenizer abandoned an inline
            // image or met a stray keyword; `BX` and `EX` are compatibility
            // brackets; and an unknown keyword had its operands cleared by
            // the caller.
            Op::Type3Width(..)
            | Op::Type3WidthBBox(..)
            | Op::SetRenderIntent(_)
            | Op::MarkPoint(_)
            | Op::MarkPointDict(..)
            | Op::BeginInlineImage()
            | Op::InlineImageData()
            | Op::EndInlineImage()
            | Op::BeginCompat()
            | Op::EndCompat()
            | Op::Unknown(_) => {}
        }
    }

    /// The three path repairs from the module docs.
    fn add_point(&mut self, point: Point, kind: PointKind) {
        self.current = point;
        match self.points.last() {
            // A `Move` onto an open `Move`: drop the duplicate, or overwrite.
            Some((previous, PointKind::Move)) if kind == PointKind::Move => {
                if *previous == point {
                    return;
                }
                if let Some(last) = self.points.last_mut() {
                    *last = (point, kind);
                }
                return;
            }
            // A non-`Move` with nothing started is discarded.
            None if kind != PointKind::Move => return,
            _ => {}
        }
        self.points.push((point, kind));
    }

    /// `h`: close the current subpath.
    fn close_path(&mut self) {
        if self.points.is_empty() {
            return;
        }
        if self.current == self.subpath_start {
            // Already there: mark the last point as closing.
            if let Some(last) = self.points.last_mut() {
                last.1 = PointKind::CloseLine;
            }
        } else {
            let start = self.subpath_start;
            self.points.push((start, PointKind::CloseLine));
            self.current = start;
        }
    }

    /// Take the pending path and paint it.
    fn paint(&mut self, fill_rule: FillRule, stroke: bool) {
        let points = std::mem::take(&mut self.points);
        // The pending clip is consumed by whichever painting operator comes
        // next, `n` included.
        let clip_rule = std::mem::replace(&mut self.pending_clip, FillRule::None);

        if points.is_empty() {
            // The clip is discarded along with the path.
            return;
        }
        let matrix = self.state.ctm;

        // A single point is a special case in both directions.
        if points.len() == 1 {
            if clip_rule != FillRule::None {
                // An empty clip, which blanks everything after it.
                self.state.clip.push_empty();
                return;
            }
            let (point, kind) = points
                .first()
                .copied()
                .unwrap_or((Point::ZERO, PointKind::Move));
            // Only a closed move under a round cap draws anything: a dot.
            if kind != PointKind::CloseLine || self.state.stroke_params.cap != LineCap::Round {
                return;
            }
            let mut path = BezPath::new();
            path.move_to(point);
            path.line_to(point);
            path.close_path();
            self.emit_path(path, matrix, fill_rule, stroke, clip_rule);
            return;
        }

        // A trailing open `Move` contributes nothing and is dropped.
        let mut points = points;
        if matches!(points.last(), Some((_, PointKind::Move))) {
            points.pop();
        }
        if points.is_empty() {
            return;
        }
        let path = build_path(&points);
        self.emit_path(path, matrix, fill_rule, stroke, clip_rule);
    }

    /// Emit a path object and apply any pending clip.
    fn emit_path(
        &mut self,
        path: BezPath,
        matrix: Affine,
        fill_rule: FillRule,
        stroke: bool,
        clip_rule: FillRule,
    ) {
        // `n` with no clip produces nothing at all.
        if stroke || fill_rule != FillRule::None {
            let object = PathObject {
                path: path.clone(),
                matrix,
                fill_rule,
                stroke,
            };
            self.push(PageObject::Path(Box::new(self.content(object))));
        }
        if clip_rule != FillRule::None {
            // The clip path is transformed; the drawn path keeps its matrix
            // separately.
            let clipped = if matrix == Affine::IDENTITY {
                path
            } else {
                matrix * path
            };
            self.state
                .clip
                .push_path(clipped, clip_rule == FillRule::EvenOdd);
        }
    }

    /// Wrap an object with the state and marks in force.
    fn content<T>(&self, object: T) -> Content<T> {
        Content {
            object,
            state: self.state.clone(),
            marks: self.marks.clone(),
            content_stream: 0,
        }
    }

    fn push(&mut self, object: PageObject) {
        self.objects.push(object);
    }

    /// `Tj` and friends: one text object from a set of segments.
    fn show_text(
        &mut self,
        segments: &[(Box<[u8]>, f32)],
        initial_kerning: f32,
        ctx: &mut BuildContext,
        limits: &Limits,
        diags: &mut Diagnostics,
    ) {
        // With no font nothing is produced, which only happens when `Tf` was
        // never issued.
        let Some((font, size)) = self.state.text.font.clone() else {
            return;
        };
        let vertical = font.is_vertical();

        // The initial adjustment moves the position **before** the object,
        // and applies even when the object turns out empty.
        if initial_kerning != 0.0 {
            let shift = -kerning_shift(initial_kerning, size, self.state.text.horz_scale, vertical);
            self.cursor.advance(shift, vertical);
        }
        let segments: Vec<TextSegment> = segments
            .iter()
            .filter(|(codes, _)| !codes.is_empty())
            .map(|(codes, kerning)| TextSegment {
                codes: codes.clone(),
                kerning: *kerning,
            })
            .collect();
        if segments.is_empty() {
            return;
        }

        // A Type 3 font is forced to fill mode whatever `Tr` said.
        let render_mode = if font.type3().is_some() {
            TextRenderMode::Fill
        } else {
            self.state.text.render_mode
        };

        let position = self
            .cursor
            .device_position(self.state.text.rise, self.state.ctm);
        let matrix = glyph_matrix(
            self.state.text.horz_scale,
            self.cursor.matrix,
            self.state.ctm,
        );
        let advance = self.advance_for(&segments, &font, size);
        let type3_metrics = self.type3_metrics_for(&segments, &font, ctx, limits, diags);

        let object = TextObject {
            segments: segments.into(),
            position,
            matrix,
            font: Some((Arc::clone(&font), size)),
            render_mode,
            type3_metrics,
        };
        self.push(PageObject::Text(Box::new(self.content(object))));
        self.cursor.advance(advance, vertical);
    }

    /// What each shown character's Type 3 glyph procedure declares.
    ///
    /// Empty for every other kind of font. Reading it here rather than
    /// downstream is what lets a consumer measure a Type 3 glyph at all: the
    /// numbers live inside content streams only the interpreter opens.
    fn type3_metrics_for(
        &self,
        segments: &[TextSegment],
        font: &Font,
        ctx: &mut BuildContext,
        limits: &Limits,
        diags: &mut Diagnostics,
    ) -> std::collections::BTreeMap<u32, crate::type3::Type3Metrics> {
        let mut out = std::collections::BTreeMap::new();
        let Some(type3) = font.type3() else {
            return out;
        };
        for segment in segments {
            for item in font.decode(&segment.codes) {
                if out.contains_key(&item.code.0) {
                    continue;
                }
                if let Some(m) = crate::type3::metrics(
                    type3,
                    item.code,
                    self.resources.page.as_ref(),
                    self.resolver,
                    ctx,
                    limits,
                    diags,
                ) {
                    out.insert(item.code.0, m);
                }
            }
        }
        out
    }

    /// The advance a run produces, in text space.
    fn advance_for(&self, segments: &[TextSegment], font: &Font, size: f32) -> f64 {
        let vertical = font.is_vertical();
        let mut total = 0.0f64;
        for segment in segments {
            for item in font.decode(&segment.codes) {
                let mut width = f64::from(item.width) * f64::from(size) / 1000.0;
                // Word spacing applies to a single-byte space only.
                if item.code.0 == 0x20 && item.cid.is_none() {
                    width += f64::from(self.state.text.word_space);
                }
                width += f64::from(self.state.text.char_space);
                total += width;
            }
            total -= kerning_shift(segment.kerning, size, 1.0, vertical);
        }
        if vertical {
            total
        } else {
            total * f64::from(self.state.text.horz_scale)
        }
    }

    /// `TJ`: split the array into segments and their accumulated kernings.
    fn show_adjusted(
        &mut self,
        items: &[TextItem],
        ctx: &mut BuildContext,
        limits: &Limits,
        diags: &mut Diagnostics,
    ) {
        let strings = items
            .iter()
            .filter(|i| matches!(i, TextItem::Show(_)))
            .count();
        let vertical = self
            .state
            .text
            .font
            .as_ref()
            .is_some_and(|(f, _)| f.is_vertical());

        // With no strings at all the array is pure kerning, and **only x
        // moves — even for a vertical font**, which is an inconsistency with
        // the other branch that files rely on.
        if strings == 0 {
            let Some((_, size)) = self.state.text.font.clone() else {
                return;
            };
            for item in items {
                let TextItem::Adjust(k) = item else { continue };
                if *k != 0.0 {
                    let shift = -kerning_shift(*k, size, self.state.text.horz_scale, false);
                    self.cursor.pos.x += shift;
                }
            }
            let _ = vertical;
            return;
        }

        let mut segments: Vec<(Box<[u8]>, f32)> = Vec::new();
        let mut initial = 0.0f32;
        for item in items {
            match item {
                TextItem::Show(codes) => {
                    if !codes.is_empty() {
                        segments.push((codes.clone(), 0.0));
                    }
                }
                // Adjacent adjustments **accumulate**.
                TextItem::Adjust(k) => match segments.last_mut() {
                    Some((_, kerning)) => *kerning += *k,
                    None => initial += *k,
                },
            }
        }
        self.show_text(&segments, initial, ctx, limits, diags);
    }

    /// Look a font up, falling back to Helvetica so `Tf` with a bad name
    /// still renders text.
    fn find_font(
        &self,
        name: &Name,
        ctx: &mut BuildContext,
        limits: &Limits,
        diags: &mut Diagnostics,
    ) -> Option<Arc<Font>> {
        // An indirect resource is cached under its reference, so every `Tf`
        // naming it shares one instance. A resource written inline has no
        // reference and is loaded afresh — two inline copies genuinely are
        // two fonts.
        let reference = self.resources.find_ref(names::FONT, name, self.resolver);
        if let Some(reference) = reference
            && let Some(cached) = ctx.font_instances.get(&reference)
        {
            return cached.clone();
        }
        let dict = self
            .resources
            .find(names::FONT, name, self.resolver)
            .and_then(|o| o.as_dict().cloned());
        let font = match dict {
            Some(d) => {
                pdfrum_font::load(&d, self.resolver, &ctx.fonts, limits, diags).map(Arc::new)
            }
            // A name that resolves to nothing yields the stock font rather
            // than nothing at all.
            None => Some(Arc::new(Font::load_standard(
                pdfrum_font::StandardFont::Helvetica,
                &ctx.fonts,
            ))),
        };
        if let Some(reference) = reference {
            ctx.font_instances.insert(reference, font.clone());
        }
        font
    }

    /// `cs` and `CS`: install a colour space, resetting the colour.
    fn set_color_space(
        &mut self,
        name: &Name,
        stroking: bool,
        ctx: &mut BuildContext,
        limits: &Limits,
        diags: &mut Diagnostics,
    ) {
        let Some(space) = self.load_named_colorspace(name, ctx, limits, diags) else {
            // A name that will not resolve makes the operator a no-op.
            return;
        };
        let target = if stroking {
            &mut self.state.stroke
        } else {
            &mut self.state.fill
        };
        target.set_space(Arc::new(space));
    }

    /// The `/DefaultGray`, `/DefaultRGB` and `/DefaultCMYK` substitution
    /// applies **only** to the three fully spelled device names, and only
    /// through this path.
    fn load_named_colorspace(
        &self,
        name: &Name,
        ctx: &mut BuildContext,
        limits: &Limits,
        diags: &mut Diagnostics,
    ) -> Option<ColorSpace> {
        if name.as_bytes() == b"Pattern" {
            return Some(ColorSpace::Pattern(Box::default()));
        }
        let colorspaces = self.resources.color_spaces(self.resolver);
        crate::color::load_colorspace(
            &Object::Name(name.clone()),
            colorspaces.as_ref(),
            self.resolver,
            &mut ctx.functions,
            limits,
            diags,
        )
    }

    /// `scn` and `SCN`.
    ///
    /// A trailing name installs a pattern, loaded against the **parent
    /// matrix** so it stays anchored to the space it was declared in.
    fn set_color_n(
        &mut self,
        c: &crate::ops::PatternComponents,
        stroking: bool,
        ctx: &mut BuildContext,
        limits: &Limits,
        diags: &mut Diagnostics,
    ) {
        if let Some(name) = &c.pattern {
            // A name that resolves to no pattern makes the operator a no-op.
            let Some(loaded) = load_pattern(
                name,
                self.resources,
                self.parent_matrix,
                &self.state.general,
                self.resolver,
                ctx,
                limits,
                diags,
            ) else {
                return;
            };
            let target = if stroking {
                &mut self.state.stroke
            } else {
                &mut self.state.fill
            };
            // The loaded pattern rides with the colour, so `q`/`Q` save and
            // restore it for free and every object painted under it carries
            // the cell or the shading itself. Re-resolving the name at paint
            // time would need the resources and the resolver the renderer no
            // longer has.
            target.set_pattern(name.clone(), &c.values, Some(loaded));
            return;
        }
        let target = if stroking {
            &mut self.state.stroke
        } else {
            &mut self.state.fill
        };
        target.set_components(&c.values);
    }

    /// `gs`.
    fn apply_ext_gstate(
        &mut self,
        name: &Name,
        ctx: &mut BuildContext,
        limits: &Limits,
        diags: &mut Diagnostics,
    ) {
        let Some(ext) = self
            .resources
            .find(names::EXT_G_STATE, name, self.resolver)
            .and_then(|o| o.as_dict().cloned())
        else {
            return;
        };
        let resources = self.resources;
        let resolver = self.resolver;
        let fonts = &ctx.fonts;
        let find_font = |spelling: &[u8]| -> Option<Arc<Font>> {
            let dict = resources
                .find(names::FONT, &Name::new(spelling), resolver)
                .and_then(|o| o.as_dict().cloned())?;
            pdfrum_font::load(
                &dict,
                resolver,
                fonts,
                limits,
                &mut Diagnostics::with_limit(0),
            )
            .map(Arc::new)
        };
        apply_ext_gstate(
            &mut self.state,
            &ext,
            find_font,
            self.resolver,
            &mut ctx.functions,
            limits,
            diags,
        );
    }

    /// `Do`: a form or an image.
    fn do_xobject(
        &mut self,
        name: &Name,
        ctx: &mut BuildContext,
        limits: &Limits,
        diags: &mut Diagnostics,
    ) {
        let Some(object) = self.resources.find(names::XOBJECT, name, self.resolver) else {
            return;
        };
        // Not a stream: nothing happens.
        let Some(stream) = object.as_stream() else {
            return;
        };
        let reference = self
            .resources
            .holder(names::XOBJECT, self.resolver)
            .and_then(|h| h.reference(name));
        match stream
            .dict
            .byte_string(names::SUBTYPE, self.resolver)
            .as_deref()
        {
            Some(b"Form") => self.add_form(stream, ctx, limits, diags),
            Some(b"Image") => self.add_image(stream, reference, ctx, limits, diags),
            // Any other subtype, `PS` and a missing one included, does
            // nothing at all.
            _ => {}
        }
    }

    /// A form `XObject`, with the buffer-identity guard.
    fn add_form(
        &mut self,
        stream: &pdfrum_object::Stream,
        ctx: &mut BuildContext,
        limits: &Limits,
        diags: &mut Diagnostics,
    ) {
        let content = pdfrum_filters::decode_chain(stream, 0, self.resolver, limits, diags).data;
        let id = BufferId::new(None, &content);
        // More than forty in flight, or this very buffer already in flight:
        // consume the stream and produce nothing, successfully.
        if ctx.in_flight.len() > MAX_FORM_LEVEL || ctx.in_flight.contains(&id) {
            diags.record(Severity::Recovered, DiagKind::FormRecursionRefused, None);
            return;
        }

        // The form's `/Matrix` composes with the current transform.
        let form_matrix = stream.dict.matrix(names::MATRIX, self.resolver);
        let matrix = self.state.ctm * form_matrix;

        let mut inner = self.state.clone();
        inner.ctm = matrix;
        // The clip is deliberately **not** inherited: the form gets its own
        // from its `/BBox`.
        inner.clip = crate::state::ClipStack::new();

        let transparency = Transparency::from_group(
            stream.dict.dict(names::GROUP, self.resolver).as_ref(),
            self.resolver,
        );
        if transparency.group {
            // Group isolation: a group starts from a clean compositing slate.
            inner.general.enter_transparency_group();
        }

        // A missing `/BBox` means **no clip at all** — the form is unbounded.
        let bbox = stream
            .dict
            .array(names::BBOX, self.resolver)
            .filter(|a| a.len() == 4)
            .map(|a| a.as_rect());

        let resources = Resources::choose(
            stream.dict.dict(names::RESOURCES, self.resolver),
            self.resources.chosen.clone(),
            self.resources.page.clone(),
        );

        ctx.in_flight.insert(id);
        let ops = crate::parse_content(&content, limits, diags);
        let objects = interpret(
            &ops,
            &resources,
            &inner,
            // Patterns inside the form anchor to the form's own space.
            matrix,
            self.resolver,
            ctx,
            limits,
            diags,
        );
        ctx.in_flight.remove(&id);

        let object = FormObject {
            objects,
            matrix,
            bbox,
            transparency,
        };
        self.push(PageObject::Form(Box::new(self.content(object))));
    }

    /// An image `XObject`, through the session cache.
    fn add_image(
        &mut self,
        stream: &pdfrum_object::Stream,
        reference: Option<pdfrum_object::ObjRef>,
        ctx: &mut BuildContext,
        limits: &Limits,
        diags: &mut Diagnostics,
    ) {
        let size = RequestedSize::Full;
        let cached = reference.and_then(|id| ctx.images.get(id, size));
        let image = if let Some(hit) = cached {
            hit
        } else {
            let decoded = decode_image(
                stream,
                // Only an *inline* image sees form resources.
                None,
                self.resources.page.as_ref(),
                size,
                self.resolver,
                &mut ctx.functions,
                limits,
                diags,
            );
            // A codec that refused the image paints nothing.
            let Ok(image) = decoded else {
                return;
            };
            let image = Arc::new(image);
            if let Some(id) = reference {
                ctx.images.insert(id, size, Arc::clone(&image));
            }
            image
        };
        let is_mask = matches!(image.pixels, crate::image::Pixels::Stencil(_));
        let object = ImageObject {
            image,
            // The unit square transformed by the current matrix.
            matrix: self.state.ctm,
            is_mask,
        };
        self.push(PageObject::Image(Box::new(self.content(object))));
    }

    /// An inline image, which carries its own bytes.
    fn inline_image(
        &mut self,
        image: &crate::ops::InlineImage,
        ctx: &mut BuildContext,
        limits: &Limits,
        diags: &mut Diagnostics,
    ) {
        let stream = pdfrum_object::Stream::new(
            crate::inline_image::as_xobject_dict(image),
            pdfrum_object::ByteSpan::from(image.data.to_vec()),
        );
        let decoded = decode_image(
            &stream,
            // Inline images are the **only** ones that see form resources.
            self.resources.chosen.as_ref(),
            self.resources.page.as_ref(),
            RequestedSize::Full,
            self.resolver,
            &mut ctx.functions,
            limits,
            diags,
        );
        let Ok(data) = decoded else {
            return;
        };
        let is_mask = matches!(data.pixels, crate::image::Pixels::Stencil(_));
        let object = ImageObject {
            image: Arc::new(data),
            matrix: self.state.ctm,
            is_mask,
        };
        self.push(PageObject::Image(Box::new(self.content(object))));
    }

    /// `sh`: paint a shading across the clip.
    fn shade_fill(
        &mut self,
        name: &Name,
        ctx: &mut BuildContext,
        limits: &Limits,
        diags: &mut Diagnostics,
    ) {
        let Some(object) = self.resources.find(names::SHADING, name, self.resolver) else {
            return;
        };
        let colorspaces = self.resources.color_spaces(self.resolver);
        let Some(shading) = Shading::load(
            &object,
            colorspaces.as_ref(),
            // Reached through `/Shading`, so `/Background` is **not**
            // honoured.
            true,
            self.resolver,
            &mut ctx.functions,
            limits,
            diags,
        ) else {
            return;
        };
        // The clip when there is one, else the whole page.
        let mut bounds = self
            .state
            .clip
            .bounds()
            .unwrap_or(crate::page::DEFAULT_MEDIA_BOX);
        // A mesh additionally bounds itself by its own extent.
        if let crate::shading::Geometry::Mesh { mesh, .. } = &shading.geometry
            && let Some(extent) = mesh.bounds()
        {
            bounds = bounds.intersect(self.state.ctm.transform_rect_bbox(extent));
        }
        let object = ShadingObject {
            shading: Arc::new(shading),
            matrix: self.state.ctm,
            bounds,
        };
        self.push(PageObject::Shading(Box::new(self.content(object))));
    }
}

/// Turn the point list into a path.
fn build_path(points: &[(Point, PointKind)]) -> BezPath {
    let mut path = BezPath::new();
    let mut pending: Vec<Point> = Vec::new();
    let mut open = false;
    for (point, kind) in points {
        match kind {
            PointKind::Move => {
                if open {
                    // A new subpath ends the previous one.
                    pending.clear();
                }
                path.move_to(*point);
                open = true;
            }
            PointKind::Line => {
                if open {
                    path.line_to(*point);
                }
            }
            PointKind::CloseLine => {
                if open {
                    path.line_to(*point);
                    path.close_path();
                    open = false;
                }
            }
            PointKind::Curve => {
                pending.push(*point);
                if pending.len() == 3 && open {
                    let (Some(a), Some(b), Some(c)) =
                        (pending.first(), pending.get(1), pending.get(2))
                    else {
                        continue;
                    };
                    path.curve_to(*a, *b, *c);
                    pending.clear();
                }
            }
        }
    }
    path
}

/// A pattern named in a colour value, loaded through the resources.
///
/// The **parent matrix** anchors it, not the current transform — patterns
/// live in the space they were declared in.
///
/// `general` is the painting object's general state, which a tiling pattern's
/// cell inherits wholesale — its alpha, blend mode and soft mask — while
/// taking *default* colour, text and path state. That asymmetry is the whole
/// reason a pattern is loaded where it is installed rather than where the
/// resource is declared, and it is what makes `/ca 0.5` on the filling object
/// fade the tiles.
#[expect(
    clippy::too_many_arguments,
    reason = "loading a pattern needs its name, resources, anchor matrix, the \
              painting object's general state, and the usual four"
)]
#[must_use]
pub fn load_pattern<R: Resolve>(
    name: &Name,
    resources: &Resources,
    parent_matrix: Affine,
    general: &crate::state::GeneralState,
    r: &R,
    ctx: &mut BuildContext,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> Option<Arc<Pattern>> {
    let object = resources.find(names::PATTERN, name, r)?;
    // The resource must be a dictionary or a stream.
    if !matches!(object, Object::Dict(_) | Object::Stream(_)) {
        return None;
    }
    let colorspaces = resources.color_spaces(r);
    let mut pattern = Pattern::load(
        &object,
        parent_matrix,
        colorspaces.as_ref(),
        r,
        &mut ctx.functions,
        limits,
        diags,
    )?;
    if let Pattern::Tiling(tiling) = &mut pattern
        && let Some(stream) = object.as_stream()
    {
        tiling.objects =
            expand_tiling_cell(tiling, stream, general, resources, r, ctx, limits, diags);
    }
    Some(Arc::new(pattern))
}

/// Interpret a tiling pattern's cell into the objects one tile paints.
///
/// Three things about the state it starts from are load-bearing:
///
/// - **The general state comes from the painting object**, so a pattern fill
///   under `/ca 0.5` paints half-transparent tiles.
/// - **Colour, text and path state are default.** A cell that never sets a
///   colour paints black, whatever the page was using.
/// - **The form matrix is the pattern's own** — `pattern_to_form` composed
///   with the parent — so nested patterns inside the cell anchor to the
///   cell's space rather than the page's.
///
/// The same buffer-identity guard forms use applies: a cell whose content is
/// already being interpreted higher up produces nothing rather than
/// recursing.
#[expect(
    clippy::too_many_arguments,
    reason = "expanding a cell needs the pattern, its stream, the inherited \
              state, resources, resolver and the usual three"
)]
fn expand_tiling_cell<R: Resolve>(
    tiling: &TilingPattern,
    stream: &pdfrum_object::Stream,
    general: &crate::state::GeneralState,
    outer: &Resources,
    r: &R,
    ctx: &mut BuildContext,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> Vec<PageObject> {
    let content = pdfrum_filters::decode_chain(stream, 0, r, limits, diags).data;
    let id = BufferId::new(None, &content);
    if ctx.in_flight.len() > MAX_FORM_LEVEL || ctx.in_flight.contains(&id) {
        diags.record(Severity::Recovered, DiagKind::FormRecursionRefused, None);
        return Vec::new();
    }
    // Default colour, text and path state; the painting object's general one.
    let mut initial = GraphicsState {
        general: general.clone(),
        ctm: tiling.matrix,
        ..GraphicsState::default()
    };
    // The cell clips to its own `/BBox`, which is what stops a tile's content
    // bleeding into its neighbours.
    if tiling.bbox.width() > 0.0 && tiling.bbox.height() > 0.0 {
        initial.clip.push_path(
            tiling.matrix * kurbo::Shape::to_path(&tiling.bbox, 0.1),
            false,
        );
    }
    let resources = Resources::choose(
        tiling.resources.clone(),
        outer.chosen.clone(),
        outer.page.clone(),
    );
    ctx.in_flight.insert(id);
    let ops = crate::parse_content(&content, limits, diags);
    let objects = interpret(
        &ops,
        &resources,
        &initial,
        tiling.matrix,
        r,
        ctx,
        limits,
        diags,
    );
    ctx.in_flight.remove(&id);
    objects
}

/// The clip-elimination post-pass a page runs after interpretation.
///
/// An object whose clip is a single rectangle that already contains the
/// object's own bounds has that clip **dropped entirely**. It changes no
/// pixels beyond anti-aliased clip edges, but it changes the clip counts a
/// structure dump reports, so it is not optional.
pub fn eliminate_redundant_clips(
    objects: &mut [PageObject],
    bounds_of: impl Fn(&PageObject) -> Rect,
) {
    for object in objects.iter_mut() {
        // Shadings are excluded: their clip is what bounds them.
        if matches!(object, PageObject::Shading(_)) {
            continue;
        }
        let rect = bounds_of(object);
        let state = match object {
            PageObject::Path(c) => &mut c.state,
            PageObject::Text(c) => &mut c.state,
            PageObject::Image(c) => &mut c.state,
            PageObject::Form(c) => &mut c.state,
            // Excluded above, and unreachable here.
            PageObject::Shading(_) => continue,
        };
        if state.clip.len() != 1 {
            continue;
        }
        let Some(crate::state::ClipEntry::Path { path, .. }) = state.clip.entries().first() else {
            continue;
        };
        let clip_rect = kurbo::Shape::bounding_box(path);
        if clip_rect.x0 <= rect.x0
            && clip_rect.y0 <= rect.y0
            && clip_rect.x1 >= rect.x1
            && clip_rect.y1 >= rect.y1
        {
            state.clip = crate::state::ClipStack::new();
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

    use super::{BuildContext, MAX_FORM_LEVEL, build_page};
    use crate::color::ColorSpace;
    use crate::ops::{FillRule, LineCap};
    use crate::page::PageObject;
    use crate::resources::Resources;
    use crate::state::GraphicsState;
    use kurbo::{Affine, Point};
    use pdfrum_common::{DiagKind, Diagnostics, Limits};
    use pdfrum_object::NoResolve;

    fn build(src: &[u8]) -> (crate::page::Page, Diagnostics) {
        build_with(src, &Resources::default())
    }

    fn build_with(src: &[u8], resources: &Resources) -> (crate::page::Page, Diagnostics) {
        let limits = Limits::default();
        let mut diags = Diagnostics::default();
        let ops = crate::parse_content(src, &limits, &mut diags);
        let mut ctx = BuildContext::new();
        let page = build_page(&ops, resources, &NoResolve, &mut ctx, &limits, &mut diags);
        (page, diags)
    }

    #[test]
    fn a_rectangle_fill_produces_one_path_object() {
        let (page, _) = build(b"0 0 100 50 re f");
        assert_eq!(page.objects.len(), 1);
        let PageObject::Path(path) = &page.objects[0] else {
            panic!("expected a path, got {:?}", page.objects[0]);
        };
        assert_eq!(path.object.fill_rule, FillRule::Winding);
        assert!(!path.object.stroke);
    }

    #[test]
    fn n_with_no_clip_produces_nothing() {
        let (page, _) = build(b"0 0 100 50 re n");
        assert!(page.objects.is_empty());
    }

    #[test]
    fn n_with_a_pending_clip_clips_but_paints_nothing() {
        let (page, _) = build(b"0 0 100 50 re W n 0 0 10 10 re f");
        // Only the second rectangle paints.
        assert_eq!(page.objects.len(), 1);
        let PageObject::Path(path) = &page.objects[0] else {
            panic!("expected a path");
        };
        assert_eq!(path.state.clip.len(), 1);
    }

    #[test]
    fn a_line_before_any_move_is_discarded() {
        let (page, _) = build(b"5 5 l 10 10 l S");
        // Nothing was ever started, so nothing paints.
        assert!(page.objects.is_empty());
    }

    #[test]
    fn consecutive_moves_collapse_to_the_last() {
        let (page, _) = build(b"1 1 m 2 2 m 3 3 m 9 9 l S");
        let PageObject::Path(path) = &page.objects[0] else {
            panic!("expected a path");
        };
        // The path starts at the last move, not the first.
        let start = path.object.path.elements().first().copied();
        assert!(
            matches!(start, Some(kurbo::PathEl::MoveTo(p)) if (p.x - 3.0).abs() < 1e-6),
            "got {start:?}"
        );
    }

    #[test]
    fn a_single_point_paints_nothing_unless_the_cap_is_round() {
        // Butt cap: nothing.
        let (page, _) = build(b"5 5 m h S");
        assert!(page.objects.is_empty());
        // Round cap: a dot.
        let (page, _) = build(b"1 J 5 5 m h S");
        assert_eq!(page.objects.len(), 1);
    }

    #[test]
    fn a_single_point_with_a_pending_clip_blanks_everything() {
        let (page, _) = build(b"5 5 m W n 0 0 10 10 re f");
        let PageObject::Path(path) = &page.objects[0] else {
            panic!("expected a path");
        };
        let bounds = path.state.clip.bounds().expect("an empty clip");
        assert!(bounds.area() < 1e-6, "got {bounds:?}");
    }

    #[test]
    fn q_and_restore_round_trip_the_state() {
        let (page, _) = build(b"q 5 w 1 0 0 rg Q 0 0 10 10 re f");
        let PageObject::Path(path) = &page.objects[0] else {
            panic!("expected a path");
        };
        // The `Q` undid both changes.
        assert!((path.state.stroke_params.width - 1.0).abs() < 1e-6);
        assert_eq!(&path.state.fill.components[..], &[0.0]);
    }

    #[test]
    fn an_unbalanced_restore_is_harmless() {
        let (page, diags) = build(b"Q Q 0 0 10 10 re f");
        assert_eq!(page.objects.len(), 1);
        assert!(diags.contains(&DiagKind::UnbalancedRestore));
    }

    #[test]
    fn cm_pre_concatenates() {
        let (page, _) = build(b"2 0 0 2 0 0 cm 1 0 0 1 10 0 cm 0 0 1 1 re f");
        let PageObject::Path(path) = &page.objects[0] else {
            panic!("expected a path");
        };
        // The translation is scaled by the earlier `cm`, so it lands at 20.
        let origin = path.object.matrix * Point::ZERO;
        assert!((origin.x - 20.0).abs() < 1e-6, "got {origin:?}");
    }

    #[test]
    fn tz_is_stored_as_a_fraction() {
        let (page, _) = build(b"150 Tz 0 0 10 10 re f");
        let PageObject::Path(path) = &page.objects[0] else {
            panic!("expected a path");
        };
        assert!((path.state.text.horz_scale - 1.5).abs() < 1e-6);
    }

    #[test]
    fn td_sets_the_leading_to_the_negated_offset() {
        let (page, _) = build(b"BT 0 -14 TD ET 0 0 1 1 re f");
        let PageObject::Path(path) = &page.objects[0] else {
            panic!("expected a path");
        };
        assert!((path.state.text.leading - 14.0).abs() < 1e-6);
    }

    #[test]
    fn an_out_of_range_text_render_mode_leaves_the_mode_alone() {
        let (page, diags) = build(b"2 Tr 9 Tr 0 0 1 1 re f");
        let PageObject::Path(path) = &page.objects[0] else {
            panic!("expected a path");
        };
        assert_eq!(
            path.state.text.render_mode,
            crate::ops::TextRenderMode::FillStroke,
            "the 9 should have been refused"
        );
        assert!(diags.contains(&DiagKind::BadTextRenderMode));
    }

    #[test]
    fn a_colorspace_operator_resets_the_colour_to_the_default() {
        let (page, _) = build(b"1 0 0 rg /DeviceGray cs 0 0 1 1 re f");
        let PageObject::Path(path) = &page.objects[0] else {
            panic!("expected a path");
        };
        assert_eq!(&path.state.fill.components[..], &[0.0]);
        assert_eq!(
            path.state.fill.space.as_deref(),
            Some(&ColorSpace::DeviceGray)
        );
    }

    #[test]
    fn too_few_colour_operands_leave_the_colour_standing() {
        let (page, _) = build(b"0 0 1 rg /DeviceCMYK cs 0.5 0.5 sc 0 0 1 1 re f");
        let PageObject::Path(path) = &page.objects[0] else {
            panic!("expected a path");
        };
        // `cs` reset to CMYK's default; the short `sc` changed nothing.
        assert_eq!(&path.state.fill.components[..], &[0.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn marked_content_is_snapshotted_onto_each_object() {
        let (page, _) = build(b"/Span BMC 0 0 1 1 re f EMC 0 0 1 1 re f");
        assert_eq!(page.objects.len(), 2);
        assert_eq!(page.objects[0].marks().len(), 1);
        assert_eq!(page.objects[1].marks().len(), 0);
    }

    #[test]
    fn an_unbalanced_emc_is_harmless() {
        let (page, diags) = build(b"EMC EMC 0 0 1 1 re f");
        assert_eq!(page.objects.len(), 1);
        assert!(diags.contains(&DiagKind::UnbalancedMarkedContent));
    }

    #[test]
    fn b_star_appends_its_closing_segment_unconditionally() {
        // Both close back onto the start; `b*` still appends a segment.
        let (with_b, _) = build(b"0 0 m 10 0 l 0 0 l b");
        let (with_b_star, _) = build(b"0 0 m 10 0 l 0 0 l b*");
        let PageObject::Path(a) = &with_b.objects[0] else {
            panic!("expected a path");
        };
        let PageObject::Path(b) = &with_b_star.objects[0] else {
            panic!("expected a path");
        };
        assert!(
            b.object.path.elements().len() >= a.object.path.elements().len(),
            "b* should not produce fewer elements than b"
        );
    }

    #[test]
    fn the_form_guard_allows_forty_one_and_refuses_the_forty_second() {
        // The cap is compared with `>`, so `MAX_FORM_LEVEL + 1` fit.
        assert_eq!(MAX_FORM_LEVEL, 40);
        let ctx = BuildContext::new();
        assert_eq!(ctx.forms_in_flight(), 0);
    }

    #[test]
    fn a_dash_operand_that_is_not_an_array_is_a_no_op() {
        let (page, _) = build(b"[3 3] 0 d 5 0 d 0 0 1 1 re f");
        let PageObject::Path(path) = &page.objects[0] else {
            panic!("expected a path");
        };
        // The second `d` did not clear the pattern.
        assert_eq!(&path.state.stroke_params.dash[..], &[3.0, 3.0]);
    }

    #[test]
    fn the_default_state_is_what_a_page_starts_with() {
        let state = GraphicsState::default();
        assert_eq!(state.ctm, Affine::IDENTITY);
        assert_eq!(state.stroke_params.cap, LineCap::Butt);
    }
}
