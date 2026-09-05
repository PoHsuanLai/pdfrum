//! `pdf` (pdf-rs): open, and load every page's resources so the page tree
//! and its dictionaries are actually parsed.

use std::path::Path;

use anyhow::{Result, anyhow};
use pdf::file::FileOptions;

use crate::engines::unsupported;
use crate::model::{Ctx, Op, Output, Timed};

pub fn run(op: Op, path: &Path, ctx: &Ctx<'_>) -> Result<Timed> {
    match op {
        Op::Open => {
            let (times_ms, (pages, objects)) = ctx.measure(|| {
                let password = ctx.password.map_or(&b""[..], str::as_bytes);
                let file = FileOptions::cached()
                    .password(password)
                    .open(path)
                    .map_err(|err| anyhow!("pdf: {err}"))?;
                let mut objects = 0;
                for page in file.pages() {
                    let page = page.map_err(|err| anyhow!("pdf: {err}"))?;
                    let resources = page.resources().map_err(|err| anyhow!("pdf: {err}"))?;
                    objects += 1 + resources.fonts.len() + resources.xobjects.len();
                }
                Ok((file.num_pages() as usize, objects))
            })?;
            Ok(Timed {
                times_ms,
                output: Output::Opened {
                    pages,
                    objects: Some(objects),
                },
            })
        }
        Op::Render | Op::Text => Err(unsupported("pdf", op)),
    }
}
