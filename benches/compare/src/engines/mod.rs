//! The engines, one module each, and the registry the parent and the child
//! both consult. Every peer is behind the cargo feature of its own name so a
//! build without it still lists it — as "not compiled in" — rather than
//! silently dropping the row.

use std::path::Path;

use anyhow::{Result, bail};

use crate::model::{Ctx, Op, Timed};

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
