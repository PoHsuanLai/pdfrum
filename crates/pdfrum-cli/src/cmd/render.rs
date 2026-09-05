//! `pdfrum render`: pages to PNG.

use std::io::Write;
use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use pdfrum::{RenderOptions, RenderSession, VelloCpuBackend};

use crate::{out, pages};

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
    if !(req.scale.is_finite() && req.scale > 0.0) {
        bail!("the resolution must be a positive number");
    }
    let doc = out::open(req.file, req.password)?;
    let selected = pages::select(req.pages, doc.page_count())?;
    if req.output == "-" && selected.len() != 1 {
        bail!(
            "`--output -` writes one page's PNG to stdout; {} pages were selected (use --pages)",
            selected.len()
        );
    }
    let stem = out::stem(req.file);
    let backend = VelloCpuBackend::new();
    let mut options = RenderOptions::scaled(req.scale);
    options.annotations = req.annotations;
    let mut session = RenderSession::new();
    for index in selected {
        let page = doc.page(index)?;
        let pixmap = page.render_on(&backend, &options, &mut session)?;
        let number = out::page_number(page.index());
        if req.output == "-" {
            let png = pixmap.encode_png()?;
            std::io::stdout()
                .write_all(&png)
                .context("cannot write to stdout")?;
        } else {
            let path = req
                .output
                .replace("{n}", &number.to_string())
                .replace("{stem}", &stem);
            pixmap
                .save_png(&path)
                .with_context(|| format!("cannot write {path}"))?;
            eprintln!(
                "{path}: page {number}, {} x {} px",
                pixmap.width(),
                pixmap.height()
            );
        }
    }
    Ok(ExitCode::SUCCESS)
}
