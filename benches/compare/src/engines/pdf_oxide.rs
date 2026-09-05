//! `pdf_oxide`: the crate whose README claims "5x faster, 100% pass on 3830
//! files". Open, render through its `rendering` module, text through
//! `extract_text`.

use std::path::Path;

use anyhow::{Result, anyhow};
use pdf_oxide::PdfDocument;
use pdf_oxide::rendering::{RenderOptions, render_page};

use crate::model::{Ctx, Op, Output, Raster, Timed};

fn open(path: &Path) -> Result<PdfDocument> {
    PdfDocument::open(path).map_err(|err| anyhow!("pdf_oxide: {err}"))
}

pub fn run(op: Op, path: &Path, ctx: &Ctx<'_>) -> Result<Timed> {
    match op {
        Op::Open => {
            let (times_ms, pages) = ctx.measure(|| {
                let doc = open(path)?;
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
            let doc = open(path)?;
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
            let doc = open(path)?;
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
