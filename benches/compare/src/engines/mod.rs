//! Engines. A missing feature still lists the row as "not compiled in".

use std::path::Path;

use anyhow::{Result, bail};

use crate::model::{Ctx, Op, RenderProfile, Timed};

#[cfg(feature = "hayro")]
mod hayro;
#[cfg(feature = "lopdf")]
mod lopdf;
#[cfg(feature = "mupdf")]
mod mupdf;
#[cfg(feature = "pdf-extract")]
mod pdf_extract;
#[cfg(feature = "pdf_oxide")]
mod pdf_oxide;
#[cfg(feature = "pdf")]
mod pdf_rs;
#[cfg(feature = "pdfium-render")]
mod pdfium_render;
mod pdfrum;

/// What a table needs to know about an engine before it runs.
#[derive(Debug, Clone, Copy)]
pub struct EngineInfo {
    /// The crate name, which is also the row label and the feature name.
    pub name: &'static str,
    /// The pinned version.
    pub version: &'static str,
    /// The operations the crate's public API offers. An absent op is
    /// reported as "not supported", never skipped.
    pub ops: &'static [Op],
    /// Whether this build compiled it in.
    pub compiled: bool,
    /// Whether C or C++ is compiled or linked for it.
    pub c_in_build: bool,
    /// One line for the README on how the operation is reached.
    pub note: &'static str,
}

/// One item of comparable per-pixel work a render configuration either does
/// or does not do. The closed set is what "like-for-like" means in this
/// harness: two engines whose [`work`] rows agree are computing the same
/// thing, and a table that shows the rows cannot hide an asymmetry in prose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum WorkItem {
    /// Antialias path fills and strokes rather than sampling a pixel once.
    PathAntialias,
    /// Antialias glyph coverage.
    TextAntialias,
    /// Resample an image when `/Interpolate` asks, rather than nearest.
    ImageInterpolation,
    /// Draw the page's `/Annots` over its content.
    Annotations,
    /// Generate and draw form-field (widget) appearances the file does not
    /// already carry as a content-stream appearance.
    FormFields,
}

impl WorkItem {
    /// The column heading.
    pub fn name(self) -> &'static str {
        match self {
            WorkItem::PathAntialias => "path AA",
            WorkItem::TextAntialias => "text AA",
            WorkItem::ImageInterpolation => "image interp",
            WorkItem::Annotations => "annotations",
            WorkItem::FormFields => "form fields",
        }
    }
}

/// Whether a configuration does one [`WorkItem`], with the source that says
/// so. "I could not find out" is a third answer, never folded into "no".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Support {
    /// The configuration does this work. `source` is where that was read.
    Yes { source: &'static str },
    /// It does not. `source` is where that was read.
    No { source: &'static str },
    /// The engine has no API for this at all, so the profile cannot move it.
    NoKnob { source: &'static str },
    /// Not determinable from the crate's source or documentation.
    NotDetermined,
}

impl Support {
    /// The cell text.
    pub fn cell(self) -> &'static str {
        match self {
            Support::Yes { .. } => "yes",
            Support::No { .. } => "no",
            Support::NoKnob { .. } => "no knob",
            Support::NotDetermined => "not determined",
        }
    }

    /// Where the answer was read, for the table's footnotes.
    pub fn source(self) -> Option<&'static str> {
        match self {
            Support::Yes { source } | Support::No { source } | Support::NoKnob { source } => {
                Some(source)
            }
            Support::NotDetermined => None,
        }
    }
}

/// What engine `name`'s benchmark configuration computes under `profile`.
///
/// Every cell was read out of the named crate's own source in the local
/// registry checkout, not out of its README. `NotDetermined` is a real
/// answer and appears wherever the source did not settle the question.
#[must_use]
pub fn work(name: &str, profile: RenderProfile) -> Vec<(WorkItem, Support)> {
    use Support::{No, NoKnob, NotDetermined, Yes};
    use WorkItem::{Annotations, FormFields, ImageInterpolation, PathAntialias, TextAntialias};

    // Only render engines have a row; the text-only peers never rasterize.
    let parity = profile == RenderProfile::Parity;
    match name {
        "pdfrum" => {
            // crates/pdfrum/src/render.rs:69-72 — RenderOptions::default has
            // smooth_paths, smooth_text and interpolate_images and
            // annotations all true; --parity clears three of them.
            let src = "crates/pdfrum/src/render.rs RenderOptions::default";
            vec![
                (
                    PathAntialias,
                    if parity {
                        No { source: src }
                    } else {
                        Yes { source: src }
                    },
                ),
                // The parity profile deliberately leaves text antialiasing
                // on: every peer antialiases glyphs and none offers a knob,
                // so turning ours off would swap one asymmetry for another.
                (TextAntialias, Yes { source: src }),
                (
                    ImageInterpolation,
                    if parity {
                        No { source: src }
                    } else {
                        Yes { source: src }
                    },
                ),
                (
                    Annotations,
                    if parity {
                        No { source: src }
                    } else {
                        Yes { source: src }
                    },
                ),
                // Widget appearances reach the raster through the page's
                // generated appearance streams, which `annotations: false`
                // also suppresses.
                (
                    FormFields,
                    if parity {
                        No { source: src }
                    } else {
                        Yes { source: src }
                    },
                ),
            ]
        }
        "hayro" => {
            // hayro-0.7.1/src/lib.rs:141-145 fixes vello_cpu's RenderSettings
            // (RenderMode::OptimizeSpeed — still analytic coverage AA, only
            // a u8 rather than f32 pipeline); there is no AA switch in
            // hayro's public RenderSettings at all.
            let aa = "hayro-0.7.1/src/lib.rs:141 vello_cpu::RenderSettings (no AA knob)";
            let img =
                "hayro-0.7.1/src/renderer.rs:224 ImageQuality::Medium when /Interpolate, else Low";
            let annot = "hayro-interpret-0.7.0/src/interpret/mod.rs:121 render_annotations: true";
            vec![
                (PathAntialias, NoKnob { source: aa }),
                (TextAntialias, NoKnob { source: aa }),
                (ImageInterpolation, Yes { source: img }),
                (Annotations, Yes { source: annot }),
                (FormFields, NotDetermined),
            ]
        }
        "pdf_oxide" => {
            // pdf_oxide-0.3.77/src/rendering/mod.rs:66,80 set
            // paint.anti_alias = true unconditionally (tiny_skia); its
            // RenderOptions has no AA or interpolation field at all.
            // Its RenderOptions (page_renderer.rs:110-146) has no AA and no
            // interpolation field, so --parity cannot move either.
            let aa = "pdf_oxide-0.3.77/src/rendering/mod.rs:66,80 paint.anti_alias = true";
            let annot =
                "pdf_oxide-0.3.77/src/rendering/page_renderer.rs:140 render_annotations: true";
            vec![
                (PathAntialias, NoKnob { source: aa }),
                (TextAntialias, NoKnob { source: aa }),
                (ImageInterpolation, NotDetermined),
                (Annotations, Yes { source: annot }),
                (FormFields, NotDetermined),
            ]
        }
        "pdfium-render" => {
            // pdfium-render-0.9.3/src/pdf/document/page/render_config.rs:76
            // PdfRenderConfig::new(): every NO_SMOOTH flag false,
            // do_set_flag_render_annotations and do_render_form_data true.
            // The harness's bare `PdfRenderConfig::new()` is therefore the
            // SAME work pdfrum's defaults do, not a cheaper configuration.
            let src = "pdfium-render-0.9.3/src/pdf/document/page/render_config.rs:76 PdfRenderConfig::new";
            vec![
                (PathAntialias, Yes { source: src }),
                (TextAntialias, Yes { source: src }),
                (ImageInterpolation, Yes { source: src }),
                (Annotations, Yes { source: src }),
                (FormFields, Yes { source: src }),
            ]
        }
        "mupdf" => {
            // mupdf-0.8.0/src/context.rs:195-197 — the crate's own test
            // asserts the default aa_level/text_aa_level/graphics_aa_level
            // are 8 (full antialiasing); the harness never calls
            // set_aa_level. to_pixmap(.., show_extras = true) keeps
            // annotations (src/page.rs:49).
            let aa = "mupdf-0.8.0/src/context.rs:195 default aa_level == 8";
            let annot = "mupdf-0.8.0/src/page.rs:49 to_pixmap(show_extras = true)";
            vec![
                (PathAntialias, Yes { source: aa }),
                (TextAntialias, Yes { source: aa }),
                (ImageInterpolation, NotDetermined),
                (Annotations, Yes { source: annot }),
                (FormFields, Yes { source: annot }),
            ]
        }
        _ => Vec::new(),
    }
}

const ALL_OPS: &[Op] = &[Op::Open, Op::Render, Op::Text];
const OPEN_RENDER: &[Op] = &[Op::Open, Op::Render];
const OPEN_TEXT: &[Op] = &[Op::Open, Op::Text];
const OPEN_ONLY: &[Op] = &[Op::Open];
const TEXT_ONLY: &[Op] = &[Op::Text];

/// Every engine this harness knows, compiled in or not, in table order.
pub fn all() -> Vec<EngineInfo> {
    vec![
        EngineInfo {
            name: "pdfrum",
            version: env!("CARGO_PKG_VERSION"),
            ops: ALL_OPS,
            compiled: true,
            c_in_build: false,
            note: "the facade with default features: Document::open, Page::render_on with VelloCpuBackend, Page::text_on",
        },
        EngineInfo {
            name: "hayro",
            version: "0.7.1",
            ops: OPEN_RENDER,
            compiled: cfg!(feature = "hayro"),
            c_in_build: false,
            note: "hayro_syntax::Pdf::new, hayro::render with RenderCache held warm; no text API",
        },
        EngineInfo {
            name: "hayro-interpret",
            version: "0.7.0",
            ops: TEXT_ONLY,
            compiled: cfg!(feature = "hayro"),
            c_in_build: false,
            note: "no text API: the harness implements a Device whose draw_glyph collects Glyph::as_unicode in draw order, newline on baseline change (harness-assembled, see README)",
        },
        EngineInfo {
            name: "pdf-extract",
            version: "0.12.0",
            ops: OPEN_TEXT,
            compiled: cfg!(feature = "pdf-extract"),
            c_in_build: false,
            note: "Document::load_mem (its lopdf), output_doc_page(page 1) into PlainTextOutput",
        },
        EngineInfo {
            name: "lopdf",
            version: "0.44.0",
            ops: OPEN_TEXT,
            compiled: cfg!(feature = "lopdf"),
            c_in_build: false,
            note: "Document::load_mem then every object visited; extract_text(&[1])",
        },
        EngineInfo {
            name: "pdf",
            version: "0.10.0",
            ops: OPEN_ONLY,
            compiled: cfg!(feature = "pdf"),
            c_in_build: false,
            note: "FileOptions::cached().open, every page's resources loaded; no text or render API (pdf_render is a separate, commercial crate)",
        },
        EngineInfo {
            name: "pdf_oxide",
            version: "0.3.77",
            ops: ALL_OPS,
            compiled: cfg!(feature = "pdf_oxide"),
            c_in_build: false,
            note: "PdfDocument::open, rendering::render_page at the run's DPI as RawRgba8, extract_text(0)",
        },
        EngineInfo {
            name: "pdfium-render",
            version: "0.9.3",
            ops: ALL_OPS,
            compiled: cfg!(feature = "pdfium-render"),
            c_in_build: true,
            note: "Pdfium::bind_to_library(libpdfium.so), load_pdf_from_file, render_with_config(set_target_size), text().all()",
        },
        EngineInfo {
            name: "mupdf",
            version: "0.8.0",
            ops: ALL_OPS,
            compiled: cfg!(feature = "mupdf"),
            c_in_build: true,
            note: "Document::open, Page::to_pixmap(scale matrix, DeviceRGB, show_extras), Page::text(default options); built with default-features = false + base14-fonts",
        },
    ]
}

/// The engine named `name`, or `None` for a name this harness does not know.
pub fn info(name: &str) -> Option<EngineInfo> {
    all().into_iter().find(|engine| engine.name == name)
}

/// Runs `op` on `path` with engine `name`, in this process.
pub fn run(name: &str, op: Op, path: &Path, ctx: &Ctx<'_>) -> Result<Timed> {
    match name {
        "pdfrum" => pdfrum::run(op, path, ctx),
        #[cfg(feature = "hayro")]
        "hayro" => hayro::run(op, path, ctx),
        #[cfg(feature = "hayro")]
        "hayro-interpret" => hayro::run_text(op, path, ctx),
        #[cfg(feature = "pdf-extract")]
        "pdf-extract" => pdf_extract::run(op, path, ctx),
        #[cfg(feature = "lopdf")]
        "lopdf" => lopdf::run(op, path, ctx),
        #[cfg(feature = "pdf")]
        "pdf" => pdf_rs::run(op, path, ctx),
        #[cfg(feature = "pdf_oxide")]
        "pdf_oxide" => pdf_oxide::run(op, path, ctx),
        #[cfg(feature = "pdfium-render")]
        "pdfium-render" => pdfium_render::run(op, path, ctx),
        #[cfg(feature = "mupdf")]
        "mupdf" => mupdf::run(op, path, ctx),
        other => bail!("engine {other} is not compiled into this binary"),
    }
}

/// What an engine shares between the threads of a throughput run — the
/// shape of its own multi-threading model, not a flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sharing {
    /// One opened document, `Sync`, read by every thread: parse and
    /// rasterization both run in parallel.
    Document,
    /// One thread loads every page and records it into a display list, then
    /// N threads rasterize those lists, each on its own cloned context.
    /// This is `MuPDF`'s documented model (`docs/examples/multi-threaded.c`):
    /// `Document` and `Page` are not `Send`, `DisplayList` is `Send + Sync`,
    /// and `Context::get()` hands each thread its own `fz_clone_context` of
    /// a base context built with `FZ_LOCK_MAX` pthread mutexes as `MuPDF`'s
    /// lock callbacks. The parse is serial, the rasterization parallel.
    DisplayLists,
    /// The engine's own rules put every call on one thread.
    SingleThread {
        /// The rule, for the table's footnote.
        why: &'static str,
    },
}

impl Sharing {
    /// How many threads to actually run for a requested count.
    #[must_use]
    pub fn threads(self, requested: usize) -> usize {
        match self {
            Self::SingleThread { .. } => 1,
            Self::Document | Self::DisplayLists => requested,
        }
    }
}

/// How engine `name` shares work between the threads of a throughput run.
#[must_use]
pub fn sharing(name: &str) -> Sharing {
    match name {
        "pdfium-render" => Sharing::SingleThread {
            why: "PDFium keeps one global state; every call must be on one thread",
        },
        "mupdf" => Sharing::DisplayLists,
        _ => Sharing::Document,
    }
}

/// Renders every page of `path` at the run's DPI on `threads` threads,
/// sharing whatever [`sharing`] says this engine shares; returns the page
/// count.
pub fn render_all(name: &str, path: &Path, ctx: &Ctx<'_>, threads: usize) -> Result<usize> {
    match name {
        "pdfrum" => pdfrum::render_all(path, ctx, threads),
        #[cfg(feature = "hayro")]
        "hayro" => hayro::render_all(path, ctx, threads),
        #[cfg(feature = "pdf_oxide")]
        "pdf_oxide" => pdf_oxide::render_all(path, ctx, threads),
        #[cfg(feature = "pdfium-render")]
        "pdfium-render" => pdfium_render::render_all(path, ctx),
        #[cfg(feature = "mupdf")]
        "mupdf" => mupdf::render_all(path, ctx, threads),
        other => bail!("engine {other} does not render, or is not compiled in"),
    }
}

/// The error every engine returns for an operation its API does not offer.
pub fn unsupported(engine: &str, op: Op) -> anyhow::Error {
    anyhow::anyhow!("{engine} does not support {}", op.name())
}

#[cfg(test)]
mod tests {
    use super::{Op, RenderProfile, Support, WorkItem, all, work};

    /// The profile must move exactly the three items pdfrum has a knob for,
    /// and leave glyph antialiasing alone — the parity columns claim that,
    /// and a silent change to the set would make the published matrix lie.
    #[test]
    fn parity_moves_pdfrums_three_knobs() {
        let base = work("pdfrum", RenderProfile::Default);
        let parity = work("pdfrum", RenderProfile::Parity);
        let moved: Vec<WorkItem> = base
            .iter()
            .zip(&parity)
            .filter(|((_, a), (_, b))| a != b)
            .map(|((item, _), _)| *item)
            .collect();
        assert_eq!(
            moved,
            vec![
                WorkItem::PathAntialias,
                WorkItem::ImageInterpolation,
                WorkItem::Annotations,
                WorkItem::FormFields,
            ]
        );
        assert!(
            base.iter()
                .any(|(item, s)| *item == WorkItem::TextAntialias
                    && matches!(s, Support::Yes { .. }))
        );
    }

    /// A peer with no knob reports `NoKnob`, identically in both profiles,
    /// rather than pretending `--parity` reached it.
    #[test]
    fn a_knobless_peer_reports_no_knob_in_both_profiles() {
        for name in ["hayro", "pdf_oxide"] {
            let base = work(name, RenderProfile::Default);
            assert_eq!(base, work(name, RenderProfile::Parity), "{name}");
            assert!(
                base.iter().any(|(item, s)| *item == WorkItem::PathAntialias
                    && matches!(s, Support::NoKnob { .. })),
                "{name}"
            );
        }
    }

    /// Every engine that renders has a work row; a text-only engine has none.
    #[test]
    fn every_rendering_engine_has_a_work_row() {
        for engine in all() {
            let has_row = !work(engine.name, RenderProfile::Default).is_empty();
            assert_eq!(has_row, engine.ops.contains(&Op::Render), "{}", engine.name);
        }
    }
}
