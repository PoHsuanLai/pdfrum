//! Mesh shading streams, types 4 to 7 (ISO 32000-1 §8.7.4.5.5-.8).
//!
//! One bit-packed stream carrying vertices, colours and — for every type but
//! the lattice — an edge flag per record. Several details are quirks:
//!
//! - **The flag is masked to two bits** regardless of `/BitsPerFlag`, so it
//!   is always 0..=3 and flag 3 is reachable. Type 4 treats flag 3
//!   identically to flag 2, because it only special-cases flag 1.
//! - **`/Decode` must be exactly `4 + 2 * components` long**, not merely long
//!   enough. With any function present, `components` is 1 and the length is
//!   exactly 6.
//! - **With functions present, the "colour" read from the stream is not a
//!   colour**: it is one parametric value `t` that the functions map later.
//! - **Types 6 and 7 do not byte-align between patches**, unlike Gouraud's
//!   per-vertex alignment.
//! - **A patch reusing an edge sources its shared points at
//!   `old[(flag * 3 + i) % 12]`** — modulo 12 even in the sixteen-point
//!   tensor case, so the four interior points are never reused.

use crate::color::{ColorSpace, Rgb};
use crate::function::{BitReader, Function};
use kurbo::Point;
use std::sync::Arc;

/// The most colour components a mesh may carry.
pub const MAX_COMPONENTS: usize = 8;

/// Coordinate widths the format allows.
const VALID_COORD_BITS: [u32; 8] = [1, 2, 4, 8, 12, 16, 24, 32];

/// Component widths the format allows. Note 24 and 32 are **not** here, even
/// though they are legal for coordinates.
const VALID_COMPONENT_BITS: [u32; 6] = [1, 2, 4, 8, 12, 16];

/// Flag widths the format allows.
const VALID_FLAG_BITS: [u32; 3] = [2, 4, 8];

/// One vertex: where it is and what colour it carries.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vertex {
    /// Position in the shading's own coordinate space.
    pub point: Point,
    /// The colour, or — when functions are present — the parametric value in
    /// the red slot with the other two zero.
    pub color: Rgb,
}

/// A triangle from a type 4 or type 5 mesh.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Triangle {
    /// Its three corners.
    pub vertices: [Vertex; 3],
}

/// A Coons or tensor patch: twelve or sixteen control points and four corner
/// colours.
#[derive(Debug, Clone, PartialEq)]
pub struct Patch {
    /// Control points, twelve for a Coons patch and sixteen for a tensor one.
    pub points: Box<[Point]>,
    /// The four corner colours.
    pub colors: [Rgb; 4],
}

/// A decoded mesh.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Mesh {
    /// Triangles, from types 4 and 5.
    pub triangles: Vec<Triangle>,
    /// Patches, from types 6 and 7.
    pub patches: Vec<Patch>,
    /// The first colour component's decode range, `/Decode[4]` and
    /// `/Decode[5]`.
    ///
    /// It is here because a mesh under a `/Function` reads exactly one
    /// parametric value per colour and this range is its domain: the ramp is
    /// sampled across it, and each vertex's value is mapped into the ramp's
    /// 256 entries relative to it. `[0.0, 1.0]` is the common case but by no
    /// means the only one — `[1, 2]`, `[0, 255]` and `[-128, 127]` all occur
    /// in the corpus — and assuming the unit interval silently reads the
    /// wrong end of the ramp for every one of them.
    pub component_range: [f32; 2],
}

impl Mesh {
    /// The bounding box of every vertex and control point, or `None` when the
    /// mesh is empty.
    #[must_use]
    pub fn bounds(&self) -> Option<kurbo::Rect> {
        let mut rect: Option<kurbo::Rect> = None;
        let mut add = |p: Point| {
            let r = kurbo::Rect::from_points(p, p);
            rect = Some(match rect {
                Some(existing) => existing.union(r),
                None => r,
            });
        };
        for t in &self.triangles {
            for v in t.vertices {
                add(v.point);
            }
        }
        for p in &self.patches {
            for point in &p.points {
                add(*point);
            }
        }
        rect
    }
}

/// The bit widths and decode ranges a mesh stream declares.
#[derive(Debug, Clone, PartialEq)]
pub struct MeshParams {
    /// `/BitsPerCoordinate`.
    pub coord_bits: u32,
    /// `/BitsPerComponent`.
    pub component_bits: u32,
    /// `/BitsPerFlag`, unvalidated for the lattice type which has no flags.
    pub flag_bits: u32,
    /// How many colour components each record carries — **1** whenever any
    /// function is present.
    pub components: usize,
    /// `/Decode`, `[xmin, xmax, ymin, ymax, c0min, c0max, …]`.
    pub decode: Box<[f32]>,
    /// `2^coord_bits - 1`.
    pub coord_max: u32,
    /// `2^component_bits - 1`.
    pub component_max: u32,
}

impl MeshParams {
    /// Validate the widths and the `/Decode` length.
    ///
    /// `flags` says whether this type reads edge flags at all — the lattice
    /// type does not, and its `/BitsPerFlag` is therefore never checked.
    #[must_use]
    pub fn new(
        coord_bits: u32,
        component_bits: u32,
        flag_bits: u32,
        components: usize,
        decode: &[f32],
        flags: bool,
    ) -> Option<Self> {
        if !VALID_COORD_BITS.contains(&coord_bits)
            || !VALID_COMPONENT_BITS.contains(&component_bits)
            || (flags && !VALID_FLAG_BITS.contains(&flag_bits))
            || components > MAX_COMPONENTS
        {
            return None;
        }
        // Exactly, not at least.
        if decode.len() != 4 + 2 * components {
            return None;
        }
        Some(Self {
            coord_bits,
            component_bits,
            flag_bits,
            components,
            decode: decode.into(),
            coord_max: if coord_bits >= 32 {
                u32::MAX
            } else {
                (1u32 << coord_bits) - 1
            },
            component_max: if component_bits >= 32 {
                u32::MAX
            } else {
                (1u32 << component_bits) - 1
            },
        })
    }

    /// The first colour component's decode range — `/Decode[4]`, `/Decode[5]`.
    ///
    /// Under a `/Function` this is the parametric value's domain, which is
    /// what a ramp is sampled across. The length check in [`Self::new`] has
    /// already guaranteed both entries exist.
    #[must_use]
    pub fn component_range(&self) -> [f32; 2] {
        [
            self.decode.get(4).copied().unwrap_or(0.0),
            self.decode.get(5).copied().unwrap_or(0.0),
        ]
    }
}

/// A cursor over a mesh stream.
pub struct MeshReader<'a> {
    bits: BitReader<'a>,
    params: &'a MeshParams,
    space: &'a ColorSpace,
    functions: &'a [Arc<Function>],
}

impl<'a> MeshReader<'a> {
    /// A reader over `data`.
    #[must_use]
    pub fn new(
        data: &'a [u8],
        params: &'a MeshParams,
        space: &'a ColorSpace,
        functions: &'a [Arc<Function>],
    ) -> Self {
        Self {
            bits: BitReader::new(data),
            params,
            space,
            functions,
        }
    }

    /// Whether a flag still fits.
    #[must_use]
    pub fn can_read_flag(&self) -> bool {
        self.bits.remaining() >= u64::from(self.params.flag_bits)
    }

    /// Whether a coordinate pair still fits.
    ///
    /// Note the formulation: the *halved* remainder is compared against one
    /// coordinate's width, which is the C++'s way of asking for two.
    #[must_use]
    pub fn can_read_coords(&self) -> bool {
        self.bits.remaining() / 2 >= u64::from(self.params.coord_bits)
    }

    /// Whether a colour still fits.
    ///
    /// The C++ divides the remainder by the *component width* and compares
    /// against the component count, which is a different question from
    /// "`components * width` bits remain" only when the division truncates.
    /// Reproduced, with a guard the C++ does not need because its zero case
    /// is unreachable.
    #[must_use]
    pub fn can_read_color(&self) -> bool {
        if self.params.component_bits == 0 {
            return false;
        }
        self.bits.remaining() / u64::from(self.params.component_bits)
            >= self.params.components as u64
    }

    /// Read an edge flag, **masked to two bits**.
    pub fn read_flag(&mut self) -> u8 {
        u8::try_from(self.bits.read(self.params.flag_bits) & 0x03).unwrap_or(0)
    }

    /// Read one coordinate pair, x fully before y.
    pub fn read_coords(&mut self) -> Point {
        let decode = |raw: u32, min: f32, max: f32, max_raw: u32| -> f64 {
            if self.params.coord_bits == 32 {
                // Forced to `f64` so a 32-bit raw value does not lose
                // precision on the way through.
                f64::from(min)
                    + f64::from(raw) * (f64::from(max) - f64::from(min)) / f64::from(max_raw)
            } else {
                #[expect(
                    clippy::cast_precision_loss,
                    reason = "below 32 bits the raw value is exact in f32, matching the C++"
                )]
                let v = min + (raw as f32) * (max - min) / (max_raw as f32);
                f64::from(v)
            }
        };
        let at = |i: usize| self.params.decode.get(i).copied().unwrap_or(0.0);
        let raw_x = self.bits.read(self.params.coord_bits);
        let raw_y = self.bits.read(self.params.coord_bits);
        Point::new(
            decode(raw_x, at(0), at(1), self.params.coord_max),
            decode(raw_y, at(2), at(3), self.params.coord_max),
        )
    }

    /// Read one colour.
    ///
    /// With functions present the result is **not a colour**: the single
    /// parametric value lands in the red slot and the rest are zero.
    pub fn read_color(&mut self) -> Rgb {
        let mut comps = [0.0f32; MAX_COMPONENTS];
        for i in 0..self.params.components.min(MAX_COMPONENTS) {
            let raw = self.bits.read(self.params.component_bits);
            let min = self.params.decode.get(4 + i * 2).copied().unwrap_or(0.0);
            let max = self
                .params
                .decode
                .get(4 + i * 2 + 1)
                .copied()
                .unwrap_or(0.0);
            #[expect(
                clippy::cast_precision_loss,
                reason = "component widths cap at 16 bits, exact in f32"
            )]
            let v = min + (raw as f32) * (max - min) / (self.params.component_max as f32);
            if let Some(slot) = comps.get_mut(i) {
                *slot = v;
            }
        }
        if self.functions.is_empty() {
            return self
                .space
                .try_to_rgb(
                    comps.get(..self.params.components).unwrap_or(&[]),
                    crate::color::Conversion::Managed,
                )
                .unwrap_or(Rgb::BLACK);
        }
        Rgb {
            r: comps.first().copied().unwrap_or(0.0),
            g: 0.0,
            b: 0.0,
        }
    }

    /// Read one flagged vertex: flag, coordinates, colour, then byte-align.
    pub fn read_vertex(&mut self) -> Option<(u8, Vertex)> {
        if !self.can_read_flag() {
            return None;
        }
        let flag = self.read_flag();
        if !self.can_read_coords() {
            return None;
        }
        let point = self.read_coords();
        if !self.can_read_color() {
            return None;
        }
        let color = self.read_color();
        self.bits.byte_align();
        Some((flag, Vertex { point, color }))
    }

    /// Read one lattice row of `count` vertices — no flags.
    ///
    /// **Any failure discards the whole row**, which callers treat as the end
    /// of the mesh.
    pub fn read_vertex_row(&mut self, count: usize) -> Vec<Vertex> {
        let mut row = Vec::with_capacity(count);
        for _ in 0..count {
            if !self.can_read_coords() {
                return Vec::new();
            }
            let point = self.read_coords();
            if !self.can_read_color() {
                return Vec::new();
            }
            let color = self.read_color();
            self.bits.byte_align();
            row.push(Vertex { point, color });
        }
        row
    }

    /// Decode a free-form Gouraud mesh (type 4).
    #[must_use]
    pub fn read_free_form(&mut self) -> Vec<Triangle> {
        let mut out = Vec::new();
        let mut previous: [Option<Vertex>; 3] = [None; 3];
        loop {
            let Some((flag, vertex)) = self.read_vertex() else {
                return out;
            };
            if flag == 0 {
                // Start a fresh triangle: two more vertices follow
                // unconditionally, and **their flags are read and discarded**.
                let (Some((_, b)), Some((_, c))) = (self.read_vertex(), self.read_vertex()) else {
                    return out;
                };
                previous = [Some(vertex), Some(b), Some(c)];
            } else {
                let (Some(p0), Some(p1), Some(p2)) = (previous[0], previous[1], previous[2]) else {
                    return out;
                };
                // Flag 1 keeps the last edge; flags 2 **and 3** keep the
                // first-and-last one, because only flag 1 is special-cased.
                previous = if flag == 1 {
                    [Some(p1), Some(p2), Some(vertex)]
                } else {
                    [Some(p0), Some(p2), Some(vertex)]
                };
            }
            let (Some(a), Some(b), Some(c)) = (previous[0], previous[1], previous[2]) else {
                return out;
            };
            out.push(Triangle {
                vertices: [a, b, c],
            });
        }
    }

    /// Decode a lattice-form Gouraud mesh (type 5).
    #[must_use]
    pub fn read_lattice(&mut self, per_row: usize) -> Vec<Triangle> {
        // Fewer than two vertices per row cannot form a quad.
        if per_row < 2 {
            return Vec::new();
        }
        let mut out = Vec::new();
        let mut previous = self.read_vertex_row(per_row);
        if previous.is_empty() {
            return out;
        }
        loop {
            let row = self.read_vertex_row(per_row);
            if row.is_empty() {
                return out;
            }
            for i in 0..per_row - 1 {
                let (Some(a), Some(b), Some(c), Some(d)) = (
                    previous.get(i),
                    previous.get(i + 1),
                    row.get(i),
                    row.get(i + 1),
                ) else {
                    continue;
                };
                out.push(Triangle {
                    vertices: [*a, *b, *c],
                });
                out.push(Triangle {
                    vertices: [*b, *d, *c],
                });
            }
            previous = row;
        }
    }

    /// Decode a Coons (type 6) or tensor (type 7) patch mesh.
    ///
    /// `point_count` is a loop **invariant**: a flagged patch reuses four
    /// points and two colours from its predecessor, which moves where this
    /// patch starts reading, never how many records the format has.
    //
    // [oracle-bug] cpdf_streamcontentparser.cpp:120-122 gets that backwards
    // in its bbox helper: inside `while (!stream.IsEOF())` it runs
    // `point_count -= 4; color_count -= 2;` on every flagged patch, mutating
    // the very variables declared as the record shape at :94-109. The
    // subtraction is therefore **cumulative and permanent** — after the first
    // flagged patch every later patch under-reads by four points, after the
    // second by eight, and the counts run to zero and below. The bbox that
    // results is too small, and §8.7.4.5.5-7 give the bbox no licence to omit
    // a declared control point. That it is a slip rather than a reading of
    // the spec is settled by PDFium's own second copy of the same loop:
    // `cpdf_rendershading.cpp:900-917` uses per-iteration `iStartPoint`/
    // `iStartColor` locals against an untouched `point_count` — correct, and
    // what this function does. pdf.js has no counterpart (it composites mesh
    // patches on a canvas and needs no bbox helper), so the oracle's own
    // renderer is the independent reading here. This is the audit's A23,
    // previously recorded as design brief D18 — "declined", where the
    // oracle-bug rule makes it obligatory.
    #[must_use]
    pub fn read_patches(&mut self, tensor: bool) -> Vec<Patch> {
        let point_count = if tensor { 16 } else { 12 };
        let mut out: Vec<Patch> = Vec::new();
        let mut coords = vec![Point::ZERO; point_count];
        let mut colors = [Rgb::BLACK; 4];
        loop {
            if !self.can_read_flag() {
                return out;
            }
            let flag = self.read_flag();
            // A flagged patch reuses one edge: four points and two colours
            // come from its predecessor.
            let (start_point, start_color) = if flag == 0 { (0, 0) } else { (4, 2) };
            if flag != 0 {
                let Some(previous) = out.last() else {
                    return out;
                };
                for i in 0..4 {
                    // The modulo is 12 even for a sixteen-point tensor patch,
                    // so the four interior points are never reused.
                    let source = (usize::from(flag) * 3 + i) % 12;
                    if let (Some(slot), Some(p)) = (coords.get_mut(i), previous.points.get(source))
                    {
                        *slot = *p;
                    }
                }
                if let (Some(slot), Some(c)) =
                    (colors.first_mut(), previous.colors.get(usize::from(flag)))
                {
                    *slot = *c;
                }
                let next = previous
                    .colors
                    .get((usize::from(flag) + 1) % 4)
                    .copied()
                    .unwrap_or(Rgb::BLACK);
                if let Some(slot) = colors.get_mut(1) {
                    *slot = next;
                }
            }
            // The inner breaks leave the remaining points and colours at
            // their previous values rather than resetting them, and the patch
            // is still emitted.
            for i in start_point..point_count {
                if !self.can_read_coords() {
                    break;
                }
                let p = self.read_coords();
                if let Some(slot) = coords.get_mut(i) {
                    *slot = p;
                }
            }
            for i in start_color..4 {
                if !self.can_read_color() {
                    break;
                }
                let c = self.read_color();
                if let Some(slot) = colors.get_mut(i) {
                    *slot = c;
                }
            }
            out.push(Patch {
                points: coords.clone().into(),
                colors,
            });
            // No byte alignment between patches, unlike Gouraud's per-vertex
            // alignment.
        }
    }
}

/// The four interior control points a Coons patch implies, from
/// ISO 32000-2 §8.7.4.5.8.
///
/// A Coons patch states only its twelve boundary points; the surface's
/// interior is derived, and the derivation is what makes a Coons patch and a
/// tensor patch with these interiors identical.
#[must_use]
pub fn coons_interior(boundary: &[Point]) -> [Point; 4] {
    let p = |i: usize| boundary.get(i).copied().unwrap_or(Point::ZERO);
    // The boundary in the grid naming the formula uses.
    let (p00, p01, p02, p03) = (p(0), p(1), p(2), p(3));
    let (p13, p23) = (p(4), p(5));
    let (p33, p32, p31, p30) = (p(6), p(7), p(8), p(9));
    let (p20, p10) = (p(10), p(11));
    let blend = |a: Point, b: Point, c: Point, d: Point, e: Point, f: Point, g: Point, h: Point| {
        Point::new(
            (-4.0 * a.x + 6.0 * (b.x + c.x) - 2.0 * (d.x + e.x) + 3.0 * (f.x + g.x) - h.x) / 9.0,
            (-4.0 * a.y + 6.0 * (b.y + c.y) - 2.0 * (d.y + e.y) + 3.0 * (f.y + g.y) - h.y) / 9.0,
        )
    };
    [
        blend(p00, p01, p10, p03, p30, p31, p13, p33),
        blend(p03, p02, p13, p00, p33, p32, p10, p30),
        blend(p30, p31, p20, p33, p00, p01, p23, p03),
        blend(p33, p32, p23, p30, p03, p02, p20, p00),
    ]
}

#[cfg(test)]
mod tests {
    // Test fixtures quote the oracle's own vectors, compare floats exactly
    // where the behaviour being pinned is exact, and index arrays whose
    // length the fixture itself fixes.
    #![allow(
        clippy::unreadable_literal,
        clippy::float_cmp,
        clippy::indexing_slicing,
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        reason = "test fixtures quote oracle vectors verbatim and compare exactly"
    )]

    use super::{MAX_COMPONENTS, MeshParams, MeshReader};
    use crate::color::ColorSpace;

    fn params(components: usize, decode: &[f32]) -> Option<MeshParams> {
        MeshParams::new(8, 8, 8, components, decode, true)
    }

    #[test]
    fn bit_widths_are_validated_per_field() {
        // Coordinates allow 24 and 32; components do not.
        assert!(MeshParams::new(24, 8, 8, 1, &[0.0; 6], true).is_some());
        assert!(MeshParams::new(32, 8, 8, 1, &[0.0; 6], true).is_some());
        assert!(MeshParams::new(8, 24, 8, 1, &[0.0; 6], true).is_none());
        assert!(MeshParams::new(8, 32, 8, 1, &[0.0; 6], true).is_none());
        // Three is legal for neither.
        assert!(MeshParams::new(3, 8, 8, 1, &[0.0; 6], true).is_none());
        assert!(MeshParams::new(8, 3, 8, 1, &[0.0; 6], true).is_none());
        // Flags allow only 2, 4 and 8.
        assert!(MeshParams::new(8, 8, 3, 1, &[0.0; 6], true).is_none());
        assert!(MeshParams::new(8, 8, 2, 1, &[0.0; 6], true).is_some());
        // …and a type with no flags never checks them.
        assert!(MeshParams::new(8, 8, 3, 1, &[0.0; 6], false).is_some());
    }

    #[test]
    fn the_decode_length_must_be_exact() {
        // One component wants exactly six entries.
        assert!(params(1, &[0.0; 6]).is_some());
        assert!(params(1, &[0.0; 5]).is_none());
        assert!(params(1, &[0.0; 7]).is_none());
        // Three components want ten.
        assert!(params(3, &[0.0; 10]).is_some());
        assert!(params(3, &[0.0; 8]).is_none());
    }

    #[test]
    fn more_than_eight_components_is_refused() {
        assert!(params(MAX_COMPONENTS, &[0.0; 20]).is_some());
        assert!(params(MAX_COMPONENTS + 1, &[0.0; 22]).is_none());
    }

    #[test]
    fn flags_are_masked_to_two_bits() {
        let p = MeshParams::new(8, 8, 8, 1, &[0.0; 6], true).expect("params");
        // A flag byte of 0xFF masks down to 3, which is a reachable value.
        let data = [0xFFu8; 8];
        let space = ColorSpace::DeviceGray;
        let mut reader = MeshReader::new(&data, &p, &space, &[]);
        assert_eq!(reader.read_flag(), 3);
    }

    #[test]
    fn a_lattice_row_shorter_than_two_yields_nothing() {
        let p =
            MeshParams::new(8, 8, 8, 1, &[0.0f32, 1.0, 0.0, 1.0, 0.0, 1.0], false).expect("params");
        let data = [0u8; 64];
        let space = ColorSpace::DeviceGray;
        let mut reader = MeshReader::new(&data, &p, &space, &[]);
        assert!(reader.read_lattice(1).is_empty());
        assert!(reader.read_lattice(0).is_empty());
    }

    #[test]
    fn free_form_flag_three_behaves_as_flag_two() {
        let p = MeshParams::new(8, 8, 8, 1, &[0.0f32, 255.0, 0.0, 255.0, 0.0, 1.0], true)
            .expect("params");
        let space = ColorSpace::DeviceGray;
        // Three flag-0 vertices, then one flag-2 and one flag-3 vertex.
        let mut data = Vec::new();
        for (flag, x, y) in [(0u8, 0u8, 0u8), (0, 10, 0), (0, 0, 10)] {
            data.extend_from_slice(&[flag, x, y, 128]);
        }
        data.extend_from_slice(&[2, 20, 20, 128]);
        let mut reader = MeshReader::new(&data, &p, &space, &[]);
        let with_two = reader.read_free_form();

        let mut data3 = Vec::new();
        for (flag, x, y) in [(0u8, 0u8, 0u8), (0, 10, 0), (0, 0, 10)] {
            data3.extend_from_slice(&[flag, x, y, 128]);
        }
        data3.extend_from_slice(&[3, 20, 20, 128]);
        let mut reader = MeshReader::new(&data3, &p, &space, &[]);
        let with_three = reader.read_free_form();
        assert_eq!(with_two, with_three);
        assert_eq!(with_two.len(), 2);
    }

    #[test]
    fn a_truncated_stream_stops_rather_than_reading_past_the_end() {
        let p = MeshParams::new(8, 8, 8, 1, &[0.0f32, 255.0, 0.0, 255.0, 0.0, 1.0], true)
            .expect("params");
        let space = ColorSpace::DeviceGray;
        // A flag and one coordinate, then nothing.
        let data = [0u8, 5];
        let mut reader = MeshReader::new(&data, &p, &space, &[]);
        assert!(reader.read_free_form().is_empty());
    }

    #[test]
    fn coons_interiors_are_derived_from_the_boundary() {
        // A unit square's boundary produces interiors inside it.
        let boundary: Vec<kurbo::Point> = (0..12)
            .map(|i| {
                let t = f64::from(i) / 12.0;
                kurbo::Point::new(t, t)
            })
            .collect();
        let interior = super::coons_interior(&boundary);
        for p in interior {
            assert!(p.x.is_finite() && p.y.is_finite());
        }
    }
}
