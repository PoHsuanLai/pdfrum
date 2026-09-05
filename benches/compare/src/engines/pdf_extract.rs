//! `pdf-extract`: text through the page-scoped `output_doc_page`, on the
//! `lopdf` it re-exports.

use std::path::Path;

use anyhow::{Result, anyhow};
use pdf_extract::{Document, PlainTextOutput, output_doc_page};

use crate::engines::unsupported;
use crate::model::{Ctx, Op, Output, Timed};

fn load(bytes: &[u8], ctx: &Ctx<'_>) -> Result<Document> {
    let mut doc = Document::load_mem(bytes)?;
    if let Some(password) = ctx.password {
        doc.decrypt(password)?;
    }
    Ok(doc)
}

pub fn run(op: Op, path: &Path, ctx: &Ctx<'_>) -> Result<Timed> {
    let bytes = std::fs::read(path)?;
    match op {
        Op::Open => {
            let (times_ms, pages) = ctx.measure(|| {
                let doc = load(&bytes, ctx)?;
                Ok(doc.get_pages().len())
            })?;
            Ok(Timed {
                times_ms,
                output: Output::Opened {
                    pages,
                    objects: None,
                },
            })
        }
        Op::Text => {
            let doc = load(&bytes, ctx)?;
            let (times_ms, text) = ctx.measure(|| {
                let mut text = String::new();
                {
                    let mut output = PlainTextOutput::new(&mut text);
                    output_doc_page(&doc, &mut output, 1)
                        .map_err(|err| anyhow!("pdf-extract: {err:?}"))?;
                }
                Ok(text)
            })?;
            Ok(Timed {
                times_ms,
                output: Output::Text(text),
            })
        }
        Op::Render => Err(unsupported("pdf-extract", op)),
    }
}
