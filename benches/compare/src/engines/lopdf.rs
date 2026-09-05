//! `lopdf`: open and walk every object, and its own `extract_text`.

use std::path::Path;

use anyhow::Result;
use lopdf::{Document, LoadOptions, Object};

use crate::engines::unsupported;
use crate::model::{Ctx, Op, Output, Timed};

fn load(bytes: &[u8], ctx: &Ctx<'_>) -> Result<Document> {
    Ok(match ctx.password {
        Some(password) => Document::load_mem_with_options(
            bytes,
            LoadOptions {
                password: Some(password.to_owned()),
                ..LoadOptions::default()
            },
        )?,
        None => Document::load_mem(bytes)?,
    })
}

/// Touches every object once, so "open" means the whole file was parsed
/// rather than only its trailer.
fn walk(doc: &Document) -> usize {
    let mut count = 0;
    for object in doc.objects.values() {
        count += 1;
        if let Object::Stream(stream) = object {
            // Reading the dictionary is what a caller would do; decoding
            // every stream is not, and would measure the codecs.
            count += usize::from(!stream.dict.is_empty());
        }
    }
    count
}

pub fn run(op: Op, path: &Path, ctx: &Ctx<'_>) -> Result<Timed> {
    let bytes = std::fs::read(path)?;
    match op {
        Op::Open => {
            let (times_ms, (pages, objects)) = ctx.measure(|| {
                let doc = load(&bytes, ctx)?;
                let objects = walk(&doc);
                Ok((doc.get_pages().len(), objects))
            })?;
            Ok(Timed {
                times_ms,
                output: Output::Opened {
                    pages,
                    objects: Some(objects),
                },
            })
        }
        Op::Text => {
            let doc = load(&bytes, ctx)?;
            let (times_ms, text) = ctx.measure(|| Ok(doc.extract_text(&[1])?))?;
            Ok(Timed {
                times_ms,
                output: Output::Text(text),
            })
        }
        Op::Render => Err(unsupported("lopdf", op)),
    }
}
