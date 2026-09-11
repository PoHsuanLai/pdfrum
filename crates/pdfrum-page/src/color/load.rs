//! Building a [`ColorSpace`] from the object that names it
//! (ISO 32000-1 §8.6.6).
//!
//! The dispatch has three shapes and a family table:
//!
//! - a **name** is a stock space, and the one-letter inline abbreviations
//!   (`/G`, `/RGB`, `/CMYK`) are honoured *everywhere*, not just inline;
//! - a **stream** is scanned for the first value that is a stock-space name;
//! - an **array** dispatches on element 0, matching only its **first four
//!   bytes** — so `/CalGrayAnything` is `CalGray`, `/Lab2` is *not* `Lab`,
//!   and `[/DeviceRGB 1 2]` dispatches to `DeviceN` and then fails there.
//!
//! A one-element array recurses into its element, so `[/DeviceRGB]` works but
//! `[/CalRGB]` does not.
//!
//! # Cycles and depth
//!
//! PDFium guards recursion with two visited sets and nothing else, relying on
//! the native stack for depth. We keep both sets *and* add an explicit
//! `Limits::max_colorspace_depth`, because a ten-thousand-deep chain of
//! distinct `/Indexed` arrays would otherwise overflow a Rust stack.
//! Exceeding it yields "no colorspace" — the same observable result the
//! C++ reaches by crashing.

use super::{CalGray, CalRgb, ColorSpace, IccBased, IccProfile, Indexed, Lab};
use super::{DeviceN, PatternSpace, Separation};
use crate::function::{Function, FunctionCache};
use crate::names;
use pdfrum_common::{DiagKind, Diagnostics, Limits, Severity};
use pdfrum_filters::decode_chain;
use pdfrum_object::{Array, Dict, Name, ObjRef, Object, Resolve, Resolved};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Depth cap, since `Limits` has no colorspace field until this crate lands
/// its own additions.
pub(crate) const DEFAULT_MAX_DEPTH: u32 = 32;

/// A colorspace array. A thin alias so the CIE family modules read as
/// operating on "the array" rather than on a bare [`Array`].
pub(crate) type CsArray = Array;

/// Session-scoped colorspace memoization, keyed on the reference that named
/// the space.
///
/// Direct (inline) colorspace objects are not cached: they have no identity
/// to key on, and PDFium likewise caches only the arrays it can address.
#[derive(Debug, Default)]
pub struct ColorSpaceCache {
    entries: HashMap<ObjRef, Option<Arc<ColorSpace>>>,
}

impl ColorSpaceCache {
    /// An empty cache.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// How many spaces have been loaded through this cache.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether nothing has been cached yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Everything a colorspace load needs, threaded down the recursion.
struct Ctx<'a, R: Resolve> {
    resolver: &'a R,
    /// The `/ColorSpace` resource dictionary a name is looked up in, and the
    /// one `/DefaultRGB` and friends are consulted in.
    resources: Option<&'a Dict>,
    limits: &'a Limits,
    /// References already being loaded on this branch, which is the cycle
    /// guard.
    visited: HashSet<ObjRef>,
    /// Resource *names* already being resolved, catching name cycles that
    /// reference identity misses.
    visited_names: HashSet<Name>,
}

/// The stock space a name denotes, including the one-letter abbreviations.
///
/// Exact matches only, and case sensitive: `/devicergb` is not `DeviceRGB`.
#[must_use]
pub fn stock_for_name(name: &[u8]) -> Option<ColorSpace> {
    Some(match name {
        b"DeviceRGB" | b"RGB" => ColorSpace::DeviceRgb,
        b"DeviceGray" | b"G" => ColorSpace::DeviceGray,
        b"DeviceCMYK" | b"CMYK" => ColorSpace::DeviceCmyk,
        b"Pattern" => ColorSpace::Pattern(Box::default()),
        _ => return None,
    })
}

/// Load the colorspace `obj` names.
///
/// `resources` is the `/ColorSpace` resource dictionary, used both to resolve
/// a bare name and to find the `/DefaultGray`, `/DefaultRGB` and
/// `/DefaultCMYK` substitutes. Passing `None` disables both, which is what
/// several call sites deliberately do — an `Indexed` base, for one, never
/// sees the default spaces.
///
/// Returns `None` for every failure PDFium reports as "no colorspace", which
/// makes the operator naming it a silent no-op.
#[must_use]
pub fn load_colorspace<R: Resolve>(
    obj: &Object,
    resources: Option<&Dict>,
    r: &R,
    functions: &mut FunctionCache,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> Option<ColorSpace> {
    let mut ctx = Ctx {
        resolver: r,
        resources,
        limits,
        visited: HashSet::new(),
        visited_names: HashSet::new(),
    };
    let cs = load_inner(obj, &mut ctx, functions, diags, 0);
    if cs.is_none() {
        diags.record(Severity::Suspicious, DiagKind::ColorSpaceUnsupported, None);
    }
    cs
}

fn load_inner<R: Resolve>(
    obj: &Object,
    ctx: &mut Ctx<'_, R>,
    functions: &mut FunctionCache,
    diags: &mut Diagnostics,
    depth: u32,
) -> Option<ColorSpace> {
    if depth > DEFAULT_MAX_DEPTH.min(ctx.limits.max_object_nesting) {
        return None;
    }
    // Resolve one level, remembering the reference so cycles are caught.
    let (resolved, reference) = match obj {
        Object::Ref(id) => {
            if !ctx.visited.insert(*id) {
                return None;
            }
            let fetched = ctx.resolver.fetch(*id).ok()?;
            (Resolved::Indirect(fetched), Some(*id))
        }
        direct => (Resolved::Direct(direct), None),
    };
    let out = load_direct(resolved.get(), ctx, functions, diags, depth);
    if let Some(id) = reference {
        ctx.visited.remove(&id);
    }
    out
}

fn load_direct<R: Resolve>(
    obj: &Object,
    ctx: &mut Ctx<'_, R>,
    functions: &mut FunctionCache,
    diags: &mut Diagnostics,
    depth: u32,
) -> Option<ColorSpace> {
    match obj {
        Object::Name(name) => load_name(name, ctx, functions, diags, depth),
        // A stream's dictionary is scanned for the first value that is a
        // stock-space name. Real files have exactly one such key.
        Object::Stream(stream) => stream
            .dict
            .iter()
            .find_map(|(_, v)| v.as_name().and_then(|n| stock_for_name(n.as_bytes()))),
        Object::Array(array) => load_array(array, ctx, functions, diags, depth),
        _ => None,
    }
}

/// A bare name: the three device names consult the `/Default*` substitutes,
/// everything else is a resource lookup.
fn load_name<R: Resolve>(
    name: &Name,
    ctx: &mut Ctx<'_, R>,
    functions: &mut FunctionCache,
    diags: &mut Diagnostics,
    depth: u32,
) -> Option<ColorSpace> {
    if let Some(stock) = stock_for_name(name.as_bytes()) {
        // `/DefaultGray`, `/DefaultRGB` and `/DefaultCMYK` substitute for the
        // three *fully spelled* device names only — never for the one-letter
        // abbreviations, and never for `/Pattern`.
        let default_key = match name.as_bytes() {
            b"DeviceGray" => Some(names::DEFAULT_GRAY),
            b"DeviceRGB" => Some(names::DEFAULT_RGB),
            b"DeviceCMYK" => Some(names::DEFAULT_CMYK),
            _ => None,
        };
        if let (Some(key), Some(res)) = (default_key, ctx.resources)
            && let Some(obj) = res.raw(key)
            && let Some(substitute) = load_inner(&obj.clone(), ctx, functions, diags, depth + 1)
        {
            return Some(substitute);
        }
        return Some(stock);
    }
    // A non-stock name is a resource lookup, guarded against name cycles.
    let res = ctx.resources?;
    let obj = res.raw(name)?.clone();
    if !ctx.visited_names.insert(name.clone()) {
        return None;
    }
    let out = load_inner(&obj, ctx, functions, diags, depth + 1);
    ctx.visited_names.remove(name);
    out
}

/// An array: dispatch on element 0's **first four bytes**.
fn load_array<R: Resolve>(
    array: &Array,
    ctx: &mut Ctx<'_, R>,
    functions: &mut FunctionCache,
    diags: &mut Diagnostics,
    depth: u32,
) -> Option<ColorSpace> {
    if array.is_empty() {
        return None;
    }
    let family = array.raw_at(0)?.to_byte_string();
    // A one-element array is just the stock space its element names, so
    // `[/DeviceRGB]` works while `[/CalRGB]` does not.
    if array.len() == 1 {
        return stock_for_name(&family);
    }
    // The four-byte prefix match, zero-padded, exactly as `FXBSTR_ID` does.
    let mut prefix = [0u8; 4];
    for (slot, b) in prefix.iter_mut().zip(family.iter()) {
        *slot = *b;
    }
    match &prefix {
        b"CalG" => CalGray::load(array, ctx.resolver).map(|g| ColorSpace::CalGray(Box::new(g))),
        b"CalR" => CalRgb::load(array, ctx.resolver).map(|g| ColorSpace::CalRgb(Box::new(g))),
        b"Lab\0" => Lab::load(array, ctx.resolver).map(|l| ColorSpace::Lab(Box::new(l))),
        b"ICCB" => load_icc(array, ctx, functions, diags, depth),
        // `Inde` and the inline `I` abbreviation both mean Indexed.
        b"Inde" | b"I\0\0\0" => load_indexed(array, ctx, functions, diags, depth),
        b"Sepa" => load_separation(array, ctx, functions, diags, depth),
        // `Devi` also catches `[/DeviceRGB 1 2]`, which then fails below —
        // exactly the C++'s behaviour.
        b"Devi" => load_device_n(array, ctx, functions, diags, depth),
        b"Patt" => load_pattern(array, ctx, functions, diags, depth),
        _ => None,
    }
}

fn load_icc<R: Resolve>(
    array: &Array,
    ctx: &mut Ctx<'_, R>,
    functions: &mut FunctionCache,
    diags: &mut Diagnostics,
    depth: u32,
) -> Option<ColorSpace> {
    // Element 1 must be a *stream*: a dictionary or a name fails.
    let stream = array.stream_at(1, ctx.resolver)?;
    let n = stream.dict.int(names::N, ctx.resolver).unwrap_or(0);
    if !super::is_valid_icc_components(n) {
        return None;
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the validity check above restricts n to 1, 3 or 4"
    )]
    let n = n as u8;

    let data = decode_chain(&stream, 0, ctx.resolver, ctx.limits, diags).data;
    let profile = Arc::new(IccProfile::load(&data, usize::from(n)));

    // The alternate is consulted whenever the profile is not *supported*,
    // which includes the sRGB special case even though `to_rgb` never reaches
    // the base for it.
    let base = if profile.is_supported() {
        None
    } else {
        let alternate = stream
            .dict
            .raw(names::ALTERNATE)
            .cloned()
            .and_then(|obj| load_inner(&obj, ctx, functions, diags, depth + 1))
            .filter(|cs| {
                // Pattern is refused, and the component counts must agree
                // exactly — a mismatch silently discards the alternate.
                if matches!(cs, ColorSpace::Pattern(_)) {
                    return false;
                }
                if cs.n_components() != usize::from(n) {
                    diags.record(Severity::Suspicious, DiagKind::IccAlternateMismatch, None);
                    return false;
                }
                true
            });
        if let Some(cs) = alternate {
            diags.record(Severity::Recovered, DiagKind::IccAlternateUsed, None);
            Some(Box::new(cs))
        } else {
            // The sRGB special case reaches here too, but its colour never
            // consults the base, so it is not a fallback worth reporting.
            if !profile.is_srgb() {
                diags.record(Severity::Recovered, DiagKind::IccStockFallback, None);
            }
            IccBased::stock_alternate(n).map(Box::new)
        }
    };

    // `/Range` is stored and, matching the C++'s standing TODO, never read.
    let ranges: Box<[f32]> = match stream.dict.array(names::RANGE, ctx.resolver) {
        Some(a) if a.len() >= usize::from(n) * 2 => (0..usize::from(n) * 2)
            .map(|i| a.number_at_or_zero(i))
            .collect(),
        _ => (0..n).flat_map(|_| [0.0, 1.0]).collect(),
    };

    Some(ColorSpace::IccBased(Box::new(IccBased {
        profile,
        n,
        base,
        ranges,
    })))
}

fn load_indexed<R: Resolve>(
    array: &Array,
    ctx: &mut Ctx<'_, R>,
    functions: &mut FunctionCache,
    diags: &mut Diagnostics,
    depth: u32,
) -> Option<ColorSpace> {
    if array.len() < 4 {
        return None;
    }
    // The base loads with **no resources**, so `/DefaultRGB` never applies to
    // an indexed base.
    let base_obj = array.raw_at(1)?.clone();
    let saved = ctx.resources.take();
    let base = load_inner(&base_obj, ctx, functions, diags, depth + 1);
    ctx.resources = saved;
    let base = base?;
    // ISO 32000-1 §8.6.6.3: the base may be neither Indexed nor Pattern.
    if matches!(base, ColorSpace::Indexed(_) | ColorSpace::Pattern(_)) {
        return None;
    }

    let component_ranges: Box<[(f32, f32)]> = (0..base.n_components())
        .map(|i| {
            let (_, min, max) = base.default_value(i);
            // After this the second field is the *range*, not the maximum.
            (min, max - min)
        })
        .collect();

    let hival = array.int_at(2).unwrap_or(0);
    if !(0..=255).contains(&hival) {
        diags.record(Severity::Recovered, DiagKind::IndexedHivalClamped, None);
    }
    let max_index = u8::try_from(hival.clamp(0, 255)).unwrap_or(0);

    // The table may be a string or a stream; anything else leaves it empty
    // *without* failing the load, so every lookup then finds no colour.
    let lookup: Box<[u8]> = match array.get(3, ctx.resolver).as_deref() {
        Some(Object::Str(s)) => s.as_bytes().into(),
        Some(Object::Stream(stream)) => decode_chain(stream, 0, ctx.resolver, ctx.limits, diags)
            .data
            .into(),
        Some(_) => Box::default(),
        None => return None,
    };

    Some(ColorSpace::Indexed(Box::new(Indexed {
        base: Box::new(base),
        max_index,
        lookup,
        component_ranges,
    })))
}

fn load_separation<R: Resolve>(
    array: &Array,
    ctx: &mut Ctx<'_, R>,
    functions: &mut FunctionCache,
    diags: &mut Diagnostics,
    depth: u32,
) -> Option<ColorSpace> {
    // `/None` short-circuits with no alternate and no transform. Note there
    // is deliberately no `/All` case: the C++ has none either.
    if array.raw_at(1).map(Object::to_byte_string).as_deref() == Some(b"None") {
        return Some(ColorSpace::Separation(Box::new(Separation {
            none: true,
            alternate: None,
            tint: None,
        })));
    }
    let alternate_obj = array.raw_at(2)?.clone();
    let alternate = load_inner(&alternate_obj, ctx, functions, diags, depth + 1)?;
    if alternate.is_special() {
        return None;
    }
    // The tint transform is optional here: a name is skipped outright, and a
    // failed load or too few outputs leaves the space usable.
    let tint = match array.raw_at(3) {
        Some(Object::Name(_)) | None => None,
        Some(obj) => {
            let loaded = functions.load(obj, ctx.resolver, ctx.limits, diags);
            match loaded {
                Some(f) if f.output_count() >= alternate.n_components() => Some(f),
                Some(_) | None => {
                    diags.record(Severity::Suspicious, DiagKind::TintTransformDropped, None);
                    None
                }
            }
        }
    };
    Some(ColorSpace::Separation(Box::new(Separation {
        none: false,
        alternate: Some(Box::new(alternate)),
        tint,
    })))
}

fn load_device_n<R: Resolve>(
    array: &Array,
    ctx: &mut Ctx<'_, R>,
    functions: &mut FunctionCache,
    diags: &mut Diagnostics,
    depth: u32,
) -> Option<ColorSpace> {
    // `/Names` must be an array, which is what rejects `[/DeviceRGB 1 2]`.
    let names_array = array.array_at(1, ctx.resolver)?;
    let colorants: Box<[Name]> = names_array
        .iter()
        .map(|o| Name::new(o.to_byte_string()))
        .collect();
    if colorants.is_empty() {
        return None;
    }
    let alternate_obj = array.raw_at(2)?.clone();
    let alternate = load_inner(&alternate_obj, ctx, functions, diags, depth + 1)?;
    if alternate.is_special() {
        return None;
    }
    // Mandatory here, unlike `Separation`: a missing or short transform fails
    // the whole space.
    let tint = functions.load(array.raw_at(3)?, ctx.resolver, ctx.limits, diags)?;
    if tint.output_count() < alternate.n_components() {
        return None;
    }
    Some(ColorSpace::DeviceN(Box::new(DeviceN {
        names: colorants,
        alternate: Box::new(alternate),
        tint,
    })))
}

fn load_pattern<R: Resolve>(
    array: &Array,
    ctx: &mut Ctx<'_, R>,
    functions: &mut FunctionCache,
    diags: &mut Diagnostics,
    depth: u32,
) -> Option<ColorSpace> {
    let Some(base_obj) = array.raw_at(1).cloned() else {
        return Some(ColorSpace::Pattern(Box::default()));
    };
    // The base loads without resources, like `Indexed`'s.
    let saved = ctx.resources.take();
    let base = load_inner(&base_obj, ctx, functions, diags, depth + 1);
    ctx.resources = saved;
    match base {
        // A missing or unloadable base is **not** an error — it is the
        // uncoloured-pattern case, a one-component space with no base.
        None => Some(ColorSpace::Pattern(Box::default())),
        Some(ColorSpace::Pattern(_)) => None,
        Some(cs) if cs.n_components() > super::MAX_PATTERN_COMPONENTS => None,
        Some(cs) => Some(ColorSpace::Pattern(Box::new(PatternSpace {
            base: Some(Box::new(cs)),
        }))),
    }
}

/// Unused import guard: `Function` is named in the cache's signature docs.
const _: Option<&Function> = None;

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

    use super::{load_colorspace, stock_for_name};
    use crate::color::{ColorSpace, Family};
    use crate::function::FunctionCache;
    use pdfrum_common::{Diagnostics, Limits};
    use pdfrum_object::{Array, Dict, Name, NoResolve, Object, PdfString};

    fn load(obj: &Object, res: Option<&Dict>) -> Option<ColorSpace> {
        let mut funcs = FunctionCache::new();
        let mut diags = Diagnostics::default();
        load_colorspace(
            obj,
            res,
            &NoResolve,
            &mut funcs,
            &Limits::default(),
            &mut diags,
        )
    }

    fn name(s: &str) -> Object {
        Object::Name(Name::from(s))
    }

    #[test]
    fn stock_names_and_their_abbreviations() {
        assert_eq!(stock_for_name(b"DeviceRGB"), Some(ColorSpace::DeviceRgb));
        assert_eq!(stock_for_name(b"RGB"), Some(ColorSpace::DeviceRgb));
        assert_eq!(stock_for_name(b"G"), Some(ColorSpace::DeviceGray));
        assert_eq!(stock_for_name(b"CMYK"), Some(ColorSpace::DeviceCmyk));
        assert!(stock_for_name(b"devicergb").is_none());
        assert!(stock_for_name(b"CalRGB").is_none());
    }

    #[test]
    fn a_one_element_array_is_its_stock_space() {
        let obj = Object::Array(Array::of([name("DeviceRGB")]));
        assert_eq!(load(&obj, None), Some(ColorSpace::DeviceRgb));
        // But `[/CalRGB]` is not, since CalRGB is not a stock name.
        let obj = Object::Array(Array::of([name("CalRGB")]));
        assert!(load(&obj, None).is_none());
    }

    #[test]
    fn device_rgb_with_extra_elements_dispatches_to_device_n_and_fails() {
        let obj = Object::Array(Array::of([
            name("DeviceRGB"),
            Object::Int(1),
            Object::Int(2),
        ]));
        assert!(load(&obj, None).is_none());
    }

    #[test]
    fn the_family_prefix_is_exactly_four_bytes() {
        let dict = Dict::from_pairs([(
            Name::from("WhitePoint"),
            Object::Array(Array::of([
                Object::Real(0.95),
                Object::Real(1.0),
                Object::Real(1.09),
            ])),
        )]);
        // `CalGrayAnything` matches `CalG`.
        let obj = Object::Array(Array::of([
            name("CalGrayAnything"),
            Object::Dict(dict.clone()),
        ]));
        assert_eq!(load(&obj, None).map(|c| c.family()), Some(Family::CalGray));
        // `Lab2` does not match `Lab\0`.
        let obj = Object::Array(Array::of([name("Lab2"), Object::Dict(dict)]));
        assert!(load(&obj, None).is_none());
    }

    #[test]
    fn white_point_must_have_y_exactly_one() {
        let bad = Dict::from_pairs([(
            Name::from("WhitePoint"),
            Object::Array(Array::of([
                Object::Real(0.9505),
                Object::Real(0.99999),
                Object::Real(1.089),
            ])),
        )]);
        let obj = Object::Array(Array::of([name("CalRGB"), Object::Dict(bad)]));
        assert!(load(&obj, None).is_none());
    }

    #[test]
    fn indexed_hival_clamps_rather_than_failing() {
        let make = |hival: i64| {
            Object::Array(Array::of([
                name("Indexed"),
                name("DeviceGray"),
                Object::Int(hival),
                Object::Str(PdfString::literal([0u8, 255])),
            ]))
        };
        let Some(ColorSpace::Indexed(cs)) = load(&make(-5), None) else {
            panic!("expected an indexed space");
        };
        assert_eq!(cs.max_index, 0);
        let Some(ColorSpace::Indexed(cs)) = load(&make(999), None) else {
            panic!("expected an indexed space");
        };
        assert_eq!(cs.max_index, 255);
    }

    #[test]
    fn indexed_needs_four_elements_and_a_non_special_base() {
        let short = Object::Array(Array::of([
            name("Indexed"),
            name("DeviceGray"),
            Object::Int(1),
        ]));
        assert!(load(&short, None).is_none());

        let pattern_base = Object::Array(Array::of([
            name("Indexed"),
            name("Pattern"),
            Object::Int(1),
            Object::Str(PdfString::literal([0u8])),
        ]));
        assert!(load(&pattern_base, None).is_none());
    }

    #[test]
    fn separation_none_paints_nothing_and_all_is_ordinary() {
        let none = Object::Array(Array::of([
            name("Separation"),
            name("None"),
            name("DeviceGray"),
        ]));
        let Some(ColorSpace::Separation(sep)) = load(&none, None) else {
            panic!("expected a separation");
        };
        assert!(sep.none);

        // `/All` gets no special treatment: it needs a working alternate like
        // any other colorant name.
        let all = Object::Array(Array::of([
            name("Separation"),
            name("All"),
            name("DeviceGray"),
        ]));
        let Some(ColorSpace::Separation(sep)) = load(&all, None) else {
            panic!("expected a separation");
        };
        assert!(!sep.none);
        assert!(sep.tint.is_none(), "a missing transform is not fatal");
    }

    #[test]
    fn device_n_without_a_transform_fails() {
        let obj = Object::Array(Array::of([
            name("DeviceN"),
            Object::Array(Array::of([name("Spot")])),
            name("DeviceGray"),
        ]));
        assert!(load(&obj, None).is_none());
    }

    #[test]
    fn a_pattern_with_an_unloadable_base_still_loads() {
        let obj = Object::Array(Array::of([name("Pattern"), name("NotASpace")]));
        let Some(ColorSpace::Pattern(cs)) = load(&obj, None) else {
            panic!("expected a pattern space");
        };
        assert!(cs.base.is_none());
        assert_eq!(cs.n_components(), 1);
    }

    #[test]
    fn default_colorspaces_substitute_for_the_three_device_names() {
        let res = Dict::from_pairs([(Name::from("DefaultRGB"), name("DeviceCMYK"))]);
        // `/DeviceRGB` is substituted…
        assert_eq!(
            load(&name("DeviceRGB"), Some(&res)).map(|c| c.family()),
            Some(Family::DeviceCmyk)
        );
        // …but the abbreviation is not.
        assert_eq!(
            load(&name("RGB"), Some(&res)).map(|c| c.family()),
            Some(Family::DeviceRgb)
        );
        // Nor is a different device name.
        assert_eq!(
            load(&name("DeviceGray"), Some(&res)).map(|c| c.family()),
            Some(Family::DeviceGray)
        );
    }

    #[test]
    fn a_named_space_resolves_through_the_colorspace_resources() {
        let res = Dict::from_pairs([(Name::from("CS0"), name("DeviceCMYK"))]);
        assert_eq!(
            load(&name("CS0"), Some(&res)).map(|c| c.family()),
            Some(Family::DeviceCmyk)
        );
        assert!(load(&name("CS0"), None).is_none());
    }

    #[test]
    fn a_name_cycle_terminates() {
        let res = Dict::from_pairs([(Name::from("A"), name("B")), (Name::from("B"), name("A"))]);
        assert!(load(&name("A"), Some(&res)).is_none());
    }

    #[test]
    fn an_empty_or_non_array_object_yields_nothing() {
        assert!(load(&Object::Array(Array::new()), None).is_none());
        assert!(load(&Object::Int(3), None).is_none());
        assert!(load(&Object::Null, None).is_none());
    }
}
