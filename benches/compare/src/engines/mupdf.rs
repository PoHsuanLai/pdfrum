//! `mupdf`: `MuPDF` through its safe Rust wrapper, the C compiled in by
//! `mupdf-sys` at build time.

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

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

/// Every page at the run's DPI on `threads` threads, in `MuPDF`'s own
/// multi-threading model (`docs/examples/multi-threaded.c`): `Document` and
/// `Page` are not `Send`, so the calling thread opens the document, loads
/// every page and records it into a `DisplayList` (which is `Send + Sync`);
/// then `threads` workers pull list indices off an `AtomicUsize` and
/// rasterize in parallel, each worker's first mupdf call cloning its own
/// `fz_context` out of the wrapper's base context via `Context::get()`.
///
/// The display-list build is serial and it is **inside the timed pass** —
/// as the parse is for every other engine in this table, none of which is
/// given a free warm document either. It is therefore mupdf's Amdahl bound:
/// no thread count can take the run below the time that loop costs.
pub fn render_all(path: &Path, ctx: &Ctx<'_>, threads: usize) -> Result<usize> {
    let doc = open(path, ctx)?;
    let count = doc.page_count().map_err(|err| anyhow!("mupdf: {err}"))? as usize;
    let scale = ctx.scale() as f32;
    let matrix = Matrix::new_scale(scale, scale);

    // Serial: load and record. `to_display_list(true)` keeps annotations,
    // matching `to_pixmap(.., show_extras = true)` on the single-page path.
    let mut lists = Vec::with_capacity(count);
    for index in 0..count {
        lists.push(
            doc.load_page(index as i32)
                .map_err(|err| anyhow!("mupdf: {err}"))?
                .to_display_list(true)
                .map_err(|err| anyhow!("mupdf: {err}"))?,
        );
    }

    // Parallel: rasterize, same DPI and colorspace as the single-thread path.
    let threads = threads.clamp(1, count.max(1));
    let next = AtomicUsize::new(0);
    let lists = &lists;
    let next = &next;
    let matrix = &matrix;
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(move || -> Result<()> {
                    let colorspace = Colorspace::device_rgb();
                    loop {
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        let Some(list) = lists.get(index) else {
                            return Ok(());
                        };
                        list.to_pixmap(matrix, &colorspace, false)
                            .map_err(|err| anyhow!("mupdf: {err}"))?;
                    }
                })
            })
            .collect();
        for handle in handles {
            handle
                .join()
                .map_err(|_| anyhow!("mupdf: render thread panicked"))??;
        }
        Ok::<(), anyhow::Error>(())
    })?;
    Ok(count)
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

#[cfg(test)]
mod tests {
    use super::{Colorspace, Matrix, Path};
    use mupdf::Document;

    /// The parallel path rasterizes a recorded `DisplayList`; the
    /// single-page path rasterizes the `Page` directly. Same page, same
    /// matrix, same colorspace: the pixmaps must be identical, or the
    /// throughput table would be timing different work from the render
    /// table. Checked on the corpus's first file, at the run default DPI.
    #[test]
    fn display_list_pixmap_equals_page_pixmap() {
        let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("../corpus/forms_combo_box.pdf");
        assert!(file.is_file(), "corpus file missing: {}", file.display());
        let doc = Document::open(file.as_path()).expect("open");
        let scale = 150.0_f32 / 72.0;
        let matrix = Matrix::new_scale(scale, scale);
        let colorspace = Colorspace::device_rgb();
        let count = doc.page_count().expect("page count");
        assert!(count > 0);
        for index in 0..count {
            let page = doc.load_page(index).expect("load page");
            let direct = page
                .to_pixmap(&matrix, &colorspace, false, true)
                .expect("page pixmap");
            let list = page.to_display_list(true).expect("display list");
            let via_list = list
                .to_pixmap(&matrix, &colorspace, false)
                .expect("list pixmap");
            assert_eq!(
                (direct.width(), direct.height()),
                (via_list.width(), via_list.height())
            );
            assert_eq!(direct.n(), via_list.n());
            assert_eq!(direct.samples(), via_list.samples(), "page {index} differs");
        }
    }
}
