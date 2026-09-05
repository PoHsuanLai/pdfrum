//! The subject, through the facade exactly as `cargo add pdfrum` delivers it.

use std::path::Path;

use anyhow::{Result, anyhow};
use pdfrum::{
    Document, RenderOptions, RenderSession, SubstitutionOptions, VelloCpuBackend,
};

use crate::model::{Ctx, Op, Output, Raster, Timed};

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

/// Every page on `threads` threads sharing one `Document` (`Sync`), each
/// thread with its own backend and `RenderSession`, pages dealt round-robin.
pub fn render_all(path: &Path, ctx: &Ctx<'_>, threads: usize) -> Result<usize> {
    let doc = open(path, ctx)?;
    let pages = doc.page_count() as usize;
    let threads = threads.clamp(1, pages.max(1));
    let options = RenderOptions::scaled(ctx.scale());
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..threads)
            .map(|first| {
                let doc = &doc;
                let options = &options;
                scope.spawn(move || -> Result<()> {
                    let backend = VelloCpuBackend::new();
                    let mut session = session(doc, ctx);
                    for index in (first..pages).step_by(threads) {
                        doc.page(index as u32)?
                            .render_on(&backend, options, &mut session)?;
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

pub fn run(op: Op, path: &Path, ctx: &Ctx<'_>) -> Result<Timed> {
    match op {
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
        Op::Render => {
            let doc = open(path, ctx)?;
            let page = doc.page(0)?;
            let backend = VelloCpuBackend::new();
            let options = RenderOptions::scaled(ctx.scale());
            let mut session = session(&doc, ctx);
            let (times_ms, pixmap) =
                ctx.measure(|| Ok(page.render_on(&backend, &options, &mut session)?))?;
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
        Op::Text => {
            let doc = open(path, ctx)?;
            let page = doc.page(0)?;
            let mut session = session(&doc, ctx);
            let (times_ms, text) = ctx.measure(|| Ok(page.text_on(&mut session).to_string()))?;
            Ok(Timed {
                times_ms,
                output: Output::Text(text),
            })
        }
    }
}
