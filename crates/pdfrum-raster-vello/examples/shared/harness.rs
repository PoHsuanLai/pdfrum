//! Shared plumbing for the two harness binaries.
//!
//! Both `gpu-tier-c` and `gpu-bench` need the same thing: a corpus document
//! opened, page 0 built into a page graph, and that **one** graph rendered
//! through two different `RasterBackend`s. Building it once and handing the
//! same value to both is not an optimisation — it is what makes the comparison
//! mean anything. If each column built its own page, a difference in the
//! output could be a difference in the *build*, and a difference in the timing
//! would include a build the other column also paid for.
//!
//! That is also why this is a `mod` included by both binaries rather than a
//! third one they shell out to.
//!
//! # Why these enter at `pdfrum_render`'s seam and not the facade's
//!
//! `pdfrum-render`'s own bench (`crates/pdfrum-render/benches/render.rs`)
//! enters through the facade, and says why: the oracle's `pdfium_test` renders
//! a whole document, so a number compared against *the oracle's column* has to
//! describe the same work.
//!
//! These harnesses compare two of our own backends against each other, and the
//! facade cannot express that — `pdfrum::Backend` is a closed enum, and adding
//! a GPU arm to it would put `wgpu` in the facade's dependency tree. So both
//! columns enter at
//! `render_page`, which is generic over the backend, with the same
//! page, the same options and the same fresh caches. The comparison is
//! *symmetric*, which is the property that matters here. It is deliberately
//! **not** comparable to the oracle's column.
//!
//! One consequence worth stating: annotation appearances are not overlaid
//! here, because that is the facade's step and it is private. So the `forms`
//! class measures the page content under the widgets rather than the widgets.
//! Both columns lose it equally, so the comparison stands; the absolute
//! numbers for that class are not the facade's.

#![allow(dead_code, reason = "each binary uses a different subset")]

use std::path::Path;
use std::time::{Duration, Instant};

use pdfrum_page::Page as PageGraph;
use pdfrum_render::{Pixmap, RasterBackend, RenderOptions};

/// A corpus document's page 0, built and ready to render repeatedly.
pub struct Subject {
    /// The corpus stem, for reporting.
    pub stem: String,
    /// The page graph every column renders.
    pub page: PageGraph,
    /// The render options every column renders with.
    pub options: RenderOptions,
    /// The device size the page renders at, for reporting complexity.
    pub size: (u32, u32),
}

impl Subject {
    /// Device pixels in the target — the denominator a per-pixel cost needs.
    pub fn pixels(&self) -> u64 {
        u64::from(self.size.0) * u64::from(self.size.1)
    }
}

/// One page axis in device pixels, or `None` if it is not a usable size.
///
/// `f64::to_u32` has no total conversion in std, and a bare `as` saturates
/// silently — including turning a NaN crop box into zero. This is the explicit
/// spelling.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the guards above establish finite, >= 1.0 and <= u32::MAX, and \
              the value is already floored, so the conversion is exact"
)]
fn device_axis(points: f64) -> Option<u32> {
    if !points.is_finite() || points < 1.0 {
        return None;
    }
    let rounded = points.floor();
    (rounded <= f64::from(u32::MAX)).then_some(rounded as u32)
}

/// Build page 0 of a corpus document.
///
/// Returns `None` — rather than failing the run — for a document that will not
/// open or has no pages, because the harness's job is to report on the corpus
/// it can measure rather than to stop at the first oddity.
pub fn subject(path: &Path, stem: &str) -> Option<Subject> {
    let doc = pdfrum::Document::open(path).ok()?;
    if doc.page_count() == 0 {
        return None;
    }
    let page = doc.page(0).ok()?;
    let crop = page.crop_box();
    let (pw, ph) = match page.rotation() {
        pdfrum::Rotation::Quarter | pdfrum::Rotation::ThreeQuarter => (crop.height(), crop.width()),
        pdfrum::Rotation::None | pdfrum::Rotation::Half => (crop.width(), crop.height()),
    };
    // One pixel per PDF point, which is the convention every other measurement
    // in this project uses (§0). Clamped into range before the conversion
    // rather than cast and hoped for: a `/MediaBox` is untrusted input and can
    // be negative, enormous, or NaN, and `as` would silently saturate.
    let (w, h) = (device_axis(pw)?, device_axis(ph)?);

    // `objects()` is the facade's documented escape hatch onto `pdfrum-page`
    // and builds with a default `BuildContext`. It therefore does *not* set
    // the per-page decode target the facade's own render path sets, so an
    // image here is decoded at full resolution rather than at its device
    // footprint. That is deliberate: **both columns render the identical
    // graph**, so whatever the decode did, it is not a difference between
    // them. It does mean the absolute image-class numbers here are not the
    // facade's.
    let graph = page.objects();

    // Identity, like the facade's default: the engine composes the page's own
    // `display_matrix` and the y flip from the page graph itself
    // (`walk.rs:298-304`), so a transform set here would be an *extra* one.
    Some(Subject {
        stem: stem.to_owned(),
        page: graph,
        options: RenderOptions::default(),
        size: (w, h),
    })
}

/// Render a subject once through `backend`, from cold caches.
pub fn render_once<B: RasterBackend>(subject: &Subject, backend: &B) -> Option<Pixmap> {
    let mut diags = pdfrum_common::Diagnostics::default();
    pdfrum_render::render_page(&subject.page, &subject.options, backend, &mut diags).ok()
}

/// Time `iterations` renders through `backend`, returning the median.
///
/// The **median**, not the mean: a first render on this machine pays
/// pipeline and allocator warm-up no later one does, and a mean would smear
/// that across the number instead of leaving it in one sample where it
/// belongs. A warm-up render runs before the timed window for the same reason.
///
/// The timed region includes everything `RasterBackend::finish` does, which on
/// the GPU column is the texture allocation, the dispatch, the
/// `copy_texture_to_buffer` and the host stall waiting for the readback. That
/// is deliberate and is what means by "include upload and
/// readback": an embedder rendering a page to a texture pays them, and a
/// number that excluded them would be marketing.
pub fn time<B: RasterBackend>(
    subject: &Subject,
    backend: &B,
    iterations: usize,
) -> Option<Duration> {
    render_once(subject, backend)?;
    let mut samples = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let start = Instant::now();
        let pixmap = render_once(subject, backend);
        let elapsed = start.elapsed();
        pixmap?;
        samples.push(elapsed);
    }
    samples.sort_unstable();
    samples.get(samples.len() / 2).copied()
}
