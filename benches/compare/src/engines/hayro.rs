//! `hayro` — the closest pure-Rust rasterizer — and `hayro-interpret`'s
//! glyph stream, which this harness assembles into text because the crate
//! has no text API of its own.

use std::path::Path;

use anyhow::{Result, anyhow};
use hayro::hayro_interpret::hayro_syntax::Pdf;
use hayro::hayro_interpret::{
    BlendMode, ClipPath, Context, Device, GlyphDrawMode, Image, InterpreterCache,
    InterpreterSettings, Paint, PathDrawMode, SoftMask, TransformExt,
    font::{Glyph, OutlineGlyph},
    interpret_page,
};
use hayro::{RenderCache, RenderSettings};
use kurbo::{Affine, BezPath, Rect};

use crate::engines::unsupported;
use crate::model::{Ctx, Op, Output, Raster, Timed};

fn load(path: &Path, ctx: &Ctx<'_>) -> Result<Pdf> {
    let bytes = std::fs::read(path)?;
    match ctx.password {
        Some(password) => Pdf::new_with_password(bytes, password),
        None => Pdf::new(bytes),
    }
    .map_err(|err| anyhow!("hayro: {err:?}"))
}

fn settings(ctx: &Ctx<'_>) -> RenderSettings {
    let scale = ctx.scale() as f32;
    RenderSettings {
        x_scale: scale,
        y_scale: scale,
        width: None,
        height: None,
        bg_color: hayro::vello_cpu::color::palette::css::WHITE,
    }
}

/// Every page on `threads` threads sharing one `Pdf`, each thread with its
/// own `RenderCache` (which holds `Rc`s and cannot cross threads).
pub fn render_all(path: &Path, ctx: &Ctx<'_>, threads: usize) -> Result<usize> {
    let pdf = load(path, ctx)?;
    let pages = pdf.pages();
    let count = pages.len();
    let threads = threads.clamp(1, count.max(1));
    let settings = settings(ctx);
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..threads)
            .map(|first| {
                scope.spawn(move || {
                    let cache = RenderCache::new();
                    let interpreter = InterpreterSettings::default();
                    for index in (first..count).step_by(threads) {
                        if let Some(page) = pages.get(index) {
                            hayro::render(page, &cache, &interpreter, &settings);
                        }
                    }
                })
            })
            .collect();
        for handle in handles {
            handle
                .join()
                .map_err(|_| anyhow!("hayro: render thread panicked"))?;
        }
        Ok::<(), anyhow::Error>(())
    })?;
    Ok(count)
}

pub fn run(op: Op, path: &Path, ctx: &Ctx<'_>) -> Result<Timed> {
    match op {
        Op::Open => {
            let (times_ms, pages) = ctx.measure(|| Ok(load(path, ctx)?.pages().len()))?;
            Ok(Timed {
                times_ms,
                output: Output::Opened {
                    pages,
                    objects: None,
                },
            })
        }
        Op::Render => {
            let pdf = load(path, ctx)?;
            let page = pdf
                .pages()
                .first()
                .ok_or_else(|| anyhow!("hayro: no pages"))?;
            let cache = RenderCache::new();
            let interpreter = InterpreterSettings::default();
            let settings = settings(ctx);
            let (times_ms, pixmap) =
                ctx.measure(|| Ok(hayro::render(page, &cache, &interpreter, &settings)))?;
            Ok(Timed {
                times_ms,
                output: Output::Rendered(Raster {
                    width: u32::from(pixmap.width()),
                    height: u32::from(pixmap.height()),
                    rgba: pixmap.data_as_u8_slice().to_vec(),
                    premultiplied: true,
                }),
            })
        }
        Op::Text => Err(unsupported("hayro", op)),
    }
}

/// `hayro-interpret` as a text source: the glyphs in draw order, a newline
/// when the baseline moves, a space when the pen jumps more than a glyph's
/// advance would explain. The interpreter is the crate's; the assembly is
/// the harness's, and the tables say so.
pub fn run_text(op: Op, path: &Path, ctx: &Ctx<'_>) -> Result<Timed> {
    if op != Op::Text {
        return Err(unsupported("hayro-interpret", op));
    }
    let pdf = load(path, ctx)?;
    let page = pdf
        .pages()
        .first()
        .ok_or_else(|| anyhow!("hayro-interpret: no pages"))?;
    let cache = InterpreterCache::new();
    let settings = InterpreterSettings::default();
    let (times_ms, text) = ctx.measure(|| {
        let (width, height) = page.render_dimensions();
        let initial = page.initial_transform(true).to_kurbo();
        let mut context = Context::new(
            initial,
            Rect::new(0.0, 0.0, f64::from(width), f64::from(height)),
            &cache,
            page.xref(),
            settings.clone(),
        );
        let mut device = TextDevice::default();
        interpret_page(page, &mut context, &mut device);
        Ok(device.finish())
    })?;
    Ok(Timed {
        times_ms,
        output: Output::Text(text),
    })
}

#[derive(Default)]
struct TextDevice {
    out: String,
    last: Option<(f64, f64)>,
}

impl TextDevice {
    fn finish(self) -> String {
        self.out
    }
}

impl<'a> Device<'a> for TextDevice {
    fn set_soft_mask(&mut self, _: Option<SoftMask<'a>>) {}
    fn set_blend_mode(&mut self, _: BlendMode) {}
    fn draw_path(&mut self, _: &BezPath, _: Affine, _: &Paint<'a>, _: &PathDrawMode) {}
    fn push_clip_path(&mut self, _: &ClipPath) {}
    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}
    fn draw_glyph(
        &mut self,
        glyph: &Glyph<'a>,
        transform: Affine,
        glyph_transform: Affine,
        _: &Paint<'a>,
        _: &GlyphDrawMode,
    ) {
        // `glyph_transform` maps glyph units (1/1000 em) to user space, so
        // its x coefficient times 1000 is the em in device units.
        let to_device = transform * glyph_transform;
        let origin = to_device * kurbo::Point::ZERO;
        let (x, y) = (origin.x, origin.y);
        let scale = to_device.as_coeffs()[0].abs();
        let em = scale * 1000.0;
        if let Some((last_x, last_y)) = self.last {
            if (y - last_y).abs() > 0.1 * em.max(1.0) {
                self.out.push('\n');
            } else if x - last_x > 0.12 * em
                && !self.out.ends_with(' ')
                && !self.out.ends_with('\n')
            {
                self.out.push(' ');
            }
        }
        let advance = f64::from(
            glyph
                .as_outline()
                .and_then(OutlineGlyph::advance_width)
                .unwrap_or(0.0),
        );
        self.last = Some((x + advance * scale, y));
        match glyph.as_unicode() {
            Some(hayro::hayro_interpret::hayro_cmap::BfString::Char(ch)) => self.out.push(ch),
            Some(hayro::hayro_interpret::hayro_cmap::BfString::String(s)) => self.out.push_str(&s),
            None => {}
        }
    }
    fn draw_image(&mut self, _: Image<'a, '_>, _: Affine) {}
    fn pop_clip_path(&mut self) {}
    fn pop_transparency_group(&mut self) {}
}

/// The outline half of a glyph, when it has one.
trait AsOutline {
    fn as_outline(&self) -> Option<&OutlineGlyph>;
}

impl AsOutline for Glyph<'_> {
    fn as_outline(&self) -> Option<&OutlineGlyph> {
        match self {
            Glyph::Outline(g) => Some(g),
            Glyph::Type3(_) => None,
        }
    }
}
