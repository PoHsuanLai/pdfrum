//! The three text encodings an SVG document is made of: numbers, path data
//! and attribute-safe strings.
//!
//! Kept in one module because all three are decisions about *bytes on the
//! wire* rather than about drawing, and because getting any of them wrong is
//! silent — an unescaped `&` in a base64 payload or an `inf` where a
//! coordinate belongs produces a file a parser rejects wholesale rather than
//! a pixel that moves.

use core::fmt::Write as _;

use kurbo::{Affine, BezPath, PathEl, Point};

/// How many fractional digits a coordinate keeps.
///
/// Three is a hundredth of a device pixel at the board's 72 dpi, which is
/// below any rasterizer's sampling grid, and it is what keeps a page of
/// several thousand path segments from doubling in size for digits no
/// renderer can act on.
const PLACES: usize = 3;

/// Append `value` in the shortest form that reads back to the same number at
/// [`PLACES`] digits.
///
/// SVG has no syntax for a non-finite number, so one is written as `0` — the
/// coordinate is already meaningless and a `NaN` in the output would take the
/// whole `<path>` element down with it.
pub fn number(out: &mut String, value: f64) {
    if !value.is_finite() {
        out.push('0');
        return;
    }
    // `{:.3}` then trimmed, rather than `{}`: the default float format prints
    // `0.30000000000000004` for arithmetic a transform routinely produces.
    let mut buf = String::new();
    let _ = write!(buf, "{value:.PLACES$}");
    let trimmed = if buf.contains('.') {
        buf.trim_end_matches('0').trim_end_matches('.')
    } else {
        buf.as_str()
    };
    // `-0` and the empty string are both what trimming `-0.000` and `0.000`
    // leave behind.
    out.push_str(match trimmed {
        "" | "-0" | "-" => "0",
        other => other,
    });
}

/// A space-separated pair, the way every SVG path command takes its operands.
fn point(out: &mut String, p: Point) {
    number(out, p.x);
    out.push(' ');
    number(out, p.y);
}

/// `path`'s `d` attribute, in absolute commands.
///
/// Quadratics are not emitted: `kurbo::BezPath` has a `QuadTo` element and SVG
/// has `Q`, so the mapping is exact for every one of the five element kinds.
#[must_use]
pub fn path_data(path: &BezPath) -> String {
    let mut out = String::new();
    for el in path.elements() {
        if !out.is_empty() {
            out.push(' ');
        }
        match *el {
            PathEl::MoveTo(p) => {
                out.push('M');
                point(&mut out, p);
            }
            PathEl::LineTo(p) => {
                out.push('L');
                point(&mut out, p);
            }
            PathEl::QuadTo(c, p) => {
                out.push('Q');
                point(&mut out, c);
                out.push(' ');
                point(&mut out, p);
            }
            PathEl::CurveTo(c1, c2, p) => {
                out.push('C');
                point(&mut out, c1);
                out.push(' ');
                point(&mut out, c2);
                out.push(' ');
                point(&mut out, p);
            }
            PathEl::ClosePath => out.push('Z'),
        }
    }
    out
}

/// `t` as an SVG `matrix(...)`, or `None` when it is the identity.
///
/// The identity case is `None` rather than `matrix(1 0 0 1 0 0)` so the
/// commonest transform costs no attribute at all: the engine has usually
/// folded the page matrix into the geometry before the device call, and a
/// `transform` on every element would be the single largest term in the
/// output's size.
#[must_use]
pub fn matrix(t: Affine) -> Option<String> {
    let c = t.as_coeffs();
    // Bit-exact equality is the right test and not an approximation: the
    // question is whether the engine handed us `Affine::IDENTITY` itself, so
    // that the attribute can be omitted. A transform that is *nearly* the
    // identity is a real transform and is written out; treating it as the
    // identity would move geometry by whatever the tolerance was.
    if c.iter()
        .zip(Affine::IDENTITY.as_coeffs())
        .all(|(a, b)| a.to_bits() == b.to_bits())
    {
        return None;
    }
    let mut out = String::from("matrix(");
    for (i, v) in c.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        number(&mut out, *v);
    }
    out.push(')');
    Some(out)
}

/// `#rrggbb`, with the alpha dropped — SVG carries it in `fill-opacity`
/// instead, which is what lets a `<g opacity>` multiply cleanly over it.
#[must_use]
pub fn rgb_hex(color: peniko::Color) -> String {
    let [r, g, b, _] = color.to_rgba8().to_u8_array();
    let mut out = String::with_capacity(7);
    out.push('#');
    for byte in [r, g, b] {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// `color`'s alpha in `[0, 1]`.
#[must_use]
pub fn alpha_of(color: peniko::Color) -> f32 {
    f32::from(color.to_rgba8().a) / 255.0
}

/// Escape the five characters XML gives meaning to inside an attribute value.
///
/// Every attribute this crate writes goes through here even when its content
/// is generated — a base64 payload contains none of these, but the rule that
/// *nothing* reaches the output unescaped is the one that survives a later
/// edit.
pub fn escape(out: &mut String, text: &str) {
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            other => out.push(other),
        }
    }
}

/// Standard base64 with padding, for a `data:` URI's payload.
///
/// Hand-rolled rather than pulled in: is a closed set, this is
/// nineteen lines, and `pdfrum-svg` is meant to ship with no dependency
/// beyond the geometry vocabulary the engine already speaks.
#[must_use]
pub fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let (b0, b1, b2) = (
            u32::from(chunk.first().copied().unwrap_or(0)),
            u32::from(chunk.get(1).copied().unwrap_or(0)),
            u32::from(chunk.get(2).copied().unwrap_or(0)),
        );
        let word = (b0 << 16) | (b1 << 8) | b2;
        // Three input bytes make four characters; a short final chunk makes
        // `len + 1` of them and pads the rest with `=`, which is what tells a
        // decoder how many bytes were really there.
        for i in 0..4 {
            if i <= chunk.len() {
                let index = ((word >> (18 - 6 * i)) & 0x3f) as usize;
                out.push(char::from(ALPHABET.get(index).copied().unwrap_or(b'A')));
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_are_trimmed_and_finite() {
        let mut s = String::new();
        number(&mut s, 1.0);
        number(&mut s, -0.0001);
        number(&mut s, f64::NAN);
        number(&mut s, f64::INFINITY);
        assert_eq!(s, "1000");
    }

    #[test]
    fn a_number_keeps_three_places() {
        let mut s = String::new();
        number(&mut s, 1.234_56);
        assert_eq!(s, "1.235");
    }

    #[test]
    fn path_data_covers_every_element_kind() {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((1.0, 0.0));
        p.quad_to((2.0, 1.0), (3.0, 0.0));
        p.curve_to((4.0, 1.0), (5.0, 1.0), (6.0, 0.0));
        p.close_path();
        assert_eq!(path_data(&p), "M0 0 L1 0 Q2 1 3 0 C4 1 5 1 6 0 Z");
    }

    #[test]
    fn the_identity_transform_is_no_attribute() {
        assert_eq!(matrix(Affine::IDENTITY), None);
        assert_eq!(
            matrix(Affine::translate((2.0, 3.0))).as_deref(),
            Some("matrix(1 0 0 1 2 3)")
        );
    }

    #[test]
    fn colour_splits_into_hex_and_alpha() {
        let c = peniko::Color::from_rgba8(0x12, 0xab, 0xff, 128);
        assert_eq!(rgb_hex(c), "#12abff");
        assert!((alpha_of(c) - 128.0 / 255.0).abs() < 1e-6);
    }

    #[test]
    fn every_xml_metacharacter_is_escaped() {
        let mut s = String::new();
        escape(&mut s, "a&b<c>d\"e'f");
        assert_eq!(s, "a&amp;b&lt;c&gt;d&quot;e&apos;f");
    }

    #[test]
    fn base64_matches_the_rfc_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }
}
