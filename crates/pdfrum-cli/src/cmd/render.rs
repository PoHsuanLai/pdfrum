//! `pdfrum render`: pages to PNG.

use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use pdfrum::{Document, Pixmap, RenderOptions, RenderSession, VelloCpuBackend};

use crate::{out, pages};

/// `scale` when it is a usable one: finite and positive.
pub fn checked_scale(scale: f64) -> Result<f64> {
    if !(scale.is_finite() && scale > 0.0) {
        bail!("the resolution must be a positive number");
    }
    Ok(scale)
}

/// One page rendered at `scale` pixels per point, with or without its
/// annotations, through `session` so glyphs and images are cached between
/// pages.
pub fn render_page(
    doc: &Document,
    index: u32,
    scale: f64,
    annotations: bool,
    session: &mut RenderSession,
) -> Result<Pixmap> {
    let page = doc.page(index)?;
    let mut options = RenderOptions::scaled(scale);
    options.annotations = annotations;
    Ok(page.render_on(VelloCpuBackend, &options, session)?)
}

/// Everything a render was asked for.
pub struct Request<'a> {
    pub file: &'a Path,
    pub password: Option<&'a str>,
    /// The output template, or `-`.
    pub output: &'a str,
    pub pages: Option<&'a str>,
    /// Pixels per point.
    pub scale: f64,
    pub annotations: bool,
}

pub fn run(req: &Request<'_>) -> Result<ExitCode> {
    let scale = checked_scale(req.scale)?;
    let to_stdout = out::Sink::new(Path::new(req.output), "PNG")?.is_stdout();
    let doc = out::open(req.file, req.password)?;
    let selected = pages::select(req.pages, doc.page_count())?;
    if to_stdout && selected.len() != 1 {
        bail!(
            "`--output -` writes one page's PNG to stdout; {} pages were selected (use --pages)",
            selected.len()
        );
    }
    let stem = out::stem(req.file);
    let mut session = RenderSession::new();
    for index in selected {
        let pixmap = render_page(&doc, index, scale, req.annotations, &mut session)?;
        let number = index + 1;
        if to_stdout {
            out::write_bytes(&pixmap.encode_png()?);
            out::notice(
                Path::new("-"),
                &format!("page {number}, {} x {} px", pixmap.width(), pixmap.height()),
            );
        } else {
            let path = req
                .output
                .replace("{n}", &number.to_string())
                .replace("{stem}", &stem);
            pixmap
                .save_png(&path)
                .with_context(|| format!("cannot write {path}"))?;
            if !out::quiet() {
                eprintln!(
                    "{path}: page {number}, {} x {} px",
                    pixmap.width(),
                    pixmap.height()
                );
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}
