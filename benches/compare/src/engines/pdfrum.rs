//! The subject, through the facade exactly as `cargo add pdfrum` delivers it.

use std::path::Path;

use anyhow::Result;
use pdfrum::{
    BuildContext, Document, RenderOptions, RenderSession, SubstitutionOptions, VelloCpuBackend,
};

use crate::model::{Ctx, Op, Output, Raster, Timed};

fn open(path: &Path, ctx: &Ctx<'_>) -> Result<Document> {
    Ok(match ctx.password {
        Some(password) => Document::open_with_password(path, password.as_bytes())?,
        None => open(path, ctx)?,
    })
}

/// A session whose font substitution reads the oracle's hermetic directory,
/// so a non-embedded font resolves to the same face on both sides. The knob
/// is the facade's public `BuildContext::with_substitution`; without a
/// `--font-dir` the session is the plain default.
fn session(ctx: &Ctx<'_>) -> RenderSession {
    match ctx.font_dir {
        Some(dir) => RenderSession {
            build: BuildContext::with_substitution(SubstitutionOptions {
                font_dirs: vec![dir.to_path_buf()],
                croscore_font_names: true,
                ..SubstitutionOptions::default()
            }),
            ..RenderSession::default()
        },
        None => RenderSession::new(),
    }
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
            let mut session = session(ctx);
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
            let mut session = session(ctx);
            let (times_ms, text) = ctx.measure(|| Ok(page.text_on(&mut session).to_string()))?;
            Ok(Timed {
                times_ms,
                output: Output::Text(text),
            })
        }
    }
}
