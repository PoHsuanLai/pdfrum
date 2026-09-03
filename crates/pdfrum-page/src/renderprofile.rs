//! Where a *whole page render* goes, from the page dictionary to the pixels.
//!
//! The companion to `pdfrum_render::walkprofile` and one level above it.
//! `walkprofile` splits the engine half of the rasterizing walk; this splits
//! the pipeline that walk sits inside — the content parse, the page-object
//! build, the annotation and appearance pass, and the raster — so that the
//! buckets sum to what a caller's clock reads rather than to what the walk
//! reads.
//!
//! The distinction is why this module exists. An instrument that lives inside
//! the raster cannot see the work in front of it, so a document that spends
//! four fifths of itself building form faces reports a well-behaved raster and
//! a total nobody can reconcile with it. The buckets here are placed so that
//! their sum plus one *unattributed* remainder is the whole render, and the
//! remainder's size is the instrument's own honesty check.
//!
//! **Costs nothing when the `profiling` feature is off**: every entry point
//! compiles to an empty inline function, exactly as `walkprofile`'s do, and
//! the flag is the same one — `scripts/profile.nu --walk` turns on both.
//!
//! The accumulator is a thread-local and [`take`] resets it, so a rayon render
//! reports per-thread totals rather than a contended one.

// One accumulator rather than one per crate. The stages cross three crates —
// the facade paints, `pdfrum-doc` overlays, this crate caches the form faces —
// and the lowest of the three is this one, which is why the record lives here
// rather than beside `walkprofile`. `pdfrum-render` forwards the feature so
// there is still exactly one flag to turn on.
//
// The reporting half — `Profile` itself, and `Stage`'s `index`/`name`/`ALL` —
// carries `#[cfg(feature = "profiling")]` because with the feature off
// nothing ever produces a `Profile` to report. `Stage`'s *variants* are
// unconditional: the recording half names them at every call site whether or
// not the feature is on.

#[cfg(feature = "profiling")]
use core::time::Duration;

/// One stage of a page render, from the dictionary to the pixels.
///
/// The set is closed and the stages are **disjoint**, which is the property
/// that lets them be summed: no stage's span contains another's. That is not
/// true of [`Phase`](pdfrum_render_phase), whose buckets nest deliberately, and
/// it is the reason this is a separate set rather than more variants there.
///
/// [pdfrum_render_phase]: https://docs.rs/pdfrum-render
#[cfg(feature = "profiling")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Tokenizing the content stream into operators: the `/Contents` decode
    /// and the operator scan, before any of them is interpreted.
    ContentParse,
    /// Folding the operators into a page-object graph — the graphics-state
    /// machine, the resource lookups, the image decodes.
    ///
    /// Disjoint from [`Stage::ContentParse`]: the parse runs to completion
    /// first and this is what reads its output.
    Interpretation,
    /// Loading the page's annotation list: the `/Annots` walk, the pop-up
    /// synthesis and the ordering.
    AnnotList,
    /// Building the interactive form's default-resource faces.
    ///
    /// Counted as well as timed, and the miss count beside the call count is
    /// what says whether the memoization is working: a call that misses builds
    /// every face the form declares, and a document with no form still builds
    /// the fallback and the substitutes.
    FormFonts,
    /// Generating the appearance streams the file does not carry — a widget
    /// with no `/AP`, a markup annotation's synthesized note, a free-text
    /// field's text.
    GenerateAppearances,
    /// Reading the catalog's open action for the `/Hide` that rewrites the
    /// flag words the visibility test reads.
    OpenAction,
    /// The per-annotation loop: the visibility test, the appearance form's
    /// placement matrix, and building each one into the page graph.
    AnnotLoop,
    /// The rasterizing walk itself — everything `render_page_with` does,
    /// which is the whole subject of `pdfrum_render::walkprofile`.
    Raster,
}

/// Everything one page render accumulated.
///
/// A record of facts: the counters are public and the reporting lives in
/// whoever reads them.
#[cfg(feature = "profiling")]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Profile {
    /// Time in each stage, indexed as [`Stage`] orders them.
    pub stage_time: [Duration; 8],
    /// How many times each stage was entered.
    pub stage_calls: [u64; 8],
    /// How many of [`Stage::FormFonts`]' calls missed the memoization and
    /// built the faces.
    ///
    /// Its own counter rather than a ninth stage, because it is a property of
    /// a stage that is already timed rather than a span of its own: the
    /// interesting reading is calls against misses on one row.
    pub form_font_misses: u64,
}

#[cfg(feature = "profiling")]
impl Stage {
    /// The index this stage occupies in [`Profile::stage_time`].
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Stage::ContentParse => 0,
            Stage::Interpretation => 1,
            Stage::AnnotList => 2,
            Stage::FormFonts => 3,
            Stage::GenerateAppearances => 4,
            Stage::OpenAction => 5,
            Stage::AnnotLoop => 6,
            Stage::Raster => 7,
        }
    }

    /// The stages in the order the arrays index them, which is also the order
    /// a render runs them in.
    pub const ALL: [Stage; 8] = [
        Stage::ContentParse,
        Stage::Interpretation,
        Stage::AnnotList,
        Stage::FormFonts,
        Stage::GenerateAppearances,
        Stage::OpenAction,
        Stage::AnnotLoop,
        Stage::Raster,
    ];

    /// A short name for a report row.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Stage::ContentParse => "content parse",
            Stage::Interpretation => "interpretation",
            Stage::AnnotList => "annot list",
            Stage::FormFonts => "form fonts",
            Stage::GenerateAppearances => "generate aps",
            Stage::OpenAction => "open action",
            Stage::AnnotLoop => "annot loop",
            Stage::Raster => "raster",
        }
    }

    /// Whether this stage is part of the annotation and appearance pass.
    ///
    /// The pass has no span of its own — it is the five stages between the
    /// graph build and the raster — so a report that wants a subtotal for it
    /// asks here rather than hard-coding the list.
    #[must_use]
    pub const fn in_annotation_pass(self) -> bool {
        matches!(
            self,
            Stage::AnnotList
                | Stage::FormFonts
                | Stage::GenerateAppearances
                | Stage::OpenAction
                | Stage::AnnotLoop
        )
    }
}

#[cfg(feature = "profiling")]
mod imp {
    use core::cell::Cell;
    use core::time::Duration;

    use super::{Profile, Stage};

    thread_local! {
        /// The accumulator. A `Cell<Profile>` rather than a `RefCell`, for the
        /// reason `walkprofile`'s is: the record is `Copy`, so a
        /// read-modify-write needs no borrow and cannot panic on a re-entrant
        /// one — and these stages *are* re-entrant, since an appearance form
        /// is interpreted from inside the annotation loop.
        static PROFILE: Cell<Profile> = const { Cell::new(Profile {
            stage_time: [Duration::ZERO; 8],
            stage_calls: [0; 8],
            form_font_misses: 0,
        }) };
    }

    /// Read the accumulator and clear it.
    pub fn take() -> Profile {
        PROFILE.replace(Profile::default())
    }

    /// Record one [`Stage::FormFonts`] call that missed the memoization.
    pub fn form_font_miss() {
        let mut p = PROFILE.get();
        p.form_font_misses = p.form_font_misses.saturating_add(1);
        PROFILE.set(p);
    }

    /// Run `body` and charge its elapsed time to `stage`.
    ///
    /// A stage entered from inside another — a form's content parse, run from
    /// the annotation loop — is charged to its own bucket and to the enclosing
    /// one, so the two overlap. Every *outermost* span is disjoint, which is
    /// what the sum relies on; a reader that finds the sum over the total is
    /// looking at nesting, and the report says so.
    pub fn stage<T>(stage: Stage, body: impl FnOnce() -> T) -> T {
        let started = std::time::Instant::now();
        let out = body();
        let elapsed = started.elapsed();
        let mut p = PROFILE.get();
        let i = stage.index();
        if let (Some(t), Some(c)) = (p.stage_time.get_mut(i), p.stage_calls.get_mut(i)) {
            *t = t.saturating_add(elapsed);
            *c = c.saturating_add(1);
        }
        PROFILE.set(p);
        out
    }
}

#[cfg(not(feature = "profiling"))]
mod imp {
    /// A no-op without the feature.
    ///
    /// The only entry point the feature-off build keeps. `stage` is not here
    /// because nothing in this crate times a stage — the two crates above it
    /// do, through their own `profiling`, and with the feature off they
    /// never name this module at all.
    #[inline]
    pub fn form_font_miss() {}
}

pub use imp::form_font_miss;
#[cfg(feature = "profiling")]
pub use imp::{stage, take};

#[cfg(test)]
mod tests {
    use super::*;

    /// The arrays exist only with the feature on, so their indexing does too.
    #[cfg(feature = "profiling")]
    #[test]
    fn the_indices_are_dense_and_distinct() {
        // The arrays are indexed by these, so a duplicate or a gap would
        // silently merge two rows of a report.
        let stages: Vec<usize> = Stage::ALL.iter().map(|s| s.index()).collect();
        assert_eq!(stages, (0..Stage::ALL.len()).collect::<Vec<_>>());
    }

    /// The one entry point both builds carry must compile and run in both.
    #[test]
    fn the_miss_counter_is_callable_either_way() {
        form_font_miss();
    }

    /// With the feature on, the wrapper is transparent as well as clocked.
    #[cfg(feature = "profiling")]
    #[test]
    fn the_stage_wrapper_returns_the_body_s_value() {
        assert_eq!(stage(Stage::Raster, || 7_u32), 7);
    }

    /// The annotation pass is exactly the five stages between the graph build
    /// and the raster.
    #[cfg(feature = "profiling")]
    #[test]
    fn the_annotation_pass_is_the_five_stages_between_the_build_and_the_raster() {
        let pass: Vec<&str> = Stage::ALL
            .iter()
            .filter(|s| s.in_annotation_pass())
            .map(|s| s.name())
            .collect();
        assert_eq!(
            pass,
            [
                "annot list",
                "form fonts",
                "generate aps",
                "open action",
                "annot loop"
            ]
        );
        // The three that are not in it are the ones a page with no
        // annotations still pays.
        for stage in [Stage::ContentParse, Stage::Interpretation, Stage::Raster] {
            assert!(!stage.in_annotation_pass(), "{stage:?}");
        }
    }

    #[cfg(feature = "profiling")]
    #[test]
    fn taking_the_profile_clears_it() {
        let _ = take();
        stage(Stage::FormFonts, form_font_miss);
        let first = take();
        assert_eq!(first.stage_calls.get(Stage::FormFonts.index()), Some(&1));
        assert_eq!(first.form_font_misses, 1);
        assert_eq!(take(), Profile::default());
    }
}
