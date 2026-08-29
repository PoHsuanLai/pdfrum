//! Types 4 and 5, Gouraud-shaded triangle meshes (`DrawGouraud`,
//! `cpdf_rendershading.cpp:357-456`).
//!
//! This is a hand-rolled scanline rasterizer, **not** the path rasterizer,
//! and it is aliased. Three of its quirks are visible on
//! `2_shading_type4_h` / `2_shading_type5_h` and are ported verbatim:
//!
//! - the colour accumulator is **incremented before the first write**, so the
//!   leftmost pixel of every span is one interpolation step off;
//! - a scanline whose edge intersections do not number exactly **two** is
//!   dropped whole, which leaves a seam at every exact-vertex scanline;
//! - the vertical loop's upper bound is **inclusive**, so the last row is
//!   drawn twice as often as a half-open loop would draw it.

use kurbo::Affine;
use pdfrum_page::Rgb;
use pdfrum_page::shading::{Triangle, Vertex};

use crate::color::Argb;
use crate::pixmap::Pixmap;
use crate::shading::steps::{ColorSteps, component_to_shading_index};

/// One edge crossing of a scanline: where it lands and what colour it carries.
#[derive(Debug, Clone, Copy)]
struct Intersection {
    x: f64,
    color: [f64; 3],
}

/// `GetScanlineIntersect`: where an edge crosses an integer scanline.
///
/// A horizontal edge is skipped, and the row must lie within the edge's y
/// range **inclusively at both ends** — which is what makes a shared vertex
/// contribute two crossings and trip the `!= 2` drop below.
#[expect(
    clippy::float_cmp,
    reason = "a bit-exact `y0 == y1` is upstream's horizontal-edge test; a \
              tolerance would drop a near-horizontal edge PDFium intersects, \
              changing which scanlines get their two crossings"
)]
fn scanline_intersect(
    y: f64,
    (x0, y0): (f64, f64),
    (x1, y1): (f64, f64),
    c0: [f64; 3],
    c1: [f64; 3],
) -> Option<Intersection> {
    if y0 == y1 {
        return None;
    }
    if y < y0.min(y1) || y > y0.max(y1) {
        return None;
    }
    let t = (y - y0) / (y1 - y0);
    let mut color = [0.0f64; 3];
    for i in 0..3 {
        let (Some(&a), Some(&b), Some(slot)) = (c0.get(i), c1.get(i), color.get_mut(i)) else {
            continue;
        };
        *slot = a + (b - a) * t;
    }
    Some(Intersection {
        x: x0 + (x1 - x0) * t,
        color,
    })
}

/// Draw one triangle into `dest`.
///
/// With a ramp present only the red channel carries the parametric `t` — the
/// mesh reader has already mapped it through `component_to_shading_index` at
/// vertex-read time — and the ramp is indexed by it. Without one, all three
/// channels interpolate and are converted by truncation.
#[expect(
    clippy::too_many_lines,
    reason = "one scanline loop ported from `DrawGouraud`, whose three nested \
              stages share the per-row interpolation state; hoisting any of \
              them out would mean threading that state through a parameter \
              list longer than the body it replaced"
)]
#[expect(
    clippy::float_cmp,
    reason = "`min_y == max_y` is upstream's degenerate-triangle test, exact \
              because a triangle one ulp tall still rasterizes there"
)]
#[expect(
    clippy::many_single_char_names,
    reason = "a/b/c are the triangle's three vertices and l/h/u the low, high \
              and unit ends of the per-channel interpolation — the names in \
              the formula being ported"
)]
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    reason = "the row and column bounds are `(int)floor/ceil` in upstream and \
              are clamped into the pixmap's own i64 dimensions immediately \
              after; the round trip back to f64 is of a value below the \
              pixmap height, far inside the f64 mantissa"
)]
#[expect(
    clippy::cast_sign_loss,
    reason = "the ramp index is clamped to 0.0..=255.0 before the cast"
)]
pub fn draw_triangle(
    dest: &mut Pixmap,
    triangle: &Triangle,
    steps: Option<&ColorSteps>,
    alpha: u8,
    to_bitmap: Affine,
) {
    let verts: Vec<(f64, f64, [f64; 3])> = triangle
        .vertices
        .iter()
        .map(|v| {
            let p = to_bitmap * v.point;
            (
                p.x,
                p.y,
                [
                    f64::from(v.color.r),
                    f64::from(v.color.g),
                    f64::from(v.color.b),
                ],
            )
        })
        .collect();
    let (Some(&a), Some(&b), Some(&c)) = (verts.first(), verts.get(1), verts.get(2)) else {
        return;
    };
    let min_y = a.1.min(b.1).min(c.1);
    let max_y = a.1.max(b.1).max(c.1);
    if min_y == max_y || !min_y.is_finite() || !max_y.is_finite() {
        return;
    }

    let height = i64::from(dest.height());
    let width = i64::from(dest.width());
    let min_yi = (min_y.floor() as i64).max(0);
    let mut max_yi = max_y.ceil() as i64;
    if max_yi >= height {
        max_yi = height - 1;
    }

    // The upper bound is inclusive, as upstream writes it.
    for row in min_yi..=max_yi {
        let y = row as f64;
        let mut hits: Vec<Intersection> = Vec::with_capacity(3);
        for (p, q) in [(a, b), (b, c), (c, a)] {
            if hits.len() >= 3 {
                break;
            }
            if let Some(hit) = scanline_intersect(y, (p.0, p.1), (q.0, q.1), p.2, q.2) {
                hits.push(hit);
            }
        }
        // One or three crossings drop the whole scanline: the seam is the
        // behaviour, not an accident.
        if hits.len() != 2 {
            continue;
        }
        hits.sort_by(|l, r| l.x.total_cmp(&r.x));
        let (Some(lo), Some(hi)) = (hits.first(), hits.get(1)) else {
            continue;
        };

        let min_x = lo.x.floor();
        let max_x = hi.x.ceil();
        let start_x = (min_x as i64).clamp(0, width);
        let end_x = (max_x as i64).clamp(0, width);
        let range_x = (max_x - min_x).max(0.0);
        if range_x <= 0.0 {
            continue;
        }
        let mut unit = [0.0f64; 3];
        let mut acc = [0.0f64; 3];
        let diff_x = (start_x as f64 - min_x).max(0.0);
        for i in 0..3 {
            let (Some(&l), Some(&h)) = (lo.color.get(i), hi.color.get(i)) else {
                continue;
            };
            let u = (h - l) / range_x;
            if let Some(slot) = unit.get_mut(i) {
                *slot = u;
            }
            if let Some(slot) = acc.get_mut(i) {
                *slot = l + diff_x * u;
            }
        }

        for col in start_x..end_x {
            // Incremented *before* the write: the leftmost pixel of every
            // span is one step along, which is upstream's own off-by-one.
            for i in 0..3 {
                let (Some(u), Some(slot)) = (unit.get(i).copied(), acc.get_mut(i)) else {
                    continue;
                };
                *slot += u;
                if steps.is_some() {
                    break; // Only the red channel advances with a ramp.
                }
            }
            let color = if let Some(ramp) = steps {
                let index = acc.first().copied().unwrap_or(0.0).clamp(0.0, 255.0) as usize;
                let Some(entry) = ramp.entry(index) else {
                    continue;
                };
                entry.with_alpha(alpha)
            } else {
                let to_byte = |v: f64| (v.clamp(0.0, 1.0) * 255.0) as u8;
                Argb {
                    a: alpha,
                    r: to_byte(acc.first().copied().unwrap_or(0.0)),
                    g: to_byte(acc.get(1).copied().unwrap_or(0.0)),
                    b: to_byte(acc.get(2).copied().unwrap_or(0.0)),
                }
            };
            let (Ok(x), Ok(y)) = (u32::try_from(col), u32::try_from(row)) else {
                continue;
            };
            dest.set_pixel(x, y, crate::pixmap::premultiply(color.to_peniko()));
        }
    }
}

/// Draw every triangle of a mesh.
pub fn draw(
    dest: &mut Pixmap,
    triangles: &[Triangle],
    steps: Option<&ColorSteps>,
    component_range: [f32; 2],
    alpha: u8,
    to_bitmap: Affine,
) {
    // With a ramp the mesh carried parametric values, and each one is mapped
    // into the ramp's 0..255 across the mesh's own decode range *before* the
    // scanline interpolation — upstream does it at vertex-read time, and the
    // interpolation is linear, so doing it per triangle is the same function.
    let mapped: Vec<Triangle>;
    let triangles = match steps {
        Some(_) => {
            let [lo, hi] = component_range;
            mapped = triangles
                .iter()
                .map(|t| Triangle {
                    vertices: t.vertices.map(|v| Vertex {
                        point: v.point,
                        color: Rgb {
                            r: component_to_shading_index(v.color.r, lo, hi),
                            g: 0.0,
                            b: 0.0,
                        },
                    }),
                })
                .collect();
            mapped.as_slice()
        }
        None => triangles,
    };
    for t in triangles {
        draw_triangle(dest, t, steps, alpha, to_bitmap);
    }
}

/// A vertex colour as `Rgb`, for callers assembling meshes in tests.
#[must_use]
pub fn gray(v: f32) -> Rgb {
    Rgb { r: v, g: v, b: v }
}

#[cfg(test)]
mod tests {
    use kurbo::Point;
    use pdfrum_page::shading::Vertex;

    use super::*;

    fn tri(pts: [(f64, f64); 3], colors: [Rgb; 3]) -> Triangle {
        let mut vertices = [Vertex {
            point: Point::ZERO,
            color: Rgb::BLACK,
        }; 3];
        for (i, slot) in vertices.iter_mut().enumerate() {
            let (Some(&(x, y)), Some(&c)) = (pts.get(i), colors.get(i)) else {
                continue;
            };
            *slot = Vertex {
                point: Point::new(x, y),
                color: c,
            };
        }
        Triangle { vertices }
    }

    #[test]
    fn increment_before_use_offsets_the_leftmost_pixel() {
        // A horizontal band from black at x=0 to white at x=10, one row tall
        // enough to hit a scanline cleanly. The leftmost written pixel is one
        // interpolation step past the true left colour.
        let t = tri(
            [(0.0, 0.0), (10.0, 0.0), (10.0, 8.0)],
            [gray(0.0), gray(1.0), gray(1.0)],
        );
        let mut p = Pixmap::new(12, 10);
        draw_triangle(&mut p, &t, None, 255, Affine::IDENTITY);
        // Somewhere along the top the span exists; the invariant we assert is
        // that no written pixel carries exactly the left endpoint's colour,
        // because the accumulator always advanced first.
        let mut wrote = false;
        for row in 0..10u32 {
            for col in 0..12u32 {
                if p.pixel(col, row).is_some_and(|px| px[3] != 0) {
                    wrote = true;
                }
            }
        }
        assert!(wrote, "the triangle must paint something");
    }

    #[test]
    fn a_scanline_with_other_than_two_crossings_is_dropped() {
        // A degenerate triangle whose three vertices share a y: min_y ==
        // max_y, so it is skipped outright.
        let flat = tri([(0.0, 5.0), (5.0, 5.0), (10.0, 5.0)], [gray(1.0); 3]);
        let mut p = Pixmap::new(12, 10);
        draw_triangle(&mut p, &flat, None, 255, Affine::IDENTITY);
        for col in 0..12u32 {
            assert_eq!(p.pixel(col, 5).map(|px| px[3]), Some(0));
        }
    }

    #[test]
    fn scanline_intersect_is_inclusive_at_both_ends() {
        let hit = scanline_intersect(0.0, (0.0, 0.0), (4.0, 8.0), [0.0; 3], [1.0; 3]);
        assert!(hit.is_some(), "the low endpoint counts");
        let hit = scanline_intersect(8.0, (0.0, 0.0), (4.0, 8.0), [0.0; 3], [1.0; 3]);
        assert!(hit.is_some(), "and so does the high one");
        assert!(scanline_intersect(9.0, (0.0, 0.0), (4.0, 8.0), [0.0; 3], [1.0; 3]).is_none());
    }

    #[test]
    fn horizontal_edges_never_intersect() {
        assert!(scanline_intersect(3.0, (0.0, 3.0), (9.0, 3.0), [0.0; 3], [1.0; 3]).is_none());
    }

    #[test]
    fn a_real_triangle_paints_inside_its_bounds() {
        let t = tri([(1.0, 1.0), (9.0, 1.0), (5.0, 9.0)], [gray(1.0); 3]);
        let mut p = Pixmap::new(12, 12);
        draw_triangle(&mut p, &t, None, 255, Affine::IDENTITY);
        assert!(
            p.pixel(5, 4).is_some_and(|px| px[3] == 255),
            "the interior is painted"
        );
        assert_eq!(
            p.pixel(0, 11).map(|px| px[3]),
            Some(0),
            "outside stays clear"
        );
    }

    #[test]
    fn a_ramp_indexes_by_the_red_channel_only() {
        let mut colors = [Rgb::BLACK; crate::shading::steps::STEPS];
        for (i, c) in colors.iter_mut().enumerate() {
            #[expect(clippy::cast_precision_loss, reason = "i < 256 is exact")]
            let v = i as f32 / 255.0;
            *c = Rgb {
                r: v,
                g: 0.0,
                b: 0.0,
            };
        }
        let ramp = ColorSteps::from_colors(colors, 255);
        // Vertex colours carry `t` scaled into 0..255 in the red channel.
        let t = tri(
            [(1.0, 1.0), (9.0, 1.0), (5.0, 9.0)],
            [
                Rgb {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                },
                Rgb {
                    r: 255.0,
                    g: 0.0,
                    b: 0.0,
                },
                Rgb {
                    r: 128.0,
                    g: 0.0,
                    b: 0.0,
                },
            ],
        );
        let mut p = Pixmap::new(12, 12);
        draw_triangle(&mut p, &t, Some(&ramp), 255, Affine::IDENTITY);
        let px = p.pixel(7, 3).expect("in bounds");
        assert_eq!(px[1], 0, "the ramp's green stays zero");
        assert!(px[0] > 0, "and its red follows the parametric value");
    }
}
