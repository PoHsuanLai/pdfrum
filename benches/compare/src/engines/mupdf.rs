//! `mupdf`: `MuPDF` through its safe Rust wrapper, the C compiled in by
//! `mupdf-sys` at build time.

use std::path::Path;

use anyhow::{Result, anyhow};
use mupdf::{Colorspace, Document, Matrix, TextExtractOptions};

use crate::model::{Ctx, Op, Output, Raster, Timed};

fn open(path: &Path, ctx: &Ctx<'_>) -> Result<Document> {
    let mut doc = Document::open(path).map_err(|err| anyhow!("mupdf: {err}"))?;
    if let Some(password) = ctx.password
        && !doc
            .authenticate(password)
            .map_err(|err| anyhow!("mupdf: {err}"))?
    {
        return Err(anyhow!("mupdf: password rejected"));
    }
    Ok(doc)
}

/// Every page, one thread. The `threads` argument is accepted and ignored
/// until the display-list path lands in the next commit.
pub fn render_all(path: &Path, ctx: &Ctx<'_>, _threads: usize) -> Result<usize> {
    let doc = open(path, ctx)?;
    let count = doc.page_count().map_err(|err| anyhow!("mupdf: {err}"))?;
    let scale = ctx.scale() as f32;
    let matrix = Matrix::new_scale(scale, scale);
    let colorspace = Colorspace::device_rgb();
    for index in 0..count {
        doc.load_page(index)
            .map_err(|err| anyhow!("mupdf: {err}"))?
            .to_pixmap(&matrix, &colorspace, false, true)
            .map_err(|err| anyhow!("mupdf: {err}"))?;
    }
    Ok(count as usize)
}

pub fn run(op: Op, path: &Path, ctx: &Ctx<'_>) -> Result<Timed> {
    match op {
        Op::Open => {
            let (times_ms, pages) = ctx.measure(|| {
                let doc = open(path, ctx)?;
                Ok(doc.page_count().map_err(|err| anyhow!("mupdf: {err}"))? as usize)
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
            let page = doc.load_page(0).map_err(|err| anyhow!("mupdf: {err}"))?;
            let scale = ctx.scale() as f32;
            let matrix = Matrix::new_scale(scale, scale);
            let colorspace = Colorspace::device_rgb();
            let (times_ms, pixmap) = ctx.measure(|| {
                page.to_pixmap(&matrix, &colorspace, false, true)
                    .map_err(|err| anyhow!("mupdf: {err}"))
            })?;
            let (width, height) = (pixmap.width(), pixmap.height());
            let n = usize::from(pixmap.n());
            let samples = pixmap.samples();
            let mut rgba = Vec::with_capacity((width * height * 4) as usize);
            for px in samples.chunks_exact(n) {
                rgba.extend_from_slice(&[px[0], px[1], px[2], if n == 4 { px[3] } else { 255 }]);
            }
            Ok(Timed {
                times_ms,
                output: Output::Rendered(Raster {
                    width,
                    height,
                    rgba,
                    premultiplied: false,
                }),
            })
        }
        Op::Text => {
            let doc = open(path, ctx)?;
            let page = doc.load_page(0).map_err(|err| anyhow!("mupdf: {err}"))?;
            let (times_ms, text) = ctx.measure(|| {
                page.text(TextExtractOptions::default())
                    .map_err(|err| anyhow!("mupdf: {err}"))
            })?;
            Ok(Timed {
                times_ms,
                output: Output::Text(text),
            })
        }
    }
}
