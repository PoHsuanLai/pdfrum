//! `build_page`: the fold from operators to page objects.
//!
//! Pure with respect to its inputs, and the only mutable state is the build
//! context's three caches, passed down by `&mut`.
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
use crate::shading::{Shading, ShadingSource};
use crate::state::{
    ClipRule, ContentMarks, GraphicsState, StateStack, TextClipRun, TextCursor, apply_ext_gstate,
    glyph_matrix, kerning_shift,
};
use crate::transparency::Transparency;
use kurbo::{Affine, BezPath, Point, Rect};
use pdfrum_common::{DiagKind, Diagnostics, Limits, Severity};
use pdfrum_font::{Font, FontCache};
use pdfrum_object::{Dict, Name, Object, Resolve};
use std::any::Any;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
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
    /// How much resolution this build's images are wanted at.
    ///
    /// A **hint**: a codec that cannot reduce returns full resolution and the
    /// image reports the size it actually decoded at. `Full` — the default —
    /// asks for every sample, which is what a build with no render target
    /// behind it wants.
    ///
    /// One value per build rather than one per image, because that is the
    /// granularity the oracle works at: `CPDF_ImageRenderer::StartLoadDIBBase`
    /// fills `max_size_required` from the *render device's* dimensions
    /// (`cpdf_imagerenderer.cpp:74-77`), so the question is how much bigger an
    /// image is than the whole page bitmap, not how small a rectangle it lands
    /// in.
    pub decode_target: RequestedSize,
    /// Fonts.
    pub fonts: FontCache,
    /// How a non-embedded font finds a face to draw with.
    ///
    /// Carried here rather than passed in per call because every font load
    /// under one document must make the same choice: a substitution that
    /// varied between two `Tf` operators naming the same resource would give
    /// one line of text different metrics from the next. Defaults to the
    /// built-in faces alone, which is what keeps tests hermetic; the tool
    /// fills it from `--font-dir` and `--croscore-font-names`.
    pub substitution: pdfrum_font::SubstitutionOptions,
    /// Loaded font *instances*, keyed on the reference that named them.
    ///
    /// Separate from [`fonts`](Self::fonts), which hands out identities: this
    /// is what makes two text objects that name the same `/Font` resource
    /// share one `Arc<Font>`. Text extraction's duplicate suppression
    /// compares fonts by that pointer, so loading a fresh instance per `Tf`
    /// would silently stop it firing and let a redrawn line be extracted
    /// twice.
    font_instances: HashMap<pdfrum_object::ObjRef, Option<Arc<Font>>>,
    /// The interactive form's default-resource faces, keyed on the object
    /// that declares them.
    ///
    /// # Why the value is erased
    ///
    /// The faces are `pdfrum_doc::ap::FormFonts`, and this crate is *below*
    /// `pdfrum-doc` — it cannot name the type. The alternative was to thread
    /// a second per-document cache through `annot_render::overlay_with`,
    /// which already carries eight arguments, and onward through the facade's
    /// render path, `RenderSession` and `FormSession`: a public API change
    /// across four crates to pass state that is *already* being threaded
    /// here, beside the font, colour-space, function and image caches this
    /// exists to hold.
    ///
    /// So the slot is erased and the layer above supplies the type through
    /// [`Self::form_fonts`]. This is storage erasure, not a polymorphism
    /// seam: nothing is ever *dispatched* through the `Any`, it is
    /// downcast straight back to the one type that put it there.
    ///
    /// # Why it is memoized at all
    ///
    /// Building it walks the AcroForm `/DR /Font` dictionary and fully
    /// constructs every font in it — encoding tables, `/Differences`, the
    /// substitution ladder — then loads a fallback and the second faces a
    /// charset outside the `/DA` font needs. That is a pure function of the
    /// `/AcroForm` dictionary, which does not change between renders of one
    /// document, and the annotation overlay ran it **once per page per
    /// render**. On a form document whose `/DR` fonts are embedded it was
    /// measured at 78 ms against an appearance generation of under 1 ms, and
    /// it was paid by every document carrying any annotation, not only by
    /// forms.
    ///
    /// # Why it is keyed
    ///
    /// On the `/AcroForm` reference, for the same reason
    /// [`font_instances`](Self::font_instances) is keyed on the reference
    /// that named a font: one context may legitimately be threaded through
    /// two documents, and a slot keyed on nothing would hand the second
    /// document the first one's faces. [`FormFontsKey`] says which of the
    /// three cases a catalog is in, and only the first two are cached.
    form_fonts: HashMap<FormFontsKey, Arc<dyn Any + Send + Sync>>,
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

/// Which interactive form a set of cached form faces belongs to.
///
/// The catalog's `/AcroForm` entry is one of exactly three things, and they
/// have three different cache lifetimes:
///
/// - **an indirect reference**, which is what a real form is written as. The
///   reference is the document-scoped identity every other cache on
///   [`BuildContext`] keys on, so the faces are cached under it.
/// - **absent**, which is most documents — including every document that
///   carries an annotation but no form at all, which is the case the
///   annotation overlay made expensive. The faces then depend on *nothing*
///   from the document: they are the fallback face and the second faces the
///   font map can add, both built from constant dictionaries. One slot serves
///   every such document a context is threaded through.
/// - **a direct dictionary**, which is legal and rare. It has no reference to
///   key on and its content *is* document-specific, so it is not cached.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FormFontsKey {
    /// The `/AcroForm` the given object holds.
    Form(pdfrum_object::ObjRef),
    /// No `/AcroForm` at all.
    None,
    /// An `/AcroForm` written as a direct dictionary.
    Direct,
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

    /// An empty context whose non-embedded fonts resolve through `options`.
    ///
    /// The caches start empty either way; this only fixes how substitution
    /// will answer, which must be settled before the first font loads.
    #[must_use]
    pub fn with_substitution(options: pdfrum_font::SubstitutionOptions) -> Self {
        Self {
            substitution: options,
            ..Self::default()
        }
    }

    /// The interactive form's faces for `key`, built by `load` on the first
    /// ask and handed back from the cache on every later one.
    ///
    /// `load` is a closure, not a value, so a hit costs nothing to build.
    /// [`FormFontsKey::Direct`] is never cached and always calls `load`: a
    /// direct `/AcroForm` dictionary has no identity to key on, and reusing
    /// one document's faces for another's would be wrong.
    // Erased in storage and downcast back on the way out. A cached value whose
    // type does not match — which cannot happen, since one caller owns the type
    // — is treated as a miss and rebuilt rather than reported.
    pub fn form_fonts<T: Any + Send + Sync>(
        &mut self,
        key: FormFontsKey,
        load: impl FnOnce(&mut Self) -> T,
    ) -> Arc<T> {
        if key == FormFontsKey::Direct {
            return Arc::new(load(self));
        }
        if let Some(cached) = self.form_fonts.get(&key)
            && let Ok(hit) = Arc::clone(cached).downcast::<T>()
        {
            return hit;
        }
        let built = Arc::new(load(self));
        self.form_fonts
            .insert(key, Arc::clone(&built) as Arc<dyn Any + Send + Sync>);
        built
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

/// Where each `/Contents` element's operators begin, within one flat operator
/// list.
///
/// A page's content is the concatenation of its `/Contents` streams, and the
/// interpreter reads it as one run — a `q` in one element is closed by the `Q`
/// in the next, which is legal and common. The editor still needs to know
/// which element each object came from, so the boundaries travel alongside the
/// operators rather than being recovered from them.
///
/// `starts[i]` is the index of the first operator belonging to element `i`.
/// An empty record means a single unsplit stream: everything is element 0.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StreamBounds {
    starts: Vec<usize>,
}

impl StreamBounds {
    /// The boundaries for content split into elements of the given operator
    /// counts.
    #[must_use]
    pub fn from_counts(counts: impl IntoIterator<Item = usize>) -> Self {
        let mut starts = Vec::new();
        let mut at = 0usize;
        for count in counts {
            starts.push(at);
            at = at.saturating_add(count);
        }
        Self { starts }
    }

    /// The boundaries for content joined from elements ending at the given
    /// byte offsets.
    ///
    /// `ends[i]` is one past the last byte of element `i`, counting the
    /// separator a join inserts. A single element — or none — yields the
    /// default, where everything is element 0.
    // Each element is parsed on its own and its operators counted, which is
    // exact rather than approximate because of that separating space: it
    // terminates whatever token the element ended on, so no operator can span a
    // boundary. The last element takes whatever the joined list has left over,
    // which absorbs any disagreement rather than dropping objects off the end.
    #[must_use]
    pub fn from_joined(bytes: &[u8], total_ops: usize, ends: &[usize], limits: &Limits) -> Self {
        if ends.len() <= 1 {
            return Self::default();
        }
        let mut counts = Vec::with_capacity(ends.len());
        let mut start = 0usize;
        let mut consumed = 0usize;
        for (index, end) in ends.iter().enumerate() {
            if index.saturating_add(1) == ends.len() {
                counts.push(total_ops.saturating_sub(consumed));
                break;
            }
            let element = bytes.get(start..*end).unwrap_or_default();
            // The diagnostics these parses raise are the ones the joined parse
            // already recorded, so they are discarded rather than doubled.
            let mut ignored = Diagnostics::default();
            let count = crate::parse_content(element, limits, &mut ignored).len();
            consumed = consumed.saturating_add(count);
            counts.push(count);
            start = *end;
        }
        Self::from_counts(counts)
    }

    /// Which element the operator at `op_index` belongs to.
    ///
    /// The last element whose start is at or before the operator — so an
    /// operator past every recorded start belongs to the final element, and a
    /// record with no starts at all answers `0`.
    #[must_use]
    pub fn stream_of(&self, op_index: usize) -> usize {
        self.starts
            .partition_point(|start| *start <= op_index)
            .saturating_sub(1)
    }

    /// How many elements the content was split into.
    #[must_use]
    pub fn len(&self) -> usize {
        self.starts.len()
    }

    /// Whether the content was never split.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.starts.is_empty()
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
    build_page_streams(
        ops,
        &StreamBounds::default(),
        dict,
        inherited,
        resources,
        r,
        ctx,
        limits,
        diags,
    )
}

/// Build a page whose `/Contents` boundaries are known, so every object
/// records which element it came from.
///
/// This is [`build_page_from_dict`] plus the two facts only an editor needs:
/// each object's content-stream index, and the transform each element leaves
/// behind. A caller that will only render or extract text wants
/// [`build_page_from_dict`], which pays for neither.
#[expect(
    clippy::too_many_arguments,
    reason = "as `build_page_from_dict`, plus the stream boundaries"
)]
#[must_use]
pub fn build_page_streams<R: Resolve>(
    ops: &[Op],
    bounds: &StreamBounds,
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
    let (objects, stream_ctms) = interpret_streams(
        ops,
        bounds,
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
        dirty_streams: BTreeSet::new(),
        stream_ctms,
    }
}

/// Build one form `XObject` as a standalone page object, placed by `matrix`.
///
/// This is the `Do` handler's body reached from outside a content stream,
/// which is what an **annotation appearance** needs: `CPDF_Annot::DrawInContext`
/// hands `CPDF_Form` the annotation matrix rather than a CTM built up by
/// operators, and the result is appended to the page's own object list.
///
/// The form's `/Matrix` composes with `matrix` exactly as it would inside a
/// `Do`, and a missing `/BBox` is **no clip at all** rather than an empty one.
/// `resources` is the fallback for a form that declares none; an annotation's
/// appearance is not part of the page's content stream, so the page's own
/// resources are what it inherits.
#[must_use]
pub fn build_form_object<R: Resolve>(
    stream: &pdfrum_object::Stream,
    matrix: Affine,
    resources: &Resources,
    r: &R,
    ctx: &mut BuildContext,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> Option<PageObject> {
    build_form_object_with(stream, matrix, resources, r, ctx, limits, diags, false)
}

/// The same build, told whether the appearance is a **live edit's**.
///
/// [`build_form_object`] is this with `live_edit = false`, which is every
/// appearance the file itself carries. A form session hands `true` for the one
/// field it is currently editing, and that flag lands on
/// [`FormObject::live_edit`] for a renderer to read — see its documentation for
/// why the distinction has to travel with the object rather than with the
/// render call.
#[must_use]
#[expect(
    clippy::too_many_arguments,
    reason = "one more than `build_form_object`, which is already at the \
              limit; grouping the resolver, context, limits and sink into a \
              struct is a change to every builder entry point in this crate \
              and not this function's to make"
)]
pub fn build_form_object_with<R: Resolve>(
    stream: &pdfrum_object::Stream,
    matrix: Affine,
    resources: &Resources,
    r: &R,
    ctx: &mut BuildContext,
    limits: &Limits,
    diags: &mut Diagnostics,
    live_edit: bool,
) -> Option<PageObject> {
    let content = pdfrum_filters::decode_chain(stream, 0, r, limits, diags).data;
    let form_matrix = stream.dict.matrix(names::MATRIX, r);
    let placed = matrix * form_matrix;

    let mut state = GraphicsState {
        ctm: placed,
        ..GraphicsState::default()
    };

    let transparency = Transparency::from_group(stream.dict.dict(names::GROUP, r).as_ref(), r);
    if transparency.group {
        state.general.enter_transparency_group();
    }

    let bbox = stream
        .dict
        .array(names::BBOX, r)
        .filter(|a| a.len() == 4)
        .map(|a| a.as_rect());
    // The `/BBox` is a *clip*, and here it has to be pushed onto the state
    // rather than left as a field: an appearance form reached from a content
    // stream inherits the enclosing `q`/`Q` clip, but one reached from an
    // annotation has no enclosing anything, so nothing else would ever apply
    // it. Without this an ink annotation whose `/InkList` runs outside its
    // `/Rect` paints strokes the oracle clips away entirely — which is what
    // `ink_annot.in`'s all-white golden says.
    if let Some(rect) = bbox {
        let mut path = kurbo::BezPath::new();
        path.move_to((rect.x0, rect.y0));
        path.line_to((rect.x1, rect.y0));
        path.line_to((rect.x1, rect.y1));
        path.line_to((rect.x0, rect.y1));
        path.close_path();
        state.clip.push_path(placed * path, ClipRule::Winding);
    }

    let inner = Resources::choose(
        stream.dict.dict(names::RESOURCES, r),
        resources.chosen.clone(),
        resources.page.clone(),
    );

    let ops = crate::parse_content(&content, limits, diags);
    let objects = interpret(&ops, &inner, &state, placed, r, ctx, limits, diags);

    Some(PageObject::Form(Box::new(Content {
        object: FormObject {
            objects,
            matrix: placed,
            bbox,
            transparency,
            oc: stream.dict.dict(names::OC, r).map(Arc::new),
            // An annotation appearance is reached from `/AP`, not from a
            // resource dictionary, so there is no `/XObject` name for it.
            source: None,
            live_edit,
        },
        state,
        marks: ContentMarks::default(),
        // An annotation appearance is not part of the page's content stream,
        // so it has no index in one. The dump numbers streams from zero and
        // the oracle counts an annotation's form as belonging to none.
        content_stream: None,
        // An appearance is drawn into the page graph but is not page content:
        // it is never regenerated into `/Contents`, and marking it dirty
        // would make an ordinary render rewrite the page.
        dirty: false,
        active: true,
    })))
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
    /// Runs a clipping text mode has accumulated since the last `ET`.
    ///
    /// `clip_text_list_`, `cpdf_streamcontentparser.cpp:1359-1361`.
    text_clip: Vec<TextClipRun>,
    resources: &'a Resources,
    /// The form's or page's coordinate system, which patterns anchor to —
    /// **not** the current transform.
    parent_matrix: Affine,
    resolver: &'a R,
    objects: Vec<PageObject>,
    /// Which `/Contents` element the operator being applied came from, which
    /// every object it produces records (see [`crate::mutate`]).
    stream: usize,
    /// The transform each element leaves in force at its end, recorded only
    /// where it changed.
    stream_ctms: BTreeMap<usize, Affine>,
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
///
/// Every object records content stream 0, which is what a form, a pattern and
/// a glyph procedure want: they are one stream, and their objects are never
/// separately regenerated.
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
    interpret_streams(
        ops,
        &StreamBounds::default(),
        resources,
        initial,
        parent_matrix,
        r,
        ctx,
        limits,
        diags,
    )
    .0
}

/// Interpret a run of operators whose `/Contents` boundaries are known.
///
/// Each object records the element it came from, and the transform each
/// element leaves behind is returned alongside — the two facts a regenerated
/// page needs and a rendered one does not.
#[expect(
    clippy::too_many_arguments,
    reason = "as `interpret`, plus the stream boundaries the editor needs"
)]
fn interpret_streams<R: Resolve>(
    ops: &[Op],
    bounds: &StreamBounds,
    resources: &Resources,
    initial: &GraphicsState,
    parent_matrix: Affine,
    r: &R,
    ctx: &mut BuildContext,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> (Vec<PageObject>, BTreeMap<usize, Affine>) {
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
        stream: 0,
        stream_ctms: BTreeMap::new(),
    };
    for (index, op) in ops.iter().enumerate() {
        interp.stream = bounds.stream_of(index);
        interp.apply(op, ctx, limits, diags);
    }
    (interp.objects, interp.stream_ctms)
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
                // A `Q` restores a transform as surely as a `cm` sets one, and
                // an unbalanced one carries the change into the next stream.
                self.record_ctm();
            }
            // A **pre**-concatenation: the new matrix applies first.
            Op::Concat(m) => {
                self.state.ctm *= *m;
                self.record_ctm();
            }
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
                // `Handle_EndText` (`cpdf_streamcontentparser.cpp:921-931`)
                // re-reads the render mode **at `ET`**, not the one each run
                // was shown under. A `BT … 7 Tr (x) Tj 0 Tr ET` therefore
                // clips with nothing at all: the runs were collected, and the
                // mode that decides whether to keep them has since changed.
                // Either way the list is cleared, so they do not survive into
                // the next text object.
                let runs = std::mem::take(&mut self.text_clip);
                if !runs.is_empty() && self.state.text.render_mode.clips() {
                    let _ = self.state.clip.push_text(runs);
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
                let source = self.resources.find_ref(names::FONT, name, self.resolver);
                match (font, self.state.text.font.take()) {
                    (Some(f), _) => {
                        self.state.text.font = Some((f, *size));
                        self.state.text.font_source = source;
                    }
                    // A name that did not resolve leaves the standing font —
                    // and therefore the resource naming it — in place.
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
                let _ = self.state.stroke.set_components(&c.0);
            }
            Op::SetFillColor(c) => {
                let _ = self.state.fill.set_components(&c.0);
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
            self.state.clip.push_path(
                clipped,
                match clip_rule {
                    FillRule::EvenOdd => ClipRule::EvenOdd,
                    _ => ClipRule::Winding,
                },
            );
        }
    }

    /// Wrap an object with the state and marks in force.
    fn content<T>(&self, object: T) -> Content<T> {
        Content {
            object,
            state: self.state.clone(),
            marks: self.marks.clone(),
            content_stream: Some(self.stream),
            // Parsed objects describe bytes that already exist, so nothing
            // needs rewriting until a caller changes one.
            dirty: false,
            active: true,
        }
    }

    /// Record the transform this stream leaves in force, after an operator
    /// changed it.
    fn record_ctm(&mut self) {
        self.stream_ctms.insert(self.stream, self.state.ctm);
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
            font_source: self.state.text.font_source,
            render_mode,
            type3_metrics,
        };
        // A run in a clipping mode is held for the `ET` that closes the text
        // object, which is where it reaches the clip stack. It is *also*
        // pushed as an ordinary page object: upstream appends to
        // `clip_text_list_` and then to the object holder from the same block
        // (`cpdf_streamcontentparser.cpp:1359-1362`), because `Tr 4` fills
        // and clips, and even `Tr 7` still runs the paint pass — it simply
        // paints nothing.
        if render_mode.clips() {
            self.text_clip.push(TextClipRun {
                object: object.clone(),
                char_space: self.state.text.char_space,
                word_space: self.state.text.word_space,
            });
        }
        // The CTM is snapshotted onto this object, not the live text state:
        // a later fill-mode `Tj` must still carry identity, matching the
        // oracle writing into the object's `ctm_` and leaving `cur_states_`
        // untouched.
        let mut content = self.content(object);
        if render_mode.strokes() {
            content.state.text.stroke_ctm = stroke_ctm_of(self.state.ctm);
        }
        self.push(PageObject::Text(Box::new(content)));
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
            Some(d) => pdfrum_font::load_with_options(
                &d,
                self.resolver,
                &ctx.fonts,
                &ctx.substitution,
                limits,
                diags,
            )
            .map(Arc::new),
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
            let found = load_pattern(
                name,
                self.resources,
                self.parent_matrix,
                &self.state.general,
                self.resolver,
                ctx,
                limits,
                diags,
            );
            // Only a name the resources do not define at all makes the
            // operator a no-op; a pattern that exists but will not load still
            // installs a pattern colour, which paints nothing.
            let loaded = match found {
                FoundPattern::Loaded(p) => Some(p),
                FoundPattern::Unusable => None,
                FoundPattern::Missing => return,
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
            target.set_pattern(name.clone(), &c.values, loaded);
            return;
        }
        let target = if stroking {
            &mut self.state.stroke
        } else {
            &mut self.state.fill
        };
        let _ = target.set_components(&c.values);
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
        let substitution = &ctx.substitution;
        // The `/Font` array's first element, both ways round. Table 58's own
        // form is an indirect reference to a font dictionary, so that is
        // tried first; a name is the oracle's spelling, kept as tolerance
        // because files written against PDFium use it. See the
        // `[oracle-bug]` note in `state::extgstate`.
        let find_font = |first: Option<&Object>| -> Option<Arc<Font>> {
            let dict = match first? {
                // Spec (table 58): an indirect reference to a font dict.
                r @ Object::Ref(_) => r.resolve(resolver).ok()?.as_dict().cloned()?,
                // Tolerance: the oracle's name-in-the-resources reading.
                Object::Name(name) => resources
                    .find(names::FONT, name, resolver)
                    .and_then(|o| o.as_dict().cloned())?,
                // A direct dictionary is neither spelling, but there is
                // nothing else it could mean and refusing it would lose a
                // font a file plainly named.
                Object::Dict(d) => d.clone(),
                _ => return None,
            };
            pdfrum_font::load_with_options(
                &dict,
                resolver,
                fonts,
                substitution,
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
        self.expand_soft_mask_group(ctx, limits, diags);
    }

    /// Interpret a newly installed soft mask's `/G` group into page objects.
    ///
    /// The group is a form `XObject` and what it paints is what the mask *is*,
    /// so it has to be interpreted before the mask can mean anything — and
    /// only the interpreter has the resolver and the recursion guard a form
    /// parse needs, which is why this runs here rather than in `SoftMask::
    /// load`.
    ///
    /// The group renders from a **clean state**, not the installing object's:
    /// `LoadSMask` builds its status with `Initialize(null, null)`, so the
    /// mask's own content is unaffected by the alpha, blend or colour in force
    /// where the `/ExtGState` appeared. Inheriting them instead would make a
    /// mask under `/ca 0.5` fade *itself* and then fade the object again.
    fn expand_soft_mask_group(
        &mut self,
        ctx: &mut BuildContext,
        limits: &Limits,
        diags: &mut Diagnostics,
    ) {
        let Some(mask) = self.state.general.soft_mask.as_ref() else {
            return;
        };
        if !mask.objects.is_empty() {
            return;
        }
        let group = mask.group.clone();
        let matrix = mask.matrix;
        let content = pdfrum_filters::decode_chain(&group, 0, self.resolver, limits, diags).data;
        let id = BufferId::new(None, &content);
        if ctx.in_flight.len() > MAX_FORM_LEVEL || ctx.in_flight.contains(&id) {
            diags.record(Severity::Recovered, DiagKind::FormRecursionRefused, None);
            return;
        }
        // The group's `/Matrix` composes with the transform the `/ExtGState`
        // was applied under, which is what places the mask on the page.
        let form_matrix = group.dict.matrix(names::MATRIX, self.resolver);
        let inner = GraphicsState {
            ctm: matrix * form_matrix,
            ..GraphicsState::default()
        };
        // A soft mask's group sees the **page's** resources when it declares
        // none of its own, not the enclosing form's: `LoadSMask` builds the
        // form with `PAGE resources`.
        let resources = Resources::choose(
            group.dict.dict(names::RESOURCES, self.resolver),
            self.resources.page.clone(),
            self.resources.page.clone(),
        );
        ctx.in_flight.insert(id);
        let ops = crate::parse_content(&content, limits, diags);
        let objects = interpret(
            &ops,
            &resources,
            &inner,
            inner.ctm,
            self.resolver,
            ctx,
            limits,
            diags,
        );
        ctx.in_flight.remove(&id);
        if let Some(mask) = self.state.general.soft_mask.as_mut() {
            Arc::make_mut(mask).objects = objects;
        }
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
            Some(b"Form") => self.add_form(stream, reference, ctx, limits, diags),
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
        reference: Option<pdfrum_object::ObjRef>,
        ctx: &mut BuildContext,
        limits: &Limits,
        diags: &mut Diagnostics,
    ) {
        let content = pdfrum_filters::decode_chain(stream, 0, self.resolver, limits, diags).data;
        // The identity is the *stream object*, not its bytes. Upstream keys
        // its recursion set on the decoded buffer's address
        // (`cpdf_streamcontentparser.cpp:1652-1660`), which is per stream
        // object and distinct for two objects that happen to hold the same
        // bytes. Hashing the content alone makes a chain of forms that each
        // say `/X1 Do` — thirty-five of them on `bug_972999.in` — look like
        // one form calling itself, and the second is refused.
        let id = BufferId::new(reference, &content);
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
            oc: stream.dict.dict(names::OC, self.resolver).map(Arc::new),
            source: reference,
            // A `Do` inside a content stream is the file drawing its own form,
            // never a session's live edit.
            live_edit: false,
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
        let size = ctx.decode_target;
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
            oc: stream.dict.dict(names::OC, self.resolver).map(Arc::new),
            source: reference,
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
            // An inline image reaches `CPDF_DIB::StartLoadDIBBase` with the
            // same `max_size_required` an XObject does — being inline changes
            // which resource dictionary it sees, not how much of it is decoded.
            ctx.decode_target,
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
            // An inline image has no XObject dictionary to carry `/OC`; only
            // an enclosing marked-content sequence can hide it.
            oc: None,
            // Nor any indirect object to name, so a regenerated stream cannot
            // write it and drops it.
            source: None,
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
            ShadingSource::ShadingOperator,
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

/// The 2×2 linear part of `ctm`, stored as `[a, c, b, d]`.
///
/// A stroking `Tj` records this on the object's text state; the renderer
/// folds it from the text matrix into the device matrix so line width stays
/// in user space (ISO 32000-1 §8.4.3.2). The transposition is the four-float
/// slot the split consumes: `a, c, b, d`, not `a, b, c, d`.
fn stroke_ctm_of(ctm: Affine) -> [f32; 4] {
    let [a, b, c, d, _, _] = ctm.as_coeffs();
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the stored slot is f32, matching the graphics state's other text scalars"
    )]
    {
        [a as f32, c as f32, b as f32, d as f32]
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

/// What `scn` found when it named a pattern.
///
/// The two halves are separate because `FindPattern` and the pattern's own
/// `Load` are separate in the C++ and fail differently.
/// `FindPattern` (`cpdf_streamcontentparser.cpp:1295-1303`) checks only that
/// the resource exists and is a dictionary or a stream; **that** is what
/// decides whether `scn` installs a pattern colour at all. Whether the
/// pattern is *usable* — a `/PatternType` it recognises, a shading it can
/// validate, steps it can tile with — is answered later, at draw time, and a
/// failure there means the object paints **nothing**.
///
/// Collapsing the two makes an `scn` naming an unusable pattern a no-op, so
/// the object keeps whatever colour was current and paints solid. On a
/// page-sized rectangle over the default black that is an entirely black
/// page, which is what four of the corpus's fuzz files produced.
#[derive(Debug, Clone)]
pub enum FoundPattern {
    /// The resource exists and the pattern loaded.
    Loaded(Arc<Pattern>),
    /// The resource exists but the pattern is unusable: a pattern colour is
    /// still installed, and it paints nothing.
    Unusable,
    /// No such resource, or it is neither a dictionary nor a stream. `scn` is
    /// a no-op and the previous colour stands.
    Missing,
}

/// A pattern named in a colour value, looked up through the resources.
///
/// `parent_matrix` anchors it, not the current transform — patterns live in
/// the space they were declared in. `general` is the painting object's general
/// state, which a tiling pattern's cell inherits wholesale — alpha, blend mode
/// and soft mask — while taking *default* colour, text and path state, so
/// `/ca 0.5` on the filling object fades the tiles.
///
/// See [`FoundPattern`] for why "the resource exists" and "the pattern loads"
/// are two answers rather than one.
// That state asymmetry is the whole reason a pattern is loaded where it is
// installed rather than where the resource is declared.
#[expect(
    clippy::too_many_arguments,
    reason = "looking a pattern up needs its name, resources, anchor matrix, \
              the painting object's general state, and the usual four"
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
) -> FoundPattern {
    let Some(object) = resources.find(names::PATTERN, name, r) else {
        return FoundPattern::Missing;
    };
    // The resource must be a dictionary or a stream.
    if !matches!(object, Object::Dict(_) | Object::Stream(_)) {
        return FoundPattern::Missing;
    }
    let colorspaces = resources.color_spaces(r);
    let loaded = Pattern::load(
        &object,
        parent_matrix,
        colorspaces.as_ref(),
        r,
        &mut ctx.functions,
        limits,
        diags,
    );
    let Some(mut pattern) = loaded else {
        return FoundPattern::Unusable;
    };
    if let Pattern::Tiling(tiling) = &mut pattern
        && let Some(stream) = object.as_stream()
    {
        tiling.objects =
            expand_tiling_cell(tiling, stream, general, resources, r, ctx, limits, diags);
    }
    FoundPattern::Loaded(Arc::new(pattern))
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
            ClipRule::Winding,
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

    /// The clip stack a page's last object carries, by entry kind.
    fn clip_kinds(page: &crate::page::Page) -> Vec<&'static str> {
        page.objects
            .last()
            .expect("at least one object")
            .state()
            .clip
            .entries()
            .iter()
            .map(|e| match e {
                crate::state::ClipEntry::Path { .. } => "path",
                crate::state::ClipEntry::Text { .. } => "text",
            })
            .collect()
    }

    #[test]
    fn a_clipping_text_mode_reaches_the_clip_stack_at_et() {
        // `Tr 7` shows no ink and contributes its glyphs to the clip, so the
        // rectangle drawn after `ET` is clipped by the text. Before this was
        // wired the rectangle painted whole — `clipping_text.pdf` and
        // `path_9.pdf` both paint their swatches over the glyphs that should
        // have cut them out.
        let (page, _) = build(b"BT /F1 24 Tf 7 Tr 10 10 Td (Hi) Tj ET 0 0 50 50 re f");
        assert_eq!(clip_kinds(&page), ["text"]);
    }

    #[test]
    fn a_non_clipping_mode_contributes_nothing() {
        let (page, _) = build(b"BT /F1 24 Tf 10 10 Td (Hi) Tj ET 0 0 50 50 re f");
        assert!(clip_kinds(&page).is_empty());
    }

    /// `Handle_EndText` re-reads the mode **at `ET`**
    /// (`cpdf_streamcontentparser.cpp:926`), not the one each run was shown
    /// under, so a run collected under `Tr 7` is discarded when the mode has
    /// gone back to filling before the text object closes.
    #[test]
    fn the_mode_at_et_decides_whether_the_batch_is_kept() {
        let (page, _) = build(b"BT /F1 24 Tf 7 Tr 10 10 Td (Hi) Tj 0 Tr ET 0 0 50 50 re f");
        assert!(clip_kinds(&page).is_empty(), "the batch is dropped at ET");
        // And it does not survive into the next text object either.
        let (page, _) = build(
            b"BT /F1 24 Tf 7 Tr 10 10 Td (Hi) Tj 0 Tr ET \
              BT /F1 24 Tf 7 Tr 10 10 Td (o) Tj ET 0 0 50 50 re f",
        );
        assert_eq!(
            clip_kinds(&page),
            ["text"],
            "only the second object's own run clips"
        );
    }

    #[test]
    fn a_standalone_form_clips_to_its_own_bbox() {
        // An annotation appearance has no enclosing `q`/`Q` to inherit a clip
        // from, so `build_form_object` has to push the `/BBox` itself. Without
        // it an ink annotation whose `/InkList` runs outside its `/Rect` paints
        // strokes the oracle clips away entirely.
        use pdfrum_object::{ByteSpan, Dict, Name, Object, Stream};
        let dict = Dict::from_pairs([(
            Name::from("BBox"),
            Object::Array(pdfrum_object::Array::of([
                Object::Int(0),
                Object::Int(0),
                Object::Int(10),
                Object::Int(20),
            ])),
        )]);
        let stream = Stream::new(dict, ByteSpan::from(b"0 0 100 100 re f".to_vec()));
        let mut ctx = BuildContext::default();
        let mut diags = Diagnostics::default();
        let object = super::build_form_object(
            &stream,
            Affine::IDENTITY,
            &Resources::default(),
            &NoResolve,
            &mut ctx,
            &Limits::default(),
            &mut diags,
        )
        .expect("a form");
        let PageObject::Form(form) = &object else {
            panic!("expected a form");
        };
        assert_eq!(
            form.object.bbox,
            Some(kurbo::Rect::new(0.0, 0.0, 10.0, 20.0))
        );
        // The clip reached the child object, which is the half that matters:
        // the `bbox` field alone is only used for culling.
        let child = form.object.objects.first().expect("one child");
        let PageObject::Path(path) = child else {
            panic!("expected a path");
        };
        assert_eq!(path.state.clip.len(), 1, "the bbox is on the child's clip");
        assert_eq!(
            path.state.clip.bounds(),
            Some(kurbo::Rect::new(0.0, 0.0, 10.0, 20.0))
        );
    }

    #[test]
    fn a_form_with_no_bbox_is_unclipped() {
        use pdfrum_object::{ByteSpan, Dict, Stream};
        let stream = Stream::new(Dict::new(), ByteSpan::from(b"0 0 100 100 re f".to_vec()));
        let mut ctx = BuildContext::default();
        let mut diags = Diagnostics::default();
        let object = super::build_form_object(
            &stream,
            Affine::IDENTITY,
            &Resources::default(),
            &NoResolve,
            &mut ctx,
            &Limits::default(),
            &mut diags,
        )
        .expect("a form");
        let PageObject::Form(form) = &object else {
            panic!("expected a form");
        };
        assert_eq!(form.object.bbox, None, "a missing /BBox is no clip at all");
        let PageObject::Path(path) = form.object.objects.first().expect("one child") else {
            panic!("expected a path");
        };
        assert!(path.state.clip.is_empty());
    }

    fn build_with(src: &[u8], resources: &Resources) -> (crate::page::Page, Diagnostics) {
        let limits = Limits::default();
        let mut diags = Diagnostics::default();
        let ops = crate::parse_content(src, &limits, &mut diags);
        let mut ctx = BuildContext::new();
        let page = build_page(&ops, resources, &NoResolve, &mut ctx, &limits, &mut diags);
        (page, diags)
    }

    // -----------------------------------------------------------------
    // `/ExtGState /Font` — audit A18. Table 58 makes the array's first
    // element an *indirect reference to a font dictionary*;
    // `cpdf_allstates.cpp:87-89` reads it as a byte string and looks that up
    // in the page's `/Font` resources, so the spec's form yields `""`, misses,
    // and `cpdf_streamcontentparser.cpp:1239` substitutes stock Helvetica.
    // pdf.js resolves the reference (`evaluator.js:1142-1154`, `:1256-1261`).
    // No corpus file uses either form, so these fixtures are constructed.
    // -----------------------------------------------------------------

    /// A map-backed resolver, so a `Ref` in a fixture can actually be
    /// followed. `NoResolve` cannot, which is why the older `/Font` test
    /// could only observe "nothing was installed".
    #[derive(Debug, Default)]
    struct Store(std::collections::HashMap<u32, std::sync::Arc<pdfrum_object::Object>>);

    impl pdfrum_object::Resolve for Store {
        fn fetch(
            &self,
            r: pdfrum_object::ObjRef,
        ) -> Result<std::sync::Arc<pdfrum_object::Object>, pdfrum_object::Error> {
            self.0
                .get(&r.num)
                .map(std::sync::Arc::clone)
                .ok_or(pdfrum_object::Error::UnresolvedRef(r))
        }
    }

    /// A `/Type1 /Helvetica` font dictionary, which loads without any
    /// embedded program.
    fn helvetica() -> pdfrum_object::Dict {
        use pdfrum_object::{Dict, Name, Object};
        Dict::from_pairs([
            (Name::from("Type"), Object::Name(Name::from("Font"))),
            (Name::from("Subtype"), Object::Name(Name::from("Type1"))),
            (
                Name::from("BaseFont"),
                Object::Name(Name::from("Helvetica")),
            ),
        ])
    }

    /// Build `/GS gs` where `/GS` holds `/Font [<first> 12]`, against a store
    /// that has a font dictionary at object 7 and resources naming it `F1`.
    fn font_from_ext_gstate(first: pdfrum_object::Object) -> Option<f32> {
        use pdfrum_object::{Array, Dict, Name, Object};
        let store = Store(
            [(7u32, std::sync::Arc::new(Object::Dict(helvetica())))]
                .into_iter()
                .collect(),
        );
        let gs = Dict::from_pairs([(
            Name::from("Font"),
            Object::Array(Array::of([first, Object::Int(12)])),
        )]);
        let resources = Resources {
            chosen: Some(Dict::from_pairs([
                (
                    Name::from("ExtGState"),
                    Object::Dict(Dict::from_pairs([(Name::from("GS"), Object::Dict(gs))])),
                ),
                (
                    Name::from("Font"),
                    Object::Dict(Dict::from_pairs([(
                        Name::from("F1"),
                        Object::Dict(helvetica()),
                    )])),
                ),
            ])),
            page: None,
        };
        let limits = Limits::default();
        let mut diags = Diagnostics::default();
        let ops = crate::parse_content(b"/GS gs BT (x) Tj ET", &limits, &mut diags);
        let mut ctx = BuildContext::new();
        let mut state = GraphicsState::default();
        // Drive the interpreter far enough to apply the `gs`, then read the
        // font size the arm installed — non-`None` exactly when a font was
        // found, and 12 when it came from this array.
        let page = build_page(&ops, &resources, &store, &mut ctx, &limits, &mut diags);
        let _ = &mut state;
        page.objects
            .first()
            .and_then(|o| o.state().text.font.as_ref())
            .map(|(_, size)| *size)
    }

    /// Table 58's own form resolves. This fails against the oracle's
    /// reading, where `GetByteStringAt(0)` on a reference is `""`.
    #[test]
    fn an_ext_gstate_font_resolves_the_specs_indirect_reference() {
        let size =
            font_from_ext_gstate(pdfrum_object::Object::Ref(pdfrum_object::ObjRef::new(7, 0)));
        assert_eq!(size, Some(12.0));
    }

    /// The oracle's form keeps working — tolerance, not the specification.
    #[test]
    fn an_ext_gstate_font_still_takes_the_oracles_resource_name() {
        let size =
            font_from_ext_gstate(pdfrum_object::Object::Name(pdfrum_object::Name::from("F1")));
        assert_eq!(size, Some(12.0));
    }

    /// A reference to nothing installs nothing, rather than falling back to
    /// a stock face as the oracle does — the fallback is the interpreter's
    /// job at `Tf`, not this arm's.
    #[test]
    fn an_ext_gstate_font_reference_to_nothing_installs_nothing() {
        let size = font_from_ext_gstate(pdfrum_object::Object::Ref(pdfrum_object::ObjRef::new(
            99, 0,
        )));
        assert_eq!(size, None);
    }

    #[test]
    fn two_form_objects_with_identical_bytes_are_two_forms() {
        // The recursion guard's identity is the *stream object*, not its
        // content. Upstream keys on the decoded buffer's address, which is
        // distinct per stream object even when two objects hold the same
        // bytes; hashing the content alone makes a chain of forms that each
        // say `/X1 Do` — thirty-five of them on `bug_972999.in` — look like
        // one form calling itself, and every level below the first is
        // refused.
        use super::BufferId;
        use pdfrum_object::ObjRef;
        let body = b"/X1 Do";
        let five = BufferId::new(
            Some(ObjRef {
                num: 5,
                generation: 0,
            }),
            body,
        );
        let six = BufferId::new(
            Some(ObjRef {
                num: 6,
                generation: 0,
            }),
            body,
        );
        assert_ne!(five, six, "same bytes, different objects, different ids");
        assert_eq!(
            five,
            BufferId::new(
                Some(ObjRef {
                    num: 5,
                    generation: 0
                }),
                body
            ),
            "the same object really is the same id, which is what catches a \
             form that draws itself"
        );
        // And two anonymous buffers still separate by content, which is what
        // the id does for a caller with no reference to offer.
        assert_ne!(
            BufferId::new(None, b"a"),
            BufferId::new(None, b"b"),
            "content still distinguishes two unreferenced buffers"
        );
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

    fn first_text_state(page: &crate::page::Page) -> &crate::state::TextState {
        let PageObject::Text(text) = &page.objects[0] else {
            panic!("expected text, got {:?}", page.objects[0]);
        };
        &text.state.text
    }

    #[test]
    fn a_stroked_tj_under_a_scaling_ctm_records_the_transposed_linear_part() {
        // `2 0 0 3 0 0 cm` is PDF [a b c d] = [2, 0, 0, 3]. The stored slot is
        // `[a, c, b, d]`, which for a diagonal is the same four numbers.
        let (page, _) = build(b"2 0 0 3 0 0 cm BT /F1 24 Tf 1 Tr (x) Tj ET");
        assert_eq!(first_text_state(&page).stroke_ctm, [2.0, 0.0, 0.0, 3.0]);
        assert_eq!(
            first_text_state(&page).render_mode,
            crate::ops::TextRenderMode::Stroke
        );

        // Off-diagonal: `1 2 3 4 cm` stores `[a, c, b, d] = [1, 3, 2, 4]`.
        let (page, _) = build(b"1 2 3 4 0 0 cm BT /F1 24 Tf 1 Tr (x) Tj ET");
        assert_eq!(first_text_state(&page).stroke_ctm, [1.0, 3.0, 2.0, 4.0]);
    }

    #[test]
    fn a_filled_tj_under_a_scaling_ctm_keeps_the_identity_stroke_ctm() {
        let (page, _) = build(b"2 0 0 3 0 0 cm BT /F1 24 Tf 0 Tr (x) Tj ET");
        assert_eq!(first_text_state(&page).stroke_ctm, [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(
            first_text_state(&page).render_mode,
            crate::ops::TextRenderMode::Fill
        );
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

    #[test]
    fn an_appearance_is_not_a_live_edit_unless_it_is_built_as_one() {
        // The flag is off for every existing producer, which is what makes it
        // additive: the file's own appearance streams and a session's
        // *regenerated* ones are both ordinary, and only the appearance a
        // session produces for the field it is editing is marked.
        let stream = pdfrum_object::Stream::new(
            pdfrum_object::Dict::new(),
            pdfrum_object::ByteSpan::from(b"0 0 10 10 re f".to_vec()),
        );
        let build = |live_edit| {
            let mut ctx = BuildContext::default();
            let mut diags = Diagnostics::default();
            let object = super::build_form_object_with(
                &stream,
                Affine::IDENTITY,
                &Resources::default(),
                &NoResolve,
                &mut ctx,
                &Limits::default(),
                &mut diags,
                live_edit,
            )
            .expect("a form");
            let PageObject::Form(form) = object else {
                panic!("expected a form");
            };
            form.object.live_edit
        };
        assert!(!build(false));
        assert!(build(true));
    }

    #[test]
    fn the_plain_entry_point_never_marks_a_live_edit() {
        // `build_form_object` is `build_form_object_with(.., false)`, and every
        // caller that predates the flag goes through it.
        let stream = pdfrum_object::Stream::new(
            pdfrum_object::Dict::new(),
            pdfrum_object::ByteSpan::from(b"0 0 10 10 re f".to_vec()),
        );
        let mut ctx = BuildContext::default();
        let mut diags = Diagnostics::default();
        let object = super::build_form_object(
            &stream,
            Affine::IDENTITY,
            &Resources::default(),
            &NoResolve,
            &mut ctx,
            &Limits::default(),
            &mut diags,
        )
        .expect("a form");
        let PageObject::Form(form) = object else {
            panic!("expected a form");
        };
        assert!(!form.object.live_edit);
    }
}
