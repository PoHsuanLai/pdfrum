//! `pdf_oxide`: the crate whose README claims "5x faster, 100% pass on 3830
//! files". Open, render through its `rendering` module, text through
//! `extract_text`.

use std::path::Path;

use anyhow::{Result, anyhow};
use pdf_oxide::PdfDocument;
use pdf_oxide::rendering::{RenderOptions, render_page};

use crate::model::{Ctx, Op, Output, Raster, Timed};

fn open(path: &Path, ctx: &Ctx<'_>) -> Result<PdfDocument> {
    let doc = PdfDocument::open(path).map_err(|err| anyhow!("pdf_oxide: {err}"))?;
    if let Some(password) = ctx.password
        && !doc
            .authenticate(password.as_bytes())
            .map_err(|err| anyhow!("pdf_oxide: {err}"))?
    {
        return Err(anyhow!("pdf_oxide: password rejected"));
    }
    Ok(doc)
}

/// Every page on `threads` threads sharing one `PdfDocument`.
pub fn render_all(path: &Path, ctx: &Ctx<'_>, threads: usize) -> Result<usize> {
    let doc = open(path, ctx)?;
    let pages = doc
        .page_count()
        .map_err(|err| anyhow!("pdf_oxide: {err}"))?;
    let threads = threads.clamp(1, pages.max(1));
    let options = RenderOptions::with_dpi(ctx.dpi.round() as u32).as_raw();
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..threads)
            .map(|first| {
                let doc = &doc;
                let options = &options;
                scope.spawn(move || -> Result<()> {
                    for index in (first..pages).step_by(threads) {
                        render_page(doc, index, options)
                            .map_err(|err| anyhow!("pdf_oxide: {err}"))?;
                    }
                    Ok(())
                })
            })
            .collect();
        for handle in handles {
            handle
                .join()
                .map_err(|_| anyhow!("pdf_oxide: render thread panicked"))??;
        }
        Ok::<(), anyhow::Error>(())
    })?;
    Ok(pages)
}

pub fn run(op: Op, path: &Path, ctx: &Ctx<'_>) -> Result<Timed> {
    match op {
        Op::Open => {
            let (times_ms, pages) = ctx.measure(|| {
                let doc = open(path, ctx)?;
                doc.page_count().map_err(|err| anyhow!("pdf_oxide: {err}"))
            })?;
            Ok(Timed {
                times_ms,
                output: Output::Opened {
                    pages,
                    objects: None,
                },
            })
        }
        Op::Render => {
            let doc = open(path, ctx)?;
            let options = RenderOptions::with_dpi(ctx.dpi.round() as u32).as_raw();
            let (times_ms, image) = ctx.measure(|| {
                render_page(&doc, 0, &options).map_err(|err| anyhow!("pdf_oxide: {err}"))
            })?;
            Ok(Timed {
                times_ms,
                output: Output::Rendered(Raster {
                    width: image.width,
                    height: image.height,
                    rgba: image.data,
                    premultiplied: true,
                }),
            })
        }
        Op::Text => {
            let doc = open(path, ctx)?;
            let (times_ms, text) = ctx.measure(|| {
                doc.extract_text(0)
                    .map_err(|err| anyhow!("pdf_oxide: {err}"))
            })?;
            Ok(Timed {
                times_ms,
                output: Output::Text(text),
            })
        }
    }
}
