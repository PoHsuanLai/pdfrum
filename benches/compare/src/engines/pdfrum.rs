//! The subject, through the facade exactly as `cargo add pdfrum` delivers it.

use std::path::Path;

use anyhow::{Result, anyhow};
use pdfrum::{
    CharIndex, Document, RenderOptions, RenderSession, SubstitutionOptions, TextPage,
    VelloCpuBackend,
};

use crate::model::{Ctx, Op, Output, Raster, RenderProfile, Timed};

/// The `chars` stream as text, which is the stream `pdfium_test --txt`
/// writes: `FPDFText_GetUnicode` for every `i` in `FPDFText_CountChars`
/// (`testing/pdfium_test/write.cc:364-370`), unfiltered.
///
/// Deliberately **not** `TextPage`'s `Display`, which is the search text and
/// drops the control characters and hyphen sentinels the oracle keeps
/// (`crates/pdfrum-text/src/lib.rs:417-419`). It is the same construction the
/// conformance runner's tier-A `--txt` dump uses
/// (`crates/pdfrum-tool/src/text.rs::to_utf32le`), so the harness and the
/// board compare the same bytes. A code point the oracle writes that is not a
/// scalar value — a lone surrogate — is dropped rather than replaced, since
/// `String` cannot hold one.
fn chars_stream(page: &TextPage) -> String {
    (0..page.char_count())
        .filter_map(|i| page.char(CharIndex::new(i)).ok())
        .filter_map(|info| char::from_u32(info.unicode))
        .collect()
}

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

/// The render options the run's profile asks for.
///
/// `Default` is `RenderOptions::scaled`, which is what a caller who writes
/// no options gets: `smooth_paths`, `interpolate_images` and `annotations`
/// all on (`crates/pdfrum/src/render.rs:69-72`). `Parity` clears the three,
/// which is the cheapest configuration the public API offers and what
/// `docs/benchmarks/README.md`'s parity columns measure. Glyph antialiasing
/// stays on in both: no peer offers a knob for it, so turning it off would
/// swap one asymmetry for another.
fn options(ctx: &Ctx<'_>) -> RenderOptions {
    let options = RenderOptions::scaled(ctx.scale());
    match ctx.profile {
        RenderProfile::Default => options,
        RenderProfile::Parity => RenderOptions {
            smooth_paths: false,
            interpolate_images: false,
            annotations: false,
            ..options
        },
    }
}

/// Every page on `threads` threads sharing one `Document` (`Sync`), each
/// thread with its own backend and `RenderSession`, pages dealt round-robin.
pub fn render_all(path: &Path, ctx: &Ctx<'_>, threads: usize) -> Result<usize> {
    let doc = open(path, ctx)?;
    let pages = doc.page_count() as usize;
    let threads = threads.clamp(1, pages.max(1));
    let options = options(ctx);
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
            let options = options(ctx);
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
            let (times_ms, text) = ctx.measure(|| Ok(chars_stream(&page.text_on(&mut session))))?;
            Ok(Timed {
                times_ms,
                output: Output::Text(text),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::time::Duration;

    use super::{Ctx, Op, Output, RenderProfile, run};

    fn ctx(profile: RenderProfile) -> Ctx<'static> {
        Ctx {
            dpi: 150.0,
            font_dir: None,
            pdfium_lib: None,
            password: None,
            warm_runs: 0,
            budget: Duration::from_secs(30),
            profile,
        }
    }

    fn render(profile: RenderProfile) -> Vec<u8> {
        let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("../corpus/vector_paths_1751.pdf");
        assert!(file.is_file(), "corpus file missing: {}", file.display());
        let timed = run(Op::Render, &file, &ctx(profile)).expect("render");
        match timed.output {
            Output::Rendered(raster) => raster.rgba,
            other => panic!("not a raster: {other:?}"),
        }
    }

    /// The whole point of `--parity` is that it renders different pixels: a
    /// profile that quietly produced the default raster would report a
    /// speed gap that bought nothing. A path-heavy corpus file must differ
    /// once path antialiasing is off.
    #[test]
    fn parity_changes_the_pixels() {
        let default = render(RenderProfile::Default);
        let parity = render(RenderProfile::Parity);
        assert_eq!(default.len(), parity.len(), "same page, same size");
        assert_ne!(
            default, parity,
            "parity produced the default raster: the profile is not reaching RenderOptions"
        );
    }
}
