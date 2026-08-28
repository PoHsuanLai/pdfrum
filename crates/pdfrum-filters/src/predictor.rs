//! The two `/Predictor` transforms Flate and LZW may be followed by
//! (ISO 32000-1 table 10): PNG's per-row filters and TIFF's horizontal
//! differencing.
//!
//! Both undo a reversible transform the producer applied to make the data
//! compress better, and both are applied to the *decompressed* bytes, so they
//! never see the compressed stream. Which one runs is decided by a range, not
//! a value: `/Predictor` 2 means TIFF, anything 10 or greater means PNG, and
//! everything else — 0, 1, a negative number — means neither. The specific PNG
//! number (10 through 15) is ignored, because each row of a PNG-predicted
//! stream carries its own filter tag.
//!
//! # Two ways this is more forgiving than PNG itself
//!
//! An unrecognized row tag is copied verbatim rather than rejected. Tag 0
//! means exactly that (no filtering), and it reaches the copy through the same
//! branch as tag 7 or tag 200 — so a stream with garbage tags decodes to
//! garbage instead of failing, which is what keeps a slightly-corrupt
//! cross-reference stream readable.
//!
//! And a final row shorter than a full row is predicted over the bytes that
//! exist. The row count rounds *up*, so the short row is a row, and only its
//! actual length reaches the output.

use pdfrum_object::{Resolve, names};

use crate::Error;

/// Which predictor a `/DecodeParms` selects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PredictorKind {
    /// No predictor; the data passes through unchanged.
    None,
    /// TIFF predictor 2, horizontal differencing (`/Predictor 2`).
    Tiff,
    /// PNG predictors, tagged per row (`/Predictor` 10 through 15).
    Png,
}

impl PredictorKind {
    /// Classify a `/Predictor` value by range, as PDFium does.
    ///
    /// ```
    /// use pdfrum_filters::PredictorKind;
    ///
    /// assert_eq!(PredictorKind::from_predictor(2), PredictorKind::Tiff);
    /// // Every PNG predictor is the same to us: the row tags decide.
    /// assert_eq!(PredictorKind::from_predictor(10), PredictorKind::Png);
    /// assert_eq!(PredictorKind::from_predictor(15), PredictorKind::Png);
    /// assert_eq!(PredictorKind::from_predictor(1), PredictorKind::None);
    /// ```
    #[must_use]
    pub fn from_predictor(n: i64) -> PredictorKind {
        if n >= 10 {
            PredictorKind::Png
        } else if n == 2 {
            PredictorKind::Tiff
        } else {
            PredictorKind::None
        }
    }
}

/// The sample geometry a predictor needs, from `/DecodeParms`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PredictorParams {
    /// Which predictor to apply.
    pub kind: PredictorKind,
    /// `/Colors`: colour components per sample. Default 1.
    pub colors: u32,
    /// `/BitsPerComponent`: bits per component. Default 8.
    pub bits_per_component: u32,
    /// `/Columns`: samples per row. Default 1.
    pub columns: u32,
}

impl Default for PredictorParams {
    fn default() -> Self {
        Self {
            kind: PredictorKind::None,
            colors: 1,
            bits_per_component: 8,
            columns: 1,
        }
    }
}

impl PredictorParams {
    /// Read `/Predictor`, `/Colors`, `/BitsPerComponent` and `/Columns` from a
    /// `/DecodeParms` dictionary.
    ///
    /// ```
    /// use pdfrum_filters::{PredictorKind, PredictorParams};
    /// use pdfrum_object::{Dict, NoResolve, Object, names};
    ///
    /// let parms = Dict::from_pairs([
    ///     (names::PREDICTOR.clone(), Object::Int(12)),
    ///     (names::COLUMNS.clone(), Object::Int(5)),
    /// ]);
    /// let params = PredictorParams::from_dict(&parms, &NoResolve)?;
    /// assert_eq!(params.kind, PredictorKind::Png);
    /// assert_eq!(params.columns, 5);
    /// assert_eq!(params.colors, 1, "the default survives");
    /// # Ok::<(), pdfrum_filters::Error>(())
    /// ```
    ///
    /// # Errors
    ///
    /// [`Error::BadPredictorParams`] for a negative count or for a row wider
    /// than a 32-bit signed row size can describe.
    ///
    /// Note this validation runs whatever `/Predictor` says, including when it
    /// selects no predictor at all: PDFium checks the geometry before it looks
    /// at the predictor, so an unusable `/Columns` discards a perfectly good
    /// Flate stream that would never have been predicted.
    pub fn from_dict(d: &pdfrum_object::Dict, r: &impl Resolve) -> Result<Self, Error> {
        let predictor = d.int(names::PREDICTOR, r).unwrap_or(0);
        let colors = d.int(names::COLORS, r).unwrap_or(1);
        let bits_per_component = d.int(names::BITS_PER_COMPONENT, r).unwrap_or(8);
        let columns = d.int(names::COLUMNS, r).unwrap_or(1);

        if colors < 0 || bits_per_component < 0 || columns < 0 {
            return Err(Error::BadPredictorParams(
                "/Colors, /BitsPerComponent and /Columns must not be negative",
            ));
        }
        // The row's bit count must fit a signed 32-bit value with room for the
        // seven bits the byte rounding adds.
        let bits = i64::from(i32::MAX) - 7;
        if columns
            .checked_mul(colors)
            .and_then(|n| n.checked_mul(bits_per_component))
            .is_none_or(|n| n > bits)
        {
            return Err(Error::BadPredictorParams(
                "/Columns * /Colors * /BitsPerComponent is too large for a row",
            ));
        }

        // The three conversions cannot fail: each value is non-negative and the
        // product checked above bounds every one of them below `i32::MAX`.
        let fit = |n: i64| u32::try_from(n).map_err(|_| Error::SizeOverflow);
        Ok(Self {
            kind: PredictorKind::from_predictor(predictor),
            colors: fit(colors)?,
            bits_per_component: fit(bits_per_component)?,
            columns: fit(columns)?,
        })
    }

    /// Bytes per predicted row, `ceil(columns * colors * bpc / 8)`.
    fn row_size(self) -> Option<usize> {
        let bits = u64::from(self.bits_per_component)
            .checked_mul(u64::from(self.colors))?
            .checked_mul(u64::from(self.columns))?
            .checked_add(7)?;
        usize::try_from(bits / 8).ok()
    }

    /// Bytes per sample, `ceil(colors * bpc / 8)` — the PNG filters' stride.
    fn bytes_per_pixel(self) -> usize {
        let bits = u64::from(self.colors) * u64::from(self.bits_per_component) + 7;
        // Bounded by `from_dict`'s check, so the cast cannot lose anything.
        (bits / 8) as usize
    }
}

/// Undo a `/Predictor` transform over already-decompressed data.
///
/// [`PredictorKind::None`] returns `data` untouched, which is the common case:
/// most streams have no predictor at all.
///
/// ```
/// use pdfrum_filters::{PredictorKind, PredictorParams, predictor};
///
/// // One three-byte row, tagged 2 (Up) with no row above it, so the tag adds
/// // nothing and the bytes pass through.
/// let params = PredictorParams {
///     kind: PredictorKind::Png,
///     colors: 3,
///     columns: 1,
///     ..PredictorParams::default()
/// };
/// assert_eq!(predictor(vec![2, 10, 20, 30], params)?, vec![10, 20, 30]);
/// # Ok::<(), pdfrum_filters::Error>(())
/// ```
///
/// # Errors
///
/// [`Error::BadPredictorParams`] when the parameters describe a zero-byte row,
/// or when PNG data is too short to hold even one row's tag byte, and
/// [`Error::SizeOverflow`] when the output size does not fit in memory.
pub fn predictor(data: Vec<u8>, params: PredictorParams) -> Result<Vec<u8>, Error> {
    match params.kind {
        PredictorKind::None => Ok(data),
        PredictorKind::Png => png_predictor(&data, params),
        PredictorKind::Tiff => tiff_predictor(data, params),
    }
}

/// Undo PNG's per-row filters. Each source row is a tag byte plus `row_size`
/// bytes; the last row may be short.
fn png_predictor(src: &[u8], params: PredictorParams) -> Result<Vec<u8>, Error> {
    let row_size = params.row_size().ok_or(Error::SizeOverflow)?;
    if row_size == 0 {
        return Err(Error::BadPredictorParams("the row is zero bytes wide"));
    }
    let src_row_size = row_size + 1;
    // Rounding up: a final row missing some of its bytes is still a row.
    let row_count = (src.len() + row_size) / src_row_size;
    if row_count == 0 {
        return Err(Error::BadPredictorParams(
            "the data is shorter than one row's tag byte",
        ));
    }
    let last_row_size = src.len() % src_row_size;
    let mut dest_size = row_size.checked_mul(row_count).ok_or(Error::SizeOverflow)?;
    if last_row_size != 0 {
        // The short final row contributes only the bytes it actually has.
        dest_size = dest_size
            .checked_sub(src_row_size - last_row_size)
            .ok_or(Error::SizeOverflow)?;
    }

    let bpp = params.bytes_per_pixel();
    let mut out: Vec<u8> = Vec::with_capacity(dest_size);
    let mut at = 0usize;
    // Where the previous decoded row starts in `out`; `None` for the first.
    let mut previous: Option<usize> = None;

    for _ in 0..row_count {
        let Some(&tag) = src.get(at) else { break };
        let row = src.get(at + 1..).unwrap_or_default();
        let len = row_size.min(row.len());
        let row = row.get(..len).unwrap_or_default();
        let start = out.len();

        for i in 0..len {
            let raw = row.get(i).copied().unwrap_or(0);
            let left = if i >= bpp {
                out.get(start + i - bpp).copied().unwrap_or(0)
            } else {
                0
            };
            // The row above is always at least as long as this one, because
            // only the last row can be short.
            let up = previous.and_then(|p| out.get(p + i)).copied().unwrap_or(0);
            let upper_left = if i >= bpp {
                previous
                    .and_then(|p| out.get(p + i - bpp))
                    .copied()
                    .unwrap_or(0)
            } else {
                0
            };
            let value = match tag {
                1 => raw.wrapping_add(left),
                2 => raw.wrapping_add(up),
                // The average is taken on the *sum* of the two bytes before
                // the halving, so 200 and 200 average to 200, not to 72.
                3 => raw.wrapping_add(u8::midpoint(up, left)),
                4 => raw.wrapping_add(paeth(left, up, upper_left)),
                // Tag 0 (none) and every tag that is not a filter at all.
                _ => raw,
            };
            out.push(value);
        }

        previous = Some(start);
        at += len + 1;
    }
    Ok(out)
}

/// PNG's Paeth predictor: whichever neighbour is closest to `a + b - c`.
fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let p = i32::from(a) + i32::from(b) - i32::from(c);
    let pa = (p - i32::from(a)).abs();
    let pb = (p - i32::from(b)).abs();
    let pc = (p - i32::from(c)).abs();
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

/// Undo TIFF horizontal differencing, in place, one row at a time. A final
/// short row is predicted over the bytes it has.
fn tiff_predictor(mut data: Vec<u8>, params: PredictorParams) -> Result<Vec<u8>, Error> {
    let row_size = params.row_size().ok_or(Error::SizeOverflow)?;
    if row_size == 0 {
        return Err(Error::BadPredictorParams("the row is zero bytes wide"));
    }
    let mut at = 0usize;
    while at < data.len() {
        let len = row_size.min(data.len() - at);
        if let Some(row) = data.get_mut(at..at + len) {
            tiff_predict_row(row, params);
        }
        at += len;
    }
    Ok(data)
}

/// One row of TIFF differencing. Three shapes, by bit depth.
fn tiff_predict_row(row: &mut [u8], params: PredictorParams) {
    if params.bits_per_component == 1 {
        tiff_predict_row_1bpc(row, params);
        return;
    }
    // Deliberately the C++'s truncating division rather than a rounding one:
    // for 2 or 4 bits per component with few colours this is zero, and the
    // stride-zero loop that follows doubles every byte. Nonsensical, but it is
    // what the oracle produces, and no corpus file exercises it.
    let stride = (params.bits_per_component as usize * params.colors as usize) / 8;
    if params.bits_per_component == 16 {
        // Big-endian 16-bit samples add as whole numbers.
        let mut i = stride;
        while i + 1 < row.len() {
            let previous = u16::from_be_bytes([
                row.get(i - stride).copied().unwrap_or(0),
                row.get(i - stride + 1).copied().unwrap_or(0),
            ]);
            let current = u16::from_be_bytes([
                row.get(i).copied().unwrap_or(0),
                row.get(i + 1).copied().unwrap_or(0),
            ]);
            let sum = current.wrapping_add(previous).to_be_bytes();
            if let Some(slot) = row.get_mut(i..i + 2) {
                slot.copy_from_slice(&sum);
            }
            i += 2;
        }
        return;
    }
    for i in stride..row.len() {
        let previous = row.get(i - stride).copied().unwrap_or(0);
        if let Some(slot) = row.get_mut(i) {
            *slot = slot.wrapping_add(previous);
        }
    }
}

/// One-bit differencing is an XOR accumulation across the row's bits.
fn tiff_predict_row_1bpc(row: &mut [u8], params: PredictorParams) {
    // The declared bit count is what the parameters promise; the available one
    // is what the row actually holds. A short row stops at its own end.
    let declared =
        u64::from(params.bits_per_component) * u64::from(params.colors) * u64::from(params.columns);
    let available = row.len().saturating_mul(8);
    let row_bits = usize::try_from(declared)
        .unwrap_or(usize::MAX)
        .min(available);

    let mut previous = 0usize;
    for i in 1..row_bits {
        let bit = |at: usize| row.get(at / 8).map_or(0, |byte| (byte >> (7 - at % 8)) & 1);
        let set = bit(i) ^ bit(previous);
        if let Some(byte) = row.get_mut(i / 8) {
            let mask = 1u8 << (7 - i % 8);
            if set == 1 {
                *byte |= mask;
            } else {
                *byte &= !mask;
            }
        }
        previous = i;
    }
}

#[cfg(test)]
mod tests {
    use super::{PredictorKind, PredictorParams, paeth, predictor};
    use crate::Error;
    use pdfrum_object::{Dict, Name, NoResolve, Object, names};

    fn parms(pairs: &[(&Name, i64)]) -> Dict {
        Dict::from_pairs(
            pairs
                .iter()
                .map(|(key, value)| ((*key).clone(), Object::Int(*value))),
        )
    }

    fn png(colors: u32, bits_per_component: u32, columns: u32) -> PredictorParams {
        PredictorParams {
            kind: PredictorKind::Png,
            colors,
            bits_per_component,
            columns,
        }
    }

    fn tiff(colors: u32, bits_per_component: u32, columns: u32) -> PredictorParams {
        PredictorParams {
            kind: PredictorKind::Tiff,
            colors,
            bits_per_component,
            columns,
        }
    }

    #[test]
    fn absent_parameters_give_the_defaults_and_no_predictor() {
        let params = PredictorParams::from_dict(&Dict::new(), &NoResolve).expect("valid");
        assert_eq!(params, PredictorParams::default());
        assert_eq!(params.kind, PredictorKind::None);
        assert_eq!(
            (params.colors, params.bits_per_component, params.columns),
            (1, 8, 1)
        );
    }

    #[test]
    fn the_predictor_number_classifies_by_range() {
        let cases: [(i64, PredictorKind); 8] = [
            (0, PredictorKind::None),
            (1, PredictorKind::None),
            (2, PredictorKind::Tiff),
            (3, PredictorKind::None),
            (9, PredictorKind::None),
            (10, PredictorKind::Png),
            (15, PredictorKind::Png),
            (100, PredictorKind::Png),
        ];
        for (n, expected) in cases {
            let d = parms(&[
                (names::PREDICTOR, n),
                (names::COLORS, 3),
                (names::BITS_PER_COMPONENT, 8),
                (names::COLUMNS, 4),
            ]);
            let params = PredictorParams::from_dict(&d, &NoResolve).expect("valid");
            assert_eq!(params.kind, expected, "/Predictor {n}");
        }
    }

    #[test]
    fn negative_geometry_is_rejected() {
        for (key, value) in [
            (names::COLORS, -1i64),
            (names::BITS_PER_COMPONENT, -8),
            (names::COLUMNS, -1),
        ] {
            let d = parms(&[(names::PREDICTOR, 12), (key, value)]);
            assert!(
                matches!(
                    PredictorParams::from_dict(&d, &NoResolve),
                    Err(Error::BadPredictorParams(_))
                ),
                "{key:?} = {value}"
            );
        }
    }

    #[test]
    fn a_row_too_wide_for_a_signed_row_size_is_rejected() {
        let d = parms(&[
            (names::PREDICTOR, 12),
            (names::COLORS, 0x1_0000),
            (names::BITS_PER_COMPONENT, 32),
            (names::COLUMNS, 0x1_0000),
        ]);
        assert!(matches!(
            PredictorParams::from_dict(&d, &NoResolve),
            Err(Error::BadPredictorParams(_))
        ));
    }

    #[test]
    fn geometry_is_validated_even_when_no_predictor_is_selected() {
        // PDFium checks before it branches, so a bad /Colors discards the
        // stream even though nothing would have been predicted.
        let d = parms(&[(names::PREDICTOR, 0), (names::COLORS, -1)]);
        assert!(PredictorParams::from_dict(&d, &NoResolve).is_err());
    }

    #[test]
    fn a_zero_width_row_fails_at_predict_time_not_parse_time() {
        let d = parms(&[
            (names::PREDICTOR, 12),
            (names::COLORS, 0),
            (names::BITS_PER_COMPONENT, 8),
            (names::COLUMNS, 4),
        ]);
        let params = PredictorParams::from_dict(&d, &NoResolve).expect("parses");
        assert_eq!(params.colors, 0);
        assert!(matches!(
            predictor(vec![0; 16], params),
            Err(Error::BadPredictorParams(_))
        ));
    }

    #[test]
    fn no_predictor_returns_the_data_untouched() {
        let data = vec![1u8, 2, 3, 4, 5];
        assert_eq!(
            predictor(data.clone(), PredictorParams::default()).expect("ok"),
            data
        );
    }

    // Three rows of three RGB-ish samples, one row per filter tag.
    #[test]
    fn each_png_row_tag_over_a_known_image() {
        let params = png(3, 8, 3); // row_size 9, bpp 3

        // Tag 0: verbatim.
        let src = [0u8, 1, 2, 3, 4, 5, 6, 7, 8, 9];
        assert_eq!(
            predictor(src.to_vec(), params).expect("ok"),
            vec![1, 2, 3, 4, 5, 6, 7, 8, 9]
        );

        // Tag 1 (Sub): each sample adds the one three bytes to its left.
        let src = [1u8, 1, 2, 3, 10, 10, 10, 100, 100, 100];
        assert_eq!(
            predictor(src.to_vec(), params).expect("ok"),
            vec![1, 2, 3, 11, 12, 13, 111, 112, 113]
        );

        // Tag 2 (Up) with no row above adds nothing; with one, it adds it.
        let src = [
            0u8, 10, 20, 30, 40, 50, 60, 70, 80, 90, // verbatim row
            2, 1, 1, 1, 1, 1, 1, 1, 1, 1, // each += the row above
        ];
        assert_eq!(
            predictor(src.to_vec(), params).expect("ok"),
            vec![
                10, 20, 30, 40, 50, 60, 70, 80, 90, 11, 21, 31, 41, 51, 61, 71, 81, 91
            ]
        );

        // Tag 3 (Average): (up + left) / 2, integer division on the sum.
        let src = [
            0u8, 8, 8, 8, 8, 8, 8, 8, 8, 8, //
            3, 1, 1, 1, 1, 1, 1, 1, 1, 1,
        ];
        let out = predictor(src.to_vec(), params).expect("ok");
        // First sample of row 2: left is 0, up is 8 -> 1 + 4 = 5.
        assert_eq!(out.get(9), Some(&5));
        // Second sample: left is 5, up is 8 -> 1 + 6 = 7.
        assert_eq!(out.get(12), Some(&7));

        // Tag 4 (Paeth).
        let src = [
            0u8, 1, 2, 3, 4, 5, 6, 7, 8, 9, //
            4, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ];
        // With a zero delta, Paeth reproduces the row above.
        assert_eq!(
            predictor(src.to_vec(), params).expect("ok").get(9..),
            Some(&[1u8, 2, 3, 4, 5, 6, 7, 8, 9][..])
        );
    }

    // Not an error, not a diagnostic: an unknown tag is a verbatim copy, the
    // same branch tag 0 takes.
    #[test]
    fn an_invalid_row_tag_copies_verbatim() {
        let params = png(1, 8, 4);
        for tag in [0u8, 5, 6, 7, 200, 255] {
            let src = [tag, 9, 8, 7, 6];
            assert_eq!(
                predictor(src.to_vec(), params).expect("never fails"),
                vec![9, 8, 7, 6],
                "tag {tag}"
            );
        }
    }

    #[test]
    fn a_truncated_final_row_is_predicted_over_the_bytes_it_has() {
        let params = png(1, 8, 4); // row_size 4, src_row_size 5
        // Two full rows and a third with only two of its four bytes.
        let src = [
            0u8, 1, 1, 1, 1, //
            1, 1, 1, 1, 1, //
            1, 5, 5,
        ];
        let out = predictor(src.to_vec(), params).expect("ok");
        assert_eq!(out.len(), 4 + 4 + 2, "row_size * (rows - 1) + last_row_len");
        // The short row still gets its Sub filter over its two bytes.
        assert_eq!(out.get(8..), Some(&[5u8, 10][..]));
    }

    #[test]
    fn data_shorter_than_one_row_still_yields_one_row() {
        let params = png(1, 8, 100); // row_size 100
        let src = [0u8, 1, 2, 3];
        let out = predictor(src.to_vec(), params).expect("ok");
        assert_eq!(out, vec![1, 2, 3], "input.len() - 1 bytes");
    }

    #[test]
    fn png_data_with_only_a_tag_byte_yields_an_empty_row() {
        let params = png(1, 8, 4);
        assert_eq!(predictor(vec![2], params).expect("ok"), Vec::<u8>::new());
    }

    #[test]
    fn empty_png_data_has_no_rows_at_all() {
        let params = png(1, 8, 4);
        assert!(matches!(
            predictor(Vec::new(), params),
            Err(Error::BadPredictorParams(_))
        ));
    }

    #[test]
    fn png_stride_follows_the_sample_size() {
        // bpc 16, colors 3 -> six bytes per sample.
        let params = png(3, 16, 2); // row_size 12
        let mut src = vec![1u8]; // Sub
        src.extend([0u8; 6]); // first sample: nothing to its left
        src.extend([1u8; 6]); // second sample: each byte += six back
        let out = predictor(src, params).expect("ok");
        assert_eq!(out.get(6..), Some(&[1u8; 6][..]));

        // bpc 1, colors 1, columns 8 -> one byte per row, one per sample.
        let params = png(1, 1, 8);
        let out = predictor(vec![1, 0b1010_1010], params).expect("ok");
        assert_eq!(out, vec![0b1010_1010]);
    }

    #[test]
    fn tiff_eight_bit_differencing_uses_the_colour_count_as_its_stride() {
        let params = tiff(3, 8, 3); // row_size 9, stride 3
        let data = vec![1u8, 2, 3, 1, 1, 1, 1, 1, 1];
        assert_eq!(
            predictor(data, params).expect("ok"),
            vec![1, 2, 3, 2, 3, 4, 3, 4, 5]
        );
    }

    #[test]
    fn tiff_sixteen_bit_differencing_adds_big_endian_samples() {
        let params = tiff(1, 16, 3); // row_size 6, stride 2
        let data = vec![0x01, 0x00, 0x00, 0x01, 0x00, 0x02];
        // 0x0100, then 0x0100 + 0x0001, then that + 0x0002.
        assert_eq!(
            predictor(data, params).expect("ok"),
            vec![0x01, 0x00, 0x01, 0x01, 0x01, 0x03]
        );
    }

    #[test]
    fn tiff_one_bit_differencing_accumulates_by_xor() {
        let params = tiff(1, 1, 8); // one byte per row
        // Bit 0 stays; each later bit becomes itself XOR the bit before it.
        assert_eq!(
            predictor(vec![0b1000_0000], params).expect("ok"),
            vec![0b1111_1111]
        );
        assert_eq!(
            predictor(vec![0b0000_0000], params).expect("ok"),
            vec![0b0000_0000]
        );
        assert_eq!(
            predictor(vec![0b1010_1010], params).expect("ok"),
            vec![0b1100_1100]
        );
    }

    #[test]
    fn tiff_one_bit_stops_at_the_bytes_that_exist() {
        // The row claims 32 bits but only one byte is there.
        let params = tiff(1, 1, 32);
        let out = predictor(vec![0b1000_0000], params).expect("never overruns");
        assert_eq!(out, vec![0b1111_1111]);
    }

    #[test]
    fn a_tiff_partial_final_row_is_predicted_over_its_own_bytes() {
        let params = tiff(1, 8, 4); // row_size 4
        let data = vec![1u8, 1, 1, 1, 5, 5];
        // Row one accumulates to 1,2,3,4; the two-byte row two to 5,10.
        assert_eq!(
            predictor(data, params).expect("ok"),
            vec![1, 2, 3, 4, 5, 10]
        );
    }

    #[test]
    fn a_zero_width_tiff_row_is_rejected() {
        let params = tiff(0, 8, 4);
        assert!(matches!(
            predictor(vec![1, 2, 3], params),
            Err(Error::BadPredictorParams(_))
        ));
    }

    // Open question Q3 in the design brief, resolved as "reproduce the
    // oracle": for two or four bits per component with one colour the stride
    // computes to zero, and the C++ loop then adds every byte to itself.
    #[test]
    fn tiff_with_a_zero_stride_doubles_every_byte() {
        for bits_per_component in [2u32, 4] {
            let params = tiff(1, bits_per_component, 8);
            let data = vec![3u8, 100, 200];
            assert_eq!(
                predictor(data, params).expect("ok"),
                vec![6, 200, 144],
                "bpc {bits_per_component}"
            );
        }
    }

    #[test]
    fn paeth_picks_the_closest_neighbour() {
        // p = a + b - c, and whichever of a, b, c is nearest it wins, with
        // ties going to a and then b.
        assert_eq!(paeth(0, 0, 0), 0);
        // p = 0: |0-10| = 10 beats |0-20| = 20 and |0-30| = 30.
        assert_eq!(paeth(10, 20, 30), 10);
        // p = 150: b is 50 away, a and c are 50 and 0 away, so c wins.
        assert_eq!(paeth(200, 100, 150), 150);
        // p = 0: a is 1 away, the closest.
        assert_eq!(paeth(1, 2, 3), 1);
        // c far away leaves a and b tied at zero distance; a wins.
        assert_eq!(paeth(5, 5, 200), 5);
        // A pure Up column: b is exact.
        assert_eq!(paeth(0, 40, 0), 40);
    }
}
