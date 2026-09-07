//! Hard resource limits, defaulting to PDFium-equivalent values.
//!
//! Every field is a cap **some reader consults**, and the defaults are the
//! C++ constants.
//!
//! A knob nothing reads is not a limit, it is a promise the type cannot keep,
//! so the rule here is that a field earns its place by having a caller. Where
//! PDFium has no cap and we chose to add one anyway the field says so and
//! names what it bounds (`max_decoded_stream_len`, `max_cmap_ranges`,
//! `max_name_tree_depth`, the script budgets); where PDFium has no cap and we
//! enforce none either, there is no field.
//!
//! Two fields are **off by default** rather than PDFium-equivalent, because
//! they are a host's ceiling on untrusted input and not a parser's: the render
//! pixel cap and the deadline. Both answer with a [`LimitExceeded`] rather
//! than a diagnostic wherever the entry point can fail — a caller who set a
//! ceiling wants to hear that it was hit, not a result with a hole in it —
//! and the infallible entry points record [`DiagKind::TimeLimitReached`](crate::DiagKind::TimeLimitReached)
//! beside the partial result they return.

use std::fmt;
use std::time::Duration;

use crate::PageIndex;
use crate::deadline::{Deadline, Operation};

/// Caps applied while reading a document. Plain configuration data: pass it
/// down, never store it in a parser struct that also owns state.
///
/// Which operations honour which cap: the parser's caps (nesting, array
/// length, xref size, object numbers, the scans, word length, the page tree,
/// stream length) are read while opening and while fetching objects; the
/// CMap and name-tree caps while loading fonts and document-level trees; the
/// script budgets by the script engine alone. [`Limits::max_render_pixels`]
/// is read by the facade's render entry points, before a target is allocated.
/// [`Limits::deadline`] is read at every boundary listed on that field.
///
/// ```
/// use pdfrum_common::Limits;
///
/// // PDFium-equivalent defaults, with one cap tightened for a fuzz budget.
/// let limits = Limits { max_array_len: 1 << 20, ..Limits::default() };
/// assert_eq!(limits.max_object_nesting, 64);
/// assert_eq!(limits.max_object_number, 25_165_824);
/// ```
// Deliberately *not* `#[non_exhaustive]`: makes struct-update
// syntax over `Default` the way callers configure options, and the attribute
// forbids exactly that outside this crate. New fields are additive here.
//
// `Clone` and not `Copy`: a [`Deadline`] shares a stop
// flag between its clones, and a `Copy` of an `Arc` is not a thing. Every
// reader takes `&Limits`; the few owners clone once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Limits {
    /// Maximum depth of nested arrays/dictionaries accepted while parsing an
    /// object body. Enforced at parse time so access code may recurse freely.
    pub max_object_nesting: u32,
    /// Maximum number of elements in one array. PDFium has no such cap; ours
    /// is consulted by the object parser (`pdfrum_parser::syntax`), the
    /// `ToUnicode` CMap reader and the Type 1 charstring decoder.
    pub max_array_len: usize,
    /// Maximum number of entries a cross-reference section may declare;
    /// one past the largest legal object number.
    pub max_xref_size: u32,
    /// Largest legal object number. PDFium: `kMaxObjectNumber` (24·2²⁰).
    pub max_object_number: u32,
    /// How far from the start of the file the `%PDF-` header is searched for.
    pub header_scan: u64,
    /// How far back from the end of the file `startxref` is searched for.
    pub startxref_scan: u64,
    /// Maximum number of bytes kept from one syntax token (names, keywords).
    /// Longer tokens are truncated, matching PDFium's word buffer.
    pub max_word_len: usize,
    /// Maximum depth of the page tree walk before it gives up.
    pub max_page_tree_depth: u32,
    /// Maximum number of pages a document may report.
    pub max_page_count: u32,
    /// Maximum number of bytes any single stream filter may produce.
    ///
    /// PDFium has *no* cap here: Flate and LZW decode until they stop, and
    /// only the size it *reports* saturates, at `kMaxTotalOutSize` = 1 GiB
    /// (`flatemodule.cpp`), silently truncating anything larger. We decline to
    /// inherit that zip-bomb surface and turn the same ceiling into a hard
    /// rejection instead; past 1 GiB the oracle's reported size has already
    /// stopped tracking its content, so no stream that decodes faithfully in
    /// the C++ changes behavior. `RunLengthDecode` keeps its own, much
    /// smaller and behaviorally load-bearing 20 MiB cap in `pdfrum-filters`.
    pub max_decoded_stream_len: usize,
    /// Maximum number of codespace ranges, and separately of wide-code CID
    /// ranges, one embedded CMap program may declare.
    ///
    /// PDFium has no cap: both lists grow with the program. The default here
    /// is one range per possible two-byte code, which no real CMap approaches
    /// and which bounds a hostile program's memory at a few megabytes; past it
    /// further ranges are dropped with a diagnostic rather than erroring, in
    /// the same spirit as `max_decoded_stream_len`.
    pub max_cmap_ranges: usize,

    /// Maximum depth of a name tree, number tree, structure tree, form-field
    /// `/Parent` walk, field-name trie, or chained `/Next` action.
    ///
    /// PDFium caps four of those six at 32 and leaves the number tree and the
    /// action chain uncapped — both of which are unbounded recursion on a
    /// cyclic file. One knob covers all six; no real document approaches it,
    /// and exceeding it answers "not found" with a diagnostic rather than
    /// erroring.
    pub max_name_tree_depth: u32,

    // ---- The script engine ----
    //
    // The first three map one-for-one onto `boa`'s `RuntimeLimits`; the
    // fourth is ours. What they do **not** bound is heap growth and regex
    // backtracking, which no `RuntimeLimits` field covers — and which a
    // V8-enabled PDFium does not bound either, measured rather than assumed
    // by a probe run against a V8-enabled build. So pdfrum is bounded where the
    // oracle hangs, and unbounded only where the oracle is too. A host
    // running untrusted documents in a shared process applies an external
    // wall-clock and RSS cap, which is the only thing that works for either.
    /// How many loop iterations one script may run before it is stopped.
    ///
    /// Roughly ten seconds of the tightest possible loop. No real form script
    /// iterates a thousand times; the number is a ceiling on a hostile file,
    /// not a budget a legitimate one has to fit inside. Exhausting it is a
    /// `Diagnostic` and the *refusing* answer from the hook that was running
    /// — never a hang, never a panic, and never a silent acceptance.
    ///
    /// PDFium has no equivalent at all: `while(true){}` under V8 runs until
    /// the process is killed from outside.
    pub max_script_loop_iterations: u64,
    /// How deep one script may recurse. `boa`'s own default.
    pub max_script_recursion: usize,
    /// How large one script's value stack may grow. `boa`'s own default.
    pub max_script_stack: usize,
    /// How deep a calculation may trigger another calculation.
    ///
    /// **One**, because a calculation that runs during another calculation
    /// is refused rather than counted down: the outer sweep is authoritative
    /// and every nested call returns immediately. The field makes that depth
    /// configurable rather than looser.
    pub max_calculate_depth: u32,

    // ---- A host's ceilings on untrusted input ----
    /// The most pixels one render may produce: width × height of the target
    /// under the render transform. `None` — the default — is no cap.
    ///
    /// PDFium has none: `pdfium_test --scale` sizes the bitmap and only
    /// `CFX_DIBitmap`'s pitch overflow refuses it. Read by the facade before
    /// the target is allocated, so a request above the cap costs nothing;
    /// exceeding it is [`LimitExceeded::RenderPixels`].
    pub max_render_pixels: Option<u64>,
    /// When every operation on the document must have stopped. `None` — the
    /// default — is no limit.
    ///
    /// A [`Deadline`] is a stop flag any thread raises
    /// ([`Deadline::manual`] and [`Deadline::stop`], the mechanism every
    /// target has) and, on targets with a clock, a budget that raises it for
    /// you ([`Deadline::after`]). A host on `wasm32` has only the flag, and
    /// its own timer.
    ///
    /// Honoured cooperatively, at the boundaries the engine already has:
    /// opening (once on entry, then every 4096 tokens of a cross-reference
    /// rebuild scan), loading a page, interpreting a content stream (every
    /// 256 operators), rasterizing (every object), extracting a page's text
    /// (on entry), and running a script (on entry). The fallible entries —
    /// open, page load, render — answer [`LimitExceeded::Time`] for a spent
    /// budget and [`LimitExceeded::Stopped`] for a raised flag; the
    /// infallible ones — the interpreter, the extractor — stop where they
    /// are, record [`DiagKind::TimeLimitReached`](crate::DiagKind::TimeLimitReached) and return what they have;
    /// a script refuses as an exhausted script budget does. PDFium has no
    /// equivalent: a host wraps the process in a timer.
    pub deadline: Option<Deadline>,
}

impl Limits {
    /// `Ok` unless a deadline is set and has passed — the one call every
    /// cooperative boundary makes. Costs one branch without a deadline.
    ///
    /// # Errors
    ///
    /// [`LimitExceeded::Time`] once the deadline has passed.
    pub fn check_deadline(&self, during: Operation) -> Result<(), LimitExceeded> {
        match &self.deadline {
            Some(deadline) => deadline.check(during),
            None => Ok(()),
        }
    }
}

/// A caller-set ceiling was hit. The error a render or an open answers when
/// a [`Limits`] field that defaults to *off* was set and exceeded.
///
/// Distinct from the parser's own caps, which are damage tolerance and answer
/// with a diagnostic or a truncated value: these are the host's, and the host
/// asked to be told. The message names the cap and what would satisfy it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LimitExceeded {
    /// A render target of `width` × `height` pixels has more than
    /// [`Limits::max_render_pixels`] allows.
    RenderPixels {
        /// The target's width in pixels.
        width: u32,
        /// The target's height in pixels.
        height: u32,
        /// The cap, in pixels.
        allowed: u64,
    },
    /// [`Limits::deadline`] passed while `during` was under way.
    Time {
        /// How much time was allowed.
        budget: Duration,
        /// What was being done.
        during: Operation,
        /// The page it was being done to, where the caller knew one.
        page: Option<PageIndex>,
    },
    /// [`Limits::deadline`] was raised by [`Deadline::stop`] while `during`
    /// was under way.
    Stopped {
        /// What was being done.
        during: Operation,
        /// The page it was being done to, where the caller knew one.
        page: Option<PageIndex>,
    },
}

impl LimitExceeded {
    /// The same error naming `page`, for the caller who knows which page the
    /// engine below it was working on. Only [`LimitExceeded::Time`] has a
    /// page; the pixel cap already names its size.
    #[must_use]
    pub fn on_page(self, page: PageIndex) -> LimitExceeded {
        match self {
            LimitExceeded::Time { budget, during, .. } => LimitExceeded::Time {
                budget,
                during,
                page: Some(page),
            },
            LimitExceeded::Stopped { during, .. } => LimitExceeded::Stopped {
                during,
                page: Some(page),
            },
            other => other,
        }
    }
}

/// A budget as a person reads it: whole seconds as `5 s`, anything finer as
/// milliseconds.
fn budget_text(budget: Duration) -> String {
    if budget.subsec_nanos() == 0 {
        format!("{} s", budget.as_secs())
    } else {
        format!("{} ms", budget.as_millis())
    }
}

/// `px` as megapixels with one decimal where it has one: `100`, `1.2`.
fn megapixels(px: u64) -> String {
    let whole = px / 1_000_000;
    let tenths = (px % 1_000_000) / 100_000;
    if tenths == 0 {
        whole.to_string()
    } else {
        format!("{whole}.{tenths}")
    }
}

impl fmt::Display for LimitExceeded {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LimitExceeded::RenderPixels {
                width,
                height,
                allowed,
            } => write!(
                f,
                "render of {width} x {height} px ({} megapixels) is above the cap of {} \
                 megapixels; render at a smaller scale or raise `Limits::max_render_pixels`",
                megapixels(u64::from(*width) * u64::from(*height)),
                megapixels(*allowed),
            ),
            LimitExceeded::Time {
                budget,
                during,
                page,
            } => write!(
                f,
                "time limit of {} exceeded while {}; allow more time or do less",
                budget_text(*budget),
                during.describe(*page),
            ),
            LimitExceeded::Stopped { during, page } => {
                write!(f, "stopped by the caller while {}", during.describe(*page))
            }
        }
    }
}

impl std::error::Error for LimitExceeded {}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_object_nesting: 64,
            max_array_len: usize::MAX,
            max_xref_size: 25_165_825,
            max_object_number: 25_165_824,
            header_scan: 1024,
            startxref_scan: 4096,
            max_word_len: 256,
            max_page_tree_depth: 1024,
            max_page_count: 0x000F_FFFF,
            max_decoded_stream_len: 1024 * 1024 * 1024,
            max_cmap_ranges: 65_536,
            max_name_tree_depth: 32,
            max_script_loop_iterations: 10_000_000,
            max_script_recursion: 512,
            max_script_stack: 10_240,
            max_calculate_depth: 1,
            max_render_pixels: None,
            deadline: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Deadline, LimitExceeded, Limits, Operation, PageIndex};
    use std::time::Duration;

    #[test]
    fn pdfium_equivalent_defaults() {
        let l = Limits::default();
        assert_eq!(l.max_object_nesting, 64);
        assert_eq!(l.max_xref_size, 25_165_825);
        assert_eq!(l.max_object_number, 25_165_824);
        assert_eq!(l.max_xref_size, l.max_object_number + 1);
        assert_eq!(l.header_scan, 1024);
        assert_eq!(l.startxref_scan, 4096);
        assert_eq!(l.max_word_len, 256);
        assert_eq!(l.max_page_tree_depth, 1024);
        assert_eq!(l.max_page_count, 1_048_575);
        assert_eq!(l.max_decoded_stream_len, 1024 * 1024 * 1024);
        assert_eq!(l.max_cmap_ranges, 65_536);
        assert_eq!(l.max_name_tree_depth, 32);
        assert_eq!(l.max_array_len, usize::MAX);
        assert_eq!(l.max_render_pixels, None, "the host's ceilings are off");
        assert_eq!(l.deadline, None);
    }

    #[test]
    fn no_deadline_is_never_exceeded_and_a_spent_one_always_is() {
        assert_eq!(Limits::default().check_deadline(Operation::Open), Ok(()));
        let spent = Limits {
            deadline: Some(Deadline::after(Duration::ZERO)),
            ..Limits::default()
        };
        assert!(matches!(
            spent.check_deadline(Operation::Extract),
            Err(LimitExceeded::Time {
                during: Operation::Extract,
                page: None,
                ..
            })
        ));
    }

    #[test]
    fn the_time_message_names_the_budget_the_work_and_the_page() {
        let e = LimitExceeded::Time {
            budget: Duration::from_secs(5),
            during: Operation::Render,
            page: Some(PageIndex::new(3)),
        };
        assert_eq!(
            e.to_string(),
            "time limit of 5 s exceeded while rendering page 3; allow more time or do less"
        );
        let e = LimitExceeded::Time {
            budget: Duration::from_millis(250),
            during: Operation::Open,
            page: None,
        };
        assert_eq!(
            e.to_string(),
            "time limit of 250 ms exceeded while opening the document; allow more time or do less"
        );
        let e = LimitExceeded::Time {
            budget: Duration::from_secs(1),
            during: Operation::Interpret,
            page: None,
        };
        assert!(e.to_string().contains("while interpreting page;"));
        let e = LimitExceeded::Stopped {
            during: Operation::PageLoad,
            page: Some(PageIndex::new(7)),
        };
        assert_eq!(e.to_string(), "stopped by the caller while loading page 7");
    }

    #[test]
    fn a_raised_flag_is_a_stop_and_the_page_is_added_by_the_caller() {
        let stop = Deadline::manual();
        let limits = Limits {
            deadline: Some(stop.clone()),
            ..Limits::default()
        };
        assert_eq!(limits.check_deadline(Operation::Render), Ok(()));
        stop.stop();
        assert_eq!(
            limits
                .check_deadline(Operation::Render)
                .map_err(|e| e.on_page(PageIndex::new(1))),
            Err(LimitExceeded::Stopped {
                during: Operation::Render,
                page: Some(PageIndex::new(1)),
            })
        );
    }

    #[test]
    fn the_pixel_message_names_the_size_the_cap_and_the_remedy() {
        let e = LimitExceeded::RenderPixels {
            width: 20_000,
            height: 20_000,
            allowed: 100_000_000,
        };
        assert_eq!(
            e.to_string(),
            "render of 20000 x 20000 px (400 megapixels) is above the cap of 100 megapixels; \
             render at a smaller scale or raise `Limits::max_render_pixels`"
        );
        let e = LimitExceeded::RenderPixels {
            width: 1_234,
            height: 1_000,
            allowed: 1_050_000,
        };
        assert!(e.to_string().contains("(1.2 megapixels)"));
        assert!(e.to_string().contains("cap of 1 megapixels"));
    }

    #[test]
    fn struct_update_syntax_keeps_the_rest() {
        let l = Limits {
            max_array_len: 8,
            ..Limits::default()
        };
        assert_eq!(l.max_array_len, 8);
        assert_eq!(l.max_object_nesting, 64);
    }
}
