//! Transparency groups and soft masks (ISO 32000-1 §11.4, §11.6.5).
//!
//! # `/S` defaults to luminosity
//!
//! A soft mask's `/S` selects between alpha and luminosity sources, and the
//! test is **`value != "Alpha"`** — so a missing `/S`, a misspelled one, and
//! `/Luminosity` itself all mean luminosity. Only the exact string `Alpha`
//! selects the other.
//!
//! # `/K` is parsed and not honoured
//!
//! Knockout groups are **unimplemented in PDFium**: `/K` is never read
//! anywhere in its core, whose transparency model carries only "is a group"
//! and "is isolated". We parse `/K` into the model because it is genuinely in
//! the file, and the renderer ignores it so output matches the oracle.
//!
//! # Pages are always isolated
//!
//! A page's constructor sets isolation unconditionally, whatever its
//! `/Group` says — so the page-level group is always isolated even when the
//! dictionary declares otherwise.

use crate::color::ColorSpace;
use crate::function::FunctionCache;
use crate::names;
use crate::transfer::CHANNEL_SAMPLES;
use kurbo::Affine;
use pdfrum_common::{Diagnostics, Limits};
use pdfrum_object::{Dict, Object, Resolve, Stream};

/// A transparency group's attributes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Transparency {
    /// Whether the content forms a transparency group at all.
    pub group: bool,
    /// Whether the group composites against a blank backdrop rather than the
    /// page's.
    pub isolated: bool,
    /// Whether the group is a knockout group. Parsed, never honoured.
    pub knockout: bool,
}

impl Transparency {
    /// Read a `/Group` dictionary.
    ///
    /// The dictionary must have `/S` exactly `Transparency`; anything else
    /// means no group at all. `/I` and `/K` are true for **any non-zero
    /// value**, integers included, which is how a `/I 1` works alongside a
    /// `/I true`.
    #[must_use]
    pub fn from_group(group: Option<&Dict>, r: &impl Resolve) -> Self {
        let Some(group) = group else {
            return Self::default();
        };
        if group.byte_string(names::S, r).as_deref() != Some(b"Transparency") {
            return Self::default();
        }
        Self {
            group: true,
            isolated: truthy(group, names::I, r),
            knockout: truthy(group, names::K, r),
        }
    }

    /// A page's group, which is isolated whatever the dictionary says.
    #[must_use]
    pub fn for_page(group: Option<&Dict>, r: &impl Resolve) -> Self {
        Self {
            isolated: true,
            ..Self::from_group(group, r)
        }
    }
}

/// Whether a key holds anything non-zero: a `true`, or any non-zero number.
fn truthy(dict: &Dict, key: &pdfrum_object::Name, r: &impl Resolve) -> bool {
    match dict.get(key, r).as_deref() {
        Some(Object::Bool(b)) => *b,
        Some(Object::Int(i)) => *i != 0,
        Some(Object::Real(v)) => *v != 0.0,
        _ => false,
    }
}

/// Which channel of the group a soft mask reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SoftMaskKind {
    /// The group's rendered luminosity. **The default**, reached by every
    /// `/S` that is not exactly `Alpha`.
    #[default]
    Luminosity,
    /// The group's alpha.
    Alpha,
}

/// A soft mask from an `/ExtGState`'s `/SMask`.
#[derive(Debug, Clone, PartialEq)]
pub struct SoftMask {
    /// The group whose rendering becomes the mask.
    pub group: Stream,
    /// Which channel to read.
    pub kind: SoftMaskKind,
    /// The backdrop the group composites against, in luminosity mode.
    /// **Opaque black** by default.
    pub backdrop: crate::color::Rgb,
    /// The transfer applied to each mask value, when `/TR` supplied one.
    pub transfer: Option<Box<[u8; CHANNEL_SAMPLES]>>,
    /// The transformation in force when the `/ExtGState` was applied, which
    /// is what places the mask.
    pub matrix: Affine,
    /// The group's own page objects, interpreted from `group`.
    ///
    /// A soft mask's `/G` is a form `XObject` like any other, and what it paints
    /// is what the mask *is*: a luminosity mask reads the group's rendered
    /// luminance, an alpha mask its coverage. Left empty the mask is the `/BC`
    /// backdrop alone — exact for a backdrop-only mask and wrong for every
    /// other, which is how it silently blanked or revealed whole objects.
    ///
    /// Filled in by the interpreter, which is the only layer with a resolver;
    /// `load` leaves it empty.
    pub objects: Vec<crate::page::PageObject>,
}

impl SoftMask {
    /// Read a `/SMask` dictionary.
    ///
    /// Returns `None` — meaning no mask — when the value is not a dictionary
    /// at all, which is how `/SMask /None` reaches the correct answer, and
    /// when `/G` is not a stream.
    #[must_use]
    pub fn load<R: Resolve>(
        value: &Object,
        matrix: Affine,
        r: &R,
        functions: &mut FunctionCache,
        limits: &Limits,
        diags: &mut Diagnostics,
    ) -> Option<Self> {
        // A name — including `/None` — is not a dictionary, which is exactly
        // the spec's semantics reached by accident.
        let resolved = value.resolve(r).ok()?;
        let dict = resolved.as_dict()?;
        // `/G` **must** be a stream or the whole mask is dropped.
        let group = dict.stream(names::G, r)?;

        // Only the exact string `Alpha` selects alpha; everything else,
        // absent keys included, is luminosity.
        let kind = if dict.byte_string(names::S, r).as_deref() == Some(b"Alpha") {
            SoftMaskKind::Alpha
        } else {
            SoftMaskKind::Luminosity
        };

        let backdrop = match kind {
            // `/BC` is consulted only in luminosity mode.
            SoftMaskKind::Luminosity => backdrop_color(dict, &group, r, functions, limits, diags),
            SoftMaskKind::Alpha => crate::color::Rgb::BLACK,
        };

        // `/TR` is accepted only as a dictionary or a stream, so
        // `/TR /Identity` is correctly ignored.
        let transfer = dict
            .raw(names::TR)
            .filter(|obj| {
                matches!(
                    obj.resolve(r).as_deref(),
                    Ok(Object::Dict(_) | Object::Stream(_))
                )
            })
            .and_then(|obj| functions.load(obj, r, limits, diags))
            .map(|func| {
                let mut lut = Box::new([0u8; CHANNEL_SAMPLES]);
                let mut results = vec![0.0f32; func.output_count().max(1)];
                for (i, slot) in lut.iter_mut().enumerate() {
                    #[expect(
                        clippy::cast_precision_loss,
                        reason = "an index below 256 is exact in f32"
                    )]
                    let input = (i as f32) / 255.0;
                    let identity = u8::try_from(i).unwrap_or(u8::MAX);
                    if func.eval_into(&[input], &mut results) == 0 {
                        *slot = identity;
                        continue;
                    }
                    // Only output component 0 is used.
                    #[expect(
                        clippy::cast_possible_truncation,
                        clippy::cast_sign_loss,
                        reason = "the clamp bounds the product to 0..=255"
                    )]
                    let byte = (results.first().copied().unwrap_or(0.0).clamp(0.0, 1.0) * 255.0)
                        .round() as u8;
                    *slot = byte;
                }
                lut
            });

        Some(Self {
            group,
            kind,
            backdrop,
            transfer,
            matrix,
            // Filled in by the interpreter, which has the resolver and the
            // recursion guards a form parse needs.
            objects: Vec::new(),
        })
    }

    /// The mask value for one luminosity or alpha byte.
    #[must_use]
    pub fn apply_transfer(&self, value: u8) -> u8 {
        self.transfer
            .as_ref()
            .and_then(|lut| lut.get(usize::from(value)))
            .copied()
            .unwrap_or(value)
    }
}

/// The `/BC` backdrop colour, defaulting to **opaque black**.
///
/// The colour space is the `/G` stream's own `/Group` `/CS`, not the soft
/// mask dictionary's — a distinction that matters because the two are
/// different dictionaries. An unsupported group space (`Lab`, any special
/// family, or a non-normal `ICCBased`) falls back to black.
fn backdrop_color<R: Resolve>(
    smask: &Dict,
    group_stream: &Stream,
    r: &R,
    functions: &mut FunctionCache,
    limits: &Limits,
    diags: &mut Diagnostics,
) -> crate::color::Rgb {
    let default = crate::color::Rgb::BLACK;
    let Some(bc) = smask.array(names::BC, r) else {
        return default;
    };
    let Some(cs_obj) = group_stream
        .dict
        .dict(names::GROUP, r)
        .and_then(|g| g.raw(names::CS).cloned())
    else {
        return default;
    };
    let Some(space) = crate::color::load_colorspace(&cs_obj, None, r, functions, limits, diags)
    else {
        return default;
    };
    if matches!(space, ColorSpace::Lab(_)) || space.is_special() {
        return default;
    }
    if matches!(space, ColorSpace::IccBased(_)) && !space.is_normal() {
        return default;
    }
    // At most eight values are read, and the buffer is padded to at least
    // eight — so a nine-component `DeviceN` reads only its first eight.
    let count = bc.len().min(8);
    let width = space.n_components().max(8);
    let mut comps = vec![0.0f32; width];
    for i in 0..count {
        if let Some(slot) = comps.get_mut(i) {
            *slot = bc.number_at_or_zero(i);
        }
    }
    space.to_rgb(&comps)
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

    use super::{SoftMask, SoftMaskKind, Transparency};
    use crate::function::FunctionCache;
    use kurbo::Affine;
    use pdfrum_common::{Diagnostics, Limits};
    use pdfrum_object::{ByteSpan, Dict, Name, NoResolve, ObjRef, Object, Resolve, Stream};
    use std::sync::Arc;

    /// The reference a test dictionary's `/G` points at.
    const GROUP_REF: ObjRef = ObjRef::new(1, 0);

    /// A store holding one group stream, since a dictionary value may never
    /// be a direct stream (ISO 32000-1 §7.3.8.1).
    struct GroupStore;

    impl Resolve for GroupStore {
        fn fetch(&self, r: ObjRef) -> Result<Arc<Object>, pdfrum_object::Error> {
            if r.num == GROUP_REF.num {
                return Ok(Arc::new(Object::Stream(Stream::new(
                    Dict::new(),
                    ByteSpan::empty(),
                ))));
            }
            Err(pdfrum_object::Error::UnresolvedRef(r))
        }
    }

    fn group_ref() -> Object {
        Object::Ref(GROUP_REF)
    }

    fn load_mask(pairs: Vec<(Name, Object)>) -> Option<SoftMask> {
        let mut funcs = FunctionCache::new();
        let mut diags = Diagnostics::default();
        SoftMask::load(
            &Object::Dict(Dict::from_pairs(pairs)),
            Affine::IDENTITY,
            &GroupStore,
            &mut funcs,
            &Limits::default(),
            &mut diags,
        )
    }

    #[test]
    fn a_group_needs_s_to_be_exactly_transparency() {
        let good = Dict::from_pairs([(Name::from("S"), Object::Name(Name::from("Transparency")))]);
        assert!(Transparency::from_group(Some(&good), &NoResolve).group);

        let wrong = Dict::from_pairs([(Name::from("S"), Object::Name(Name::from("Transp")))]);
        assert!(!Transparency::from_group(Some(&wrong), &NoResolve).group);
        assert!(!Transparency::from_group(None, &NoResolve).group);
    }

    #[test]
    fn isolation_and_knockout_are_true_for_any_non_zero_value() {
        let make = |key: &str, value: Object| {
            Dict::from_pairs([
                (Name::from("S"), Object::Name(Name::from("Transparency"))),
                (Name::from(key), value),
            ])
        };
        assert!(
            Transparency::from_group(Some(&make("I", Object::Bool(true))), &NoResolve).isolated
        );
        assert!(Transparency::from_group(Some(&make("I", Object::Int(1))), &NoResolve).isolated);
        assert!(!Transparency::from_group(Some(&make("I", Object::Int(0))), &NoResolve).isolated);
        assert!(
            Transparency::from_group(Some(&make("K", Object::Bool(true))), &NoResolve).knockout
        );
    }

    #[test]
    fn a_page_is_isolated_whatever_its_group_says() {
        let not_isolated = Dict::from_pairs([
            (Name::from("S"), Object::Name(Name::from("Transparency"))),
            (Name::from("I"), Object::Bool(false)),
        ]);
        assert!(Transparency::for_page(Some(&not_isolated), &NoResolve).isolated);
        // Even with no group at all.
        assert!(Transparency::for_page(None, &NoResolve).isolated);
    }

    #[test]
    fn a_soft_mask_defaults_to_luminosity() {
        let mask = load_mask(vec![(Name::from("G"), group_ref())]).expect("should load");
        assert_eq!(mask.kind, SoftMaskKind::Luminosity);

        // `/Luminosity` is the same answer.
        let mask = load_mask(vec![
            (Name::from("G"), group_ref()),
            (Name::from("S"), Object::Name(Name::from("Luminosity"))),
        ])
        .expect("should load");
        assert_eq!(mask.kind, SoftMaskKind::Luminosity);

        // …and so is garbage.
        let mask = load_mask(vec![
            (Name::from("G"), group_ref()),
            (Name::from("S"), Object::Name(Name::from("Nonsense"))),
        ])
        .expect("should load");
        assert_eq!(mask.kind, SoftMaskKind::Luminosity);
    }

    #[test]
    fn only_the_exact_string_alpha_selects_alpha() {
        let mask = load_mask(vec![
            (Name::from("G"), group_ref()),
            (Name::from("S"), Object::Name(Name::from("Alpha"))),
        ])
        .expect("should load");
        assert_eq!(mask.kind, SoftMaskKind::Alpha);
    }

    #[test]
    fn a_mask_without_a_stream_g_is_dropped() {
        assert!(load_mask(vec![(Name::from("G"), Object::Int(7))]).is_none());
        assert!(load_mask(vec![]).is_none());
    }

    #[test]
    fn smask_none_yields_no_mask() {
        let mut funcs = FunctionCache::new();
        let mut diags = Diagnostics::default();
        let got = SoftMask::load(
            &Object::Name(Name::from("None")),
            Affine::IDENTITY,
            &NoResolve,
            &mut funcs,
            &Limits::default(),
            &mut diags,
        );
        assert!(got.is_none());
    }

    #[test]
    fn the_default_backdrop_is_opaque_black() {
        let mask = load_mask(vec![(Name::from("G"), group_ref())]).expect("should load");
        assert_eq!(mask.backdrop, crate::color::Rgb::BLACK);
    }

    #[test]
    fn a_tr_name_is_ignored_rather_than_installed() {
        let mask = load_mask(vec![
            (Name::from("G"), group_ref()),
            (Name::from("TR"), Object::Name(Name::from("Identity"))),
        ])
        .expect("should load");
        assert!(mask.transfer.is_none());
        // With no transfer the value passes through.
        assert_eq!(mask.apply_transfer(128), 128);
    }
}
