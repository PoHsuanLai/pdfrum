//! Shadings, types 1 to 7 (ISO 32000-1 §8.7.4.5).
//!
//! A shading is a colorspace, up to four functions, and a geometry. Loading
//! validates all three together, because the arity a function must have
//! depends on both the type and the colorspace's component count:
//!
//! | Type | Functions |
//! |---|---|
//! | 1 | one 2-in *N*-out, or *N* 2-in 1-out. **Required.** |
//! | 2, 3 | one 1-in *N*-out, or *N* 1-in 1-out. **Required.** |
//! | 4-7 | none, or either shape above. **Optional.** |
//!
//! Because the `/Function` array is capped at **four** entries, a colorspace
//! with more than four components can never satisfy the "*N* one-to-one
//! functions" branch through an array — only through the single-function
//! form.
//!
//! # `/Extend` is stricter than it looks
//!
//! `GetBooleanAt` returns its default for a non-Boolean element, so
//! `/Extend [0 1]` — integers, not booleans — yields **false, false**, not
//! false and true. Files written that way do not extend.

mod axial;
mod function_based;
mod mesh;
mod radial;
mod steps;

pub use axial::Axial;
pub use function_based::FunctionBased;
pub use mesh::{
    MAX_COMPONENTS, Mesh, MeshParams, MeshReader, Patch, Triangle, Vertex, coons_interior,
};
pub use radial::Radial;
pub use steps::{ColorSteps, STEPS};

use crate::color::{ColorSpace, Rgb};
use crate::function::{Function, FunctionCache};
use crate::names;
use kurbo::{Affine, Rect};
use pdfrum_common::{DiagKind, Diagnostics, Limits, Severity};
use pdfrum_filters::decode_chain;
use pdfrum_object::{Dict, Object, Resolve};
use std::sync::Arc;

/// The most functions a `/Function` **array** may hold. A literal in the C++,
/// not a named constant, and the reason a five-function array silently loses
/// its last entry.
pub const MAX_FUNCTIONS: usize = 4;

/// Which geometry a shading paints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ShadingKind {
    /// Type 1: a function over a rectangle of the shading's own space.
    FunctionBased = 1,
    /// Type 2: a linear gradient along an axis.
    Axial = 2,
    /// Type 3: a gradient between two circles.
    Radial = 3,
    /// Type 4: free-form Gouraud triangles.
    FreeFormMesh = 4,
    /// Type 5: lattice-form Gouraud triangles.
    LatticeMesh = 5,
    /// Type 6: Coons patches.
    CoonsMesh = 6,
    /// Type 7: tensor-product patches.
    TensorMesh = 7,
}

impl ShadingKind {
    /// The kind `n` names, or `None` for anything outside 1..=7.
    #[must_use]
    pub fn from_int(n: i64) -> Option<Self> {
        Some(match n {
            1 => Self::FunctionBased,
            2 => Self::Axial,
            3 => Self::Radial,
            4 => Self::FreeFormMesh,
            5 => Self::LatticeMesh,
            6 => Self::CoonsMesh,
            7 => Self::TensorMesh,
            _ => return None,
        })
    }

    /// Whether this kind reads its geometry from a stream of vertices.
    #[must_use]
    pub fn is_mesh(self) -> bool {
        matches!(
            self,
            Self::FreeFormMesh | Self::LatticeMesh | Self::CoonsMesh | Self::TensorMesh
        )
    }
}

/// A loaded, validated shading.
#[derive(Debug, Clone, PartialEq)]
pub struct Shading {
    /// The geometry and its parameters.
    pub geometry: Geometry,
    /// The colour space every function's output lands in.
    pub space: Arc<ColorSpace>,
    /// The tint functions, at most four when they came from an array.
    pub functions: Box<[Arc<Function>]>,
    /// `/Background`, honoured only for a `/PatternType 2` shading pattern
    /// and never for the `sh` operator.
    pub background: Option<Rgb>,
    /// `/BBox`, in the shading's own space. A malformed array collapses this
    /// to a zero rectangle, which makes the shading invisible.
    pub bbox: Option<Rect>,
}

/// A shading's geometry.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Geometry {
    /// Type 1.
    FunctionBased(FunctionBased),
    /// Type 2.
    Axial(Axial),
    /// Type 3.
    Radial(Radial),
    /// Types 4 to 7, already decoded from the stream.
    Mesh {
        /// Which of the four mesh types this is.
        kind: ShadingKind,
        /// The decoded triangles and patches.
        mesh: Box<Mesh>,
    },
}

impl Shading {
    /// Which kind this is.
    #[must_use]
    pub fn kind(&self) -> ShadingKind {
        match &self.geometry {
            Geometry::FunctionBased(_) => ShadingKind::FunctionBased,
            Geometry::Axial(_) => ShadingKind::Axial,
            Geometry::Radial(_) => ShadingKind::Radial,
            Geometry::Mesh { kind, .. } => *kind,
        }
    }

    /// Load and validate a shading.
    ///
    /// `is_shading_object` distinguishes the two entry points: the `sh`
    /// operator names a `/Shading` resource directly, while a
    /// `/PatternType 2` pattern wraps one. Only the pattern honours
    /// `/Background`.
    ///
    /// Returns `None` for every condition PDFium treats as "unsupported
    /// shading", which paints nothing.
    #[must_use]
    pub fn load<R: Resolve>(
        obj: &Object,
        resources: Option<&Dict>,
        is_shading_object: bool,
        r: &R,
        functions_cache: &mut FunctionCache,
        limits: &Limits,
        diags: &mut Diagnostics,
    ) -> Option<Self> {
        let resolved = obj.resolve(r).ok()?;
        let (dict, stream) = match &*resolved {
            Object::Dict(d) => (d.clone(), None),
            Object::Stream(s) => (s.dict.clone(), Some(s.clone())),
            _ => {
                diags.record(Severity::Suspicious, DiagKind::ShadingUnsupported, None);
                return None;
            }
        };

        let kind = ShadingKind::from_int(dict.int(names::SHADING_TYPE, r).unwrap_or(0))?;
        // Mesh types **require** a stream; types 1 to 3 accept either.
        if kind.is_mesh() && stream.is_none() {
            diags.record(Severity::Suspicious, DiagKind::ShadingUnsupported, None);
            return None;
        }

        // `/ColorSpace` is required and may not be a Pattern space
        // (ISO 32000-1 table 78).
        let cs_obj = dict.raw(names::COLOR_SPACE)?;
        let space =
            crate::color::load_colorspace(cs_obj, resources, r, functions_cache, limits, diags)?;
        if matches!(space, ColorSpace::Pattern(_)) {
            diags.record(Severity::Suspicious, DiagKind::ShadingUnsupported, None);
            return None;
        }
        let space = Arc::new(space);

        let functions = load_functions(&dict, r, functions_cache, limits, diags);
        if !validate(kind, &space, &functions) {
            diags.record(Severity::Suspicious, DiagKind::ShadingUnsupported, None);
            return None;
        }

        let geometry = match kind {
            ShadingKind::FunctionBased => Geometry::FunctionBased(FunctionBased::load(&dict, r)),
            ShadingKind::Axial => Geometry::Axial(Axial::load(&dict, r)?),
            ShadingKind::Radial => Geometry::Radial(Radial::load(&dict, r)?),
            _ => {
                let stream = stream?;
                let mesh = load_mesh(kind, &stream, &dict, &space, &functions, r, limits, diags)?;
                Geometry::Mesh {
                    kind,
                    mesh: Box::new(mesh),
                }
            }
        };

        // `/Background` is honoured **only** for a pattern, never for `sh`,
        // and only when the array is at least as long as the space needs.
        let background = (!is_shading_object)
            .then(|| dict.array(names::BACKGROUND, r))
            .flatten()
            .filter(|a| a.len() >= space.n_components())
            .map(|a| {
                let comps: Vec<f32> = (0..space.n_components())
                    .map(|i| a.number_at_or_zero(i))
                    .collect();
                space.to_rgb(&comps)
            });

        // `/BBox` needs exactly four elements; anything else collapses to a
        // zero rectangle, which is what makes such a shading invisible.
        let bbox = dict.array(names::BBOX, r).map(|a| {
            if a.len() == 4 {
                a.as_rect()
            } else {
                Rect::ZERO
            }
        });

        Some(Self {
            geometry,
            space,
            functions,
            background,
            bbox,
        })
    }

    /// The 256-entry colour ramp an axial or radial shading indexes.
    ///
    /// `None` for a geometry that has no ramp, or when the functions produce
    /// no outputs at all.
    #[must_use]
    pub fn color_steps(&self) -> Option<ColorSteps> {
        let (t_min, t_max) = match &self.geometry {
            Geometry::Axial(a) => (a.t_min, a.t_max),
            Geometry::Radial(a) => (a.t_min, a.t_max),
            _ => return None,
        };
        ColorSteps::sample(&self.functions, &self.space, t_min, t_max)
    }

    /// Evaluate the shading's functions at one parametric position,
    /// concatenating multi-function outputs, and convert to a colour.
    #[must_use]
    pub fn color_at(&self, t: f32) -> Rgb {
        let total: usize = self.functions.iter().map(|f| f.output_count()).sum();
        let mut buffer = vec![0.0f32; total.max(self.space.n_components())];
        let mut written = 0usize;
        for f in &self.functions {
            let Some(span) = buffer.get_mut(written..) else {
                break;
            };
            written += f.eval_into(&[t], span);
        }
        self.space.to_rgb(&buffer)
    }
}

/// Read `/Function`, which may be one function or an array of up to four.
fn load_functions<R: Resolve>(
    dict: &Dict,
    r: &R,
    cache: &mut FunctionCache,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> Box<[Arc<Function>]> {
    let Some(obj) = dict.raw(names::FUNCTION) else {
        return Box::default();
    };
    // An array of functions; entries past the fourth are dropped, and an
    // entry that fails to load leaves a hole that `validate` then rejects.
    if let Some(array) = obj.as_array() {
        let mut out = Vec::new();
        let mut had_hole = false;
        for i in 0..array.len().min(MAX_FUNCTIONS) {
            match array
                .raw_at(i)
                .and_then(|o| cache.load(o, r, limits, diags))
            {
                Some(f) => out.push(f),
                None => had_hole = true,
            }
        }
        if had_hole {
            // A null entry rejects the whole shading, so signal it by
            // returning a set that cannot validate.
            return Box::default();
        }
        return out.into();
    }
    match cache.load(obj, r, limits, diags) {
        Some(f) => Box::from([f]),
        None => Box::default(),
    }
}

/// The type-and-colorspace-dependent function requirements.
fn validate(kind: ShadingKind, space: &ColorSpace, functions: &[Arc<Function>]) -> bool {
    let n = space.n_components();
    let indexed = matches!(space, ColorSpace::Indexed(_));
    match kind {
        // Types 1 to 3 forbid Indexed unconditionally.
        ShadingKind::FunctionBased | ShadingKind::Axial | ShadingKind::Radial => {
            if indexed {
                return false;
            }
        }
        // Mesh types forbid it only when functions are present.
        _ => {
            if indexed && !functions.is_empty() {
                return false;
            }
        }
    }
    let inputs = if kind == ShadingKind::FunctionBased {
        2
    } else {
        1
    };
    let shapes = shape_matches(functions, 1, inputs, n) || shape_matches(functions, n, inputs, 1);
    match kind {
        // Mandatory for types 1 to 3.
        ShadingKind::FunctionBased | ShadingKind::Axial | ShadingKind::Radial => shapes,
        // Optional for the mesh types.
        _ => functions.is_empty() || shapes,
    }
}

/// Whether `functions` is exactly `count` functions of `inputs` to `outputs`.
fn shape_matches(functions: &[Arc<Function>], count: usize, inputs: usize, outputs: usize) -> bool {
    functions.len() == count
        && functions
            .iter()
            .all(|f| f.input_count() == inputs && f.output_count() == outputs)
}

/// Decode a mesh stream into triangles or patches.
#[expect(
    clippy::too_many_arguments,
    reason = "the mesh reader needs the stream, its dictionary, the colour \
              space, the functions and the usual resolver/limits/diagnostics trio"
)]
fn load_mesh<R: Resolve>(
    kind: ShadingKind,
    stream: &pdfrum_object::Stream,
    dict: &Dict,
    space: &ColorSpace,
    functions: &[Arc<Function>],
    r: &R,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> Option<Mesh> {
    if space.n_components() > MAX_COMPONENTS {
        return None;
    }
    let coord_bits = u32::try_from(dict.int(names::BITS_PER_COORDINATE, r).unwrap_or(0)).ok()?;
    let component_bits = u32::try_from(dict.int(names::BITS_PER_COMPONENT, r).unwrap_or(0)).ok()?;
    let flag_bits = u32::try_from(dict.int(names::BITS_PER_FLAG, r).unwrap_or(0)).unwrap_or(0);
    // With any function present, exactly one parametric value is read per
    // colour and the functions map it.
    let components = if functions.is_empty() {
        space.n_components()
    } else {
        1
    };
    let decode: Vec<f32> = match dict.array(names::DECODE, r) {
        Some(a) => (0..a.len()).map(|i| a.number_at_or_zero(i)).collect(),
        None => Vec::new(),
    };
    let Some(params) = MeshParams::new(
        coord_bits,
        component_bits,
        flag_bits,
        components,
        &decode,
        // The lattice type reads no flags, so its `/BitsPerFlag` is never
        // checked.
        kind != ShadingKind::LatticeMesh,
    ) else {
        diags.record(Severity::Suspicious, DiagKind::MeshDecodeMalformed, None);
        return None;
    };

    let data = decode_chain(stream, 0, r, limits, diags).data;
    let mut reader = MeshReader::new(&data, &params, space, functions);
    let mesh = match kind {
        ShadingKind::FreeFormMesh => Mesh {
            triangles: reader.read_free_form(),
            patches: Vec::new(),
        },
        ShadingKind::LatticeMesh => {
            let per_row = dict.int(names::VERTICES_PER_ROW, r).unwrap_or(0);
            // At least two, with no maximum: an absurd value simply produces
            // an empty first row and aborts.
            let per_row = usize::try_from(per_row).unwrap_or(0);
            Mesh {
                triangles: reader.read_lattice(per_row),
                patches: Vec::new(),
            }
        }
        ShadingKind::CoonsMesh | ShadingKind::TensorMesh => Mesh {
            triangles: Vec::new(),
            patches: reader.read_patches(kind == ShadingKind::TensorMesh),
        },
        _ => return None,
    };
    if mesh.triangles.is_empty() && mesh.patches.is_empty() && !data.is_empty() {
        diags.record(Severity::Recovered, DiagKind::MeshTruncated, None);
    }
    Some(mesh)
}

/// Read `/Extend`, which needs genuine booleans.
#[must_use]
pub fn read_extend(dict: &Dict, r: &impl Resolve) -> (bool, bool) {
    let Some(array) = dict.array(names::EXTEND, r) else {
        return (false, false);
    };
    // `GetBooleanAt` yields the default for a non-Boolean element, so
    // `/Extend [0 1]` gives false, false.
    (
        array.bool_at(0).unwrap_or(false),
        array.bool_at(1).unwrap_or(false),
    )
}

/// Read `/Domain`, two elements defaulting to `[0, 1]`.
///
/// A shorter array reads zero for the missing entries, and there is **no**
/// check that the low bound is below the high one.
#[must_use]
pub fn read_domain(dict: &Dict, r: &impl Resolve) -> (f32, f32) {
    match dict.array(names::DOMAIN, r) {
        Some(a) => (a.number_at_or_zero(0), a.number_at_or_zero(1)),
        None => (0.0, 1.0),
    }
}

/// Read a shading's own `/Matrix`, which is distinct from a pattern's.
#[must_use]
pub fn read_matrix(dict: &Dict, r: &impl Resolve) -> Affine {
    dict.matrix(names::MATRIX, r)
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

    use super::{ShadingKind, read_domain, read_extend, validate};
    use crate::color::ColorSpace;
    use crate::function::{Exponential, Function};
    use pdfrum_object::{Array, Dict, Name, NoResolve, Object};
    use std::sync::Arc;

    fn func(inputs: usize, outputs: usize) -> Arc<Function> {
        Arc::new(Function::Exponential(Exponential {
            domain: (0..inputs).flat_map(|_| [0.0f32, 1.0]).collect(),
            range: (0..outputs).flat_map(|_| [0.0f32, 1.0]).collect(),
            c0: vec![0.0f32; outputs].into(),
            c1: vec![1.0f32; outputs].into(),
            exponent: 1.0,
            orig_outputs: outputs,
            outputs,
        }))
    }

    #[test]
    fn only_types_one_to_seven_are_shadings() {
        assert_eq!(ShadingKind::from_int(1), Some(ShadingKind::FunctionBased));
        assert_eq!(ShadingKind::from_int(7), Some(ShadingKind::TensorMesh));
        assert!(ShadingKind::from_int(0).is_none());
        assert!(ShadingKind::from_int(8).is_none());
        assert!(ShadingKind::from_int(-1).is_none());
        assert!(ShadingKind::FreeFormMesh.is_mesh());
        assert!(!ShadingKind::Axial.is_mesh());
    }

    #[test]
    fn axial_and_radial_need_a_one_in_n_out_function() {
        let rgb = ColorSpace::DeviceRgb;
        // One 1-to-3 function.
        assert!(validate(ShadingKind::Axial, &rgb, &[func(1, 3)]));
        // Or three 1-to-1 functions.
        assert!(validate(
            ShadingKind::Axial,
            &rgb,
            &[func(1, 1), func(1, 1), func(1, 1)]
        ));
        // But not two, and not a 2-input one.
        assert!(!validate(
            ShadingKind::Axial,
            &rgb,
            &[func(1, 1), func(1, 1)]
        ));
        assert!(!validate(ShadingKind::Axial, &rgb, &[func(2, 3)]));
        // And not none at all: they are mandatory here.
        assert!(!validate(ShadingKind::Axial, &rgb, &[]));
    }

    #[test]
    fn function_based_shadings_take_two_inputs() {
        let rgb = ColorSpace::DeviceRgb;
        assert!(validate(ShadingKind::FunctionBased, &rgb, &[func(2, 3)]));
        assert!(!validate(ShadingKind::FunctionBased, &rgb, &[func(1, 3)]));
    }

    #[test]
    fn mesh_functions_are_optional() {
        let rgb = ColorSpace::DeviceRgb;
        assert!(validate(ShadingKind::FreeFormMesh, &rgb, &[]));
        assert!(validate(ShadingKind::FreeFormMesh, &rgb, &[func(1, 3)]));
        // But a wrong shape still fails.
        assert!(!validate(ShadingKind::FreeFormMesh, &rgb, &[func(2, 2)]));
    }

    #[test]
    fn indexed_is_forbidden_differently_per_type() {
        let indexed = ColorSpace::Indexed(Box::new(crate::color::Indexed {
            base: Box::new(ColorSpace::DeviceRgb),
            max_index: 1,
            lookup: Box::from(&[0u8; 6][..]),
            component_ranges: Box::from(&[(0.0f32, 1.0f32); 3][..]),
        }));
        // Types 1 to 3 refuse it whatever the functions are.
        assert!(!validate(ShadingKind::Axial, &indexed, &[func(1, 1)]));
        // Mesh types refuse it only when functions are present.
        assert!(validate(ShadingKind::FreeFormMesh, &indexed, &[]));
        assert!(!validate(
            ShadingKind::FreeFormMesh,
            &indexed,
            &[func(1, 1)]
        ));
    }

    #[test]
    fn extend_needs_genuine_booleans() {
        let with_ints = Dict::from_pairs([(
            Name::from("Extend"),
            Object::Array(Array::of([Object::Int(0), Object::Int(1)])),
        )]);
        // Integers are not booleans, so neither end extends.
        assert_eq!(read_extend(&with_ints, &NoResolve), (false, false));

        let with_bools = Dict::from_pairs([(
            Name::from("Extend"),
            Object::Array(Array::of([Object::Bool(false), Object::Bool(true)])),
        )]);
        assert_eq!(read_extend(&with_bools, &NoResolve), (false, true));

        assert_eq!(read_extend(&Dict::new(), &NoResolve), (false, false));
    }

    #[test]
    fn domain_defaults_to_zero_one_only_when_absent() {
        assert_eq!(read_domain(&Dict::new(), &NoResolve), (0.0, 1.0));
        // A short array reads zeros for what it does not state.
        let short = Dict::from_pairs([(
            Name::from("Domain"),
            Object::Array(Array::of([Object::Real(0.25)])),
        )]);
        assert_eq!(read_domain(&short, &NoResolve), (0.25, 0.0));
    }
}
