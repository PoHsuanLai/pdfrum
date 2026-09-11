//! pdfrum through the facade, one row per rasterizer.
//!
//! `pdfrum` is `cargo add pdfrum`: [`VelloCpuBackend`]. `pdfrum-agg` and
//! `pdfrum-tinyskia` are the same facade with a different [`RasterBackend`].
//! `pdfrum-vello-gpu` is the same again, behind `--features gpu`, and is
//! skipped when no adapter is present. Open and text do not go through a
//! rasterizer, so those ops stay on the `pdfrum` row.

use std::path::Path;

use anyhow::{Result, anyhow};
#[cfg(feature = "gpu")]
use pdfrum::VelloGpuBackend;
use pdfrum::{
    AggBackend, CharIndex, Document, RasterBackend, RenderOptions, RenderSession,
    SubstitutionOptions, TextPage, TinySkiaBackend, VelloCpuBackend,
};

use crate::model::{Ctx, Op, Output, Raster, RenderProfile, Timed};

/// Which rasterizer a pdfrum engine row uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// The facade default, `vello_cpu`.
    VelloCpu,
    /// The AGG-parity scanline backend, feature `agg`.
    Agg,
    /// `tiny-skia`, feature `tiny-skia`.
    TinySkia,
    /// GPU `vello` on a headless wgpu adapter, feature `vello-gpu`.
    VelloGpu,
}

impl Backend {
    /// The engine name for this rasterizer, or `None` if `name` is not ours.
    pub fn of(name: &str) -> Option<Self> {
        match name {
            "pdfrum" => Some(Self::VelloCpu),
            "pdfrum-agg" => Some(Self::Agg),
            "pdfrum-tinyskia" => Some(Self::TinySkia),
            "pdfrum-vello-gpu" => Some(Self::VelloGpu),
            _ => None,
        }
    }

    /// The engine name this rasterizer is registered under.
    pub fn engine(self) -> &'static str {
        match self {
            Self::VelloCpu => "pdfrum",
            Self::Agg => "pdfrum-agg",
            Self::TinySkia => "pdfrum-tinyskia",
            Self::VelloGpu => "pdfrum-vello-gpu",
        }
    }
}

/// Why the GPU row cannot run on this machine, or `None` if it can.
///
/// Opens (and, on success, leaks) at most one headless device in this
/// process, matching [`pdfrum_raster_vello::request_adapter`]. A container
/// or a box with no render node skips rather than failing the run.
#[cfg(feature = "gpu")]
#[must_use]
pub fn gpu_unavailable() -> Option<String> {
    match pdfrum_raster_vello::request_adapter() {
        Ok(_) => None,
        Err(err) => Some(format!("not run, {err}")),
    }
}

#[cfg(feature = "gpu")]
fn gpu_backend() -> Result<VelloGpuBackend<'static>> {
    pdfrum_raster_vello::try_real_gpu()
        .ok_or_else(|| anyhow!("pdfrum-vello-gpu: no usable wgpu adapter"))
}

#[cfg(not(feature = "gpu"))]
fn gpu_not_compiled() -> anyhow::Error {
    anyhow!("pdfrum-vello-gpu is not compiled in; build with --features gpu")
}

/// The `chars` stream as text, which is the stream `pdfium_test --txt`
/// writes: `FPDFText_GetUnicode` for every `i` in `FPDFText_CountChars`
/// (`testing/pdfium_test/write.cc:364-370`), unfiltered.
///
/// Deliberately **not** `TextPage`'s `Display`, which is the search text and
/// drops the control characters and hyphen sentinels the oracle keeps
/// (`crates/pdfrum-text/src/lib.rs:417-419`). It is the same construction the
/// conformance runner's tier-A `--txt` dump uses
/// (`crates/pdfrum-tool/src/text.rs::to_utf32le`), so the harness and the
/// board compare the same bytes. A code point the oracle writes that is not a
/// scalar value — a lone surrogate — is dropped rather than replaced, since
/// `String` cannot hold one.
fn chars_stream(page: &TextPage) -> String {
    (0..page.char_count())
        .filter_map(|i| page.char(CharIndex::new(i)).ok())
        .filter_map(|info| char::from_u32(info.unicode))
        .collect()
}

fn open(path: &Path, ctx: &Ctx<'_>) -> Result<Document> {
    Ok(match ctx.password {
        Some(password) => Document::open_with_password(path, password.as_bytes())?,
        None => Document::open(path)?,
    })
}

/// A session whose font substitution reads the oracle's hermetic directory,
/// so a non-embedded font resolves to the same face on both sides. The knob
/// is the facade's public `BuildContext::substitution`; without a
/// `--font-dir` the session is the plain default.
///
/// Built from the document either way, so every session over one document
/// shares its loaded fonts: the fonts a parallel render meets are parsed once
/// for the run rather than once per thread.
fn session(doc: &Document, ctx: &Ctx<'_>) -> RenderSession {
    let mut session = doc.render_session();
    if let Some(dir) = ctx.font_dir {
        session.build.substitution = SubstitutionOptions {
            font_dirs: vec![dir.to_path_buf()],
            croscore_font_names: true,
            ..SubstitutionOptions::default()
        };
    }
    session
}

/// The render options the run's profile asks for.
///
/// `Default` is `RenderOptions::scaled`, which is what a caller who writes
/// no options gets: `smooth_paths`, `interpolate_images` and `annotations`
/// all on (`crates/pdfrum/src/render.rs:69-72`). `Parity` clears the three,
/// which is the cheapest configuration the public API offers and what
/// `docs/benchmarks/README.md`'s parity columns measure. Glyph antialiasing
/// stays on in both: no peer offers a knob for it, so turning it off would
/// swap one asymmetry for another.
fn options(ctx: &Ctx<'_>) -> RenderOptions {
    let builder = RenderOptions::builder().scale(ctx.scale());
    match ctx.profile {
        RenderProfile::Default => builder.build(),
        RenderProfile::Parity => builder
            .smooth_paths(false)
            .interpolate_images(false)
            .annotations(false)
            .build(),
    }
}

/// Every page on `threads` threads sharing one `Document` (`Sync`), each
/// thread with its own backend and `RenderSession`, pages dealt round-robin.
pub fn render_all(backend: Backend, path: &Path, ctx: &Ctx<'_>, threads: usize) -> Result<usize> {
    match backend {
        Backend::VelloCpu => render_all_with(VelloCpuBackend::new, path, ctx, threads),
        Backend::Agg => render_all_with(AggBackend::new, path, ctx, threads),
        Backend::TinySkia => render_all_with(TinySkiaBackend::new, path, ctx, threads),
        #[cfg(feature = "gpu")]
        Backend::VelloGpu => render_all_gpu(path, ctx),
        #[cfg(not(feature = "gpu"))]
        Backend::VelloGpu => Err(gpu_not_compiled()),
    }
}

/// One wgpu device, one vello Renderer, pages in order.
#[cfg(feature = "gpu")]
fn render_all_gpu(path: &Path, ctx: &Ctx<'_>) -> Result<usize> {
    let backend = gpu_backend()?;
    let doc = open(path, ctx)?;
    let pages = doc.page_count() as usize;
    let options = options(ctx);
    let mut session = session(&doc, ctx);
    for index in 0..pages {
        doc.page(index as u32)?
            .render_on(backend, &options, &mut session)?;
    }
    Ok(pages)
}

fn render_all_with<B, F>(make: F, path: &Path, ctx: &Ctx<'_>, threads: usize) -> Result<usize>
where
    B: RasterBackend,
    F: Fn() -> B + Sync,
{
    let doc = open(path, ctx)?;
    let pages = doc.page_count() as usize;
    let threads = threads.clamp(1, pages.max(1));
    let options = options(ctx);
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..threads)
            .map(|first| {
                let doc = &doc;
                let options = &options;
                let make = &make;
                scope.spawn(move || -> Result<()> {
                    let backend = make();
                    let mut session = session(doc, ctx);
                    for index in (first..pages).step_by(threads) {
                        doc.page(index as u32)?
                            .render_on(backend, options, &mut session)?;
                    }
                    Ok(())
                })
            })
            .collect();
        for handle in handles {
            handle
                .join()
                .map_err(|_| anyhow!("pdfrum: render thread panicked"))??;
        }
        Ok::<(), anyhow::Error>(())
    })?;
    Ok(pages)
}

pub fn run(backend: Backend, op: Op, path: &Path, ctx: &Ctx<'_>) -> Result<Timed> {
    match op {
        Op::Open | Op::Text if backend != Backend::VelloCpu => {
            Err(super::unsupported(backend.engine(), op))
        }
        Op::Open => {
            let (times_ms, pages) = ctx.measure(|| {
                let doc = open(path, ctx)?;
                Ok(doc.page_count() as usize)
            })?;
            Ok(Timed {
                times_ms,
                output: Output::Opened {
                    pages,
                    objects: None,
                },
            })
        }
        Op::Render => match backend {
            Backend::VelloCpu => render_page(&VelloCpuBackend::new(), path, ctx),
            Backend::Agg => render_page(&AggBackend::new(), path, ctx),
            Backend::TinySkia => render_page(&TinySkiaBackend::new(), path, ctx),
            #[cfg(feature = "gpu")]
            Backend::VelloGpu => render_page(&gpu_backend()?, path, ctx),
            #[cfg(not(feature = "gpu"))]
            Backend::VelloGpu => Err(gpu_not_compiled()),
        },
        Op::Text => {
            let doc = open(path, ctx)?;
            let page = doc.page(0)?;
            let mut session = session(&doc, ctx);
            let (times_ms, text) = ctx.measure(|| Ok(chars_stream(&page.text_on(&mut session))))?;
            Ok(Timed {
                times_ms,
                output: Output::Text(text),
            })
        }
    }
}

fn render_page<B: RasterBackend>(backend: &B, path: &Path, ctx: &Ctx<'_>) -> Result<Timed> {
    let doc = open(path, ctx)?;
    let page = doc.page(0)?;
    let options = options(ctx);
    let mut session = session(&doc, ctx);
    let (times_ms, pixmap) =
        ctx.measure(|| Ok(page.render_on(backend, &options, &mut session)?))?;
    Ok(Timed {
        times_ms,
        output: Output::Rendered(Raster {
            width: pixmap.width(),
            height: pixmap.height(),
            rgba: pixmap.data().to_vec(),
            premultiplied: true,
        }),
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::time::Duration;

    use super::{Backend, Ctx, Op, Output, RenderProfile, run};

    fn ctx(profile: RenderProfile) -> Ctx<'static> {
        Ctx {
            dpi: 150.0,
            font_dir: None,
            pdfium_lib: None,
            password: None,
            warm_runs: 0,
            budget: Duration::from_secs(30),
            profile,
        }
    }

    fn corpus() -> std::path::PathBuf {
        let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("../corpus/vector_paths_1751.pdf");
        assert!(file.is_file(), "corpus file missing: {}", file.display());
        file
    }

    fn render(backend: Backend, profile: RenderProfile) -> Vec<u8> {
        let timed = run(backend, Op::Render, &corpus(), &ctx(profile)).expect("render");
        match timed.output {
            Output::Rendered(raster) => raster.rgba,
            other => panic!("not a raster: {other:?}"),
        }
    }

    /// The whole point of `--parity` is that it renders different pixels: a
    /// profile that quietly produced the default raster would report a
    /// speed gap that bought nothing. A path-heavy corpus file must differ
    /// once path antialiasing is off.
    #[test]
    fn parity_changes_the_pixels() {
        let default = render(Backend::VelloCpu, RenderProfile::Default);
        let parity = render(Backend::VelloCpu, RenderProfile::Parity);
        assert_eq!(default.len(), parity.len(), "same page, same size");
        assert_ne!(
            default, parity,
            "parity produced the default raster: the profile is not reaching RenderOptions"
        );
    }

    /// Each CPU backend has to produce a page, or the extra engine rows
    /// would be empty speed/SSIM cells rather than a comparison.
    #[test]
    fn every_cpu_backend_renders() {
        let n = render(Backend::VelloCpu, RenderProfile::Default).len();
        assert!(n > 0, "vello-cpu produced an empty raster");
        for backend in [Backend::Agg, Backend::TinySkia] {
            let pixels = render(backend, RenderProfile::Default);
            assert_eq!(pixels.len(), n, "{} size", backend.engine());
        }
    }

    /// Quality is a real difference, not three spellings of one rasterizer.
    /// A path-heavy page is where AGG's coverage integral and `vello_cpu`'s
    /// pipeline disagree on edge pixels.
    #[test]
    fn agg_and_vello_cpu_are_not_the_same_pixels() {
        let vello = render(Backend::VelloCpu, RenderProfile::Default);
        let agg = render(Backend::Agg, RenderProfile::Default);
        assert_eq!(vello.len(), agg.len(), "same page, same size");
        assert_ne!(
            vello, agg,
            "agg produced vello_cpu's raster: the backend is not reaching Page::render_on"
        );
    }

    /// Open and text live on the `pdfrum` row; the extra engines must not
    /// pretend they have a different parser.
    #[test]
    fn extra_backends_do_not_open_or_extract() {
        for backend in [Backend::Agg, Backend::TinySkia, Backend::VelloGpu] {
            for op in [Op::Open, Op::Text] {
                let err = run(backend, op, &corpus(), &ctx(RenderProfile::Default))
                    .expect_err("unsupported");
                let text = format!("{err:#}");
                assert!(
                    text.contains("does not support"),
                    "{} {}: {text}",
                    backend.engine(),
                    op.name()
                );
            }
        }
    }

    /// GPU is optional hardware. When an adapter is present it has to produce
    /// a page the same size as vello-cpu; when it is not, skip, never fail.
    #[cfg(feature = "gpu")]
    #[test]
    fn gpu_backend_renders_when_present() {
        if super::gpu_unavailable().is_some() {
            return;
        }
        let cpu = render(Backend::VelloCpu, RenderProfile::Default);
        let gpu = render(Backend::VelloGpu, RenderProfile::Default);
        assert_eq!(gpu.len(), cpu.len(), "same page, same size");
        assert!(!gpu.is_empty(), "gpu produced an empty raster");
    }
}
