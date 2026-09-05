//! `pdfium-render`: PDFium itself, through the Rust wrapper a crates.io user
//! gets, bound at runtime to a `libpdfium.so` the harness is pointed at.

use std::path::Path;

use anyhow::{Result, anyhow};
use pdfium_render::prelude::*;

use crate::model::{Ctx, Op, Output, Raster, Timed};

fn bind(ctx: &Ctx<'_>) -> Result<Pdfium> {
    let lib = ctx
        .pdfium_lib
        .ok_or_else(|| anyhow!("pdfium-render: no --pdfium-lib given (needs libpdfium.so)"))?;
    let bindings =
        Pdfium::bind_to_library(lib).map_err(|err| anyhow!("pdfium-render: bind: {err:?}"))?;
    Ok(Pdfium::new(bindings))
}

thread_local! {
    // The crate binds a library once per process (`bind_to_library` refuses
    // a second call), and PDFium wants every call on one thread — so one
    // binding, on the thread that made it.
    static PDFIUM: std::cell::OnceCell<Pdfium> = const { std::cell::OnceCell::new() };
}

fn with_pdfium<T>(ctx: &Ctx<'_>, f: impl FnOnce(&Pdfium) -> Result<T>) -> Result<T> {
    PDFIUM.with(|cell| {
        if cell.get().is_none() {
            let pdfium = bind(ctx)?;
            let _ = cell.set(pdfium);
        }
        f(cell
            .get()
            .ok_or_else(|| anyhow!("pdfium-render: binding unavailable"))?)
    })
}

/// Every page, one thread: PDFium requires every call on the thread that
/// initialised it.
pub fn render_all(path: &Path, ctx: &Ctx<'_>) -> Result<usize> {
    with_pdfium(ctx, |pdfium| render_all_with(pdfium, path, ctx))
}

fn render_all_with(pdfium: &Pdfium, path: &Path, ctx: &Ctx<'_>) -> Result<usize> {
    let doc = pdfium
        .load_pdf_from_file(path, ctx.password)
        .map_err(|err| anyhow!("pdfium-render: {err:?}"))?;
    let count = doc.pages().len();
    for index in 0..count {
        let page = doc
            .pages()
            .get(index)
            .map_err(|err| anyhow!("pdfium-render: {err:?}"))?;
        let (width, height) = ctx.oracle_size(
            f64::from(page.width().value),
            f64::from(page.height().value),
        );
        let config = PdfRenderConfig::new().set_target_size(width as i32, height as i32);
        page.render_with_config(&config)
            .map_err(|err| anyhow!("pdfium-render: {err:?}"))?;
    }
    Ok(count as usize)
}

pub fn run(op: Op, path: &Path, ctx: &Ctx<'_>) -> Result<Timed> {
    with_pdfium(ctx, |pdfium| run_with(pdfium, op, path, ctx))
}

fn run_with(pdfium: &Pdfium, op: Op, path: &Path, ctx: &Ctx<'_>) -> Result<Timed> {
    let open = || {
        pdfium
            .load_pdf_from_file(path, ctx.password)
            .map_err(|err| anyhow!("pdfium-render: {err:?}"))
    };
    match op {
        Op::Open => {
            let (times_ms, pages) = ctx.measure(|| Ok(open()?.pages().len() as usize))?;
            Ok(Timed {
                times_ms,
                output: Output::Opened {
                    pages,
                    objects: None,
                },
            })
        }
        Op::Render => {
            let doc = open()?;
            let page = doc
                .pages()
                .get(0)
                .map_err(|err| anyhow!("pdfium-render: {err:?}"))?;
            let (width, height) = ctx.oracle_size(
                f64::from(page.width().value),
                f64::from(page.height().value),
            );
            let config = PdfRenderConfig::new().set_target_size(width as i32, height as i32);
            let (times_ms, bitmap) = ctx.measure(|| {
                page.render_with_config(&config)
                    .map_err(|err| anyhow!("pdfium-render: {err:?}"))
            })?;
            Ok(Timed {
                times_ms,
                output: Output::Rendered(Raster {
                    width: bitmap.width() as u32,
                    height: bitmap.height() as u32,
                    rgba: bitmap.as_rgba_bytes(),
                    premultiplied: false,
                }),
            })
        }
        Op::Text => {
            let doc = open()?;
            let page = doc
                .pages()
                .get(0)
                .map_err(|err| anyhow!("pdfium-render: {err:?}"))?;
            let (times_ms, text) = ctx.measure(|| {
                Ok(page
                    .text()
                    .map_err(|err| anyhow!("pdfium-render: {err:?}"))?
                    .all())
            })?;
            Ok(Timed {
                times_ms,
                output: Output::Text(text),
            })
        }
    }
}
