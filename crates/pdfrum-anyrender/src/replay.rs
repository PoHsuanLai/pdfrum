//! A recorded scene replayed onto a pdfrum canvas. anyrender's layers are a push/pop stack; a
//! pdfrum canvas has only nested saved states (`Canvas::saved` takes a closure, so an unbalanced
//! `q`/`Q` cannot be written). The recording turns one into the other: a pushed layer's
//! commands, up to its pop, are replayed inside one saved state.

use crate::area::{GlyphArea, GlyphTally};
use crate::glyphs;
use crate::paint;
use crate::prepare::Prepared;
use crate::sources::Sources;
use anyrender::Paint as Brush;
use anyrender::recording::{FillCommand, RenderCommand, StrokeCommand};
use pdfrum_edit::{Canvas, Fill, Paint};
use peniko::kurbo::{Affine, BezPath, Rect};
use peniko::{BlendMode, ImageBrush};
use std::slice::Iter;

/// Replays scenes, counting what became of their glyphs.
pub(crate) struct Replay<'p> {
    prepared: &'p Prepared,
    sources: &'p Sources,
    area: GlyphArea,
    pub(crate) tally: GlyphTally,
}

impl<'p> Replay<'p> {
    pub(crate) fn new(prepared: &'p Prepared, sources: &'p Sources, area: GlyphArea) -> Self {
        Replay {
            prepared,
            sources,
            area,
            tally: GlyphTally::default(),
        }
    }

    /// Replay `commands` up to the pop that closes the current layer (or their end).
    pub(crate) fn run(
        &mut self,
        canvas: &mut Canvas<'_, '_>,
        commands: &mut Iter<'_, RenderCommand>,
    ) {
        while let Some(command) = commands.next() {
            match command {
                RenderCommand::PushLayer(layer) => {
                    let clip = layer.transform * layer.clip.clone();
                    canvas.saved(|canvas| {
                        Self::layer(canvas, &clip, layer.blend, layer.alpha);
                        self.run(canvas, commands);
                    });
                }
                RenderCommand::PushClipLayer(layer) => {
                    let clip = layer.transform * layer.clip.clone();
                    canvas.saved(|canvas| {
                        Self::layer(canvas, &clip, BlendMode::default(), 1.0);
                        self.run(canvas, commands);
                    });
                }
                RenderCommand::PopLayer => return,
                RenderCommand::Fill(fill) => self.fill(canvas, fill),
                RenderCommand::Stroke(stroke) => stroke_shape(canvas, stroke),
                RenderCommand::GlyphRun(run) => {
                    let key = crate::faces::FaceKey::of(&run.font_data, &run.normalized_coords);
                    if let Some((font, metrics)) = self.prepared.faces.get(&key) {
                        self.tally = self.tally
                            + glyphs::draw(canvas, run, (font, metrics), self.sources, self.area);
                    }
                }
                // No shadows in a printout, and PDF has no blur to draw one with.
                RenderCommand::BoxShadow(_) => {}
            }
        }
    }

    /// A layer's clip, blend mode and opacity, inside its saved state.
    fn layer(canvas: &mut Canvas<'_, '_>, clip: &BezPath, blend: BlendMode, alpha: f32) {
        if !clip.elements().is_empty() {
            canvas.clip(clip.clone(), Fill::NonZero);
        }
        if let Some(mode) = paint::blend(blend) {
            canvas.blend(mode);
        }
        if alpha < 1.0 {
            canvas.opacity(f64::from(alpha.max(0.0)));
        }
    }

    fn fill(&self, canvas: &mut Canvas<'_, '_>, fill: &FillCommand) {
        let rule = paint::fill(fill.fill);
        let shape = fill.transform * fill.shape.clone();
        match &fill.brush {
            Brush::Solid(colour) => canvas.draw(shape, Paint::Fill(*colour), rule),
            Brush::Gradient(gradient) => match paint::gradient(gradient) {
                Some(gradient) => canvas.fill_gradient(
                    shape,
                    rule,
                    &gradient,
                    fill.transform * fill.brush_transform.unwrap_or_default(),
                ),
                None => {
                    if let Some(colour) = paint::colour(&fill.brush) {
                        canvas.draw(shape, Paint::Fill(colour), rule);
                    }
                }
            },
            Brush::Image(brush) => self.image(canvas, fill, brush, shape),
            Brush::Resource(_) | Brush::Custom(_) => {}
        }
    }

    /// An image brush: the image drawn once in its own pixel grid, clipped to the shape.
    fn image(
        &self,
        canvas: &mut Canvas<'_, '_>,
        fill: &FillCommand,
        brush: &ImageBrush,
        shape: BezPath,
    ) {
        let Some(image) = self.prepared.images.get(&brush.image.data.id()) else {
            return;
        };
        let (width, height) = (f64::from(brush.image.width), f64::from(brush.image.height));
        let placed = fill.transform
            * fill.brush_transform.unwrap_or_default()
            // A PDF image's first row is at the top of its unit square, which in the y-down
            // scene is its largest y: turn it over within its own height.
            * Affine::new([1.0, 0.0, 0.0, -1.0, 0.0, height]);
        let alpha = brush.sampler.alpha;
        canvas.saved(|canvas| {
            canvas.clip(shape, Fill::NonZero);
            if alpha < 1.0 {
                canvas.opacity(f64::from(alpha.max(0.0)));
            }
            canvas.transform(placed);
            canvas.image(image, Rect::new(0.0, 0.0, width, height));
        });
    }
}

/// A stroke, in its shape's own space so its width scales with the transform.
fn stroke_shape(canvas: &mut Canvas<'_, '_>, stroke: &StrokeCommand) {
    let Some(colour) = paint::colour(&stroke.brush) else {
        return;
    };
    let pen = paint::stroke(&stroke.style, colour);
    canvas.saved(|canvas| {
        canvas.transform(stroke.transform);
        canvas.stroke(stroke.shape.clone(), pen);
    });
}
