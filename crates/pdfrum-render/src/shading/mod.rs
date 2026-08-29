//! The seven shading rasterizers and their common entry
//! (`CPDF_RenderShading::Draw`, `cpdf_rendershading.cpp:1008-1118`).
//!
//! Six of the seven produce a [`Pixmap`] by pure engine code and are then
//! blitted; only the Coons/tensor pair draws through a rasterizer, and even
//! that one goes into a scratch buffer first (see [`patch`]). That structure
//! is what lets Tier C demand the two backends receive identical shading
//! pixels: the per-pixel maths never touches a rasterizer.
//!
//! # The device buffer is not scaled
//!
//! `CPDF_DeviceBuffer::CalculateMatrix` caps resolution at 150 dpi — but only
//! under `BUILDFLAG(IS_WIN)`. On the oracle's platform it is a pure
//! translation, so a shading is rasterized at exactly device resolution, one
//! shading pixel per device pixel, and blitted with a normal blend. The `150`
//! is dead here and is recorded only because it is the first thing a reader
//! looks for.

pub mod axial;
pub mod function;
pub mod gouraud;
pub mod patch;
pub mod radial;
pub mod steps;

use kurbo::Affine;
use pdfrum_page::Shading;
use pdfrum_page::shading::{Geometry, ShadingKind};

use crate::color::Argb;
use crate::device::RenderDevice;
use crate::options::{ColorMode, RenderOptions};
use crate::path::IntRect;
use crate::pixmap::Pixmap;
use steps::ColorSteps;

pub use steps::{STEPS, component_to_shading_index};

/// The `/Background` colour a shading clears its buffer to, if any.
///
/// Only honoured for a shading *pattern*; a bare `sh` operator ignores it —
/// which `pdfrum-page` has already resolved by leaving `background` `None` in
/// that case.
#[must_use]
pub fn background_color(shading: &Shading) -> Option<Argb> {
    shading.background.map(|rgb| {
        // Truncated, unlike the ramp's rounding.
        let [r, g, b] = rgb.to_bytes_truncating();
        Argb { a: 255, r, g, b }
    })
}

/// Rasterize a shading into a fresh pixmap covering `rect` in device space.
///
/// `to_device` maps shading space to device space; the buffer's own origin is
/// `rect`'s top-left, so the matrix handed to each rasterizer is `to_device`
/// translated back by that origin.
///
/// The types that draw through a rasterizer (6 and 7) are **not** handled
/// here — see [`draw_patches`], which needs a device rather than a buffer.
#[must_use]
pub fn draw_to_pixmap(
    shading: &Shading,
    rect: IntRect,
    to_device: Affine,
    alpha: u8,
    opts: &RenderOptions,
) -> Option<Pixmap> {
    let (w, h) = (
        u32::try_from(rect.width()).ok()?,
        u32::try_from(rect.height()).ok()?,
    );
    if w == 0 || h == 0 {
        return None;
    }
    let mut buffer = match background_color(shading) {
        Some(bg) => Pixmap::filled(w, h, bg.to_peniko()),
        None => Pixmap::new(w, h),
    };
    let to_bitmap = Affine::translate((-f64::from(rect.left), -f64::from(rect.top))) * to_device;

    match &shading.geometry {
        Geometry::Axial(a) => {
            let ramp = ColorSteps::sample(shading, a.t_min, a.t_max, alpha)?;
            axial::draw(&mut buffer, a, &ramp, to_bitmap);
        }
        Geometry::Radial(r) => {
            let ramp = ColorSteps::sample(shading, r.t_min, r.t_max, alpha)?;
            radial::draw(&mut buffer, r, &ramp, to_bitmap);
        }
        Geometry::FunctionBased(f) => {
            function::draw(&mut buffer, shading, f, alpha, to_bitmap);
        }
        Geometry::Mesh { kind, mesh } => match kind {
            ShadingKind::FreeFormMesh | ShadingKind::LatticeMesh => {
                let ramp = (!shading.functions.is_empty())
                    .then(|| ColorSteps::sample(shading, 0.0, 1.0, alpha))
                    .flatten();
                gouraud::draw(
                    &mut buffer,
                    &mesh.triangles,
                    ramp.as_ref(),
                    alpha,
                    to_bitmap,
                );
            }
            // Patches need a rasterizer; the caller reaches `draw_patches`.
            ShadingKind::CoonsMesh | ShadingKind::TensorMesh => return None,
            ShadingKind::FunctionBased | ShadingKind::Axial | ShadingKind::Radial => return None,
        },
        _ => return None,
    }

    post_process(&mut buffer, opts.color_mode);
    Some(buffer)
}

/// Rasterize a Coons or tensor mesh into a scratch device.
///
/// Every cell is drawn opaque, so abutting cells overpaint identically on
/// both backends; the shading's alpha belongs to the caller's blit, never to
/// a cell.
pub fn draw_patches(
    scratch: &mut dyn RenderDevice,
    shading: &Shading,
    rect: IntRect,
    to_device: Affine,
    tensor: bool,
) {
    let Geometry::Mesh { mesh, .. } = &shading.geometry else {
        return;
    };
    let to_bitmap = Affine::translate((-f64::from(rect.left), -f64::from(rect.top))) * to_device;
    // The ramp is sampled at full alpha for the same reason: the cells are
    // opaque and the alpha is applied once, at the blit.
    let ramp = (!shading.functions.is_empty())
        .then(|| ColorSteps::sample(shading, 0.0, 1.0, 255))
        .flatten();
    let (Ok(w), Ok(h)) = (u32::try_from(rect.width()), u32::try_from(rect.height())) else {
        return;
    };
    for p in &mesh.patches {
        if patch::patch_is_offscreen(p, to_bitmap, w, h) {
            continue;
        }
        patch::draw_patch(scratch, p, ramp.as_ref(), to_bitmap, tensor);
    }
}

/// The colour-mode post-pass a shading buffer takes
/// (`cpdf_rendershading.cpp:1111-1115`).
///
/// In `kAlpha` mode the red channel is set from the alpha — the group's
/// drawing wrote alpha as gray and the mask readback reinterprets it. In
/// `kGray` mode `ConvertColorScale(is_white_on_black = false)` runs, which at
/// 32 bits per pixel **ignores its flag entirely and never inverts** — so a
/// grayscale shading is grayscaled, not inverted, and the flag is dead.
fn post_process(buffer: &mut Pixmap, mode: ColorMode) {
    match mode {
        ColorMode::Alpha => {
            for chunk in buffer.data_mut().chunks_exact_mut(4) {
                let Some(&a) = chunk.get(3) else { continue };
                if let Some(slot) = chunk.first_mut() {
                    *slot = a;
                }
            }
        }
        ColorMode::Gray => {
            for chunk in buffer.data_mut().chunks_exact_mut(4) {
                let (Some(&r), Some(&g), Some(&b)) = (chunk.first(), chunk.get(1), chunk.get(2))
                else {
                    continue;
                };
                let v = crate::color::rgb_to_gray(r, g, b);
                for slot in chunk.iter_mut().take(3) {
                    *slot = v;
                }
                // Alpha is left alone, per the >8bpp arm's behaviour.
            }
        }
        ColorMode::Normal | ColorMode::Forced(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use kurbo::Point;
    use pdfrum_page::shading::Axial;
    use pdfrum_page::{ColorSpace, Rgb};

    use super::*;

    fn axial_shading(background: Option<Rgb>) -> Shading {
        Shading {
            geometry: Geometry::Axial(Axial {
                start: Point::new(0.0, 0.0),
                end: Point::new(8.0, 0.0),
                t_min: 0.0,
                t_max: 1.0,
                extend_start: false,
                extend_end: false,
            }),
            space: Arc::new(ColorSpace::DeviceGray),
            functions: Box::new([]),
            background,
            bbox: None,
        }
    }

    fn rect(w: i32, h: i32) -> IntRect {
        IntRect {
            left: 0,
            top: 0,
            right: w,
            bottom: h,
        }
    }

    #[test]
    fn background_is_truncated_not_rounded() {
        let bg = background_color(&axial_shading(Some(Rgb {
            r: 0.5,
            g: 0.5,
            b: 0.5,
        })));
        assert_eq!(
            bg.map(|c| c.r),
            Some(127),
            "the /Background conversion truncates"
        );
    }

    #[test]
    fn background_clears_the_buffer_before_rasterizing() {
        let shading = axial_shading(Some(Rgb {
            r: 1.0,
            g: 0.0,
            b: 0.0,
        }));
        let p = draw_to_pixmap(
            &shading,
            rect(4, 1),
            Affine::translate((100.0, 0.0)),
            255,
            &RenderOptions::default(),
        )
        .expect("rasterizes");
        // The axis is far to the right and unextended, so every pixel keeps
        // the background rather than being painted.
        assert_eq!(p.pixel(0, 0), Some([255, 0, 0, 255]));
    }

    #[test]
    fn an_empty_rect_produces_nothing() {
        let shading = axial_shading(None);
        assert!(
            draw_to_pixmap(
                &shading,
                rect(0, 4),
                Affine::IDENTITY,
                255,
                &RenderOptions::default()
            )
            .is_none()
        );
    }

    #[test]
    fn gray_mode_grayscales_without_inverting() {
        // D17: at >8bpp ConvertColorScale ignores is_white_on_black entirely.
        let mut p = Pixmap::filled(1, 1, peniko::Color::from_rgba8(255, 0, 0, 255));
        post_process(&mut p, ColorMode::Gray);
        assert_eq!(
            p.pixel(0, 0),
            Some([76, 76, 76, 255]),
            "grayscaled, not inverted"
        );
    }

    #[test]
    fn alpha_mode_sets_red_from_alpha() {
        let mut p = Pixmap::filled(1, 1, peniko::Color::from_rgba8(0, 0, 0, 128));
        post_process(&mut p, ColorMode::Alpha);
        assert_eq!(p.pixel(0, 0).map(|px| px[0]), Some(128));
    }

    #[test]
    fn patches_are_declined_by_the_pixmap_path() {
        // Types 6 and 7 need a rasterizer, so the buffer path refuses them
        // rather than producing a wrong answer.
        let shading = Shading {
            geometry: Geometry::Mesh {
                kind: ShadingKind::CoonsMesh,
                mesh: Box::new(pdfrum_page::Mesh::default()),
            },
            ..axial_shading(None)
        };
        assert!(
            draw_to_pixmap(
                &shading,
                rect(4, 4),
                Affine::IDENTITY,
                255,
                &RenderOptions::default()
            )
            .is_none()
        );
    }
}
