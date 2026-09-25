//! Which instance of a variable face a glyph run was laid out at, and the two
//! spellings the writer needs of it.
//!
//! A renderer holds an instance as *normalized* coordinates (`F2Dot14`, one per
//! `fvar` axis, after `avar`), which is what glyph metrics are computed at.
//! The subsetter instantiates from *user* values (`wght` 700). Going from the
//! first to the second undoes `avar` — its segment maps are monotonic, so the
//! inverse is the same piecewise-linear map read the other way — and then the
//! `fvar` normalization.

use skrifa::instance::{Location, NormalizedCoord};
use skrifa::raw::TableProvider as _;
use skrifa::{FontRef, MetadataProvider as _, Tag};

/// The instance of a variable face a glyph run is drawn at.
///
/// ```
/// use pdfrum_edit::{AxisValue, FontInstance};
///
/// let bold = FontInstance::User(vec![AxisValue { tag: *b"wght", value: 700.0 }]);
/// assert_ne!(bold, FontInstance::Default);
/// ```
#[derive(Debug, Clone, PartialEq, Default)]
pub enum FontInstance {
    /// The face as stored: its default instance, or a static face.
    #[default]
    Default,
    /// Normalized coordinates, one `F2Dot14` per `fvar` axis in the face's own
    /// order, after `avar`: what a shaper or rasteriser holds.
    Normalized(Vec<i16>),
    /// User-space axis values; an axis not named stays at its default.
    User(Vec<AxisValue>),
}

/// One axis at one user-space value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AxisValue {
    /// The axis tag's four bytes: `*b"wght"`.
    pub tag: [u8; 4],
    /// The value, in the axis's own units (a weight of 700).
    pub value: f32,
}

/// `instance` as user values, which the subsetter takes; empty for the
/// default instance.
pub(crate) fn user_values(font: &FontRef<'_>, instance: &FontInstance) -> Vec<AxisValue> {
    match instance {
        FontInstance::Default => Vec::new(),
        FontInstance::User(values) => values.clone(),
        FontInstance::Normalized(coords) if coords.iter().all(|&c| c == 0) => Vec::new(),
        FontInstance::Normalized(coords) => {
            let maps = segment_maps(font);
            font.axes()
                .iter()
                .zip(coords)
                .enumerate()
                .map(|(index, (axis, &coord))| {
                    let normalized = f32::from(coord) / 16384.0;
                    let before_avar = maps
                        .get(index)
                        .map_or(normalized, |map| invert(map, normalized));
                    AxisValue {
                        tag: axis.tag().to_be_bytes(),
                        value: denormalize(
                            before_avar,
                            axis.min_value(),
                            axis.default_value(),
                            axis.max_value(),
                        ),
                    }
                })
                .collect()
        }
    }
}

/// `instance` as the location glyph metrics are read at.
pub(crate) fn location(font: &FontRef<'_>, instance: &FontInstance) -> Location {
    match instance {
        FontInstance::Default => Location::default(),
        FontInstance::Normalized(coords) => {
            let mut location = Location::new(coords.len());
            location
                .coords_mut()
                .iter_mut()
                .zip(coords)
                .for_each(|(slot, &coord)| *slot = NormalizedCoord::from_bits(coord));
            location
        }
        FontInstance::User(values) => font
            .axes()
            .location(values.iter().map(|axis| (Tag::new(&axis.tag), axis.value))),
    }
}

/// Each axis's `avar` segment map as (from, to) pairs; empty without `avar`.
fn segment_maps(font: &FontRef<'_>) -> Vec<Vec<(f32, f32)>> {
    let Ok(avar) = font.avar() else {
        return Vec::new();
    };
    avar.axis_segment_maps()
        .iter()
        .map(|maps| {
            maps.map(|maps| {
                maps.axis_value_maps()
                    .iter()
                    .map(|pair| {
                        (
                            pair.from_coordinate().to_f32(),
                            pair.to_coordinate().to_f32(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
        })
        .collect()
}

/// The coordinate `map` sends to `to`: the segment map read backwards. A map
/// with fewer than two pairs is the identity, as `avar` defines it.
fn invert(map: &[(f32, f32)], to: f32) -> f32 {
    map.windows(2)
        .find_map(|pair| match pair {
            [(from_a, to_a), (from_b, to_b)] if *to_a <= to && to <= *to_b => {
                Some(if (to_b - to_a).abs() < f32::EPSILON {
                    *from_a
                } else {
                    from_a + (to - to_a) * (from_b - from_a) / (to_b - to_a)
                })
            }
            _ => None,
        })
        .unwrap_or(to)
}

/// The user value a normalized `value` (-1..=1) stands for on an axis
/// `min..=max` around `default`.
fn denormalize(value: f32, min: f32, default: f32, max: f32) -> f32 {
    if value < 0.0 {
        default + value * (default - min)
    } else {
        default + value * (max - default)
    }
}

#[cfg(test)]
mod tests {
    use super::{denormalize, invert};

    #[test]
    fn denormalizing_follows_each_side_of_the_default() {
        for (normalized, want) in [(-1.0, 100.0), (-0.5, 250.0), (0.0, 400.0), (1.0, 900.0)] {
            assert!(
                (denormalize(normalized, 100.0, 400.0, 900.0) - want).abs() < 1e-4,
                "{normalized}"
            );
        }
    }

    #[test]
    fn avar_is_read_backwards() {
        let map = [(-1.0, -1.0), (0.0, 0.0), (0.5, 0.8), (1.0, 1.0)];
        assert!((invert(&map, 0.8) - 0.5).abs() < 1e-6);
        assert!((invert(&map, 0.4) - 0.25).abs() < 1e-6);
        assert!((invert(&map, 0.9) - 0.75).abs() < 1e-6);
        assert!((invert(&[], 0.3) - 0.3).abs() < 1e-6);
    }
}
