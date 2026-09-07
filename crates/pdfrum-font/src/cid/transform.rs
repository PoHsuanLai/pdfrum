//! The Adobe-Japan1 vertical CID transform table.
//!
//! A non-embedded Japanese font is substituted with a face that has only
//! upright glyphs, so PDFium rotates and shifts a hundred and fifty-four of
//! them itself. The table is per-CID and hand-tuned; there is no rule behind
//! it.

use pdfrum_cmap::Cid;
use pdfrum_common::kurbo::Rect;

/// A per-CID affine transform, packed into bytes.
///
/// Each of the six components is a byte read through
/// [`cid_transform_to_float`], which maps `0..=127` to `0..≈1` and `128..=255`
/// to a *negative* range — note the split point is 255, not 256, so byte 255
/// is exactly zero rather than a small negative.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CidTransform {
    /// The CID this row applies to.
    pub cid: u16,
    /// Matrix `a`.
    pub a: u8,
    /// Matrix `b`.
    pub b: u8,
    /// Matrix `c`.
    pub c: u8,
    /// Matrix `d`.
    pub d: u8,
    /// Matrix `e`, in thousandths of an em once scaled.
    pub e: u8,
    /// Matrix `f`, in thousandths of an em once scaled.
    pub f: u8,
}

include!("transform_table.rs");

/// The transform for a CID, when the table declares one.
///
/// The table is sorted by CID and binary-searched, exactly as the C++ does.
#[must_use]
pub fn japan1_transform(cid: Cid) -> Option<CidTransform> {
    JAPAN1_VERTICAL_CIDS
        .binary_search_by_key(&cid.0, |t| t.cid)
        .ok()
        .and_then(|i| JAPAN1_VERTICAL_CIDS.get(i).copied())
}

/// Unpack one byte of a transform.
///
/// `(if b < 128 { b } else { b - 255 }) / 127`. The `- 255` rather than `- 256`
/// is the detail worth checking: it makes byte 129 map to `-126/127` and byte
/// **255 map to exactly 0**, so the table's `0xFF` entries are "no offset"
/// rather than "a whole em backwards".
#[must_use]
pub fn cid_transform_to_float(b: u8) -> f32 {
    let v = if b < 128 {
        i32::from(b)
    } else {
        i32::from(b) - 255
    };
    v as f32 * (1.0 / 127.0)
}

/// Apply a transform to a glyph bounding box, taking the outer rectangle.
///
/// The translation components are scaled by 1000 because the box is already in
/// 1000/em text space while the packed bytes are in em fractions.
// `a`..`f` are the names the six affine components carry everywhere a matrix
// is written down, in the PDF specification and in this table alike; spelling
// them out would obscure which component is which.
#[allow(clippy::many_single_char_names)]
#[must_use]
pub fn apply(t: CidTransform, bbox: Rect) -> Rect {
    let (a, b, c, d) = (
        f64::from(cid_transform_to_float(t.a)),
        f64::from(cid_transform_to_float(t.b)),
        f64::from(cid_transform_to_float(t.c)),
        f64::from(cid_transform_to_float(t.d)),
    );
    let e = f64::from(cid_transform_to_float(t.e)) * 1000.0;
    let f = f64::from(cid_transform_to_float(t.f)) * 1000.0;

    let corners = [
        (bbox.x0, bbox.y0),
        (bbox.x1, bbox.y0),
        (bbox.x1, bbox.y1),
        (bbox.x0, bbox.y1),
    ];
    let mut out: Option<Rect> = None;
    for (x, y) in corners {
        let px = a * x + c * y + e;
        let py = b * x + d * y + f;
        out = Some(match out {
            None => Rect::new(px, py, px, py),
            Some(r) => Rect::new(r.x0.min(px), r.y0.min(py), r.x1.max(px), r.y1.max(py)),
        });
    }
    out.unwrap_or(Rect::ZERO)
}

#[cfg(test)]
mod tests {
    // Test expectations are exact values by design.
    #![allow(clippy::float_cmp)]
    use super::*;

    #[test]
    fn the_table_has_one_hundred_and_fifty_four_rows() {
        // The oracle's array has 154. Counted twice against the C++ source,
        // and pinned here so a future extraction that silently drops rows is
        // caught.
        assert_eq!(JAPAN1_VERTICAL_CIDS.len(), 154);
    }

    #[test]
    fn the_table_is_sorted_by_cid_so_the_binary_search_is_valid() {
        for pair in JAPAN1_VERTICAL_CIDS.windows(2) {
            let (Some(a), Some(b)) = (pair.first(), pair.get(1)) else {
                continue;
            };
            assert!(a.cid < b.cid, "{} then {}", a.cid, b.cid);
        }
    }

    #[test]
    fn the_first_and_last_rows_are_the_oracles() {
        let first = JAPAN1_VERTICAL_CIDS.first().expect("non-empty");
        assert_eq!(
            *first,
            CidTransform {
                cid: 97,
                a: 129,
                b: 0,
                c: 0,
                d: 127,
                e: 55,
                f: 0
            }
        );
        let last = JAPAN1_VERTICAL_CIDS.last().expect("non-empty");
        assert_eq!(
            *last,
            CidTransform {
                cid: 8819,
                a: 0,
                b: 129,
                c: 127,
                d: 0,
                e: 218,
                f: 108
            }
        );
    }

    #[test]
    fn lookups_hit_and_miss_correctly() {
        assert!(japan1_transform(Cid(97)).is_some());
        assert!(japan1_transform(Cid(7887)).is_some());
        assert!(japan1_transform(Cid(8819)).is_some());
        // Between the first row and the second block.
        assert!(japan1_transform(Cid(98)).is_none());
        assert!(japan1_transform(Cid(0)).is_none());
        assert!(japan1_transform(Cid(u16::MAX)).is_none());
    }

    #[test]
    fn the_byte_unpacking_splits_at_255_not_256() {
        assert_eq!(cid_transform_to_float(0), 0.0);
        assert!((cid_transform_to_float(127) - 1.0).abs() < 1e-6);
        // Byte 128 is the first negative value.
        assert!((cid_transform_to_float(128) - (-127.0 / 127.0)).abs() < 1e-6);
        assert!((cid_transform_to_float(129) - (-126.0 / 127.0)).abs() < 1e-6);
        // And byte 255 is exactly zero, which `- 256` would have made -1/127.
        assert_eq!(cid_transform_to_float(255), 0.0);
    }

    #[test]
    fn the_rotation_rows_actually_rotate() {
        // CID 7889's matrix is `{0, 129, 127, 0, …}`: a ≈ 0, b ≈ -1, c ≈ 1,
        // d = 0 — a quarter turn, which is what a vertical form needs.
        let t = japan1_transform(Cid(7889)).expect("row exists");
        assert_eq!(cid_transform_to_float(t.a), 0.0);
        assert!(cid_transform_to_float(t.b) < -0.9);
        assert!(cid_transform_to_float(t.c) > 0.9);
        assert_eq!(cid_transform_to_float(t.d), 0.0);

        // A tall narrow box comes out short and wide.
        let out = apply(t, Rect::new(0.0, 0.0, 100.0, 800.0));
        assert!(out.width() > out.height(), "{out:?}");
    }

    #[test]
    fn the_identity_ish_rows_leave_a_box_roughly_alone() {
        // CID 7887's matrix is `{127, 0, 0, 127, …}`: a ≈ d ≈ 1, no rotation,
        // just a translation.
        let t = japan1_transform(Cid(7887)).expect("row exists");
        let out = apply(t, Rect::new(0.0, 0.0, 100.0, 800.0));
        assert!((out.width() - 100.0).abs() < 1.0, "{out:?}");
        assert!((out.height() - 800.0).abs() < 1.0, "{out:?}");
        // But it is shifted.
        assert!(out.x0 > 500.0, "{out:?}");
    }

    #[test]
    fn applying_to_a_degenerate_box_is_still_a_box() {
        let t = japan1_transform(Cid(97)).expect("row exists");
        let out = apply(t, Rect::ZERO);
        assert!(out.width().abs() < 1e-6);
        assert!(out.height().abs() < 1e-6);
    }
}
