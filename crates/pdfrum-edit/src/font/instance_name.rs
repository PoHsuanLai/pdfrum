//! The PostScript name of the instance a variable face was subset at.
//!
//! A subset cut at `wght` 700 of a variable face still carries the face's own
//! PostScript name, which names the *default* instance: a Thin default makes
//! every weight read "Thin" in a viewer's font list. The name written as
//! `/BaseFont` and `/FontName` is the instance's instead:
//!
//! - an instance `fvar` names (at the same user coordinates) is called what
//!   its `postScriptNameID` says, or, without one, the family prefix and its
//!   subfamily name (`Karla-Bold`);
//! - any other instance is named by the OpenType recommendation for unnamed
//!   instances (Adobe Technical Note #5902): the prefix, then `_`, the axis
//!   value and the axis tag for every axis in `fvar` order
//!   (`Karla_550wght`), shortened with a hash past 63 characters.
//!
//! The prefix is `name` ID 25 (variations PostScript name prefix) when the
//! face has one, else the typographic family (16) or family (1) name, with
//! everything but ASCII letters, digits and `-` removed.

use std::fmt::Write as _;

use sha2::{Digest as _, Sha256};
use skrifa::string::StringId;
use skrifa::{FontRef, MetadataProvider as _};

use crate::font::instance::AxisValue;

/// The longest PostScript name the recommendation allows.
const MAX_LENGTH: usize = 63;

/// How close a coordinate must be to a named instance's to be that instance.
const SAME: f32 = 0.5;

/// The name of the instance at `values` (user space, as the subsetter got
/// them), or `None` for the default instance, which keeps the face's own.
pub(crate) fn instance_name(font: &FontRef<'_>, values: &[AxisValue]) -> Option<Vec<u8>> {
    if values.is_empty() {
        return None;
    }
    let coords = full_coords(font, values);
    let named = font.named_instances().iter().find(|instance| {
        instance
            .user_coords()
            .zip(&coords)
            .all(|(named, (_, value))| (named - value).abs() < SAME)
    });
    let prefix = prefix(font)?;
    let name = match named {
        Some(instance) => instance
            .postscript_name_id()
            .and_then(|id| string(font, id))
            .or_else(|| {
                let style = string(font, instance.subfamily_name_id())?;
                Some(format!("{prefix}-{style}"))
            })?,
        None => unnamed(&prefix, &coords),
    };
    Some(shortened(&prefix, name).into_bytes())
}

/// Every `fvar` axis with its value: the one given, else the default.
fn full_coords(font: &FontRef<'_>, values: &[AxisValue]) -> Vec<([u8; 4], f32)> {
    font.axes()
        .iter()
        .map(|axis| {
            let tag = axis.tag().to_be_bytes();
            let value = values
                .iter()
                .find(|given| given.tag == tag)
                .map_or(axis.default_value(), |given| given.value);
            (tag, value)
        })
        .collect()
}

/// `prefix_<value><tag>…` for every axis, values without trailing zeros.
fn unnamed(prefix: &str, coords: &[([u8; 4], f32)]) -> String {
    let mut name = prefix.to_owned();
    for (tag, value) in coords {
        name.push('_');
        name.push_str(&number(*value));
        name.extend(
            tag.iter()
                .map(|&byte| char::from(byte))
                .filter(char::is_ascii_alphanumeric),
        );
    }
    name
}

/// `value` in at most five decimals, trailing zeros and a bare point removed.
fn number(value: f32) -> String {
    let text = format!("{value:.5}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text == "-0" {
        "0".to_owned()
    } else {
        text.to_owned()
    }
}

/// `name` if it fits; else the prefix, `-`, the first 20 hex digits of its
/// hash and `...`, as the recommendation spells a name too long to write.
fn shortened(prefix: &str, name: String) -> String {
    if name.len() <= MAX_LENGTH {
        return name;
    }
    let digest = Sha256::digest(name.as_bytes());
    let hex = digest.iter().take(10).fold(String::new(), |mut hex, byte| {
        let _ = write!(hex, "{byte:02X}");
        hex
    });
    let keep = MAX_LENGTH.saturating_sub(hex.len() + 4);
    format!("{}-{hex}...", prefix.get(..keep).unwrap_or(prefix))
}

/// The prefix unnamed-instance names start with.
fn prefix(font: &FontRef<'_>) -> Option<String> {
    [
        StringId::VARIATIONS_POSTSCRIPT_NAME_PREFIX,
        StringId::TYPOGRAPHIC_FAMILY_NAME,
        StringId::FAMILY_NAME,
    ]
    .into_iter()
    .find_map(|id| string(font, id))
}

/// Name `id`, restricted to what a PostScript name may hold here.
fn string(font: &FontRef<'_>, id: StringId) -> Option<String> {
    let text: String = font
        .localized_strings(id)
        .english_or_first()?
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    (!text.is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::{number, shortened, unnamed};

    #[test]
    fn values_lose_trailing_zeros() {
        for (value, want) in [
            (700.0, "700"),
            (550.5, "550.5"),
            (-0.0, "0"),
            (0.125, "0.125"),
        ] {
            assert_eq!(number(value), want, "{value}");
        }
    }

    #[test]
    fn an_unnamed_instance_lists_every_axis_in_order() {
        let name = unnamed("Family", &[(*b"wght", 550.0), (*b"wdth", 87.5)]);
        assert_eq!(name, "Family_550wght_87.5wdth");
    }

    #[test]
    fn a_long_name_is_shortened_with_a_hash() {
        let long = format!("Family{}", "_1000wght".repeat(10));
        let short = shortened("Family", long);
        assert!(short.len() <= 63, "{short}");
        assert!(
            short.starts_with("Family-") && short.ends_with("..."),
            "{short}"
        );
    }
}
